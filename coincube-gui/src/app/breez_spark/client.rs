//! Gui-side client for the `coincube-spark-bridge` subprocess.
//!
//! Architecture
//! ------------
//!
//! The Breez Spark SDK lives in a sibling binary because its dep graph
//! (rusqlite / libsqlite3-sys / tokio_with_wasm) can't be unified with
//! breez-sdk-liquid's. See `coincube-spark-bridge/Cargo.toml` for the
//! companion crate.
//!
//! [`SparkClient`] owns a [`tokio::process::Child`] and three background
//! tokio tasks:
//!
//! - **writer**: pulls [`Request`] frames from an mpsc channel and
//!   writes them as JSON lines to the child's stdin.
//! - **reader**: reads JSON lines from the child's stdout, parses
//!   [`Frame`]s, and routes [`Response`]s through a shared pending map
//!   (`id -> oneshot::Sender`). [`Event`] frames go to a future event
//!   channel (not wired in Phase 3 — just logged for now).
//! - **stderr pump**: relays each stderr line from the bridge into tracing
//!   at the severity the bridge itself emitted (see `relay_bridge_log`),
//!   stripping ANSI and demoting the SDK's empty-event keepalive.
//!
//! A request goes like: allocate id, insert `oneshot::Sender` into pending
//! map, send `Request` over the writer channel, await the oneshot. The
//! reader task resolves oneshots by id as responses come back, so
//! concurrent requests don't block each other.
//!
//! Lifecycle
//! ---------
//!
//! [`SparkClient::connect`] spawns the bridge, performs the
//! [`Method::Init`] handshake, and returns the client on success. If the
//! bridge exits before responding, or returns an error, the call fails
//! and the child is cleaned up. On drop the client sends a best-effort
//! [`Method::Shutdown`] (non-blocking fire-and-forget) and kills the
//! child if it didn't exit on its own — `kill_on_drop(true)` on the
//! `Command` ensures the OS reaps it even if the graceful path fails.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use coincube_spark_protocol::{
    CheckLightningAddressAvailableParams, ClaimDepositOk, ClaimDepositParams, CrossChainAddress,
    CrossChainRoute, CrossChainRoutesOk, ErrorKind, ErrorPayload, Event, Frame,
    GetCrossChainRoutesParams, GetInfoOk, GetInfoParams, GetUserSettingsOk, InitParams,
    LightningAddressInfo, ListPaymentsOk, ListPaymentsParams, ListUnclaimedDepositsOk, Method,
    OkPayload, ParseInputOk, ParseInputParams, PrepareCrossChainParams, PrepareLnurlPayParams,
    PrepareSendOk, PrepareSendParams, ReceiveBolt11Params, ReceiveOnchainParams, ReceivePaymentOk,
    RegisterLightningAddressParams, Request, Response, ResponseResult, SendPaymentOk,
    SendPaymentParams, SetStableBalanceParams,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{broadcast, mpsc, oneshot, Mutex};
use tracing::{debug, error, info, trace, warn, Level};

use super::config::SparkConfig;

/// Shared pending-request table. Each entry maps an outstanding request
/// id to the oneshot sender that the caller is awaiting.
type PendingMap = Arc<Mutex<HashMap<u64, oneshot::Sender<Response>>>>;

/// Responses that arrived for a **state-changing** request whose caller had
/// already stopped waiting, keyed by request id.
///
/// A send that outruns its deadline is not finished — the bridge still holds
/// it and the SDK may still be moving money. When the answer finally arrives
/// there is no oneshot left to deliver it to, and the reader used to log
/// "response for unknown id — dropping". That thrown-away frame is the single
/// most authoritative statement anyone has about whether the payment happened,
/// so it is kept here for [`SparkClient::take_late_outcome`] to reconcile
/// against instead.
type LateOutcomes = Arc<Mutex<HashMap<u64, Response>>>;

/// Ids of dispatched state-changing requests whose caller stopped waiting.
/// The reader consults this to decide whether an unmatched response is a late
/// send outcome worth keeping or genuine protocol noise.
type AwaitingLate = Arc<Mutex<std::collections::HashSet<u64>>>;

/// How long to wait for an ordinary query before giving up.
///
/// Plenty for connect + info + list. A query that times out changed nothing,
/// so the caller can simply be told it failed.
const QUERY_TIMEOUT: Duration = Duration::from_secs(30);

/// How long to wait for a **send** before declaring the outcome unknown.
///
/// Deliberately longer than [`QUERY_TIMEOUT`] — an on-chain or cross-chain send
/// can legitimately take a while — but the length is not what makes this safe.
/// What makes it safe is that expiry produces
/// [`SparkClientError::OutcomeUnknown`] rather than a failure: the caller must
/// reconcile before it can send again. A send that inherited the generic query
/// timeout was reported as an ordinary failure, and "Try again" then minted a
/// fresh idempotency key — which is how one payment becomes two.
const SEND_SOFT_DEADLINE: Duration = Duration::from_secs(120);

/// What the reader does with a response frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResponseRoute {
    /// A caller is still waiting — hand it over.
    Deliver,
    /// Nobody is waiting, but this was a **send** whose caller gave up. The
    /// answer is the authoritative word on whether that payment happened, so it
    /// is kept for reconciliation rather than dropped.
    HoldForReconciliation,
    /// Genuinely unmatched: no waiter and no record of a send under this id.
    Drop,
}

/// Decide what to do with a response, given whether a caller is still waiting
/// and whether the id belongs to a send that outran its deadline.
///
/// Split out from the reader loop so the decision is testable without a live
/// subprocess — it is the difference between reconciling a late payment
/// outcome and throwing it away.
fn route_response(has_waiter: bool, is_unresolved_send: bool) -> ResponseRoute {
    match (has_waiter, is_unresolved_send) {
        (true, _) => ResponseRoute::Deliver,
        (false, true) => ResponseRoute::HoldForReconciliation,
        (false, false) => ResponseRoute::Drop,
    }
}

/// Whether a method changes state, and therefore what a missing answer means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestKind {
    /// A read. No answer means no answer; nothing moved.
    Query,
    /// A send. No answer means **unknown**, never "failed".
    StateChanging,
}

/// Handle to a running `coincube-spark-bridge` subprocess.
///
/// Clone-safe: the underlying state is `Arc`-shared, so multiple panels
/// can call methods concurrently. Dropping the last clone triggers a
/// best-effort graceful shutdown of the child process.
#[derive(Clone)]
pub struct SparkClient {
    inner: Arc<SparkClientInner>,
}

/// Shared flag: `true` once the client is shut down (explicitly or
/// via bridge crash). Shared between `SparkClientInner` and
/// `spawn_reader_task` so the reader can mark the client dead when
/// stdout closes unexpectedly.
type ClosedFlag = Arc<std::sync::atomic::AtomicBool>;

