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
    AutoConvertState, CheckLightningAddressAvailableParams, ClaimDepositOk, ClaimDepositParams,
    ConversionLimits, ConversionQuote, CrossChainAddress, CrossChainRoute, CrossChainRoutesOk,
    ErrorKind, ErrorPayload, Event, Frame, GetConversionLimitsParams, GetCrossChainRoutesParams,
    GetInfoOk, GetInfoParams, GetUserSettingsOk, InitParams, LightningAddressInfo, ListPaymentsOk,
    ListPaymentsParams, ListUnclaimedDepositsOk, Method, OkPayload, ParseInputOk, ParseInputParams,
    PrepareCrossChainParams, PrepareLnurlPayFromParams, PrepareLnurlPayParams, PrepareSendFromOk,
    PrepareSendFromParams, PrepareSendOk, PrepareSendParams, PrepareSwapOk, PrepareSwapParams,
    PrepareUsdbSendParams, ReceiveBolt11Params, ReceiveOnchainParams, ReceivePaymentOk,
    RegisterLightningAddressParams, Request, Response, ResponseResult, SendPaymentOk,
    SendPaymentParams, SetAutoConvertParams, SetStableBalanceParams, SwapAmount, SwapOk,
    SwapParams, TokenPayment, WalletAsset, CONVERSION_MAX_SLIPPAGE_BPS,
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

/// `CREATE_NO_WINDOW`. The bridge is a console-subsystem binary, so a
/// windows-subsystem GUI that spawns it without this flag gets a visible
/// console — the stray terminal that appears when a Cube opens.
#[cfg(any(test, target_os = "windows"))]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// `DETACHED_PROCESS`. [`crate::node::detach_spawned_process`] sets this so
/// bitcoind and Tor outlive the app. The bridge holds the session mnemonic
/// and must stay a `kill_on_drop` child, so this flag is never applied.
#[cfg(any(test, target_os = "windows"))]
const DETACHED_PROCESS: u32 = 0x0000_0008;

/// Windows creation flags for the Spark bridge child.
///
/// Suppresses the console window and nothing else. Masking off
/// [`DETACHED_PROCESS`] keeps a later edit from reusing the node/Tor detach
/// helper, which would leave the bridge (and the mnemonic) alive after the
/// GUI exits.
#[cfg(any(test, target_os = "windows"))]
fn bridge_windows_creation_flags() -> u32 {
    CREATE_NO_WINDOW & !DETACHED_PROCESS
}

/// How long to wait for the [`Method::Init`] handshake.
///
/// `init` is not an ordinary query. It drives `SdkBuilder::build()`, and that
/// bootstraps the cross-chain providers the bridge opts into (see
/// `cross_chain_config` in `coincube-spark-bridge`'s `mainnet_config`). Boltz
/// is built *at connect*, over HTTPS, with a 60s timeout of its own, so a
/// provider that is unreachable rather than merely down — `api.boltz.exchange`
/// black-holed by a network, DNS resolving but TCP never completing — burns
/// that full 60s before the SDK logs the failure and carries on without it. A
/// connect that would have completed in ~62s was being abandoned at 30s.
///
/// The failure that produced reads as if the bridge were broken and leaves no
/// evidence: the client gives up, drops, and `kill_on_drop` reaps the child
/// well before the SDK's own error line lands at t+60s, so the log carries the
/// gui's "Spark bridge unavailable" and not one word from the bridge.
///
/// 120s is twice the single stall the SDK can impose here (only Boltz blocks
/// `build()` — Orchestra resolves its config lazily on first use), which leaves
/// room for the sqlite migration and initial sync on top of it.
const INIT_TIMEOUT: Duration = Duration::from_secs(120);

/// How long to wait for `method` before giving up, given what a missing answer
/// would mean for it.
///
/// [`RequestKind`] says what silence *means*; this says how long to tolerate
/// it, which is not the same question — [`Method::Init`] is a query by that
/// definition (nothing moved) yet legitimately takes minutes. Split out from
/// [`SparkClient::request_with`] so the pairing is testable without a live
/// subprocess.
fn deadline_for(method: &Method, kind: RequestKind) -> Duration {
    match kind {
        RequestKind::StateChanging => SEND_SOFT_DEADLINE,
        RequestKind::Query => match method {
            Method::Init(_) => INIT_TIMEOUT,
            _ => QUERY_TIMEOUT,
        },
    }
}

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

/// What a state-changing request handed the bridge. Each one gets its own
/// wording for an unknown outcome and its own way to reconcile it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Handover {
    /// A send.
    Payment,
    /// A swap: a self-transfer payment, so it keeps the payment wording. An
    /// [`ErrorKind::Sdk`] answer can follow submission (see [`Method::Swap`]),
    /// so it is unknown too, and so is [`ErrorKind::NotConnected`] (see
    /// [`swap_error`]).
    Swap,
    /// An auto-convert switch, or the legacy Stable Balance switch the
    /// bridge treats as one since S2b. Not a payment, and
    /// [`SparkClient::get_auto_convert`] reconciles it, so a late answer has
    /// no reader and is dropped rather than held forever. An
    /// [`ErrorKind::NotConnected`] answer is unknown, as for a swap (see
    /// [`auto_convert_error`]).
    AutoConvert,
}

impl Handover {
    fn of(method: &Method) -> Self {
        match method {
            Method::Swap(_) => Self::Swap,
            Method::SetAutoConvert(_) | Method::SetStableBalance(_) => Self::AutoConvert,
            _ => Self::Payment,
        }
    }

    /// The [`SparkClientError::OutcomeUnknown`] message when the bridge goes
    /// away with the request in hand.
    fn bridge_gone(self) -> String {
        match self {
            Self::Payment | Self::Swap => {
                "The Spark bridge stopped responding after the payment was sent to it."
            }
            Self::AutoConvert => {
                "The Spark bridge stopped responding while changing auto-convert. Check \
                 whether auto-convert is on before changing it again."
            }
        }
        .to_string()
    }

    /// The [`SparkClientError::OutcomeUnknown`] message when `secs` pass with
    /// no answer.
    fn deadline_passed(self, secs: u64) -> String {
        match self {
            Self::Payment | Self::Swap => format!(
                "The Spark bridge did not answer within {secs}s of being given this payment."
            ),
            Self::AutoConvert => format!(
                "The Spark bridge did not answer within {secs}s while changing auto-convert. \
                 Check whether auto-convert is on before changing it again."
            ),
        }
    }

    /// Whether a late answer is held for [`SparkClient::take_late_outcome`]
    /// or [`SparkClient::take_late_swap_outcome`].
    fn holds_late_answer(self) -> bool {
        !matches!(self, Self::AutoConvert)
    }
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
    /// Whether the bridge is gone (or the client was shut down). Every
    /// request after this fails at once, and the client is never respawned:
    /// a state check against it can't settle anything.
    pub fn is_closed(&self) -> bool {
        self.inner.closed.load(Ordering::SeqCst)
    }

    /// Spawn the bridge subprocess, hand it a mnemonic + config, and
    /// return the connected client.
    ///
    /// `mnemonic` is passed into the bridge over stdin and then dropped
    /// by the caller (see [`super::mod::load_spark_client`] for the
    /// zeroizing wrapper). The bridge keeps it in memory for the
    /// session lifetime.
    pub async fn connect(config: SparkConfig, mnemonic: &str) -> Result<Self, SparkClientError> {
        Self::connect_with_mode(config, mnemonic, false).await
    }

    pub(crate) async fn connect_receive_only(
        config: SparkConfig,
        mnemonic: &str,
    ) -> Result<Self, SparkClientError> {
        Self::connect_with_mode(config, mnemonic, true).await
    }