struct SparkClientInner {
    next_id: AtomicU64,
    request_tx: mpsc::UnboundedSender<Request>,
    pending: PendingMap,
    /// See [`LateOutcomes`].
    late_outcomes: LateOutcomes,
    /// See [`AwaitingLate`].
    awaiting_late: AwaitingLate,
    /// Broadcast channel into which the reader task pushes every
    /// [`Event`] frame received from the bridge.
    event_tx: broadcast::Sender<Event>,
    child: Mutex<Option<Child>>,
    /// True once `shutdown()` was called, the client was dropped, or
    /// the reader task detected that the bridge subprocess exited —
    /// further requests short-circuit with [`SparkClientError::BridgeUnavailable`].
    closed: ClosedFlag,
}

impl std::fmt::Debug for SparkClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SparkClient").finish_non_exhaustive()
    }
}

impl SparkClient {
    /// Spawn the bridge subprocess, hand it a mnemonic + config, and
    /// return the connected client.
    ///
    /// `mnemonic` is passed into the bridge over stdin and then dropped
    /// by the caller (see [`super::mod::load_spark_client`] for the
    /// zeroizing wrapper). The bridge keeps it in memory for the
    /// session lifetime.
    pub async fn connect(config: SparkConfig, mnemonic: &str) -> Result<Self, SparkClientError> {
        if config.api_key.is_empty() {
            return Err(SparkClientError::Config(
                "Spark SDK API key is empty — set BREEZ_API_KEY at build time".to_string(),
            ));
        }

        let bridge_path = resolve_bridge_path()?;
        debug!("spawning Spark bridge at {:?}", bridge_path);

        let mut child = Command::new(&bridge_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                SparkClientError::BridgeUnavailable(format!(
                    "failed to spawn {}: {}",
                    bridge_path.display(),
                    e
                ))
            })?;

        let stdin = child.stdin.take().ok_or_else(|| {
            SparkClientError::BridgeUnavailable("bridge stdin was not piped".to_string())
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            SparkClientError::BridgeUnavailable("bridge stdout was not piped".to_string())
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            SparkClientError::BridgeUnavailable("bridge stderr was not piped".to_string())
        })?;

        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let (request_tx, request_rx) = mpsc::unbounded_channel::<Request>();
        // Buffer 64 events — at the bridge's event rate (one per SDK
        // sync tick + one per payment state change) that's several
        // minutes of headroom even if a subscriber is paused.
        let (event_tx, _) = broadcast::channel::<Event>(64);

        let closed: ClosedFlag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let late_outcomes: LateOutcomes = Arc::new(Mutex::new(HashMap::new()));
        let awaiting_late: AwaitingLate = Arc::new(Mutex::new(std::collections::HashSet::new()));

        spawn_writer_task(stdin, request_rx, Arc::clone(&pending), Arc::clone(&closed));
        spawn_reader_task(
            stdout,
            Arc::clone(&pending),
            event_tx.clone(),
            Arc::clone(&closed),
            Arc::clone(&late_outcomes),
            Arc::clone(&awaiting_late),
        );
        spawn_stderr_task(stderr);

        let inner = Arc::new(SparkClientInner {
            next_id: AtomicU64::new(1),
            request_tx,
            pending,
            late_outcomes,
            awaiting_late,
            event_tx,
            child: Mutex::new(Some(child)),
            closed,
        });
        let client = Self { inner };

        // Perform the init handshake. If this fails, drop the client so
        // the child process is killed via `kill_on_drop`.
        let init_params = InitParams {
            api_key: config.api_key,
            network: config.network,
            mnemonic: mnemonic.to_string(),
            mnemonic_passphrase: None,
            storage_dir: config
                .storage_dir
                .to_str()
                .ok_or_else(|| {
                    SparkClientError::Config(
                        "Spark storage_dir contains non-UTF-8 bytes".to_string(),
                    )
                })?
                .to_string(),
        };

        match client.request(Method::Init(init_params)).await? {
            OkPayload::Init {} => Ok(client),
            other => Err(SparkClientError::Protocol(format!(
                "init returned unexpected payload: {:?}",
                other
            ))),
        }
    }

    /// Fetch wallet info (balance, identity pubkey).
    pub async fn get_info(&self) -> Result<GetInfoOk, SparkClientError> {
        match self
            .request(Method::GetInfo(GetInfoParams {
                ensure_synced: Some(true),
            }))
            .await?
        {
            OkPayload::GetInfo(info) => Ok(info),
            other => Err(SparkClientError::Protocol(format!(
                "get_info returned unexpected payload: {:?}",
                other
            ))),
        }
    }

    /// List recent payments.
    pub async fn list_payments(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<ListPaymentsOk, SparkClientError> {
        match self
            .request(Method::ListPayments(ListPaymentsParams { limit, offset }))
            .await?
        {
            OkPayload::ListPayments(list) => Ok(list),
            other => Err(SparkClientError::Protocol(format!(
                "list_payments returned unexpected payload: {:?}",
                other
            ))),
        }
    }

    /// Phase 4e: classify a user-supplied destination string.
    ///
    /// Calls `BreezSdk::parse` on the bridge side and returns a
    /// high-level [`ParseInputOk`] tag the gui can branch on. The Send
    /// panel uses this before `prepare_send` to route LNURL /
    /// Lightning-address inputs to [`Self::prepare_lnurl_pay`].
    pub async fn parse_input(&self, input: String) -> Result<ParseInputOk, SparkClientError> {
        match self
            .request(Method::ParseInput(ParseInputParams { input }))
            .await?
        {
            OkPayload::ParseInput(ok) => Ok(ok),
            other => Err(SparkClientError::Protocol(format!(
                "parse_input returned unexpected payload: {:?}",
                other
            ))),
        }
    }

    /// Phase 4e: prepare an LNURL-pay / Lightning-address send.
    ///
    /// Companion to [`Self::prepare_send`] for the LNURL code path.
    /// Returns the same [`PrepareSendOk`] shape so the gui's state
    /// machine doesn't need a parallel send branch — the bridge
    /// remembers which pending map the handle belongs to and dispatches
    /// to `sdk.lnurl_pay` vs `sdk.send_payment` transparently when the
    /// gui calls [`Self::send_payment`] with the handle.
    ///
    /// `amount_sat` is required (LNURL servers always specify a
    /// min/max range). `comment` is forwarded if the server allows
    /// comments.
    pub async fn prepare_lnurl_pay(
        &self,
        input: String,
        amount_sat: u64,
        comment: Option<String>,
    ) -> Result<PrepareSendOk, SparkClientError> {
        match self
            .request(Method::PrepareLnurlPay(PrepareLnurlPayParams {
                input,
                amount_sat,
                comment,
            }))
            .await?
        {
            OkPayload::PrepareSend(ok) => Ok(ok),
            other => Err(SparkClientError::Protocol(format!(
                "prepare_lnurl_pay returned unexpected payload: {:?}",
                other
            ))),
        }
    }

    /// Phase 4c: parse a destination + compute a send preview.
    ///
    /// `input` accepts BOLT11 invoices, BIP21 URIs, and on-chain Bitcoin
    /// addresses. LNURL / Lightning Address destinations should go
    /// through [`Self::parse_input`] + [`Self::prepare_lnurl_pay`]
    /// instead — `prepare_send` rejects them at the SDK level.
    ///
    /// `amount_sat` is required for amountless invoices and on-chain
    /// sends; ignored otherwise. Returns a [`PrepareSendOk`] whose
    /// `handle` must be echoed back to [`Self::send_payment`] to
    /// execute the send. The bridge holds the full SDK prepare
    /// response under that key — the handle is single-use.
    pub async fn prepare_send(
        &self,
        input: String,
        amount_sat: Option<u64>,
    ) -> Result<PrepareSendOk, SparkClientError> {
        match self
            .request(Method::PrepareSend(PrepareSendParams { input, amount_sat }))
            .await?
        {
            OkPayload::PrepareSend(prepare) => Ok(prepare),
            other => Err(SparkClientError::Protocol(format!(
                "prepare_send returned unexpected payload: {:?}",
                other
            ))),
        }
    }

    /// Phase 4c: execute a previously-prepared send.
    ///
    /// `prepare_handle` must come from a prior [`Self::prepare_send`]
    /// response. The bridge consumes it on success and on a failed
    /// bitcoin-rail send; a failed *cross-chain* send instead keeps it
    /// re-sendable, so a retry can re-submit the same quote (same provider swap
    /// id). Reusing a consumed handle returns a
    /// [`SparkClientError::BridgeError`] with [`ErrorKind::BadRequest`].
    ///
    /// `idempotency_key` makes a retry safe **only on the plain bitcoin rails**
    /// (Lightning / on-chain / Spark), where the SDK honours it and
    /// short-circuits a repeat send carrying the same key. The bridge **drops
    /// it for any send with a token or conversion leg** — the SDK rejects it
    /// there — which is *every* cross-chain (USDt/USDC) send, BTC-funded
    /// included. Don't rely on this key for a cross-chain retry; that safety
    /// comes from re-sending the same quote, gated by
    /// [`CrossChainQuote::retry_safe`]. Pass `None` only where a retry can't
    /// happen.
    ///
    /// This is a **state-changing** request, so it does not inherit the generic
    /// query timeout: losing the transport after dispatch yields
    /// [`SparkClientError::OutcomeUnknown`], never a failure. See
    /// [`Self::request_with`].
    pub async fn send_payment(
        &self,
        prepare_handle: String,
        idempotency_key: Option<String>,
    ) -> Result<SendPaymentOk, SparkClientError> {
        match self
            .request_with(
                Method::SendPayment(SendPaymentParams {
                    prepare_handle,
                    idempotency_key,
                }),
                RequestKind::StateChanging,
            )
            .await?
        {
            OkPayload::SendPayment(sent) => Ok(sent),
            other => Err(SparkClientError::Protocol(format!(
                "send_payment returned unexpected payload: {:?}",
                other
            ))),
        }
    }

    /// Classify a destination as a cross-chain address and list the routes
    /// that can reach it.
    ///
    /// A non-cross-chain input (BOLT11, Spark address, on-chain) is **not** an
    /// error — it comes back with `address: None`, which is how the Send panel
    /// decides between the normal and cross-chain flows. An `address` with an
    /// empty `routes` means the address parsed but nothing can currently reach
    /// it, which the panel must surface rather than silently falling back.
    pub async fn get_cross_chain_routes(
        &self,
        input: String,
    ) -> Result<CrossChainRoutesOk, SparkClientError> {
        match self
            .request(Method::GetCrossChainRoutes(GetCrossChainRoutesParams {
                input,
            }))
            .await?
        {
            OkPayload::GetCrossChainRoutes(routes) => Ok(routes),
            other => Err(SparkClientError::Protocol(format!(
                "get_cross_chain_routes returned unexpected payload: {:?}",
                other
            ))),
        }
    }

    /// Prepare a cross-chain send along a route from
    /// [`Self::get_cross_chain_routes`]. The returned [`PrepareSendOk`] carries
    /// the quote in its `cross_chain` field (amount out, fees, expiry), and its
    /// handle is executed by the ordinary [`Self::send_payment`].
    ///
    /// `destination` must be the [`CrossChainAddress`] returned by
    /// [`Self::get_cross_chain_routes`], echoed back **whole** — not just its
    /// address string. The bridge rebuilds the SDK's address details from it to
    /// re-resolve the route, and a URI destination's `contract_address` /
    /// `chain_id` only survive if they're carried along.
    ///
    /// `max_slippage_bps` must be in `10..=500`; `None` takes the SDK's 100 bps
    /// default.
    pub async fn prepare_cross_chain(
        &self,
        destination: CrossChainAddress,
        route: CrossChainRoute,
        amount_sat: u64,
        max_slippage_bps: Option<u32>,
    ) -> Result<PrepareSendOk, SparkClientError> {
        match self
            .request(Method::PrepareCrossChain(PrepareCrossChainParams {
                destination,
                route,
                amount_sat,
                max_slippage_bps,
            }))
            .await?
        {
            OkPayload::PrepareSend(prepare) => Ok(prepare),
            other => Err(SparkClientError::Protocol(format!(
                "prepare_cross_chain returned unexpected payload: {:?}",
                other
            ))),
        }
    }

    /// Phase 4c: generate a BOLT11 invoice.
    ///
    /// `amount_sat = None` produces an amountless invoice. `description`
    /// is shown to the payer's wallet. `expiry_secs = None` defers to
    /// the SDK default (typically 24h).
    pub async fn receive_bolt11(
        &self,
        amount_sat: Option<u64>,
        description: String,
        expiry_secs: Option<u32>,
    ) -> Result<ReceivePaymentOk, SparkClientError> {
        match self
            .request(Method::ReceiveBolt11(ReceiveBolt11Params {
                amount_sat,
                description,
                expiry_secs,
            }))
            .await?
        {
            OkPayload::ReceivePayment(resp) => Ok(resp),
            other => Err(SparkClientError::Protocol(format!(
                "receive_bolt11 returned unexpected payload: {:?}",
                other
            ))),
        }
    }

    /// Phase 4c: generate an on-chain Bitcoin deposit address.
    ///
    /// Note: Spark's on-chain receive model requires a separate
    /// `claim_deposit` call once the incoming tx has confirmed —
    /// that's Phase 4d work. Phase 4c just returns the address and
    /// trusts the user / background sync to complete the claim
    /// eventually.
    pub async fn receive_onchain(
        &self,
        new_address: Option<bool>,
    ) -> Result<ReceivePaymentOk, SparkClientError> {
        match self
            .request(Method::ReceiveOnchain(ReceiveOnchainParams { new_address }))
            .await?
        {
            OkPayload::ReceivePayment(resp) => Ok(resp),
            other => Err(SparkClientError::Protocol(format!(
                "receive_onchain returned unexpected payload: {:?}",
                other
            ))),
        }
    }

    /// Generate a static, reusable Spark address for native
    /// Spark-to-Spark transfers. No amount/invoice — the address is
    /// identity-bound and token-agnostic, with zero receive fee.
    pub async fn receive_spark(&self) -> Result<ReceivePaymentOk, SparkClientError> {
        match self.request(Method::ReceiveSpark).await? {
            OkPayload::ReceivePayment(resp) => Ok(resp),
            other => Err(SparkClientError::Protocol(format!(
                "receive_spark returned unexpected payload: {:?}",
                other
            ))),
        }
    }

    /// Phase 4f: list on-chain deposits the SDK has noticed but not
    /// yet claimed into the Spark wallet. Drives the "Pending
    /// deposits" card in the Receive panel.
    pub async fn list_unclaimed_deposits(
        &self,
    ) -> Result<ListUnclaimedDepositsOk, SparkClientError> {
        match self.request(Method::ListUnclaimedDeposits).await? {
            OkPayload::ListUnclaimedDeposits(resp) => Ok(resp),
            other => Err(SparkClientError::Protocol(format!(
                "list_unclaimed_deposits returned unexpected payload: {:?}",
                other
            ))),
        }
    }

    /// Phase 4f: claim a specific (txid, vout) deposit into the Spark
    /// wallet. Returns the resulting payment id + claimed amount.
    /// Fails with [`SparkClientError::BridgeError`] / [`ErrorKind::Sdk`]
    /// when the deposit isn't mature yet — the gui should gate the
    /// Claim button on the deposit's `is_mature` field to avoid
    /// firing pre-mature claims.
    pub async fn claim_deposit(
        &self,
        txid: String,
        vout: u32,
    ) -> Result<ClaimDepositOk, SparkClientError> {
        match self
            .request(Method::ClaimDeposit(ClaimDepositParams { txid, vout }))
            .await?
        {
            OkPayload::ClaimDeposit(resp) => Ok(resp),
            other => Err(SparkClientError::Protocol(format!(
                "claim_deposit returned unexpected payload: {:?}",
                other
            ))),
        }
    }

    /// Phase 6: read the SDK's `UserSettings` (Stable Balance on/off,
    /// private mode). Boolean-flattened on the bridge side so the gui
    /// never sees the USDB token label.
    pub async fn get_user_settings(&self) -> Result<GetUserSettingsOk, SparkClientError> {
        match self.request(Method::GetUserSettings).await? {
            OkPayload::GetUserSettings(resp) => Ok(resp),
            other => Err(SparkClientError::Protocol(format!(
                "get_user_settings returned unexpected payload: {:?}",
                other
            ))),
        }
    }

    /// Phase 6: activate (enabled=true) or deactivate (false) the
    /// Stable Balance feature. The bridge translates this into
    /// `update_user_settings(stable_balance_active_label = ...)`.
    pub async fn set_stable_balance(&self, enabled: bool) -> Result<(), SparkClientError> {
        match self
            .request(Method::SetStableBalance(SetStableBalanceParams { enabled }))
            .await?
        {
            OkPayload::SetStableBalance {} => Ok(()),
            other => Err(SparkClientError::Protocol(format!(
                "set_stable_balance returned unexpected payload: {:?}",
                other
            ))),
        }
    }

    /// Subscribe to bridge [`Event`] frames. Each call returns a fresh
    /// `broadcast::Receiver` — each subscriber gets its own independent
    /// cursor over the buffered events. The [`iced::Subscription`]
    /// helper below wraps this into an iced subscription stream.
    pub fn subscribe_events(&self) -> broadcast::Receiver<Event> {
        self.inner.event_tx.subscribe()
    }

    /// Build an iced [`Subscription`](iced::Subscription) over the
    /// bridge's event stream. Fires a [`SparkClientEvent`] every time
    /// the bridge forwards an SDK event, and silently resumes when a
    /// subscriber lags.
    ///
    /// The state parameter hashes on a per-client identity (the
    /// `event_tx` pointer) so swapping out the SparkClient on a
    /// reconnect produces a fresh subscription instead of re-binding
    /// to the old channel.
    pub fn event_subscription(&self) -> iced::Subscription<SparkClientEvent> {
        iced::Subscription::run_with(
            SparkEventSubscriptionState {
                client: self.clone(),
            },
            make_spark_event_stream,
        )
    }

    /// Phase 4g: debounced availability hint for the claim-flow UI.
    ///
    /// Thin round-trip to the Breez-hosted LNURL server. Callers use
    /// this only as a UX nicety (green ✓ / red ✗ while typing); our
    /// API's reserve endpoint still does the authoritative uniqueness
    /// check at claim time.
    pub async fn check_lightning_address_available(
        &self,
        username: String,
    ) -> Result<bool, SparkClientError> {
        match self
            .request(Method::CheckLightningAddressAvailable(
                CheckLightningAddressAvailableParams { username },
            ))
            .await?
        {
            OkPayload::CheckLightningAddressAvailable(ok) => Ok(ok.available),
            other => Err(SparkClientError::Protocol(format!(
                "check_lightning_address_available returned unexpected payload: {:?}",
                other
            ))),
        }
    }

    /// Phase 4g: bind `<username>@<lnurl_domain>` to this wallet's
    /// Spark leaf on the Breez-hosted LNURL server.
    ///
    /// Call after the API's reserve endpoint has returned 2xx — the
    /// Go API persists the record and the `confirm` step stamps it
    /// permanent, but only after the SDK registration succeeds. On
    /// any failure here the caller must release the reservation via
    /// the API's DELETE endpoint.
    pub async fn register_lightning_address(
        &self,
        username: String,
        description: Option<String>,
    ) -> Result<LightningAddressInfo, SparkClientError> {
        match self
            .request(Method::RegisterLightningAddress(
                RegisterLightningAddressParams {
                    username,
                    description,
                },
            ))
            .await?
        {
            OkPayload::RegisterLightningAddress(ok) => Ok(ok.info),
            other => Err(SparkClientError::Protocol(format!(
                "register_lightning_address returned unexpected payload: {:?}",
                other
            ))),
        }
    }

    /// Phase 4g: fetch the Lightning Address currently bound to this
    /// wallet on the Breez LNURL server (falling back to a server-side
    /// recovery when the SDK's local cache is empty). Drives the
    /// startup auto-reconcile — see the App's Spark-connect handler.
    pub async fn get_lightning_address(
        &self,
    ) -> Result<Option<LightningAddressInfo>, SparkClientError> {
        match self.request(Method::GetLightningAddress).await? {
            OkPayload::GetLightningAddress(ok) => Ok(ok.info),
            other => Err(SparkClientError::Protocol(format!(
                "get_lightning_address returned unexpected payload: {:?}",
                other
            ))),
        }
    }

    /// Phase 4g: unregister the Lightning Address bound to this
    /// wallet. Idempotent at the SDK level — returns Ok even when no
    /// address is currently registered.
    pub async fn delete_lightning_address(&self) -> Result<(), SparkClientError> {
        match self.request(Method::DeleteLightningAddress).await? {
            OkPayload::DeleteLightningAddress {} => Ok(()),
            other => Err(SparkClientError::Protocol(format!(
                "delete_lightning_address returned unexpected payload: {:?}",
                other
            ))),
        }
    }

    /// Gracefully shut down the bridge subprocess. After this returns
    /// the client is no longer usable.
    pub async fn shutdown(&self) -> Result<(), SparkClientError> {
        if self.inner.closed.swap(true, Ordering::SeqCst) {
            return Ok(());
        }

        // Best-effort: send Shutdown and wait up to 5s for the child
        // to exit, otherwise kill it.
        let shutdown_result =
            tokio::time::timeout(Duration::from_secs(5), self.request(Method::Shutdown)).await;
        match shutdown_result {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => warn!("Spark bridge shutdown RPC failed: {}", e),
            Err(_) => warn!("Spark bridge shutdown RPC timed out"),
        }

        let mut guard = self.inner.child.lock().await;
        if let Some(mut child) = guard.take() {
            match tokio::time::timeout(Duration::from_secs(2), child.wait()).await {
                Ok(Ok(status)) => debug!("Spark bridge exited with status {}", status),
                Ok(Err(e)) => warn!("failed to wait() for Spark bridge: {}", e),
                Err(_) => {
                    warn!("Spark bridge did not exit within 2s, killing");
                    let _ = child.kill().await;
                }
            }
        }
        Ok(())
    }

    /// Send a request and await its response.
    ///
    /// Wires up an oneshot channel in the pending map keyed by a fresh
    /// monotonic id, pushes the [`Request`] through the writer channel,
    /// and awaits the oneshot. Any error response is translated into
    /// [`SparkClientError::BridgeError`].
    async fn request(&self, method: Method) -> Result<OkPayload, SparkClientError> {
        self.request_with(method, RequestKind::Query).await
    }

    /// Send a request and await its response under a per-method policy.
    ///
    /// The policy decides two things at once, and they belong together: how
    /// long to wait, and what a missing answer *means*. A query that goes
    /// unanswered changed nothing and is reported as unavailable. A **send**
    /// that goes unanswered has already been handed to the bridge, so the only
    /// honest report is [`SparkClientError::OutcomeUnknown`] — including when
    /// the bridge dies mid-send, which says nothing about whether the SDK
    /// finished the payment first.
    async fn request_with(
        &self,
        method: Method,
        kind: RequestKind,
    ) -> Result<OkPayload, SparkClientError> {
        // Allow Shutdown through even after closed is set — shutdown()
        // flips the flag first to block new RPCs, then sends the
        // Shutdown request itself. Every other method is rejected once
        // closed is true.
        if !matches!(method, Method::Shutdown) && self.inner.closed.load(Ordering::SeqCst) {
            return Err(SparkClientError::BridgeUnavailable(
                "Spark client has been shut down".to_string(),
            ));
        }

        let id = self.inner.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel::<Response>();
        self.inner.pending.lock().await.insert(id, tx);

        let request = Request { id, method };
        if self.inner.request_tx.send(request).is_err() {
            // Writer task exited before this was handed over. Nothing was
            // dispatched, so this is a definite failure even for a send.
            self.inner.pending.lock().await.remove(&id);
            return Err(SparkClientError::BridgeUnavailable(
                "Spark bridge writer task exited".to_string(),
            ));
        }

        let deadline = match kind {
            RequestKind::Query => QUERY_TIMEOUT,
            RequestKind::StateChanging => SEND_SOFT_DEADLINE,
        };

        let response = match tokio::time::timeout(deadline, rx).await {
            Ok(Ok(resp)) => resp,
            // The reader closed the channel: the bridge is gone. For a query
            // that is a plain failure; for a send in flight it is unknown.
            Ok(Err(_)) => {
                self.inner.pending.lock().await.remove(&id);
                return Err(match kind {
                    RequestKind::Query => SparkClientError::BridgeUnavailable(
                        "Spark bridge reader closed the response channel".to_string(),
                    ),
                    RequestKind::StateChanging => SparkClientError::OutcomeUnknown {
                        request_id: id,
                        message: "The Spark bridge stopped responding after the payment \
                                  was sent to it."
                            .to_string(),
                    },
                });
            }
            Err(_) => {
                self.inner.pending.lock().await.remove(&id);
                return Err(match kind {
                    RequestKind::Query => SparkClientError::BridgeUnavailable(format!(
                        "Spark bridge did not respond within {}s (id={})",
                        QUERY_TIMEOUT.as_secs(),
                        id
                    )),
                    RequestKind::StateChanging => {
                        // Keep listening: the bridge still owes an answer, and
                        // when it arrives it is the authoritative one.
                        self.inner.awaiting_late.lock().await.insert(id);
                        SparkClientError::OutcomeUnknown {
                            request_id: id,
                            message: format!(
                                "The Spark bridge did not answer within {}s of being \
                                 given this payment.",
                                SEND_SOFT_DEADLINE.as_secs()
                            ),
                        }
                    }
                });
            }
        };

        match response.result {
            ResponseResult::Ok(payload) => Ok(payload),
            ResponseResult::Err(ErrorPayload { kind, message }) => {
                Err(SparkClientError::BridgeError { kind, message })
            }
        }
    }

    /// Take the bridge's answer to a send whose caller gave up waiting, if it
    /// has arrived since.
    ///
    /// This is the first thing reconciliation should consult after an
    /// [`SparkClientError::OutcomeUnknown`]: it is the bridge's own verdict on
    /// that exact request, which beats any inference drawn from payment
    /// history. `None` means the bridge still has not answered.
    pub async fn take_late_outcome(
        &self,
        request_id: u64,
    ) -> Option<Result<SendPaymentOk, SparkClientError>> {
        let response = self.inner.late_outcomes.lock().await.remove(&request_id)?;
        self.inner.awaiting_late.lock().await.remove(&request_id);
        Some(match response.result {
            ResponseResult::Ok(OkPayload::SendPayment(sent)) => Ok(sent),
            ResponseResult::Ok(other) => Err(SparkClientError::Protocol(format!(
                "late send outcome carried unexpected payload: {:?}",
                other
            ))),
            ResponseResult::Err(ErrorPayload { kind, message }) => {
                Err(SparkClientError::BridgeError { kind, message })
            }
        })
    }
}