    async fn connect_with_mode(
        config: SparkConfig,
        mnemonic: &str,
        receive_only: bool,
    ) -> Result<Self, SparkClientError> {
        if config.api_key.is_empty() {
            return Err(SparkClientError::Config(
                "Spark SDK API key is empty — set BREEZ_API_KEY at build time".to_string(),
            ));
        }

        let bridge_path = resolve_bridge_path()?;
        debug!("spawning Spark bridge at {:?}", bridge_path);

        let mut command = Command::new(&bridge_path);
        if receive_only {
            command.arg("--receive-only");
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // The bridge is a console-subsystem binary and the release gui has no
        // console, so without this Windows opens an empty console window for
        // it. Deliberately not `node::detach_spawned_process`: the bridge must
        // die with the gui (`kill_on_drop`), not outlive it like bitcoind/Tor.
        #[cfg(target_os = "windows")]
        command.creation_flags(bridge_windows_creation_flags());
        let mut child = command.spawn().map_err(|e| {
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
        self.get_info_with_sync(true).await
    }

    pub(crate) async fn receive_identity(&self) -> Result<String, SparkClientError> {
        self.get_info_with_sync(false)
            .await
            .map(|info| info.identity_pubkey)
    }

    async fn get_info_with_sync(&self, ensure_synced: bool) -> Result<GetInfoOk, SparkClientError> {
        match self
            .request(Method::GetInfo(GetInfoParams {
                ensure_synced: Some(ensure_synced),
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
        self.prepare_send_with(input, amount_sat, false).await
    }

    /// [`Self::prepare_send`] with the fee taken out of `amount_sat`: the
    /// wallet spends exactly `amount_sat`, so the whole balance drains it.
    pub async fn prepare_send_fees_included(
        &self,
        input: String,
        amount_sat: u64,
    ) -> Result<PrepareSendOk, SparkClientError> {
        self.prepare_send_with(input, Some(amount_sat), true).await
    }

    async fn prepare_send_with(
        &self,
        input: String,
        amount_sat: Option<u64>,
        fees_included: bool,
    ) -> Result<PrepareSendOk, SparkClientError> {
        match self
            .request(Method::PrepareSend(PrepareSendParams {
                input,
                amount_sat,
                fees_included,
            }))
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

    /// Phase 6: read the user settings (Stable Balance on/off, private
    /// mode). Since S2b `stable_balance_active` is bridge-driven
    /// auto-convert, the same state [`Self::get_auto_convert`] reads.
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
    /// Stable Balance feature. Since S2b the bridge treats it as
    /// [`Self::set_auto_convert`]: off leaves the USDB balance as USDB.
    ///
    /// So it is **state-changing** in the same way, with the same deadline
    /// and the same [`SparkClientError::OutcomeUnknown`] for a lost or
    /// made-up [`ErrorKind::NotConnected`] answer: the bridge may wait for a
    /// conversion in flight before it applies the switch. Read
    /// [`Self::get_auto_convert`] to see which way it went.
    pub async fn set_stable_balance(&self, enabled: bool) -> Result<(), SparkClientError> {
        match self
            .request_with(
                Method::SetStableBalance(SetStableBalanceParams { enabled }),
                RequestKind::StateChanging,
            )
            .await?
        {
            OkPayload::SetStableBalance {} => Ok(()),
            other => Err(SparkClientError::Protocol(format!(
                "set_stable_balance returned unexpected payload: {:?}",
                other
            ))),
        }
    }

    /// Read bridge-driven auto-convert and its circuit breaker. The legacy
    /// [`Self::get_user_settings`] reports the same switch.
    pub async fn get_auto_convert(&self) -> Result<AutoConvertState, SparkClientError> {
        match self.request(Method::GetAutoConvert).await? {
            OkPayload::AutoConvert(state) => Ok(state),
            other => Err(SparkClientError::Protocol(format!(
                "get_auto_convert returned unexpected payload: {:?}",
                other
            ))),
        }
    }

    /// Turn bridge-driven auto-convert on or off, and return the state after
    /// the call. Turning it on also resets a paused breaker.
    ///
    /// This is a **state-changing** request: switching it on starts
    /// converting incoming bitcoin. A lost answer is
    /// [`SparkClientError::OutcomeUnknown`], and [`Self::get_auto_convert`]
    /// tells which way it went: it is the only reconciliation, so no late
    /// answer is held. An [`ErrorKind::NotConnected`] answer is
    /// `OutcomeUnknown` too, because this client makes that answer up for
    /// every request in flight when the bridge dies. Read
    /// [`Self::get_auto_convert`] before changing it again.
    pub async fn set_auto_convert(
        &self,
        enabled: bool,
    ) -> Result<AutoConvertState, SparkClientError> {
        match self
            .request_with(
                Method::SetAutoConvert(SetAutoConvertParams { enabled }),
                RequestKind::StateChanging,
            )
            .await?
        {
            OkPayload::AutoConvert(state) => Ok(state),
            other => Err(SparkClientError::Protocol(format!(
                "set_auto_convert returned unexpected payload: {:?}",
                other
            ))),
        }
    }

    /// Minimum amounts for a conversion out of `from`, so the Swap form can
    /// state the minimum before asking for an estimate.
    pub async fn get_conversion_limits(
        &self,
        from: WalletAsset,
    ) -> Result<ConversionLimits, SparkClientError> {
        match self
            .request(Method::GetConversionLimits(GetConversionLimitsParams {
                from,
            }))
            .await?
        {
            OkPayload::GetConversionLimits(limits) => {
                check_limits(&limits, from).map_err(|reason| {
                    SparkClientError::Protocol(format!(
                        "get_conversion_limits returned inconsistent limits: {reason}"
                    ))
                })?;
                Ok(limits)
            }
            other => Err(SparkClientError::Protocol(format!(
                "get_conversion_limits returned unexpected payload: {:?}",
                other
            ))),
        }
    }

    /// Estimate a BTC ⇄ USDB swap. The returned handle goes to
    /// [`Self::swap`]. The estimate is not a locked price; the slippage cap
    /// is the only protection. `max_slippage_bps: None` uses
    /// [`CONVERSION_MAX_SLIPPAGE_BPS`]; the accepted range for an explicit
    /// cap is on [`PrepareSwapParams::max_slippage_bps`].
    ///
    /// A quote that doesn't convert out of the requested asset is refused,
    /// so the form never shows a figure in the wrong denomination, and so is
    /// one whose cap isn't the one asked for.
    pub async fn prepare_swap(
        &self,
        amount: SwapAmount,
        max_slippage_bps: Option<u32>,
    ) -> Result<PrepareSwapOk, SparkClientError> {
        match self
            .request(Method::PrepareSwap(PrepareSwapParams {
                amount,
                max_slippage_bps,
            }))
            .await?
        {
            OkPayload::PrepareSwap(ok) => {
                let from = amount.from_asset();
                let cap = max_slippage_bps.unwrap_or(CONVERSION_MAX_SLIPPAGE_BPS);
                check_quote(&ok.quote, from)
                    .and_then(|()| check_limits(&ok.limits, from))
                    .and_then(|()| check_slippage_cap(&ok.quote, cap))
                    .map_err(|reason| {
                        SparkClientError::Protocol(format!(
                            "prepare_swap returned an inconsistent estimate: {reason}"
                        ))
                    })?;
                Ok(ok)
            }
            other => Err(SparkClientError::Protocol(format!(
                "prepare_swap returned unexpected payload: {:?}",
                other
            ))),
        }
    }

    /// Execute a swap prepared by [`Self::prepare_swap`]. The handle is
    /// single-use.
    ///
    /// This is a **state-changing** request: it moves funds. A lost answer is
    /// [`SparkClientError::OutcomeUnknown`], never a failure, and
    /// [`Self::take_late_swap_outcome`] holds the bridge's answer if it
    /// arrives later.
    ///
    /// An [`ErrorKind::Sdk`] answer is `OutcomeUnknown` too, because the SDK
    /// can fail after submitting the swap, and so is
    /// [`ErrorKind::NotConnected`], because this client makes that answer up
    /// for every request in flight when the bridge dies. Only `BadRequest`
    /// and `ShuttingDown` come back as a definite
    /// [`SparkClientError::BridgeError`]: the bridge sends them only for a
    /// swap it hasn't started. After `OutcomeUnknown`, reconcile from
    /// Activity before offering another swap.
    pub async fn swap(&self, prepare_handle: String) -> Result<SwapOk, SparkClientError> {
        match self
            .request_with(
                Method::Swap(SwapParams { prepare_handle }),
                RequestKind::StateChanging,
            )
            .await?
        {
            OkPayload::Swap(ok) => Ok(ok),
            other => Err(SparkClientError::Protocol(format!(
                "swap returned unexpected payload: {:?}",
                other
            ))),
        }
    }

    /// [`Self::prepare_send`] with an explicit balance to pay from. The
    /// preview's handle executes through [`Self::send_payment`].
    ///
    /// A preview whose token leg isn't a disclosed conversion is refused, so
    /// Review can never commit a conversion or a token spend it hasn't shown.
    /// See [`PrepareSendFromOk::conversion`] for the rules.
    pub async fn prepare_send_from(
        &self,
        input: String,
        amount_sat: Option<u64>,
        pay_from: WalletAsset,
    ) -> Result<PrepareSendFromOk, SparkClientError> {
        match self
            .request(Method::PrepareSendFrom(PrepareSendFromParams {
                input,
                amount_sat,
                pay_from,
            }))
            .await?
        {
            OkPayload::PrepareSendFrom(ok) => {
                check_send_from(&ok, pay_from).map_err(|reason| {
                    SparkClientError::Protocol(format!(
                        "prepare_send_from returned an inconsistent preview: {reason}"
                    ))
                })?;
                Ok(ok)
            }
            other => Err(SparkClientError::Protocol(format!(
                "prepare_send_from returned unexpected payload: {:?}",
                other
            ))),
        }
    }

    /// [`Self::prepare_lnurl_pay`] with an explicit balance to pay from: the
    /// LNURL / Lightning-address twin of [`Self::prepare_send_from`], with
    /// the same checks.
    pub async fn prepare_lnurl_pay_from(
        &self,
        input: String,
        amount_sat: u64,
        comment: Option<String>,
        pay_from: WalletAsset,
    ) -> Result<PrepareSendFromOk, SparkClientError> {
        match self
            .request(Method::PrepareLnurlPayFrom(PrepareLnurlPayFromParams {
                input,
                amount_sat,
                comment,
                pay_from,
            }))
            .await?
        {
            OkPayload::PrepareSendFrom(ok) => {
                check_send_from(&ok, pay_from).map_err(|reason| {
                    SparkClientError::Protocol(format!(
                        "prepare_lnurl_pay_from returned an inconsistent preview: {reason}"
                    ))
                })?;
                Ok(ok)
            }
            other => Err(SparkClientError::Protocol(format!(
                "prepare_lnurl_pay_from returned unexpected payload: {:?}",
                other
            ))),
        }
    }

    /// Send `amount` USDB base units from the USDB balance to the Spark
    /// address `input`. The preview's handle executes through
    /// [`Self::send_payment`].
    ///
    /// Only a direct USDB transfer to a Spark address of exactly `amount`,
    /// disclosed as [`PrepareSendFromOk::token_payment`], is accepted.
    pub async fn prepare_usdb_send(
        &self,
        input: String,
        amount: u128,
    ) -> Result<PrepareSendFromOk, SparkClientError> {
        match self
            .request(Method::PrepareUsdbSend(PrepareUsdbSendParams {
                input,
                amount,
            }))
            .await?
        {
            OkPayload::PrepareSendFrom(ok) => {
                check_usdb_send(&ok, amount).map_err(|reason| {
                    SparkClientError::Protocol(format!(
                        "prepare_usdb_send returned an inconsistent preview: {reason}"
                    ))
                })?;
                Ok(ok)
            }
            other => Err(SparkClientError::Protocol(format!(
                "prepare_usdb_send returned unexpected payload: {:?}",
                other
            ))),
        }
    }

    /// A client with no bridge behind it: every request fails at once.
    #[cfg(test)]
    pub(crate) fn disconnected_for_test() -> Self {
        Self::mock_transport().0
    }

    /// A client wired to channels instead of a bridge subprocess.
    #[cfg(test)]
    pub(crate) fn mock_transport() -> (Self, mpsc::UnboundedReceiver<Request>) {
        let (request_tx, request_rx) = mpsc::unbounded_channel();
        let (event_tx, _) = broadcast::channel(16);
        let client = Self {
            inner: Arc::new(SparkClientInner {
                next_id: AtomicU64::new(1),
                request_tx,
                pending: Arc::new(Mutex::new(HashMap::new())),
                late_outcomes: Arc::new(Mutex::new(HashMap::new())),
                awaiting_late: Arc::new(Mutex::new(std::collections::HashSet::new())),
                event_tx,
                child: Mutex::new(None),
                closed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            }),
        };
        (client, request_rx)
    }

    /// Answer request `id` with a bridge error, as the bridge does when it
    /// refuses a request.
    #[cfg(test)]
    pub(crate) async fn mock_refuse(&self, id: u64, kind: ErrorKind, message: &str) {
        let waiter = self
            .inner
            .pending
            .lock()
            .await
            .remove(&id)
            .expect("a pending request");
        let _ = waiter.send(Response::err(id, kind, message));
    }

    /// What the reader and writer do when the bridge goes away.
    #[cfg(test)]
    pub(crate) fn mock_close(&self) {
        self.inner.closed.store(true, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(crate) async fn mock_reply(&self, id: u64, answer: Option<OkPayload>) {
        let waiter = self
            .inner
            .pending
            .lock()
            .await
            .remove(&id)
            .expect("a pending request");
        if let Some(payload) = answer {
            let _ = waiter.send(Response::ok(id, payload));
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
    ///
    /// When the bridge dies, the writer and reader tasks answer every request
    /// still in flight with an [`ErrorKind::NotConnected`] error they make up
    /// themselves, so that error, not a closed channel, is what a caller
    /// usually sees. A send, a swap and an auto-convert change all read it as
    /// [`SparkClientError::OutcomeUnknown`] (see [`payment_error`]).
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

        let deadline = deadline_for(&method, kind);
        let handover = Handover::of(&method);
        let request = Request { id, method };
        if self.inner.request_tx.send(request).is_err() {
            // Writer task exited before this was handed over. Nothing was
            // dispatched, so this is a definite failure even for a send.
            self.inner.pending.lock().await.remove(&id);
            return Err(SparkClientError::BridgeUnavailable(
                "Spark bridge writer task exited".to_string(),
            ));
        }

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
                        message: handover.bridge_gone(),
                    },
                });
            }
            Err(_) => {
                self.inner.pending.lock().await.remove(&id);
                return Err(match kind {
                    RequestKind::Query => SparkClientError::BridgeUnavailable(format!(
                        "Spark bridge did not respond within {}s (id={})",
                        deadline.as_secs(),
                        id
                    )),
                    RequestKind::StateChanging => {
                        // Keep listening: the bridge still owes an answer, and
                        // when it arrives it is the authoritative one. Not for
                        // an auto-convert change, whose answer nothing reads.
                        if handover.holds_late_answer() {
                            self.inner.awaiting_late.lock().await.insert(id);
                        }
                        SparkClientError::OutcomeUnknown {
                            request_id: id,
                            message: handover.deadline_passed(deadline.as_secs()),
                        }
                    }
                });
            }
        };

        match response.result {
            ResponseResult::Ok(payload) => Ok(payload),
            ResponseResult::Err(ErrorPayload {
                kind: error_kind,
                message,
            }) => Err(match handover {
                Handover::Payment => payment_error(id, kind, error_kind, message),
                Handover::Swap => swap_error(id, error_kind, message),
                Handover::AutoConvert => auto_convert_error(id, error_kind, message),
            }),
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

    /// [`Self::take_late_outcome`] for a [`Self::swap`]: the bridge's answer
    /// to a swap whose caller gave up waiting, if it has arrived since.
    ///
    /// A late [`ErrorKind::Sdk`] error is still
    /// [`SparkClientError::OutcomeUnknown`], as on time. `None` also follows
    /// a swap that was reported unknown because of an SDK error: that answer
    /// has already arrived. Either way, reconcile from Activity.
    pub async fn take_late_swap_outcome(
        &self,
        request_id: u64,
    ) -> Option<Result<SwapOk, SparkClientError>> {
        let response = self.inner.late_outcomes.lock().await.remove(&request_id)?;
        self.inner.awaiting_late.lock().await.remove(&request_id);
        Some(match response.result {
            ResponseResult::Ok(OkPayload::Swap(ok)) => Ok(ok),
            ResponseResult::Ok(other) => Err(SparkClientError::Protocol(format!(
                "late swap outcome carried unexpected payload: {:?}",
                other
            ))),
            ResponseResult::Err(ErrorPayload { kind, message }) => {
                Err(swap_error(request_id, kind, message))
            }
        })
    }
}

/// A send's (or any other non-swap request's) error answer.
///
/// [`ErrorKind::NotConnected`] on a state-changing request is not a refusal:
/// it is what the writer and reader tasks make up for every request still in
/// flight when the bridge dies (see [`SparkClient::request_with`]), and the
/// SDK may have finished the payment first. Reporting it as a definite
/// failure is how a payment gets made twice, because a definite failure
/// retires the idempotency key and offers a fresh attempt. So it is
/// [`SparkClientError::OutcomeUnknown`], as for a swap. No late answer will
/// come from a dead bridge; reconciliation falls through to the payment
/// history. Every other kind is the bridge's own answer, and definite.
fn payment_error(
    request_id: u64,
    request: RequestKind,
    kind: ErrorKind,
    message: String,
) -> SparkClientError {
    if kind == ErrorKind::NotConnected && request == RequestKind::StateChanging {
        warn!(
            "Spark send id {} lost its bridge mid-request, so it may have completed: {}",
            request_id, message
        );
        return SparkClientError::OutcomeUnknown {
            request_id,
            message: Handover::Payment.bridge_gone(),
        };
    }
    SparkClientError::BridgeError { kind, message }
}

/// A swap's error answer. Only the bridge's refusals before submission are
/// definite ([`ErrorKind::BadRequest`], [`ErrorKind::ShuttingDown`]; see
/// [`Method::Swap`]). Two kinds are an unknown outcome, because the swap may
/// still complete:
///
/// - an [`ErrorKind::Sdk`] error can follow submission;
/// - an [`ErrorKind::NotConnected`] error is what the writer and reader tasks
///   make up for every request in flight when the bridge dies (see
///   [`SparkClient::request_with`]).
///
/// The error's text goes to the log, not the error, because it may say
/// "failed".
fn swap_error(request_id: u64, kind: ErrorKind, message: String) -> SparkClientError {
    // Exhaustive, so a new kind has to be classified here. `AlreadyConnected`
    // answers only `init`; it stays definite, as before.
    let unknown = match kind {
        ErrorKind::BadRequest | ErrorKind::ShuttingDown | ErrorKind::AlreadyConnected => {
            return SparkClientError::BridgeError { kind, message };
        }
        ErrorKind::Sdk => {
            "The Spark SDK reported an error after this swap was handed to it, so it may \
             still complete. Check Activity before swapping again."
        }
        ErrorKind::NotConnected => {
            "The Spark bridge stopped responding after this swap was handed to it, so it may \
             still complete. Check Activity before swapping again."
        }
    };
    warn!(
        "Spark swap id {} ended with a {:?} error, so it may still complete: {}",
        request_id, kind, message
    );
    SparkClientError::OutcomeUnknown {
        request_id,
        message: unknown.to_string(),
    }
}

/// An auto-convert change's error answer. [`ErrorKind::NotConnected`] is
/// what the writer and reader tasks make up for every request in flight when
/// the bridge dies (see [`SparkClient::request_with`]), so the change may
/// have been made: an unknown outcome, reconciled with
/// [`SparkClient::get_auto_convert`]. Every other kind is a definite refusal.
fn auto_convert_error(request_id: u64, kind: ErrorKind, message: String) -> SparkClientError {
    if kind != ErrorKind::NotConnected {
        return SparkClientError::BridgeError { kind, message };
    }
    warn!(
        "Spark auto-convert change id {} ended with a {:?} error, so it may have been made: {}",
        request_id, kind, message
    );
    SparkClientError::OutcomeUnknown {
        request_id,
        message: Handover::AutoConvert.bridge_gone(),
    }
}

/// The asset a conversion out of `from` delivers.
fn converted_into(from: WalletAsset) -> WalletAsset {
    match from {
        WalletAsset::Bitcoin => WalletAsset::Usdb,
        WalletAsset::Usdb => WalletAsset::Bitcoin,
    }
}

/// Check that a quote converts out of `from`: amount and fee in `from`, the
/// estimate in the other asset. The SDK denominates the fee in the asset
/// converted out of.
fn check_quote(quote: &ConversionQuote, from: WalletAsset) -> Result<(), String> {
    let into = converted_into(from);
    if quote.amount_in.asset != from || quote.fee.asset != from || quote.amount_out.asset != into {
        return Err(format!(
            "expected {from:?} in and {into:?} out, got {:?} in, {:?} out and a fee in {:?}",
            quote.amount_in.asset, quote.amount_out.asset, quote.fee.asset
        ));
    }
    Ok(())
}

/// Check that a quote applies the slippage cap it was asked for.
fn check_slippage_cap(quote: &ConversionQuote, cap: u32) -> Result<(), String> {
    if quote.max_slippage_bps != cap {
        return Err(format!(
            "slippage cap is {} bps, asked for {cap} bps",
            quote.max_slippage_bps
        ));
    }
    Ok(())
}

/// Check that conversion limits for `from` are denominated that way: the
/// minimum converted in `from`, the minimum received in the other asset.
fn check_limits(limits: &ConversionLimits, from: WalletAsset) -> Result<(), String> {
    let into = converted_into(from);
    if let Some(min) = limits.min_from.filter(|min| min.asset != from) {
        return Err(format!(
            "minimum converted is in {:?}, expected {from:?}",
            min.asset
        ));
    }
    if let Some(min) = limits.min_to.filter(|min| min.asset != into) {
        return Err(format!(
            "minimum received is in {:?}, expected {into:?}",
            min.asset
        ));
    }
    Ok(())
}

/// Check a pay-from preview against the balance it was asked to pay from,
/// and that it discloses any conversion it carries. The rules are on
/// [`PrepareSendFromOk::conversion`] and [`PrepareSendFromOk::token_payment`].
fn check_send_from(ok: &PrepareSendFromOk, pay_from: WalletAsset) -> Result<(), String> {
    if ok.pay_from != pay_from {
        return Err(format!(
            "prepared from {:?}, asked to pay from {pay_from:?}",
            ok.pay_from
        ));
    }
    if let Some(payment) = &ok.token_payment {
        return check_token_payment(ok, payment, "SparkInvoice");
    }
    // A conversion is the only token leg these methods prepare, so the two
    // come together.
    match (ok.prepare.has_token_leg, ok.conversion.as_deref()) {
        // Every conversion that funds a send turns USDB into bitcoin.
        (Some(true), Some(quote)) => check_quote(quote, WalletAsset::Usdb),
        (Some(false), None) if pay_from == WalletAsset::Usdb => {
            Err("paying from USDB without a conversion".to_string())
        }
        (Some(false), None) => Ok(()),
        // A direct token transfer, or a quote the bridge dropped: Review
        // would show a bitcoin payment while a token moves.
        (Some(true), None) => Err("a token leg without a conversion".to_string()),
        // A conversion the retry guard can't see: the gui would trust an
        // idempotency key the bridge drops.
        (_, Some(_)) => Err("a conversion without a token leg".to_string()),
        (None, None) => Err("no token-leg flag".to_string()),
    }
}

/// Check a [`SparkClient::prepare_usdb_send`] preview: a USDB transfer to a
/// Spark address that pays the recipient exactly the `amount` asked.
fn check_usdb_send(ok: &PrepareSendFromOk, amount: u128) -> Result<(), String> {
    let Some(payment) = &ok.token_payment else {
        return Err("no token payment".to_string());
    };
    check_token_payment(ok, payment, "SparkAddress")?;
    if payment.amount.amount != amount {
        return Err(format!(
            "a token payment of {}, asked for {amount}",
            payment.amount.amount
        ));
    }
    Ok(())
}

/// Check the token leg a preview may carry without a conversion: USDB paid
/// from USDB by `method` (a USDB invoice, or a Spark address), with its
/// figures in USDB and none in sats, so Review can't show token base units
/// as a bitcoin payment.
fn check_token_payment(
    ok: &PrepareSendFromOk,
    payment: &TokenPayment,
    method: &str,
) -> Result<(), String> {
    if ok.pay_from != WalletAsset::Usdb {
        return Err(format!("a token payment from {:?}", ok.pay_from));
    }
    if ok.conversion.is_some() {
        return Err("a token payment with a conversion".to_string());
    }
    // The retry guard reads this: the bridge drops the idempotency key.
    if ok.prepare.has_token_leg != Some(true) {
        return Err("a token payment without a token leg".to_string());
    }
    if ok.prepare.method != method || ok.prepare.cross_chain.is_some() {
        return Err(format!("a token payment by {}", ok.prepare.method));
    }
    if payment.amount.asset != WalletAsset::Usdb || payment.fee.asset != WalletAsset::Usdb {
        return Err(format!(
            "a token payment of {:?} with a fee in {:?}, expected Usdb",
            payment.amount.asset, payment.fee.asset
        ));
    }
    if payment.amount.amount == 0 {
        return Err("a token payment of nothing".to_string());
    }
    if ok.prepare.amount_sat != 0 || ok.prepare.fee_sat != 0 {
        return Err("a token payment with sats figures".to_string());
    }
    Ok(())
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

/// The version the Spark bridge this app would launch reports through
/// `--version` (`coincube-spark-bridge 0.1.0` → `0.1.0`). Runs the binary
/// found by the same discovery and trust checks as a real launch, with no
/// mnemonic and no network; gives up after five seconds.
pub async fn bridge_version() -> Result<String, SparkClientError> {
    let path = resolve_bridge_path()?;
    let mut command = Command::new(&path);
    command
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(target_os = "windows")]
    command.creation_flags(bridge_windows_creation_flags());
    let output = tokio::time::timeout(Duration::from_secs(5), command.output())
        .await
        .map_err(|_| SparkClientError::BridgeUnavailable("--version timed out".to_string()))?
        .map_err(|e| SparkClientError::BridgeUnavailable(format!("--version: {e}")))?;
    if !output.status.success() {
        return Err(SparkClientError::BridgeUnavailable(format!(
            "--version exited with {}",
            output.status
        )));
    }
    parse_bridge_version(&String::from_utf8_lossy(&output.stdout)).ok_or_else(|| {
        SparkClientError::BridgeUnavailable("--version printed no version".to_string())
    })
}

/// `coincube-spark-bridge 0.1.0\n` → `0.1.0`.
fn parse_bridge_version(stdout: &str) -> Option<String> {
    let version = stdout.trim().rsplit(' ').next()?.trim();
    (!version.is_empty() && version.chars().next()?.is_ascii_digit()).then(|| version.to_string())
}

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
            // The bridge receives the BIP39 mnemonic on stdin. An override is
            // only accepted when the file itself is not writable by anyone
            // else, so a swapped binary is not a silent path to the seed.
            return accept_bridge_binary(p);
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
                return accept_bridge_binary(sibling);
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
            return accept_bridge_binary(candidate);
        }
    }

    Err(SparkClientError::BridgeUnavailable(format!(
        "could not locate {} — set COINCUBE_SPARK_BRIDGE_PATH or run `cargo build \
         --manifest-path coincube-spark-bridge/Cargo.toml` first",
        exe_name
    )))
}

/// The bridge process is inside the trusted base: `Init` sends it the
/// mnemonic on stdin. Only a regular file that is not group- or
/// world-writable is accepted, and on Linux it must be owned by this user.
fn accept_bridge_binary(path: PathBuf) -> Result<PathBuf, SparkClientError> {
    let path = path.canonicalize().map_err(|e| {
        SparkClientError::BridgeUnavailable(format!(
            "spark bridge path {} could not be resolved: {e}",
            path.display()
        ))
    })?;
    let meta = std::fs::metadata(&path).map_err(|e| {
        SparkClientError::BridgeUnavailable(format!(
            "spark bridge path {} could not be read: {e}",
            path.display()
        ))
    })?;
    if !meta.is_file() {
        return Err(SparkClientError::BridgeUnavailable(format!(
            "spark bridge path {} is not a regular file",
            path.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.mode() & 0o022 != 0 {
            return Err(SparkClientError::BridgeUnavailable(format!(
                "spark bridge path {} is group- or world-writable",
                path.display()
            )));
        }
        #[cfg(target_os = "linux")]
        {
            let self_meta = std::fs::metadata("/proc/self").map_err(|e| {
                SparkClientError::BridgeUnavailable(format!(
                    "could not read /proc/self to check the spark bridge owner: {e}"
                ))
            })?;
            if meta.uid() != self_meta.uid() {
                return Err(SparkClientError::BridgeUnavailable(format!(
                    "spark bridge path {} is not owned by this user",
                    path.display()
                )));
            }
        }
    }
    Ok(path)
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
    /// A state-changing request was dispatched and no answer came back, or a
    /// swap was answered with an SDK error that can follow submission, or a
    /// swap or auto-convert change got the [`ErrorKind::NotConnected`] answer
    /// the client makes up when the bridge dies.
    ///
    /// **Not a failure.** The bridge has the request and the SDK may already
    /// have moved the money; the transport simply stopped telling us. Callers
    /// must reconcile — [`SparkClient::take_late_outcome`] first, then the
    /// payment history — before offering to send anything again, and must never
    /// render this as "the payment failed". A swap reconciles from Activity
    /// ([`SparkClient::take_late_swap_outcome`] only helps after a timeout),
    /// and an auto-convert change from [`SparkClient::get_auto_convert`].
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
                // `{:?}` printed the Rust variant name (`Sdk`, `NotConnected`).
                // The message carries the substance; the kind is a log tag.
                let kind = match kind {
                    ErrorKind::NotConnected => "not connected",
                    ErrorKind::AlreadyConnected => "already connected",
                    ErrorKind::Sdk => "sdk",
                    ErrorKind::BadRequest => "bad request",
                    ErrorKind::ShuttingDown => "shutting down",
                };
                write!(f, "Spark bridge error ({}): {}", kind, message)
            }
            Self::OutcomeUnknown { message, .. } => {
                // Not "payment": an auto-convert change can be unknown too.
                // A send's message still names the payment.
                write!(f, "Spark outcome unknown: {}", message)
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

    /// `init` drives the whole SDK build, including a Boltz bootstrap that
    /// spends 60s of its own before giving up when the host is unreachable.
    /// Holding it to the ordinary query deadline abandoned a connect that was
    /// still going to succeed, and reported a working bridge as unavailable.
    #[test]
    fn init_does_not_inherit_the_query_timeout() {
        let init = Method::Init(InitParams {
            api_key: "key".to_string(),
            network: coincube_spark_protocol::Network::Mainnet,
            mnemonic: "mnemonic".to_string(),
            mnemonic_passphrase: None,
            storage_dir: "/tmp/spark".to_string(),
        });

        assert_eq!(deadline_for(&init, RequestKind::Query), INIT_TIMEOUT);
        assert!(
            INIT_TIMEOUT > QUERY_TIMEOUT,
            "init must outlast the query deadline: one unreachable cross-chain \
             provider alone costs 60s at connect"
        );
        // The single stall the SDK can impose at connect is Boltz's 60s, and
        // the rest of the build has to fit alongside it.
        assert!(
            INIT_TIMEOUT >= Duration::from_secs(120),
            "init must leave room for the SDK build on top of a 60s provider stall"
        );
    }

    /// Only `init` gets the long deadline — an ordinary read that stops
    /// answering should still fail fast rather than hang the caller.
    #[test]
    fn ordinary_queries_keep_the_short_deadline() {
        let get_info = Method::GetInfo(GetInfoParams {
            ensure_synced: Some(true),
        });
        assert_eq!(deadline_for(&get_info, RequestKind::Query), QUERY_TIMEOUT);
        assert_eq!(
            deadline_for(&Method::Shutdown, RequestKind::Query),
            QUERY_TIMEOUT
        );
    }

    /// The deadline is chosen per method, but what silence *means* is still
    /// decided by the kind: a send keeps its own soft deadline whatever
    /// method carries it.
    #[test]
    fn a_send_keeps_the_send_deadline() {
        let send = Method::SendPayment(SendPaymentParams {
            prepare_handle: "handle".to_string(),
            idempotency_key: None,
        });
        assert_eq!(
            deadline_for(&send, RequestKind::StateChanging),
            SEND_SOFT_DEADLINE
        );
    }

    #[test]
    fn bridge_version_reads_the_clap_version_line() {
        assert_eq!(
            parse_bridge_version("coincube-spark-bridge 0.1.0\n").as_deref(),
            Some("0.1.0")
        );
        assert_eq!(parse_bridge_version(""), None);
        assert_eq!(parse_bridge_version("error: unexpected argument"), None);
    }

    /// The bridge's console is hidden on Windows, and the child is not
    /// detached. Detach is what lets bitcoind and Tor outlive the GUI; the
    /// bridge holds the session mnemonic and has to die with it.
    #[test]
    fn the_spark_bridge_suppresses_its_console_without_detaching() {
        let flags = bridge_windows_creation_flags();
        assert_eq!(flags, 0x0800_0000, "CREATE_NO_WINDOW");
        assert_eq!(flags & DETACHED_PROCESS, 0);
        assert_eq!(flags, CREATE_NO_WINDOW & !DETACHED_PROCESS);
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
            // Human-readable kind, not the Rust variant name.
            "Spark bridge error (not connected): bridge offline"
        );
        assert_eq!(
            SparkClientError::Protocol("wrong payload".to_string()).to_string(),
            "Spark protocol error: wrong payload"
        );
    }
}

#[cfg(test)]
mod p0_contract_tests {
    use super::*;
    use coincube_spark_protocol::{AssetAmount, AutoConvertBreaker};

    /// A client wired to channels instead of a subprocess. The test plays the
    /// bridge with [`bridge_answers`].
    fn fake_client() -> (SparkClient, mpsc::UnboundedReceiver<Request>) {
        SparkClient::mock_transport()
    }

    /// Take the next request off the wire and answer it. `None` drops the
    /// waiter instead, so the caller sees the channel close. When the bridge
    /// exits mid-request, the reader answers first; see [`bridge_exits`].
    async fn bridge_answers(
        client: &SparkClient,
        requests: &mut mpsc::UnboundedReceiver<Request>,
        answer: Option<OkPayload>,
    ) -> Method {
        let request = requests.recv().await.expect("a request was sent");
        client.mock_reply(request.id, answer).await;
        request.method
    }

    /// Take the next request off the wire and answer it with a `kind` error
    /// whose text says "failed", as an SDK error can. Returns the request id.
    async fn bridge_refuses(
        client: &SparkClient,
        requests: &mut mpsc::UnboundedReceiver<Request>,
        kind: ErrorKind,
    ) -> u64 {
        let request = requests.recv().await.expect("a request was sent");
        let waiter = client
            .inner
            .pending
            .lock()
            .await
            .remove(&request.id)
            .expect("the caller is waiting");
        let _ = waiter.send(Response::err(
            request.id,
            kind,
            "Conversion failed: timeout waiting for conversion to complete",
        ));
        request.id
    }

    /// Take the next request off the wire and answer it as the reader task
    /// does when the bridge exits mid-request: with an
    /// [`ErrorKind::NotConnected`] error it makes up itself. Returns the
    /// request id.
    async fn bridge_exits(
        client: &SparkClient,
        requests: &mut mpsc::UnboundedReceiver<Request>,
    ) -> u64 {
        let request = requests.recv().await.expect("a request was sent");
        let waiter = client
            .inner
            .pending
            .lock()
            .await
            .remove(&request.id)
            .expect("the caller is waiting");
        let _ = waiter.send(Response::err(
            request.id,
            ErrorKind::NotConnected,
            "Spark bridge subprocess exited unexpectedly",
        ));
        request.id
    }

    fn amount(asset: WalletAsset, amount: u128) -> AssetAmount {
        AssetAmount { asset, amount }
    }

    /// A consistent estimate for a conversion out of `from`.
    fn quote(from: WalletAsset) -> ConversionQuote {
        ConversionQuote {
            amount_in: amount(from, 1_000_000),
            amount_out: amount(converted_into(from), 1_500),
            fee: amount(from, 1_200),
            amount_adjustment: None,
            max_slippage_bps: coincube_spark_protocol::CONVERSION_MAX_SLIPPAGE_BPS,
        }
    }

    fn send_from(
        pay_from: WalletAsset,
        has_token_leg: Option<bool>,
        conversion: Option<ConversionQuote>,
    ) -> PrepareSendFromOk {
        PrepareSendFromOk {
            prepare: PrepareSendOk {
                handle: "h".to_string(),
                amount_sat: 1_500,
                fee_sat: 3,
                method: "Bolt11Invoice".to_string(),
                cross_chain: None,
                has_token_leg,
            },
            pay_from,
            conversion: conversion.map(Box::new),
            token_payment: None,
        }
    }

    /// The preview the bridge returns for a USDB invoice paid from USDB.
    fn usdb_invoice_from_usdb() -> PrepareSendFromOk {
        let mut ok = send_from(WalletAsset::Usdb, Some(true), None);
        ok.prepare.amount_sat = 0;
        ok.prepare.fee_sat = 0;
        ok.prepare.method = "SparkInvoice".to_string();
        ok.token_payment = Some(TokenPayment {
            amount: amount(WalletAsset::Usdb, 2_500_000),
            fee: amount(WalletAsset::Usdb, 0),
        });
        ok
    }

    #[test]
    fn a_token_leg_without_a_conversion_is_only_a_usdb_invoice_from_usdb() {
        use WalletAsset::{Bitcoin, Usdb};

        let ok = usdb_invoice_from_usdb();
        assert_eq!(check_send_from(&ok, Usdb), Ok(()));
        // Without its USDB figures it is the token leg Review can't show.
        let mut undisclosed = ok.clone();
        undisclosed.token_payment = None;
        assert!(check_send_from(&undisclosed, Usdb)
            .expect_err("undisclosed")
            .contains("token leg without a conversion"));

        type Mutation = (&'static str, fn(&mut PrepareSendFromOk));
        let mutations: [Mutation; 9] = [
            ("token payment from Bitcoin", |ok| ok.pay_from = Bitcoin),
            ("with a conversion", |ok| {
                ok.conversion = Some(Box::new(quote(Usdb)))
            }),
            ("without a token leg", |ok| {
                ok.prepare.has_token_leg = Some(false)
            }),
            ("without a token leg", |ok| ok.prepare.has_token_leg = None),
            ("by Bolt11Invoice", |ok| {
                ok.prepare.method = "Bolt11Invoice".to_string()
            }),
            ("expected Usdb", |ok| {
                ok.token_payment.as_mut().expect("payment").amount.asset = Bitcoin
            }),
            ("expected Usdb", |ok| {
                ok.token_payment.as_mut().expect("payment").fee.asset = Bitcoin
            }),
            ("of nothing", |ok| {
                ok.token_payment.as_mut().expect("payment").amount.amount = 0
            }),
            ("sats figures", |ok| ok.prepare.amount_sat = 2_500_000),
        ];
        for (why, mutate) in mutations {
            let mut bad = usdb_invoice_from_usdb();
            mutate(&mut bad);
            let asked = bad.pay_from;
            let err = check_send_from(&bad, asked).expect_err(why);
            assert!(err.contains(why), "{}: {}", why, err);
        }
        // Asked to pay from bitcoin, a USDB-paid preview is refused first.
        assert!(check_send_from(&ok, Bitcoin).is_err());
        // An address-shaped token payment isn't what `prepare_send_from` pays.
        let mut by_address = usdb_invoice_from_usdb();
        by_address.prepare.method = "SparkAddress".to_string();
        assert!(check_send_from(&by_address, Usdb).is_err());
    }

    /// The preview the bridge returns for a USDB send to a Spark address.
    fn usdb_to_address() -> PrepareSendFromOk {
        let mut ok = usdb_invoice_from_usdb();
        ok.prepare.method = "SparkAddress".to_string();
        ok
    }

    #[test]
    fn a_usdb_address_send_pays_exactly_the_amount_asked() {
        assert_eq!(check_usdb_send(&usdb_to_address(), 2_500_000), Ok(()));

        type Mutation = (&'static str, fn(&mut PrepareSendFromOk));
        let mutations: [Mutation; 6] = [
            ("asked for", |_| {}),
            ("no token payment", |ok| ok.token_payment = None),
            ("by SparkInvoice", |ok| {
                ok.prepare.method = "SparkInvoice".to_string()
            }),
            ("token payment from Bitcoin", |ok| {
                ok.pay_from = WalletAsset::Bitcoin
            }),
            ("with a conversion", |ok| {
                ok.conversion = Some(Box::new(quote(WalletAsset::Usdb)))
            }),
            ("sats figures", |ok| ok.prepare.fee_sat = 1),
        ];
        for (why, mutate) in mutations {
            let mut bad = usdb_to_address();
            mutate(&mut bad);
            // The unmutated case is checked against a different amount.
            let asked = if why == "asked for" {
                2_000_000
            } else {
                2_500_000
            };
            let err = check_usdb_send(&bad, asked).expect_err(why);
            assert!(err.contains(why), "{}: {}", why, err);
        }
    }

    #[test]
    fn a_pay_from_preview_must_disclose_its_conversion() {
        use WalletAsset::{Bitcoin, Usdb};

        // Accepted: USDB pays through a conversion; bitcoin pays plainly or
        // with a shortfall conversion.
        for ok in [
            send_from(Usdb, Some(true), Some(quote(Usdb))),
            send_from(Bitcoin, Some(false), None),
            send_from(Bitcoin, Some(true), Some(quote(Usdb))),
        ] {
            assert_eq!(check_send_from(&ok, ok.pay_from), Ok(()), "{:?}", ok);
        }

        for (ok, asked, why) in [
            (send_from(Bitcoin, Some(false), None), Usdb, "prepared from"),
            (
                send_from(Usdb, Some(true), Some(quote(Usdb))),
                Bitcoin,
                "prepared from",
            ),
            (
                send_from(Usdb, Some(true), None),
                Usdb,
                "token leg without a conversion",
            ),
            (
                send_from(Usdb, Some(false), None),
                Usdb,
                "paying from USDB without a conversion",
            ),
            // A direct token transfer (a token invoice) or a dropped shortfall
            // quote: a token would move under a bitcoin Review.
            (
                send_from(Bitcoin, Some(true), None),
                Bitcoin,
                "token leg without a conversion",
            ),
            (send_from(Bitcoin, None, None), Bitcoin, "no token-leg flag"),
            // A conversion the retry guard can't see: the gui would trust an
            // idempotency key the bridge drops.
            (
                send_from(Usdb, Some(false), Some(quote(Usdb))),
                Usdb,
                "without a token leg",
            ),
            (
                send_from(Bitcoin, None, Some(quote(Usdb))),
                Bitcoin,
                "without a token leg",
            ),
            // Funding a send means turning USDB into bitcoin.
            (
                send_from(Usdb, Some(true), Some(quote(Bitcoin))),
                Usdb,
                "expected Usdb in",
            ),
        ] {
            let err = check_send_from(&ok, asked).expect_err(why);
            assert!(err.contains(why), "{}: {}", why, err);
        }
    }

    #[test]
    fn a_quote_and_its_limits_must_convert_out_of_the_requested_asset() {
        use WalletAsset::{Bitcoin, Usdb};

        assert_eq!(check_quote(&quote(Usdb), Usdb), Ok(()));
        assert_eq!(check_quote(&quote(Bitcoin), Bitcoin), Ok(()));
        assert!(check_quote(&quote(Usdb), Bitcoin).is_err());
        let mut fee_in_sats = quote(Usdb);
        fee_in_sats.fee = amount(Bitcoin, 3);
        assert!(check_quote(&fee_in_sats, Usdb).is_err());
        let mut same_asset_out = quote(Usdb);
        same_asset_out.amount_out = amount(Usdb, 1);
        assert!(check_quote(&same_asset_out, Usdb).is_err());

        let limits = ConversionLimits {
            min_from: Some(amount(Usdb, 1_000_000)),
            min_to: Some(amount(Bitcoin, 1_000)),
        };
        assert_eq!(check_limits(&limits, Usdb), Ok(()));
        assert!(check_limits(&limits, Bitcoin).is_err());
        assert_eq!(check_limits(&ConversionLimits::default(), Bitcoin), Ok(()));
        let swapped_min_to = ConversionLimits {
            min_from: None,
            min_to: Some(amount(Usdb, 1)),
        };
        assert!(check_limits(&swapped_min_to, Usdb).is_err());
    }

    /// Losing the bridge after a swap or an auto-convert switch was handed
    /// over must read as unknown, never as failed: the swap may have moved
    /// funds, and the switch may have started converting.
    #[tokio::test]
    async fn state_changing_calls_report_an_unknown_outcome_when_the_bridge_goes_quiet() {
        let (client, mut requests) = fake_client();

        let (result, method) = tokio::join!(
            client.swap("swap-h".to_string()),
            bridge_answers(&client, &mut requests, None)
        );
        assert!(
            matches!(method, Method::Swap(SwapParams { ref prepare_handle }) if prepare_handle == "swap-h")
        );
        // A swap is a self-transfer payment and keeps the send wording.
        let Err(SparkClientError::OutcomeUnknown { message, .. }) = result else {
            panic!("expected an unknown outcome, got {:?}", result);
        };
        assert_eq!(
            message,
            "The Spark bridge stopped responding after the payment was sent to it."
        );

        let (result, method) = tokio::join!(
            client.set_auto_convert(true),
            bridge_answers(&client, &mut requests, None)
        );
        assert!(matches!(
            method,
            Method::SetAutoConvert(SetAutoConvertParams { enabled: true })
        ));
        let Err(err @ SparkClientError::OutcomeUnknown { .. }) = result else {
            panic!("expected an unknown outcome, got {:?}", result);
        };
        assert_not_a_payment(&err);
    }

    /// An auto-convert change's unknown outcome, message and `Display` alike,
    /// names auto-convert and says nothing about a payment or a send.
    fn assert_not_a_payment(err: &SparkClientError) {
        let SparkClientError::OutcomeUnknown { message, .. } = err else {
            panic!("expected an unknown outcome, got {:?}", err);
        };
        let rendered = format!("{} | {}", message, err).to_lowercase();
        assert!(rendered.contains("auto-convert"), "{}", rendered);
        for forbidden in ["payment", "send", "sent", "failed"] {
            assert!(
                !rendered.contains(forbidden),
                "an auto-convert change says {:?}: {}",
                forbidden,
                rendered
            );
        }
    }

    /// The auto-convert switch times out like any state-changing request, but
    /// it isn't a payment, and nothing reads a late answer to it:
    /// `get_auto_convert` is how the gui finds out which way it went.
    #[tokio::test(start_paused = true)]
    async fn an_unanswered_auto_convert_change_holds_no_late_answer() {
        let (client, mut requests) = fake_client();

        // The bridge never answers; the paused clock runs out the deadline.
        let err = client
            .set_auto_convert(true)
            .await
            .expect_err("no answer came back");
        let request = requests.recv().await.expect("the change was sent");
        assert!(matches!(
            request.method,
            Method::SetAutoConvert(SetAutoConvertParams { enabled: true })
        ));
        assert!(
            matches!(err, SparkClientError::OutcomeUnknown { request_id, .. } if request_id == request.id),
            "{:?}",
            err
        );
        assert_not_a_payment(&err);

        // Not awaited, so the reader drops its late answer instead of holding
        // it where nothing would ever take it.
        assert!(!client
            .inner
            .awaiting_late
            .lock()
            .await
            .contains(&request.id));
        assert_eq!(route_response(false, false), ResponseRoute::Drop);

        // The reconciliation route.
        let state = AutoConvertState {
            enabled: true,
            breaker: AutoConvertBreaker::Ready,
            held_after_swap: false,
        };
        let (result, method) = tokio::join!(
            client.get_auto_convert(),
            bridge_answers(
                &client,
                &mut requests,
                Some(OkPayload::AutoConvert(state.clone()))
            )
        );
        assert!(matches!(method, Method::GetAutoConvert));
        assert_eq!(result.expect("state"), state);
    }

    /// The legacy Stable Balance switch is an auto-convert change on the
    /// bridge since S2b, which may wait for a conversion in flight before
    /// applying it. So it gets the state-changing deadline, not the 30 s
    /// query one, and a timeout or a made-up `NotConnected` answer reads as
    /// unknown, never as a definite failure (#965 F3).
    #[tokio::test(start_paused = true)]
    async fn the_legacy_stable_balance_switch_is_an_auto_convert_change() {
        let (client, mut requests) = fake_client();

        let started = tokio::time::Instant::now();
        let err = client
            .set_stable_balance(false)
            .await
            .expect_err("no answer came back");
        assert!(
            started.elapsed() >= SEND_SOFT_DEADLINE,
            "{:?}",
            started.elapsed()
        );
        let request = requests.recv().await.expect("the change was sent");
        assert!(matches!(
            request.method,
            Method::SetStableBalance(SetStableBalanceParams { enabled: false })
        ));
        assert!(
            matches!(err, SparkClientError::OutcomeUnknown { request_id, .. } if request_id == request.id),
            "{:?}",
            err
        );
        assert_not_a_payment(&err);
        assert!(!client
            .inner
            .awaiting_late
            .lock()
            .await
            .contains(&request.id));

        let (result, id) = tokio::join!(
            client.set_stable_balance(true),
            bridge_exits(&client, &mut requests)
        );
        let Err(err @ SparkClientError::OutcomeUnknown { .. }) = result else {
            panic!("expected an unknown outcome, got {:?}", result);
        };
        assert!(
            matches!(err, SparkClientError::OutcomeUnknown { request_id, .. } if request_id == id),
            "{:?}",
            err
        );
        assert_not_a_payment(&err);

        let (result, method) = tokio::join!(
            client.set_stable_balance(true),
            bridge_answers(&client, &mut requests, Some(OkPayload::SetStableBalance {}))
        );
        assert!(matches!(
            method,
            Method::SetStableBalance(SetStableBalanceParams { enabled: true })
        ));
        assert!(result.is_ok(), "{:?}", result);
    }

    /// Sends and swaps keep the payment wording on a timeout, and their late
    /// answers are still held for `take_late_outcome` /
    /// `take_late_swap_outcome`.
    #[tokio::test(start_paused = true)]
    async fn unanswered_sends_and_swaps_still_hold_their_late_answer() {
        let (client, mut requests) = fake_client();
        let expected = "The Spark bridge did not answer within 120s of being given this payment.";

        let err = client
            .send_payment("send-h".to_string(), Some("key".to_string()))
            .await
            .expect_err("no answer came back");
        let request = requests.recv().await.expect("the send went out");
        let SparkClientError::OutcomeUnknown {
            request_id,
            message,
        } = err
        else {
            panic!("expected an unknown outcome, got {:?}", err);
        };
        assert_eq!((request_id, message.as_str()), (request.id, expected));
        assert!(client
            .inner
            .awaiting_late
            .lock()
            .await
            .contains(&request_id));

        let err = client
            .swap("swap-h".to_string())
            .await
            .expect_err("no answer came back");
        let request = requests.recv().await.expect("the swap went out");
        let SparkClientError::OutcomeUnknown {
            request_id,
            message,
        } = err
        else {
            panic!("expected an unknown outcome, got {:?}", err);
        };
        assert_eq!((request_id, message.as_str()), (request.id, expected));
        assert!(client
            .inner
            .awaiting_late
            .lock()
            .await
            .contains(&request_id));
    }

    /// An SDK error can come after the SDK sent the swap's input to the pool,
    /// so it is an unknown outcome, on time or late. Only the refusals before
    /// submission are definite. A send's SDK error is unchanged. A
    /// `NotConnected` answer is unknown too; see
    /// `a_made_up_not_connected_answer_is_unknown_for_a_swap_or_auto_convert`.
    #[tokio::test]
    async fn a_swap_sdk_error_is_unknown_and_only_refusals_are_definite() {
        let (client, mut requests) = fake_client();

        let (result, id) = tokio::join!(
            client.swap("swap-h".to_string()),
            bridge_refuses(&client, &mut requests, ErrorKind::Sdk)
        );
        let Err(SparkClientError::OutcomeUnknown {
            request_id,
            message,
        }) = result
        else {
            panic!("expected an unknown outcome, got {:?}", result);
        };
        assert_eq!(request_id, id);
        // The SDK's "failed" stays in the log.
        assert!(message.contains("Check Activity"), "{}", message);
        assert!(!message.to_lowercase().contains("failed"), "{}", message);

        for kind in [ErrorKind::BadRequest, ErrorKind::ShuttingDown] {
            let (result, _) = tokio::join!(
                client.swap("swap-h".to_string()),
                bridge_refuses(&client, &mut requests, kind)
            );
            assert!(
                matches!(result, Err(SparkClientError::BridgeError { kind: k, .. }) if k == kind),
                "{:?}: {:?}",
                kind,
                result
            );
        }

        let (result, _) = tokio::join!(
            client.send_payment("send-h".to_string(), None),
            bridge_refuses(&client, &mut requests, ErrorKind::Sdk)
        );
        assert!(
            matches!(
                result,
                Err(SparkClientError::BridgeError {
                    kind: ErrorKind::Sdk,
                    ..
                })
            ),
            "{:?}",
            result
        );

        // The same rule for an answer that arrives after the deadline.
        for (id, kind) in [(61, ErrorKind::Sdk), (62, ErrorKind::BadRequest)] {
            client.inner.awaiting_late.lock().await.insert(id);
            client
                .inner
                .late_outcomes
                .lock()
                .await
                .insert(id, Response::err(id, kind, "Conversion failed"));
        }
        assert!(matches!(
            client.take_late_swap_outcome(61).await,
            Some(Err(SparkClientError::OutcomeUnknown { request_id: 61, .. }))
        ));
        assert!(matches!(
            client.take_late_swap_outcome(62).await,
            Some(Err(SparkClientError::BridgeError {
                kind: ErrorKind::BadRequest,
                ..
            }))
        ));
    }

    /// When the bridge dies, the writer and reader answer every request still
    /// in flight with a `NotConnected` error they make up. The swap or the
    /// auto-convert change may already have been made, so both read it as
    /// unknown, with copy that says how to reconcile. So does a send: the SDK
    /// may have paid before the bridge went, and a definite failure would
    /// retire the key and offer a second payment (#950).
    #[tokio::test]
    async fn a_made_up_not_connected_answer_is_unknown_for_a_swap_or_auto_convert() {
        let (client, mut requests) = fake_client();

        let (result, id) = tokio::join!(
            client.swap("swap-h".to_string()),
            bridge_exits(&client, &mut requests)
        );
        let Err(SparkClientError::OutcomeUnknown {
            request_id,
            message,
        }) = result
        else {
            panic!("expected an unknown outcome, got {:?}", result);
        };
        assert_eq!(request_id, id);
        assert!(
            message.contains("Check Activity before swapping again"),
            "{}",
            message
        );
        assert!(!message.to_lowercase().contains("failed"), "{}", message);

        let (result, id) = tokio::join!(
            client.set_auto_convert(true),
            bridge_exits(&client, &mut requests)
        );
        let Err(err @ SparkClientError::OutcomeUnknown { .. }) = result else {
            panic!("expected an unknown outcome, got {:?}", result);
        };
        assert!(
            matches!(err, SparkClientError::OutcomeUnknown { request_id, .. } if request_id == id),
            "{:?}",
            err
        );
        assert_not_a_payment(&err);
        assert!(
            err.to_string()
                .contains("Check whether auto-convert is on before changing it again"),
            "{}",
            err
        );

        let (result, id) = tokio::join!(
            client.send_payment("send-h".to_string(), Some("key".to_string())),
            bridge_exits(&client, &mut requests)
        );
        assert!(
            matches!(
                result,
                Err(SparkClientError::OutcomeUnknown { request_id, .. }) if request_id == id
            ),
            "{:?}",
            result
        );

        // A late swap answer follows the same rule.
        client.inner.awaiting_late.lock().await.insert(63);
        client.inner.late_outcomes.lock().await.insert(
            63,
            Response::err(
                63,
                ErrorKind::NotConnected,
                "Spark bridge subprocess exited unexpectedly",
            ),
        );
        assert!(matches!(
            client.take_late_swap_outcome(63).await,
            Some(Err(SparkClientError::OutcomeUnknown { request_id: 63, .. }))
        ));
    }

    /// The quote echoes the cap applied; one that isn't the cap asked for
    /// (50 bps when the gui sends `None`) is refused.
    #[tokio::test]
    async fn a_swap_quote_must_apply_the_slippage_cap_asked_for() {
        let (client, mut requests) = fake_client();

        for (asked, echoed, accepted) in [
            (None, CONVERSION_MAX_SLIPPAGE_BPS, true),
            (None, 100, false),
            (Some(75), 75, true),
            (Some(75), CONVERSION_MAX_SLIPPAGE_BPS, false),
        ] {
            let (result, _) = tokio::join!(
                client.prepare_swap(
                    SwapAmount::Exact {
                        amount_in: amount(WalletAsset::Usdb, 1_000_000)
                    },
                    asked
                ),
                bridge_answers(
                    &client,
                    &mut requests,
                    Some(OkPayload::PrepareSwap(PrepareSwapOk {
                        handle: "swap-h".to_string(),
                        quote: ConversionQuote {
                            max_slippage_bps: echoed,
                            ..quote(WalletAsset::Usdb)
                        },
                        limits: ConversionLimits::default(),
                    }))
                )
            );
            match result {
                Ok(_) => assert!(accepted, "{:?} accepted a {} bps quote", asked, echoed),
                Err(SparkClientError::Protocol(message)) => {
                    assert!(!accepted, "{:?} refused a {} bps quote", asked, echoed);
                    assert!(message.contains("slippage cap"), "{}", message);
                }
                Err(other) => panic!("unexpected error: {:?}", other),
            }
        }
    }

    /// The reads and estimates move nothing, so losing the bridge is a plain
    /// failure for them.
    #[tokio::test]
    async fn queries_report_the_bridge_unavailable_when_it_goes_quiet() {
        let (client, mut requests) = fake_client();
        let unavailable = |result: Result<(), SparkClientError>| {
            assert!(
                matches!(result, Err(SparkClientError::BridgeUnavailable(_))),
                "{:?}",
                result
            );
        };

        let (result, _) = tokio::join!(
            client.get_auto_convert(),
            bridge_answers(&client, &mut requests, None)
        );
        unavailable(result.map(drop));
        let (result, _) = tokio::join!(
            client.get_conversion_limits(WalletAsset::Usdb),
            bridge_answers(&client, &mut requests, None)
        );
        unavailable(result.map(drop));
        let (result, _) = tokio::join!(
            client.prepare_swap(
                SwapAmount::Max {
                    from: WalletAsset::Usdb
                },
                None
            ),
            bridge_answers(&client, &mut requests, None)
        );
        unavailable(result.map(drop));
        let (result, _) = tokio::join!(
            client.prepare_send_from("lnbc1".to_string(), None, WalletAsset::Usdb),
            bridge_answers(&client, &mut requests, None)
        );
        unavailable(result.map(drop));
        let (result, _) = tokio::join!(
            client.prepare_lnurl_pay_from(
                "shop@example.test".to_string(),
                21,
                None,
                WalletAsset::Usdb
            ),
            bridge_answers(&client, &mut requests, None)
        );
        unavailable(result.map(drop));
    }

    #[tokio::test]
    async fn each_call_sends_its_method_and_returns_its_payload() {
        let (client, mut requests) = fake_client();

        // The hold after a swap comes through as the bridge sent it.
        let state = AutoConvertState {
            enabled: true,
            breaker: AutoConvertBreaker::Ready,
            held_after_swap: true,
        };
        let (result, method) = tokio::join!(
            client.get_auto_convert(),
            bridge_answers(
                &client,
                &mut requests,
                Some(OkPayload::AutoConvert(state.clone()))
            )
        );
        assert!(matches!(method, Method::GetAutoConvert));
        assert_eq!(result.expect("state"), state);

        let (result, method) = tokio::join!(
            client.prepare_swap(
                SwapAmount::Exact {
                    amount_in: amount(WalletAsset::Usdb, 1_000_000)
                },
                Some(75)
            ),
            bridge_answers(
                &client,
                &mut requests,
                Some(OkPayload::PrepareSwap(PrepareSwapOk {
                    handle: "swap-h".to_string(),
                    quote: ConversionQuote {
                        max_slippage_bps: 75,
                        ..quote(WalletAsset::Usdb)
                    },
                    limits: ConversionLimits::default(),
                }))
            )
        );
        let Method::PrepareSwap(params) = method else {
            panic!("expected prepare_swap, got {:?}", method);
        };
        assert_eq!(params.amount.from_asset(), WalletAsset::Usdb);
        assert_eq!(params.max_slippage_bps, Some(75));
        assert_eq!(result.expect("estimate").handle, "swap-h");

        let (result, method) = tokio::join!(
            client.prepare_lnurl_pay_from(
                "shop@example.test".to_string(),
                1_500,
                Some("thanks".to_string()),
                WalletAsset::Usdb
            ),
            bridge_answers(
                &client,
                &mut requests,
                Some(OkPayload::PrepareSendFrom(send_from(
                    WalletAsset::Usdb,
                    Some(true),
                    Some(quote(WalletAsset::Usdb))
                )))
            )
        );
        let Method::PrepareLnurlPayFrom(params) = method else {
            panic!("expected prepare_lnurl_pay_from, got {:?}", method);
        };
        assert_eq!(params.amount_sat, 1_500);
        assert_eq!(params.comment.as_deref(), Some("thanks"));
        assert_eq!(params.pay_from, WalletAsset::Usdb);
        assert!(result.expect("preview").conversion.is_some());

        let (result, method) = tokio::join!(
            client.prepare_usdb_send("spark1".to_string(), 2_500_000),
            bridge_answers(
                &client,
                &mut requests,
                Some(OkPayload::PrepareSendFrom(usdb_to_address()))
            )
        );
        let Method::PrepareUsdbSend(params) = method else {
            panic!("expected prepare_usdb_send, got {:?}", method);
        };
        assert_eq!(params.input, "spark1");
        assert_eq!(params.amount, 2_500_000);
        let preview = result.expect("preview");
        assert_eq!(preview.prepare.method, "SparkAddress");

        // The legacy payload for a new method is a protocol error, not a
        // preview with the conversion silently missing.
        let (result, _) = tokio::join!(
            client.prepare_send_from("lnbc1".to_string(), Some(1_500), WalletAsset::Bitcoin),
            bridge_answers(
                &client,
                &mut requests,
                Some(OkPayload::PrepareSend(
                    send_from(WalletAsset::Bitcoin, None, None).prepare
                ))
            )
        );
        assert!(
            matches!(result, Err(SparkClientError::Protocol(_))),
            "{:?}",
            result
        );
    }

    #[tokio::test]
    async fn inconsistent_answers_are_refused_before_the_gui_sees_them() {
        let (client, mut requests) = fake_client();

        // Asked to pay from USDB, prepared a plain bitcoin send.
        let (result, _) = tokio::join!(
            client.prepare_send_from("lnbc1".to_string(), Some(1_500), WalletAsset::Usdb),
            bridge_answers(
                &client,
                &mut requests,
                Some(OkPayload::PrepareSendFrom(send_from(
                    WalletAsset::Bitcoin,
                    Some(false),
                    None
                )))
            )
        );
        let Err(SparkClientError::Protocol(message)) = result else {
            panic!("expected a protocol error, got {:?}", result);
        };
        assert!(message.contains("prepare_send_from"), "{}", message);

        // Paying from bitcoin, a token leg with no quote, through either
        // method: a direct token transfer or a dropped shortfall quote.
        let (result, _) = tokio::join!(
            client.prepare_send_from("spark1".to_string(), Some(1_500), WalletAsset::Bitcoin),
            bridge_answers(
                &client,
                &mut requests,
                Some(OkPayload::PrepareSendFrom(send_from(
                    WalletAsset::Bitcoin,
                    Some(true),
                    None
                )))
            )
        );
        let Err(SparkClientError::Protocol(message)) = result else {
            panic!("expected a protocol error, got {:?}", result);
        };
        assert!(
            message.contains("token leg without a conversion"),
            "{}",
            message
        );
        let (result, _) = tokio::join!(
            client.prepare_lnurl_pay_from(
                "shop@example.test".to_string(),
                1_500,
                None,
                WalletAsset::Bitcoin
            ),
            bridge_answers(
                &client,
                &mut requests,
                Some(OkPayload::PrepareSendFrom(send_from(
                    WalletAsset::Bitcoin,
                    Some(true),
                    None
                )))
            )
        );
        let Err(SparkClientError::Protocol(message)) = result else {
            panic!("expected a protocol error, got {:?}", result);
        };
        assert!(message.contains("prepare_lnurl_pay_from"), "{}", message);
        assert!(
            message.contains("token leg without a conversion"),
            "{}",
            message
        );

        // Asked to swap out of bitcoin, estimated a swap out of USDB.
        let (result, _) = tokio::join!(
            client.prepare_swap(
                SwapAmount::Max {
                    from: WalletAsset::Bitcoin
                },
                None
            ),
            bridge_answers(
                &client,
                &mut requests,
                Some(OkPayload::PrepareSwap(PrepareSwapOk {
                    handle: "swap-h".to_string(),
                    quote: quote(WalletAsset::Usdb),
                    limits: ConversionLimits::default(),
                }))
            )
        );
        assert!(
            matches!(result, Err(SparkClientError::Protocol(_))),
            "{:?}",
            result
        );

        let (result, _) = tokio::join!(
            client.get_conversion_limits(WalletAsset::Bitcoin),
            bridge_answers(
                &client,
                &mut requests,
                Some(OkPayload::GetConversionLimits(ConversionLimits {
                    min_from: Some(amount(WalletAsset::Usdb, 1)),
                    min_to: None,
                }))
            )
        );
        assert!(
            matches!(result, Err(SparkClientError::Protocol(_))),
            "{:?}",
            result
        );
    }

    #[tokio::test]
    async fn a_late_swap_answer_is_taken_once_and_typed() {
        let (client, _requests) = fake_client();
        let ok = Response::ok(
            41,
            OkPayload::Swap(SwapOk {
                swap_id: "payment-1".to_string(),
                status: coincube_spark_protocol::SwapStatus::Pending,
            }),
        );
        client.inner.awaiting_late.lock().await.insert(41);
        client.inner.late_outcomes.lock().await.insert(41, ok);

        let taken = client
            .take_late_swap_outcome(41)
            .await
            .expect("the answer arrived");
        assert_eq!(taken.expect("a swap").swap_id, "payment-1");
        assert!(client.take_late_swap_outcome(41).await.is_none());
        assert!(!client.inner.awaiting_late.lock().await.contains(&41));

        // A send's late answer is not a swap's.
        client.inner.late_outcomes.lock().await.insert(
            42,
            Response::ok(
                42,
                OkPayload::SendPayment(SendPaymentOk {
                    payment_id: "p".to_string(),
                    amount_sat: 1,
                    fee_sat: 0,
                    status: Some("Completed".to_string()),
                }),
            ),
        );
        assert!(matches!(
            client.take_late_swap_outcome(42).await,
            Some(Err(SparkClientError::Protocol(_)))
        ));
    }
}