// Drop is implemented on `SparkClientInner` (not `SparkClient`)
// because `SparkClient` is `Clone` — panels and subscription
// descriptors create short-lived clones that are discarded
// frequently. If Drop were on `SparkClient`, every clone drop
// would kill the bridge. Putting it on the inner struct behind
// `Arc` means it fires exactly once, when the last strong
// reference is released.
impl Drop for SparkClientInner {
    fn drop(&mut self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        let _ = self.request_tx.send(Request {
            id: u64::MAX,
            method: Method::Shutdown,
        });
    }
}

// ---------------------------------------------------------------------------
// Bridge binary discovery
// ---------------------------------------------------------------------------

/// Locate the `coincube-spark-bridge` executable.
///
/// Precedence:
/// 1. `COINCUBE_SPARK_BRIDGE_PATH` env var (absolute path override).
/// 2. Sibling of the current executable, for packaged builds.
/// 3. Workspace `target/debug` / `target/release`, for `cargo run`.
fn resolve_bridge_path() -> Result<PathBuf, SparkClientError> {
    if let Ok(override_path) = std::env::var("COINCUBE_SPARK_BRIDGE_PATH") {
        let p = PathBuf::from(override_path);
        if p.exists() {
            return Ok(p);
        }
        return Err(SparkClientError::BridgeUnavailable(format!(
            "COINCUBE_SPARK_BRIDGE_PATH={} does not exist",
            p.display()
        )));
    }

    let exe_name = if cfg!(windows) {
        "coincube-spark-bridge.exe"
    } else {
        "coincube-spark-bridge"
    };

    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(dir) = current_exe.parent() {
            let sibling = dir.join(exe_name);
            if sibling.exists() {
                return Ok(sibling);
            }
        }
    }

    // Dev fallback: look relative to the workspace so `cargo run` works
    // out of the box without copying the bridge binary.
    let workspace_root = env!("CARGO_MANIFEST_DIR");
    for profile in ["debug", "release"] {
        let candidate = PathBuf::from(workspace_root)
            .join("..")
            .join("coincube-spark-bridge")
            .join("target")
            .join(profile)
            .join(exe_name);
        if candidate.exists() {
            return Ok(candidate);
        }
    }

    Err(SparkClientError::BridgeUnavailable(format!(
        "could not locate {} — set COINCUBE_SPARK_BRIDGE_PATH or run `cargo build \
         --manifest-path coincube-spark-bridge/Cargo.toml` first",
        exe_name
    )))
}

// ---------------------------------------------------------------------------
// Background tasks
// ---------------------------------------------------------------------------

fn spawn_writer_task(
    mut stdin: tokio::process::ChildStdin,
    mut request_rx: mpsc::UnboundedReceiver<Request>,
    pending: PendingMap,
    closed: ClosedFlag,
) {
    tokio::spawn(async move {
        while let Some(request) = request_rx.recv().await {
            let frame = Frame::Request(request);
            let line = match serde_json::to_string(&frame) {
                Ok(s) => s,
                Err(e) => {
                    error!("failed to serialize Spark bridge request: {}", e);
                    break;
                }
            };
            if stdin.write_all(line.as_bytes()).await.is_err()
                || stdin.write_all(b"\n").await.is_err()
                || stdin.flush().await.is_err()
            {
                warn!("Spark bridge writer: stdin closed");
                break;
            }
        }

        // Mark client as closed and drain pending requests so callers
        // fail fast instead of waiting for the full timeout.
        closed.store(true, Ordering::SeqCst);
        let mut map = pending.lock().await;
        if !map.is_empty() {
            warn!(
                "Spark bridge writer exited with {} pending request(s) — failing them",
                map.len()
            );
            for (id, sender) in map.drain() {
                let _ = sender.send(Response {
                    id,
                    result: ResponseResult::Err(ErrorPayload {
                        kind: ErrorKind::NotConnected,
                        message: "Spark bridge writer failed — stdin broken or serialization error"
                            .to_string(),
                    }),
                });
            }
        }
    });
}

fn spawn_reader_task(
    stdout: tokio::process::ChildStdout,
    pending: PendingMap,
    event_tx: broadcast::Sender<Event>,
    closed: ClosedFlag,
    late_outcomes: LateOutcomes,
    awaiting_late: AwaitingLate,
) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    if line.trim().is_empty() {
                        continue;
                    }
                    let frame: Frame = match serde_json::from_str(&line) {
                        Ok(f) => f,
                        Err(e) => {
                            error!(
                                "Spark bridge protocol error — unparseable line: {} ({})",
                                line, e
                            );
                            break;
                        }
                    };
                    match frame {
                        Frame::Response(resp) => {
                            let waiter = pending.lock().await.remove(&resp.id);
                            let unresolved = awaiting_late.lock().await.contains(&resp.id);
                            match (route_response(waiter.is_some(), unresolved), waiter) {
                                (ResponseRoute::Deliver, Some(sender)) => {
                                    let _ = sender.send(resp);
                                }
                                (ResponseRoute::HoldForReconciliation, _) => {
                                    // The late answer to a send whose caller
                                    // stopped waiting. This is the
                                    // authoritative word on whether that
                                    // payment happened, so it is kept for
                                    // reconciliation instead of dropped.
                                    warn!(
                                        "Spark bridge answered send id {} after its deadline \
                                         — holding the outcome for reconciliation",
                                        resp.id
                                    );
                                    late_outcomes.lock().await.insert(resp.id, resp);
                                }
                                (_, _) => {
                                    warn!(
                                        "Spark bridge response for unknown id {} — dropping",
                                        resp.id
                                    );
                                }
                            }
                        }
                        Frame::Event(event) => {
                            debug!("Spark bridge event: {:?}", event);
                            let _ = event_tx.send(event);
                        }
                        Frame::Request(_) => {
                            warn!("Spark bridge sent a Request frame — ignoring");
                        }
                    }
                }
                Ok(None) => {
                    debug!("Spark bridge stdout closed");
                    break;
                }
                Err(e) => {
                    warn!("Spark bridge stdout read error: {}", e);
                    break;
                }
            }
        }

        // Bridge is gone — mark the client as closed so new RPCs
        // fail immediately, then drain any in-flight requests so
        // callers don't hang for the full 30s timeout.
        closed.store(true, Ordering::SeqCst);
        let mut map = pending.lock().await;
        if !map.is_empty() {
            warn!(
                "Spark bridge reader exited with {} pending request(s) — failing them",
                map.len()
            );
            for (id, sender) in map.drain() {
                let _ = sender.send(Response {
                    id,
                    result: coincube_spark_protocol::ResponseResult::Err(
                        coincube_spark_protocol::ErrorPayload {
                            kind: ErrorKind::NotConnected,
                            message: "Spark bridge subprocess exited unexpectedly".to_string(),
                        },
                    ),
                });
            }
        }
    });
}

fn spawn_stderr_task(stderr: tokio::process::ChildStderr) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            relay_bridge_log(&line);
        }
    });
}

/// Relays one stderr line from the Spark bridge subprocess into our tracing at
/// the severity the bridge ITSELF emitted, instead of blanket-`warn!`. Blanket
/// warn made the bridge's own `INFO`/`DEBUG` show up as warnings and — together
/// with the Spark SDK's keepalive chatter — flooded `coincube.log` with a WARN
/// every few seconds. ANSI colour codes the bridge writes are stripped, and the
/// benign empty-event heartbeat (the Spark server stream's ~5s keepalive, which
/// carries no signal) is demoted to TRACE so it can't drown the log.
fn relay_bridge_log(raw: &str) {
    let line = strip_ansi(raw);
    let line = line.trim();
    if line.is_empty() {
        return;
    }
    // High-frequency, zero-signal keepalive from the SDK's event stream.
    if line.contains("Received empty event") {
        trace!(target: "spark_bridge", "{line}");
        return;
    }
    match embedded_level(line) {
        Some(Level::ERROR) => error!(target: "spark_bridge", "{line}"),
        Some(Level::WARN) => warn!(target: "spark_bridge", "{line}"),
        Some(Level::INFO) => info!(target: "spark_bridge", "{line}"),
        Some(Level::DEBUG) => debug!(target: "spark_bridge", "{line}"),
        Some(Level::TRACE) => trace!(target: "spark_bridge", "{line}"),
        // Unstructured output (e.g. a panic or raw stderr) — keep it visible.
        None => warn!(target: "spark_bridge", "{line}"),
    }
}

/// Detects the tracing level token the bridge embedded in its formatted line
/// (e.g. `<timestamp>  INFO spark::…: …`). The level is a whitespace-delimited
/// field that always precedes the `target: message` separator, so we only scan
/// the prefix before the first `": "` and require an *exact* token match. This
/// avoids false positives when the message payload itself contains a word like
/// `INFO` or ` ERROR ` (which a plain `contains()` over the whole line would
/// mis-detect, especially as ERROR is checked first).
fn embedded_level(line: &str) -> Option<Level> {
    // Everything up to the `target: message` delimiter. The message (which may
    // contain incidental level words) lives after it and is excluded.
    let prefix = match line.find(": ") {
        Some(i) => &line[..i],
        None => line,
    };
    prefix.split_whitespace().find_map(|tok| match tok {
        "ERROR" => Some(Level::ERROR),
        "WARN" => Some(Level::WARN),
        "INFO" => Some(Level::INFO),
        "DEBUG" => Some(Level::DEBUG),
        "TRACE" => Some(Level::TRACE),
        _ => None,
    })
}

/// Strips ANSI SGR escape sequences (the bridge colourises its tracing output,
/// which would otherwise land as raw escape codes in our log file).
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // Consume up to and including the sequence's terminating letter.
            for c2 in chars.by_ref() {
                if c2.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum SparkClientError {
    /// Missing / unparseable config (API key, storage dir).
    Config(String),
    /// Bridge subprocess couldn't be started or died unexpectedly.
    BridgeUnavailable(String),
    /// Bridge returned an error response for a request. A **definite**
    /// rejection: the bridge answered, and the answer was "no".
    BridgeError { kind: ErrorKind, message: String },
    /// A state-changing request was dispatched and no answer came back.
    ///
    /// **Not a failure.** The bridge has the request and the SDK may already
    /// have moved the money; the transport simply stopped telling us. Callers
    /// must reconcile — [`SparkClient::take_late_outcome`] first, then the
    /// payment history — before offering to send anything again, and must never
    /// render this as "the payment failed".
    OutcomeUnknown {
        /// The request id, for [`SparkClient::take_late_outcome`].
        request_id: u64,
        message: String,
    },
    /// JSON-RPC framing error (malformed response, unexpected payload).
    Protocol(String),
}

impl std::fmt::Display for SparkClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Config(msg) => write!(f, "Spark config error: {}", msg),
            Self::BridgeUnavailable(msg) => {
                write!(f, "Spark bridge subprocess unavailable: {}", msg)
            }
            Self::BridgeError { kind, message } => {
                write!(f, "Spark bridge returned {:?}: {}", kind, message)
            }
            Self::OutcomeUnknown { message, .. } => {
                write!(f, "Spark payment outcome unknown: {}", message)
            }
            Self::Protocol(msg) => write!(f, "Spark protocol error: {}", msg),
        }
    }
}

impl std::error::Error for SparkClientError {}

// ---------------------------------------------------------------------------
// Iced subscription for bridge events
// ---------------------------------------------------------------------------

/// Domain wrapper around [`Event`] so the app-level [`crate::app::Message`]
/// doesn't need to depend on the protocol crate directly.
///
/// Phase 4d just forwards the protocol variant as-is (zero translation
/// cost). Phase 4e / 5 can promote this to a typed enum if panels
/// start branching on event-specific data.
#[derive(Debug, Clone)]
pub struct SparkClientEvent(pub Event);

/// Subscription identity — hashes on the broadcast sender's pointer
/// so a fresh `SparkClient` (e.g. after the user re-unlocks a cube)
/// produces a brand-new subscription instead of reusing the old one.
struct SparkEventSubscriptionState {
    client: SparkClient,
}

impl std::hash::Hash for SparkEventSubscriptionState {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        let ptr = Arc::as_ptr(&self.client.inner) as usize;
        ptr.hash(state);
    }
}

/// Build the iced [`Stream`](iced::futures::Stream) that drains the
/// broadcast channel into iced's runtime. Uses `iced::stream::channel`
/// with a 100-slot buffer mirroring the Liquid subscription pattern.
fn make_spark_event_stream(
    state: &SparkEventSubscriptionState,
) -> impl iced::futures::Stream<Item = SparkClientEvent> {
    let client = state.client.clone();
    iced::stream::channel(
        100,
        move |mut output: iced::futures::channel::mpsc::Sender<SparkClientEvent>| async move {
            let mut receiver = client.subscribe_events();
            loop {
                match receiver.recv().await {
                    Ok(event) => {
                        use iced::futures::SinkExt;
                        if output.send(SparkClientEvent(event)).await.is_err() {
                            // iced runtime dropped the sink — time to
                            // stop pumping events for this subscription.
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!(
                            "Spark event subscription lagged by {} events, resuming",
                            skipped
                        );
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        // Sender dropped — the SparkClient is gone.
                        // Park the task forever so iced keeps the
                        // Subscription id alive until the parent
                        // re-instantiates the state.
                        std::future::pending::<()>().await;
                        break;
                    }
                }
            }
            std::future::pending::<()>().await;
        },
    )
}

#[cfg(test)]
mod unknown_outcome_tests {
    use super::*;

    /// A send that outran its deadline still has an answer coming, and that
    /// answer is the most authoritative statement anyone has about whether the
    /// payment happened. Dropping it as an "unknown id" — which is what the
    /// reader used to do — throws that away.
    #[test]
    fn a_late_send_answer_is_kept_for_reconciliation_not_dropped() {
        // Caller still waiting: straight through, as always.
        assert_eq!(route_response(true, false), ResponseRoute::Deliver);
        assert_eq!(route_response(true, true), ResponseRoute::Deliver);
        // Caller gave up on a send: hold it.
        assert_eq!(
            route_response(false, true),
            ResponseRoute::HoldForReconciliation
        );
        // Nothing knows about this id: unchanged behaviour.
        assert_eq!(route_response(false, false), ResponseRoute::Drop);
    }

    /// A send must not inherit the query deadline, and the two deadlines mean
    /// different things — which is the whole point of splitting them.
    #[test]
    fn a_send_does_not_inherit_the_query_timeout() {
        assert!(
            SEND_SOFT_DEADLINE > QUERY_TIMEOUT,
            "a send must not be abandoned on the query timeout"
        );
        assert_ne!(RequestKind::Query, RequestKind::StateChanging);
    }

    /// An unknown outcome must not read as a failure anywhere it is rendered
    /// or logged.
    #[test]
    fn the_unknown_outcome_error_never_claims_the_payment_failed() {
        let err = SparkClientError::OutcomeUnknown {
            request_id: 9,
            message: "The Spark bridge did not answer within 120s of being given this \
                      payment."
                .to_string(),
        };
        let rendered = err.to_string().to_lowercase();
        assert!(rendered.contains("unknown"), "{}", rendered);
        for forbidden in ["failed", "rejected", "declined", "did not send"] {
            assert!(
                !rendered.contains(forbidden),
                "an unknown outcome says {:?}, which asserts something nobody knows: {}",
                forbidden,
                rendered
            );
        }
        // And it is distinguishable from a definite bridge rejection.
        let definite = SparkClientError::BridgeError {
            kind: coincube_spark_protocol::ErrorKind::Sdk,
            message: "invoice expired".to_string(),
        };
        assert_ne!(err.to_string(), definite.to_string());
    }
}

#[cfg(test)]
mod stderr_relay_tests {
    use super::{embedded_level, strip_ansi, SparkClientError};
    use coincube_spark_protocol::ErrorKind;
    use tracing::Level;

    #[test]
    fn parses_embedded_level() {
        assert_eq!(
            embedded_level("2026-06-13T07:08:38Z  INFO spark::services::tokens: ok"),
            Some(Level::INFO)
        );
        assert_eq!(
            embedded_level("ts  WARN spark::events::server_stream: x"),
            Some(Level::WARN)
        );
        assert_eq!(embedded_level("ts ERROR foo: boom"), Some(Level::ERROR));
        // Unstructured line (e.g. a panic) has no level token.
        assert_eq!(embedded_level("thread 'main' panicked at ..."), None);
    }

    #[test]
    fn ignores_level_words_in_message_payload() {
        // A real INFO line whose message mentions ERROR/WARN must stay INFO —
        // the payload (after `target: `) is not scanned.
        assert_eq!(
            embedded_level("ts  INFO spark::x: retrying after ERROR response"),
            Some(Level::INFO)
        );
        // No structured level field, only an incidental word in the message.
        assert_eq!(
            embedded_level("ts spark::x: user tapped the INFO button"),
            None
        );
    }

    #[test]
    fn embedded_level_requires_an_exact_uppercase_level_token() {
        assert_eq!(embedded_level("ts INFORMATION spark::x: ok"), None);
        assert_eq!(embedded_level("ts info spark::x: ok"), None);
        assert_eq!(embedded_level("ts WARN spark::x: ok"), Some(Level::WARN));
    }

    #[test]
    fn embedded_level_accepts_every_structured_level_before_the_message() {
        assert_eq!(
            embedded_level("ts TRACE spark::x: verbose"),
            Some(Level::TRACE)
        );
        assert_eq!(
            embedded_level("ts DEBUG spark::x: detail"),
            Some(Level::DEBUG)
        );
        assert_eq!(embedded_level("ts INFO spark::x: ok"), Some(Level::INFO));
        assert_eq!(
            embedded_level("ts WARN spark::x: careful"),
            Some(Level::WARN)
        );
        assert_eq!(
            embedded_level("ts ERROR spark::x: boom"),
            Some(Level::ERROR)
        );
    }

    #[test]
    fn embedded_level_ignores_level_like_punctuation_and_key_value_pairs() {
        assert_eq!(embedded_level("ts INFO:spark::x: ok"), None);
        assert_eq!(embedded_level("ts level=INFO spark::x: ok"), None);
        assert_eq!(embedded_level("ts [INFO] spark::x: ok"), None);
    }

    #[test]
    fn strips_ansi_colour_codes() {
        let raw =
            "\x1b[2m2026-06-13T07:08:38Z\x1b[0m \x1b[32m INFO\x1b[0m \x1b[2mspark::x\x1b[0m: ok";
        let clean = strip_ansi(raw);
        assert!(!clean.contains('\x1b'));
        assert!(clean.contains(" INFO "));
        assert!(clean.contains("spark::x"));
        // Level is still detectable post-strip.
        assert_eq!(embedded_level(&clean), Some(Level::INFO));
    }

    #[test]
    fn strips_multi_parameter_ansi_sequences_without_touching_text() {
        let clean = strip_ansi("plain \x1b[31;1mERROR\x1b[0m spark::x: boom");
        assert_eq!(clean, "plain ERROR spark::x: boom");
        assert_eq!(embedded_level(&clean), Some(Level::ERROR));
    }

    #[test]
    fn client_errors_display_the_layer_that_failed() {
        assert_eq!(
            SparkClientError::Config("missing key".to_string()).to_string(),
            "Spark config error: missing key"
        );
        assert_eq!(
            SparkClientError::BridgeUnavailable("child exited".to_string()).to_string(),
            "Spark bridge subprocess unavailable: child exited"
        );
        assert_eq!(
            SparkClientError::BridgeError {
                kind: ErrorKind::NotConnected,
                message: "bridge offline".to_string(),
            }
            .to_string(),
            "Spark bridge returned NotConnected: bridge offline"
        );
        assert_eq!(
            SparkClientError::Protocol("wrong payload".to_string()).to_string(),
            "Spark protocol error: wrong payload"
        );
    }
}
