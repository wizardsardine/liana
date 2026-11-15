//! JSON-RPC server that reads [`Request`] frames from stdin, dispatches
//! them to the Spark SDK, and writes [`Response`]/[`Event`] frames to
//! stdout.
//!
//! Framing: line-delimited JSON. Each line is exactly one
//! [`coincube_spark_protocol::Frame`]. Errors while parsing a line produce
//! a [`Response`] with [`ErrorKind::BadRequest`] if the envelope has an
//! id, otherwise they're logged to stderr and the line is dropped.
//!
//! Concurrency: the server owns the SDK behind a [`tokio::sync::RwLock`]
//! so that `init` can mutate it exclusively while other requests read it
//! concurrently. A shutdown flag short-circuits new work while in-flight
//! requests drain.
//!
//! Scope: Phase 2 only implements Init / GetInfo / ListPayments / Shutdown.
//! Send/receive methods arrive when the UI starts consuming them.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use breez_sdk_spark::{
    AmountAdjustmentReason, CheckLightningAddressRequest, ClaimDeferredReason, ClaimDepositOutcome,
    ClaimDepositRequest, ConversionEstimate, ConversionOptions, ConversionType,
    CrossChainAddressDetails, CrossChainAddressFamily, CrossChainRouteFilter, CrossChainRoutePair,
    EventListener, GetInfoRequest, GetInfoResponse, InputType,
    LightningAddressInfo as SdkLightningAddressInfo, ListPaymentsRequest,
    ListUnclaimedDepositsRequest, LnurlPayRequest, MaxFee, MaxOnchainFee, OnchainConfirmationSpeed,
    PaymentDetails, PaymentRequest, PrepareLnurlPayRequest, PrepareLnurlPayResponse,
    PrepareSendPaymentRequest, PrepareSendPaymentResponse, ReceivePaymentMethod,
    ReceivePaymentRequest, RegisterLightningAddressRequest, SdkEvent, SendOnchainFeeQuote,
    SendPaymentMethod, SendPaymentOptions, SendPaymentRequest, SparkAsset, TokenBalance,
};
use coincube_spark_protocol::{
    AmountAdjustment, AssetAmount, AutoConvertState, CheckLightningAddressAvailableOk,
    ClaimDeferral, ClaimDepositOk, ConversionQuote, DepositInfo, ErrorKind, Event as ProtocolEvent,
    Frame, GetConversionLimitsParams, GetInfoOk, GetInfoParams, GetLightningAddressOk,
    GetUserSettingsOk, LightningAddressInfo as ProtocolLightningAddressInfo, ListPaymentsOk,
    ListUnclaimedDepositsOk, Method, OkPayload, ParseInputKind, ParseInputOk, PaymentDetailSummary,
    PaymentSummary, PrepareLnurlPayFromParams, PrepareSendFromOk, PrepareSendFromParams,
    PrepareSendOk, PrepareSwapOk, PrepareSwapParams, PrepareUsdbSendParams, ReceivePaymentOk,
    RegisterLightningAddressOk, Request, Response, SendPaymentOk, SetAutoConvertParams,
    SetStableBalanceParams, StableBalanceSnapshot, SwapOk, SwapParams, TokenPayment,
    UsdbBalanceSnapshot, WalletAsset, CONVERSION_MAX_SLIPPAGE_BPS,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, OnceCell, RwLock};
use uuid::Uuid;

use crate::auto_convert::{self, AutoConvert, SdkConverter};
use crate::sdk_adapter::{self, SdkHandle};
use crate::swap;

/// How long a pending prepare lives before the background sweep evicts
/// it. Picked at 5 minutes — long enough to cover human dwell time on
/// a Confirm screen (re-reading the fee, switching focus to confirm an
/// invoice on a phone, etc.) but short enough that a forgotten prepare
/// doesn't leak forever. The SDK's prepare responses are tied to
/// short-lived fee quotes anyway; sending after the quote expires
/// would fail at the SDK layer.
const PREPARE_TTL: Duration = Duration::from_secs(300);

/// How often the sweep task wakes up to evict expired prepares.
const PREPARE_SWEEP_INTERVAL: Duration = Duration::from_secs(60);

/// Run the stdin/stdout server until EOF on stdin or a `shutdown` RPC.
pub async fn run(receive_only: bool) -> anyhow::Result<()> {
    // Single writer task: serializes all stdout writes so responses and
    // events never interleave mid-line. We talk to it over an unbounded
    // channel so request handlers never block on IO.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Frame>();
    // `ServerState` holds a clone of the same sender so the event
    // listener registered in `handle_init` can push `Frame::Event`s
    // onto the same stdout stream the response handlers use.
    let state = Arc::new(ServerState {
        receive_only,
        ..ServerState::new(tx.clone())
    });

    // Phase 4f: background sweep that evicts pending-prepare entries
    // older than `PREPARE_TTL`. Uses a Weak reference so the sweep
    // task doesn't keep ServerState (and its event_tx sender) alive
    // after the main read loop exits — that would prevent the writer
    // task from observing channel closure and cause a deadlock at
    // shutdown.
    let sweep_weak = Arc::downgrade(&state);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(PREPARE_SWEEP_INTERVAL);
        tick.tick().await;
        loop {
            tick.tick().await;
            let Some(s) = sweep_weak.upgrade() else {
                break;
            };
            sweep_expired_prepares(&s).await;
        }
    });
    let writer_task = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(frame) = rx.recv().await {
            match serde_json::to_string(&frame) {
                Ok(line) => {
                    if stdout.write_all(line.as_bytes()).await.is_err()
                        || stdout.write_all(b"\n").await.is_err()
                        || stdout.flush().await.is_err()
                    {
                        // Parent hung up; nothing left to do.
                        break;
                    }
                }
                Err(e) => {
                    tracing::error!("failed to serialize outbound frame: {e}");
                }
            }
        }
    });

    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin).lines();

    while let Ok(Some(line)) = reader.next_line().await {
        if line.trim().is_empty() {
            continue;
        }

        let frame: Frame = match serde_json::from_str(&line) {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!("dropping unparseable line: {e}");
                continue;
            }
        };

        let request = match frame {
            Frame::Request(r) => r,
            Frame::Response(_) | Frame::Event(_) => {
                tracing::warn!("ignoring unexpected response/event frame from parent");
                continue;
            }
        };

        let id = request.id;
        // Shutdown is handled inline so we can exit the read loop after
        // the response is flushed. Everything else is spawned so slow
        // SDK calls don't block subsequent requests.
        if matches!(request.method, Method::Shutdown) {
            let _ = tx.send(Frame::Response(Response::ok(id, OkPayload::Shutdown {})));
            break;
        }

        let state_clone = Arc::clone(&state);
        let tx_clone = tx.clone();
        tokio::spawn(async move {
            let response = handle_request(request, state_clone).await;
            let _ = tx_clone.send(Frame::Response(response));
        });
    }

    // Drop ALL senders so the writer task's `rx.recv()` returns None
    // and it can exit cleanly. `state.event_tx` is a clone of the
    // same channel — if we only drop `tx` but keep `state` alive,
    // the writer hangs forever waiting for a message that will never
    // come.
    drop(tx);
    drop(state);
    let _ = writer_task.await;
    Ok(())
}

struct ServerState {
    receive_only: bool,
    /// `None` until `init` succeeds, then `Some` for the process lifetime.
    sdk: RwLock<Option<SdkHandle>>,
    /// Guards the init path so two concurrent `init` requests can't
    /// both try to build an SDK at the same time.
    init_lock: Mutex<()>,
    /// Pending `prepare_send_payment` responses keyed by the opaque
    /// handle the gui receives. The gui echoes the handle back on
    /// `send_payment`; the bridge looks it up here and removes the
    /// entry (single-use). Storing the full SDK struct bridge-side
    /// means the gui doesn't have to round-trip a complex nested
    /// response over JSON-RPC.
    ///
    /// Phase 4f adds an `Instant` alongside each entry so a background
    /// sweep task can evict prepares older than [`PREPARE_TTL`] (5
    /// minutes) — a gui that prepares without sending no longer
    /// leaks for the process lifetime.
    pending_prepares: Mutex<HashMap<String, (Instant, PrepareSendPaymentResponse)>>,
    /// Pending `prepare_lnurl_pay` responses. Separate from
    /// `pending_prepares` because the SDK's `lnurl_pay(...)` call
    /// takes a different request struct than `send_payment(...)`.
    /// [`handle_send_payment`] checks both maps and dispatches to the
    /// right SDK method based on which one contains the handle. Same
    /// TTL eviction policy as `pending_prepares`.
    pending_lnurl_prepares: Mutex<HashMap<String, (Instant, PrepareLnurlPayResponse)>>,
    /// Pending `prepare_swap` answers, under the handle the gui echoes back
    /// on `swap`. Separate from the send maps, so `send_payment` can never
    /// execute a swap and `swap` never a send. Single-use, and swept with
    /// the same TTL.
    pending_swaps: Mutex<HashMap<String, (Instant, swap::PreparedSwap)>>,
    /// Swaps answered before they finished, re-read on every sync
    /// ([`swap::refresh`]).
    swaps: Arc<Mutex<swap::SwapTracker>>,
    /// Clone of the outbound frame sender. Stored here so `handle_init`
    /// can hand a copy to the Spark SDK event listener — the listener
    /// pushes `Frame::Event`s on this channel the same way request
    /// handlers push `Frame::Response`s, so stdout stays interleave-safe.
    event_tx: tokio::sync::mpsc::UnboundedSender<Frame>,
    /// Bridge-driven auto-convert for the Cube this bridge serves, loaded
    /// from its Spark storage directory by a successful `init`. See
    /// [`crate::auto_convert`].
    auto_convert: std::sync::OnceLock<Arc<Mutex<AutoConvert>>>,
    /// Wakes the auto-convert worker; set with [`Self::auto_convert`]. A
    /// swap out of USDB holds the worker's passes, and wakes it once done.
    wake_auto_convert: std::sync::OnceLock<tokio::sync::mpsc::UnboundedSender<()>>,
    /// The wallet's own Spark address, the destination of every
    /// auto-convert self-transfer. Read from the SDK once.
    own_spark_address: Arc<OnceCell<String>>,
}

impl ServerState {
    fn new(event_tx: tokio::sync::mpsc::UnboundedSender<Frame>) -> Self {
        Self {
            receive_only: false,
            sdk: RwLock::new(None),
            init_lock: Mutex::new(()),
            pending_prepares: Mutex::new(HashMap::new()),
            pending_lnurl_prepares: Mutex::new(HashMap::new()),
            pending_swaps: Mutex::new(HashMap::new()),
            swaps: Arc::new(Mutex::new(swap::SwapTracker::default())),
            event_tx,
            auto_convert: std::sync::OnceLock::new(),
            wake_auto_convert: std::sync::OnceLock::new(),
            own_spark_address: Arc::new(OnceCell::new()),
        }
    }
}

/// Whether SDK Stable Balance was on for this Cube before S2b: its saved
/// active label, read without configuring Stable Balance
/// ([`auto_convert::legacy_stable_balance_label`]). Any failure reads as off,
/// which converts nothing.
async fn legacy_stable_balance_on(storage_dir: &str, sdk: &SdkHandle) -> bool {
    let identity = match sdk
        .sdk
        .get_info(GetInfoRequest {
            ensure_synced: Some(false),
        })
        .await
    {
        Ok(info) => info.identity_pubkey,
        Err(e) => {
            tracing::warn!("could not read the wallet identity to migrate Stable Balance: {e}");
            return false;
        }
    };
    match auto_convert::legacy_stable_balance_label(storage_dir, &identity).await {
        Ok(label) => label.as_deref() == Some(auto_convert::LEGACY_USDB_LABEL),
        Err(e) => {
            tracing::warn!("could not read the SDK Stable Balance setting to migrate it: {e}");
            false
        }
    }
}

async fn handle_request(request: Request, state: Arc<ServerState>) -> Response {
    let id = request.id;
    if state.receive_only
        && !matches!(
            &request.method,
            Method::Init(_)
                | Method::GetInfo(GetInfoParams {
                    ensure_synced: None | Some(false)
                })
                | Method::ReceiveSpark
                | Method::ReceiveOnchain(coincube_spark_protocol::ReceiveOnchainParams {
                    new_address: None | Some(false),
                })
        )
    {
        return Response::err(
            id,
            ErrorKind::BadRequest,
            "method unavailable in receive-only mode",
        );
    }
    match request.method {
        Method::Init(params) => handle_init(id, params, state).await,
        Method::GetInfo(params) => handle_get_info(id, params, state).await,
        Method::ListPayments(params) => handle_list_payments(id, params, state).await,
        Method::ParseInput(params) => handle_parse_input(id, params, state).await,
        Method::PrepareSend(params) => handle_prepare_send(id, params, state).await,
        Method::PrepareLnurlPay(params) => handle_prepare_lnurl_pay(id, params, state).await,
        Method::GetCrossChainRoutes(params) => {
            handle_get_cross_chain_routes(id, params, state).await
        }
        Method::PrepareCrossChain(params) => handle_prepare_cross_chain(id, params, state).await,
        Method::SendPayment(params) => handle_send_payment(id, params, state).await,
        Method::ReceiveBolt11(params) => handle_receive_bolt11(id, params, state).await,
        Method::ReceiveOnchain(params) => handle_receive_onchain(id, params, state).await,
        Method::ReceiveSpark => handle_receive_spark(id, state).await,
        Method::ListUnclaimedDeposits => handle_list_unclaimed_deposits(id, state).await,
        Method::ClaimDeposit(params) => handle_claim_deposit(id, params, state).await,
        Method::GetUserSettings => handle_get_user_settings(id, state).await,
        Method::SetStableBalance(params) => handle_set_stable_balance(id, params, state).await,
        Method::CheckLightningAddressAvailable(params) => {
            handle_check_lightning_address_available(id, params, state).await
        }
        Method::RegisterLightningAddress(params) => {
            handle_register_lightning_address(id, params, state).await
        }
        Method::GetLightningAddress => handle_get_lightning_address(id, state).await,
        Method::DeleteLightningAddress => handle_delete_lightning_address(id, state).await,
        Method::GetAutoConvert => handle_get_auto_convert(id, state).await,
        Method::SetAutoConvert(params) => handle_set_auto_convert(id, params, state).await,
        Method::GetConversionLimits(params) => {
            handle_get_conversion_limits(id, params, state).await
        }
        Method::PrepareSwap(params) => handle_prepare_swap(id, params, state).await,
        Method::Swap(params) => handle_swap(id, params, state).await,
        Method::PrepareSendFrom(params) => handle_prepare_send_from(id, params, state).await,
        Method::PrepareLnurlPayFrom(params) => {
            handle_prepare_lnurl_pay_from(id, params, state).await
        }
        Method::PrepareUsdbSend(params) => handle_prepare_usdb_send(id, params, state).await,
        Method::Shutdown => {
            // Handled inline in the read loop — this branch exists so the
            // match is exhaustive.
            Response::ok(id, OkPayload::Shutdown {})
        }
    }
}

/// The answer to a method the protocol defines but this bridge doesn't
/// implement yet. `BadRequest` because an older gui can't parse a new
/// [`ErrorKind`] — its reader stops on an unparseable line.
async fn handle_init(
    id: u64,
    params: coincube_spark_protocol::InitParams,
    state: Arc<ServerState>,
) -> Response {
    let _guard = state.init_lock.lock().await;
    if state.sdk.read().await.is_some() {
        return Response::err(id, ErrorKind::AlreadyConnected, "init already succeeded");
    }

    // Phase 2 skeleton only supports Mainnet; Regtest requires extra Spark
    // config we're not threading yet. Error out cleanly so the caller
    // knows the knob exists.
    if !matches!(params.network, coincube_spark_protocol::Network::Mainnet) {
        return Response::err(
            id,
            ErrorKind::BadRequest,
            "only mainnet is supported in the Phase 2 bridge skeleton",
        );
    }

    let storage_dir_str = params.storage_dir.clone();
    let storage_dir = std::path::PathBuf::from(&params.storage_dir);
    match sdk_adapter::connect_mainnet(
        params.api_key,
        params.mnemonic,
        params.mnemonic_passphrase,
        params.storage_dir,
        state.receive_only,
    )
    .await
    {
        Ok(handle) if state.receive_only => {
            *state.sdk.write().await = Some(handle);
            Response::ok(id, OkPayload::Init {})
        }
        Ok(handle) => {
            // Auto-convert's state is in place, and its worker running,
            // before the listener can report a receive. A Cube with no
            // bridge state yet adopts SDK Stable Balance's "on".
            let legacy_on = !AutoConvert::has_state_file(&storage_dir)
                && legacy_stable_balance_on(&storage_dir_str, &handle).await;
            let auto_convert = Arc::new(Mutex::new(AutoConvert::load_or_adopt(
                &storage_dir,
                legacy_on,
            )));
            let _ = state.auto_convert.set(Arc::clone(&auto_convert));
            let (wake_tx, wake_rx) = tokio::sync::mpsc::unbounded_channel();
            let _ = state.wake_auto_convert.set(wake_tx.clone());
            // Each pass borrows the SDK and the stdout sender only while it
            // runs, through a weak state reference, so the worker doesn't keep
            // either alive past shutdown. Before the SDK is published a pass
            // is skipped, and the wake below follows publication.
            let weak = Arc::downgrade(&state);
            tokio::spawn(auto_convert::run_worker(
                wake_rx,
                Arc::clone(&auto_convert),
                move || {
                    let weak = weak.clone();
                    async move {
                        let state = weak.upgrade()?;
                        let sdk = state.sdk.read().await.clone()?;
                        let events = state.event_tx.clone();
                        let converter =
                            SdkConverter::new(sdk, Arc::clone(&state.own_spark_address));
                        let emit = move |event| {
                            let _ = events.send(Frame::Event(event));
                        };
                        Some((converter, emit))
                    }
                },
            ));

            // Swaps answered before they finished are re-read on each sync,
            // through a weak state reference as above.
            let (swap_wake_tx, swap_wake_rx) = tokio::sync::mpsc::unbounded_channel();
            let weak = Arc::downgrade(&state);
            tokio::spawn(swap::run_status_worker(
                swap_wake_rx,
                Arc::clone(&state.swaps),
                move || {
                    let weak = weak.clone();
                    async move {
                        let state = weak.upgrade()?;
                        let sdk = state.sdk.read().await.clone()?;
                        let events = state.event_tx.clone();
                        let backend = SdkConverter::new(sdk, Arc::clone(&state.own_spark_address));
                        let emit = move |event| {
                            let _ = events.send(Frame::Event(event));
                        };
                        Some((backend, emit))
                    }
                },
            ));

            // Register an event listener before making the handle
            // visible to other request handlers. `add_event_listener`
            // returns a listener id string (not a Result) that we could
            // use for `remove_event_listener` if we wanted to rotate
            // listeners — Phase 4d just holds it for the process
            // lifetime, so we drop the id.
            let listener = BridgeEventListener {
                tx: state.event_tx.clone(),
                auto_convert,
                wake_auto_convert: wake_tx.clone(),
                wake_swap_status: swap_wake_tx,
            };
            let _listener_id = handle.sdk.add_event_listener(Box::new(listener)).await;
            *state.sdk.write().await = Some(handle);
            // Now that passes can convert: anything left pending by the last
            // session, or counted while the SDK wasn't published yet.
            let _ = wake_tx.send(());
            Response::ok(id, OkPayload::Init {})
        }
        Err(e) => Response::err(id, ErrorKind::Sdk, format!("spark connect failed: {e}")),
    }
}

/// Spark SDK → protocol event adapter.
///
/// Registered on the SDK via `add_event_listener` once `handle_init`
/// has successfully connected. Every `SdkEvent` fires `on_event`, which
/// translates to a `ProtocolEvent` and pushes it into the shared frame
/// writer. The writer task serializes the `Frame::Event` to a single
/// line on stdout so the gui's reader picks it up alongside
/// `Frame::Response`s without framing ambiguity.
struct BridgeEventListener {
    tx: tokio::sync::mpsc::UnboundedSender<Frame>,
    /// Counts each incoming bitcoin payment as it is reported, so it is
    /// saved even if a conversion is running or the bridge stops next.
    auto_convert: Arc<Mutex<AutoConvert>>,
    /// Wakes the auto-convert worker ([`auto_convert::run_worker`]) after a
    /// receive counted.
    wake_auto_convert: tokio::sync::mpsc::UnboundedSender<()>,
    /// Wakes the swap status worker ([`swap::run_status_worker`]) on each
    /// sync.
    wake_swap_status: tokio::sync::mpsc::UnboundedSender<()>,
}

#[async_trait]
impl EventListener for BridgeEventListener {
    async fn on_event(&self, event: SdkEvent) {
        let protocol_event = match event {
            SdkEvent::Synced => {
                let _ = self.wake_swap_status.send(());
                Some(ProtocolEvent::Synced)
            }
            SdkEvent::PaymentSucceeded { payment } => {
                if let Some(receipt) = auto_convert::eligible_receive(&payment) {
                    if self.auto_convert.lock().await.note_receive(&receipt) {
                        let _ = self.wake_auto_convert.send(());
                    }
                }
                Some(ProtocolEvent::PaymentSucceeded {
                    amount_sat: payment.amount as i64,
                    bolt11: extract_bolt11(&payment),
                    token: payment_token(&payment),
                    id: payment.id,
                })
            }
            SdkEvent::PaymentPending { payment } => Some(ProtocolEvent::PaymentPending {
                id: payment.id,
                amount_sat: payment.amount as i64,
            }),
            SdkEvent::PaymentFailed { payment } => Some(ProtocolEvent::PaymentFailed {
                id: payment.id,
                amount_sat: payment.amount as i64,
            }),
            // All three deposit-related SDK events collapse to a
            // single `DepositsChanged` signal — the gui's Receive
            // panel responds by re-running `list_unclaimed_deposits`
            // regardless of which of the three triggered the
            // refresh.
            SdkEvent::UnclaimedDeposits { .. }
            | SdkEvent::ClaimedDeposits { .. }
            | SdkEvent::NewDeposits { .. } => Some(ProtocolEvent::DepositsChanged),
            // Phase 4g: forward the LNURL registration state. Fires
            // when the user claims/drops an address on this device,
            // or when realtime-sync replays the change from another
            // device. The gui refreshes its settings view and
            // auto-re-registers from the DB-reserved username if
            // the SDK report went Some → None unexpectedly.
            SdkEvent::LightningAddressChanged { lightning_address } => {
                Some(ProtocolEvent::LightningAddressChanged {
                    info: lightning_address.map(sdk_address_info_to_protocol),
                })
            }
            // Optimization events stay swallowed until a panel
            // needs them. (0.19.0 renamed this from `Optimization`.)
            SdkEvent::AutoOptimization { .. } => None,
            // 0.25.0: progress of a unilateral exit (the user leaving Spark
            // without the operators' cooperation). Nothing in the gui drives
            // one, so there is nothing to show yet.
            SdkEvent::UnilateralExitStateChanged => None,
            // 0.28.0: a stored payment's details changed after it settled
            // (e.g. a cross-chain receive's conversion info arriving late).
            // Its status already reached the gui; this only re-reads lists.
            SdkEvent::PaymentMetadataUpdated { payment } => {
                Some(ProtocolEvent::PaymentUpdated { id: payment.id })
            }
            // 0.28.0: sync found funds that left the balance and wait to be
            // recovered on-chain with `recover_funds`. The gui reads the
            // total from `get_info` (refreshed on the `Synced` that follows);
            // nothing in it drives a recovery yet.
            SdkEvent::RecoverableFunds {
                recoverable_funds_sats,
            } => {
                tracing::warn!(
                    "SDK reports {recoverable_funds_sats} sats awaiting on-chain recovery"
                );
                None
            }
            // 0.28.0: a failed Stable Balance conversion. The bridge
            // configures no Stable Balance, so the SDK has none to report;
            // logged in case one ever runs. `retry_in_secs` is unset for a
            // per-receive conversion, which the SDK does not retry.
            SdkEvent::StableBalanceConversionFailed {
                conversion,
                error,
                retry_in_secs,
            } => {
                match retry_in_secs {
                    Some(secs) => tracing::info!(
                        "SDK reports {conversion:?} conversion failed, retrying in {secs}s: {error}"
                    ),
                    None => tracing::info!("SDK reports {conversion:?} conversion failed: {error}"),
                }
                None
            }
        };

        if let Some(ev) = protocol_event {
            let _ = self.tx.send(Frame::Event(ev));
        }
    }
}

/// Phase 4f: extract the BOLT11 invoice from a Spark `Payment` if it
/// was a Lightning payment. Returned in the `PaymentSucceeded` event
/// so the gui's Receive panel can correlate against a specific
/// generated invoice instead of advancing on any incoming payment.
///
/// For non-Lightning payments (Spark transfers, on-chain, token), or
/// for Lightning payments where the SDK didn't populate `details`,
/// returns `None` and the gui falls back to the Phase 4d behavior of
/// advancing on any incoming payment.
fn extract_bolt11(payment: &breez_sdk_spark::Payment) -> Option<String> {
    match payment.details.as_ref()? {
        PaymentDetails::Lightning { invoice, .. } => Some(invoice.clone()),
        _ => None,
    }
}

async fn handle_get_info(
    id: u64,
    params: coincube_spark_protocol::GetInfoParams,
    state: Arc<ServerState>,
) -> Response {
    let sdk = match state.sdk.read().await.clone() {
        Some(s) => s,
        None => {
            return Response::err(
                id,
                ErrorKind::NotConnected,
                "init must succeed before get_info",
            );
        }
    };

    match sdk
        .sdk
        .get_info(GetInfoRequest {
            ensure_synced: params.ensure_synced,
        })
        .await
    {
        Ok(info) => Response::ok(id, OkPayload::GetInfo(get_info_ok(info))),
        Err(e) => Response::err(id, ErrorKind::Sdk, format!("get_info failed: {e}")),
    }
}

/// The `get_info` answer for an SDK [`GetInfoResponse`].
///
/// `balance_sats` stays the SDK's bitcoin-only figure. The USDB holding is
/// reported twice: as the legacy [`StableBalanceSnapshot`], clamped to `u64`
/// for callers that still read it, and as [`usdb_balance`] at full precision.
fn get_info_ok(info: GetInfoResponse) -> GetInfoOk {
    let stable_balance = info
        .token_balances
        .get(crate::sdk_adapter::USDB_MAINNET_TOKEN_IDENTIFIER)
        .filter(|tb| tb.balance > 0)
        .map(|tb| StableBalanceSnapshot {
            balance: clamp_u128_to_u64(tb.balance),
            decimals: tb.token_metadata.decimals,
            ticker: tb.token_metadata.ticker.clone(),
        });
    GetInfoOk {
        balance_sats: info.balance_sats,
        identity_pubkey: info.identity_pubkey,
        stable_balance,
        usdb_balance: usdb_balance(&info.token_balances),
        recoverable_funds_sats: info.recoverable_funds_sats,
    }
}

/// The wallet's USDB holding at the SDK's full `u128` precision, or `None`
/// when it holds none.
///
/// Only the configured USDB token counts: the entry under
/// [`sdk_adapter::USDB_MAINNET_TOKEN_IDENTIFIER`] whose own metadata names
/// that token. The SDK keys the map by the metadata's identifier, so the
/// second check holds today; it keeps a mis-keyed entry from being shown as
/// USDB. Every other token the wallet holds is ignored.
fn usdb_balance(token_balances: &HashMap<String, TokenBalance>) -> Option<UsdbBalanceSnapshot> {
    token_balances
        .get(sdk_adapter::USDB_MAINNET_TOKEN_IDENTIFIER)
        .filter(|tb| {
            tb.balance > 0
                && tb.token_metadata.identifier == sdk_adapter::USDB_MAINNET_TOKEN_IDENTIFIER
        })
        .map(|tb| UsdbBalanceSnapshot {
            token_identifier: tb.token_metadata.identifier.clone(),
            balance: tb.balance,
            decimals: tb.token_metadata.decimals,
            ticker: tb.token_metadata.ticker.clone(),
        })
}

async fn handle_list_payments(
    id: u64,
    params: coincube_spark_protocol::ListPaymentsParams,
    state: Arc<ServerState>,
) -> Response {
    let sdk = match state.sdk.read().await.clone() {
        Some(s) => s,
        None => {
            return Response::err(
                id,
                ErrorKind::NotConnected,
                "init must succeed before list_payments",
            );
        }
    };

    match sdk
        .sdk
        .list_payments(ListPaymentsRequest {
            limit: params.limit,
            offset: params.offset,
            sort_ascending: Some(false),
            type_filter: None,
            status_filter: None,
            asset_filter: None,
            payment_details_filter: None,
            from_timestamp: None,
            to_timestamp: None,
        })
        .await
    {
        Ok(resp) => {
            let payments = resp
                .payments
                .into_iter()
                .map(payment_to_summary)
                .collect::<Vec<_>>();
            Response::ok(id, OkPayload::ListPayments(ListPaymentsOk { payments }))
        }
        Err(e) => Response::err(id, ErrorKind::Sdk, format!("list_payments failed: {e}")),
    }
}

/// Collapse a Spark SDK `Payment` into the compact [`PaymentSummary`] the
/// Phase 2 protocol carries. We intentionally stringify the status /
/// direction so the protocol crate doesn't need to mirror Spark's enums
/// yet — later phases can replace these with typed variants as the UI
/// starts branching on them.
///
/// `Payment.amount` is overloaded in the SDK ("satoshis or token base
/// units" depending on `method`). For `PaymentMethod::Token` payments
/// the value is in token base units (e.g. microUSDB at 6 decimals),
/// not sats — treating it as sats produces the wrong fiat figure and
/// a wildly inflated headline number. Token payments populate the
/// dedicated `token_*` fields and zero out the sat fields; the gui
/// renders them with the token's own ticker / decimals.
/// A human description for a cross-chain conversion leg, so payment history
/// names the real destination (e.g. "Sent USDT on Solana") instead of the raw
/// hold-invoice memo the Lightning source leg carries (e.g. "Send to TBTC
/// address"). A Boltz reverse swap / Orchestra order carries the destination
/// asset + chain on its `ConversionInfo`; AMM (Spark↔Spark token) conversions
/// and plain Lightning payments have no cross-chain destination, so they keep
/// the invoice memo.
fn cross_chain_leg_description(info: Option<&breez_sdk_spark::ConversionInfo>) -> Option<String> {
    use breez_sdk_spark::ConversionInfo;
    let (asset, chain) = match info? {
        ConversionInfo::Boltz { asset, chain, .. }
        | ConversionInfo::Orchestra { asset, chain, .. } => (asset, chain),
        ConversionInfo::Amm { .. } => return None,
    };
    // Chain names arrive lowercase ("solana"); title-case the first char for
    // display. Asset tickers are already upper ("USDT").
    let mut chars = chain.chars();
    let chain = match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => chain.clone(),
    };
    Some(format!("Sent {asset} on {chain}"))
}

/// The token a token payment moved, for the `PaymentSucceeded` event: its
/// amount is then in that token's base units, not sats.
///
/// A token payment without its metadata still gets a (nameless) token, so
/// its base units are never read as sats.
fn payment_token(p: &breez_sdk_spark::Payment) -> Option<coincube_spark_protocol::PaymentToken> {
    match &p.details {
        Some(PaymentDetails::Token { metadata, .. }) => {
            Some(coincube_spark_protocol::PaymentToken {
                identifier: metadata.identifier.clone(),
                ticker: metadata.ticker.clone(),
                decimals: metadata.decimals,
                usdb: metadata.identifier == crate::sdk_adapter::USDB_MAINNET_TOKEN_IDENTIFIER,
            })
        }
        _ if matches!(p.method, breez_sdk_spark::PaymentMethod::Token) => {
            Some(coincube_spark_protocol::PaymentToken {
                identifier: String::new(),
                ticker: String::new(),
                decimals: 0,
                usdb: false,
            })
        }
        _ => None,
    }
}

fn payment_to_summary(p: breez_sdk_spark::Payment) -> PaymentSummary {
    let description = match &p.details {
        Some(PaymentDetails::Lightning {
            description,
            conversion_info,
            ..
        }) => cross_chain_leg_description(conversion_info.as_ref()).or_else(|| description.clone()),
        _ => None,
    };
    let token_metadata = match &p.details {
        Some(PaymentDetails::Token { metadata, .. }) => Some(metadata.clone()),
        _ => None,
    };
    let is_token = matches!(p.method, breez_sdk_spark::PaymentMethod::Token);
    let mut details = detail_summary(p.details.as_ref());
    details.conversion = swap::conversion_summary(&p);

    let (amount_sat, fees_sat, token_amount, token_decimals, token_ticker) = if is_token {
        let metadata = token_metadata;
        (
            0i64,
            0u64,
            Some(clamp_u128_to_u64(p.amount)),
            metadata.as_ref().map(|m| m.decimals),
            metadata.map(|m| m.ticker),
        )
    } else {
        // Match the previous behavior: ship the magnitude as i64 and
        // let the gui derive direction from the `direction` field.
        // Consumers all call `unsigned_abs()` on this, so flipping the
        // sign here would silently no-op for some and double-flip for
        // others.
        (
            clamp_u128_to_u64(p.amount) as i64,
            clamp_u128_to_u64(p.fees),
            None,
            None,
            None,
        )
    };

    PaymentSummary {
        id: p.id,
        amount_sat,
        fees_sat,
        token_amount,
        token_decimals,
        token_ticker,
        timestamp: p.timestamp,
        status: format!("{:?}", p.status),
        direction: format!("{:?}", p.payment_type),
        method: format!("{}", p.method),
        description,
        details,
    }
}

/// The identifiers and destination Activity shows for one payment. The
/// conversion is read from the whole payment ([`payment_to_summary`]).
fn detail_summary(details: Option<&PaymentDetails>) -> PaymentDetailSummary {
    let non_empty = |value: &str| Some(value.to_owned()).filter(|value| !value.is_empty());
    match details {
        Some(PaymentDetails::Lightning {
            invoice,
            htlc_details,
            lnurl_pay_info,
            ..
        }) => PaymentDetailSummary {
            payment_hash: non_empty(&htlc_details.payment_hash),
            invoice: non_empty(invoice),
            lightning_address: lnurl_pay_info
                .as_ref()
                .and_then(|info| info.ln_address.as_deref())
                .and_then(non_empty),
            tx_id: None,
            // From the whole payment: `swap::conversion_summary`.
            conversion: None,
        },
        Some(PaymentDetails::Spark {
            invoice_details,
            htlc_details,
            ..
        }) => PaymentDetailSummary {
            payment_hash: htlc_details
                .as_ref()
                .and_then(|htlc| non_empty(&htlc.payment_hash)),
            invoice: invoice_details
                .as_ref()
                .and_then(|invoice| non_empty(&invoice.invoice)),
            ..Default::default()
        },
        Some(PaymentDetails::Token {
            tx_hash,
            invoice_details,
            ..
        }) => PaymentDetailSummary {
            invoice: invoice_details
                .as_ref()
                .and_then(|invoice| non_empty(&invoice.invoice)),
            tx_id: non_empty(tx_hash),
            ..Default::default()
        },
        Some(PaymentDetails::Withdraw { tx_id, .. } | PaymentDetails::Deposit { tx_id, .. }) => {
            PaymentDetailSummary {
                tx_id: non_empty(tx_id),
                ..Default::default()
            }
        }
        None => PaymentDetailSummary::default(),
    }
}

// ---------------------------------------------------------------------------
// Phase 4c write-path handlers
// ---------------------------------------------------------------------------

/// Rejects a prepare that came back carrying a token-conversion leg the wallet
/// cannot actually fund. `None` means the prepare is fine to hand to the gui.
///
/// **Why a plain sats send can grow a conversion leg at all.** Both
/// [`handle_prepare_send`] and [`handle_prepare_lnurl_pay`] first prepare with
/// `conversion_options: None` and `token_identifier: None` (and
/// [`handle_prepare_cross_chain`] attaches its own top-up; see
/// [`prepare_cross_chain_once`]). The SDK's Stable
/// Balance, which used to auto-attach a `ToBitcoin { <stable token> }`
/// conversion when the sat balance was below `amount + fee`, is no longer
/// configured (S2b). A conversion on the response is now the bridge's own,
/// attached when the wallet holds USDB — see [`usdb_shortfall_conversion`]. It
/// is a *feature*, and this function deliberately lets it through when it can
/// work.
///
/// **Why it needs a guard.** The SDK validates that conversion
/// against the AMM pool, not against the wallet's own token balance. A wallet
/// without enough of the stable token prepares cleanly, shows a fee, and
/// then dies inside Flashnet at send time with
/// `Wallet: Service error: generic error: Token outputs not found` — an
/// insufficient-funds condition wearing an unreadable disguise, arriving after
/// the user has already pressed Confirm and Send. Checking the holding here
/// turns it into a plain-language refusal on the amount screen instead.
///
/// The balance read forces a sync: a stale cache that under-reports the holding
/// would reject a send that would have worked, which is the worse mistake of the
/// two. It only runs on the rare prepare that actually carries a conversion.
///
/// `pay_from` picks the copy: paying from bitcoin, the conversion only made
/// up a shortfall; paying from USDB, it is the whole payment.
async fn unfundable_conversion_error(
    id: u64,
    sdk: &SdkHandle,
    estimate: Option<&ConversionEstimate>,
    pay_from: WalletAsset,
) -> Option<Response> {
    let estimate = estimate?;
    let ConversionType::ToBitcoin {
        from_token_identifier,
    } = &estimate.options.conversion_type
    else {
        // `FromBitcoin` cannot legitimately appear on these paths — the gui
        // never requests one, and the shortfall fallback only attaches
        // `ToBitcoin`. Refuse rather than pass an unrecognised leg through to
        // the AMM on the user's behalf.
        return Some(Response::err(
            id,
            ErrorKind::Sdk,
            "prepare returned an unexpected token-conversion leg",
        ));
    };

    let held = match sdk
        .sdk
        .get_info(GetInfoRequest {
            ensure_synced: Some(true),
        })
        .await
    {
        Ok(info) => info
            .token_balances
            .get(from_token_identifier)
            .map_or(0u128, |tb| tb.balance),
        Err(e) => {
            return Some(Response::err(
                id,
                ErrorKind::Sdk,
                format!("could not read the Stable Balance holding to price this send: {e}"),
            ));
        }
    };

    if held >= estimate.amount_in {
        return None;
    }

    // Deliberately no token figures in the message. `amount_in` is in token
    // base units, and printing "2500000" at a user who holds "2.50 USDB" reads
    // as a different order of magnitude entirely.
    Some(Response::err(
        id,
        ErrorKind::BadRequest,
        match pay_from {
            WalletAsset::Bitcoin => {
                "Not enough bitcoin in this Spark wallet to cover the amount and its fee. \
                 The wallet tried to make up the difference by converting USDB back to \
                 bitcoin, but the USDB holding is too small to cover it. Send a smaller \
                 amount or top up the Spark bitcoin balance."
            }
            WalletAsset::Usdb => NOT_ENOUGH_USDB,
        },
    ))
}

/// A USDB→BTC conversion to attach to a sats send the bitcoin balance alone
/// can't cover, when the wallet holds USDB to cover it with. `None` when the
/// bitcoin balance is enough, or there is no USDB to draw on.
///
/// This is what makes USDB spendable to any destination whether auto-convert
/// is on or off. The SDK's Stable Balance did the same only while it was
/// active (`stable_balance::get_conversion_options`), and it is no longer
/// configured (S2b). Without this the holding shows in the balance and can't
/// be spent. With it, the two prepare paths ask for the conversion
/// explicitly.
///
/// `needed_sats` is `amount + fee` (the SDK's own rule for when to attach).
/// The balance read is the cached one: this runs on every prepare, and a
/// stale answer costs at most one re-prepare, not a wrong send — the SDK
/// re-checks funding when the payment executes.
async fn usdb_shortfall_conversion(
    sdk: &SdkHandle,
    needed_sats: u64,
) -> Result<Option<UsdbShortfall>, breez_sdk_spark::SdkError> {
    let info = sdk
        .sdk
        .get_info(GetInfoRequest {
            ensure_synced: None,
        })
        .await?;
    let held_usdb = info
        .token_balances
        .get(sdk_adapter::USDB_MAINNET_TOKEN_IDENTIFIER)
        .map_or(0u128, |tb| tb.balance);
    Ok(conversion_for_shortfall(
        info.balance_sats,
        held_usdb,
        needed_sats,
    ))
}

/// A send the bitcoin balance can't cover, with the USDB conversion that
/// would cover it. `balance_sats` is kept for the error copy if the
/// conversion itself then fails — the user needs to know what *is*
/// spendable without it.
#[derive(Debug, Clone, PartialEq)]
struct UsdbShortfall {
    balance_sats: u64,
    conversion: ConversionOptions,
}

/// The decision half of [`usdb_shortfall_conversion`], split out so the
/// rule is testable without an SDK.
fn conversion_for_shortfall(
    balance_sats: u64,
    held_usdb: u128,
    needed_sats: u64,
) -> Option<UsdbShortfall> {
    if balance_sats >= needed_sats || held_usdb == 0 {
        return None;
    }
    Some(UsdbShortfall {
        balance_sats,
        conversion: usdb_to_bitcoin_conversion(),
    })
}

/// A USDB → bitcoin conversion funding a sats send. Attached to a send, the
/// SDK converts toward the whole `amount + fee` (`MinAmountOut`, in
/// `sdk/payments/conversion.rs` at the pin), not the shortfall: the
/// shortfall fallback attaches it only when bitcoin falls short, and paying
/// from USDB always attaches it.
fn usdb_to_bitcoin_conversion() -> ConversionOptions {
    ConversionOptions {
        conversion_type: ConversionType::ToBitcoin {
            from_token_identifier: sdk_adapter::USDB_MAINNET_TOKEN_IDENTIFIER.to_string(),
        },
        // The cap the gui shows, on every conversion the bridge asks for.
        max_slippage_bps: Some(CONVERSION_MAX_SLIPPAGE_BPS),
        // SDK default: 30 s to see the converted sats.
        completion_timeout_secs: None,
    }
}

/// Prepare a sats send funded from `pay_from`: from bitcoin, with the USDB
/// shortfall fallback ([`prepare_with_usdb_fallback`], whose arguments these
/// are); from USDB, with [`usdb_to_bitcoin_conversion`] attached to the only
/// prepare, so the whole payment is converted.
async fn prepare_funded<P, F, Fut, S, SFut>(
    pay_from: WalletAsset,
    known_amount_sat: Option<u64>,
    prepare: F,
    shortfall_conversion: S,
    needed_sats: impl Fn(&P) -> u64,
    has_conversion: impl Fn(&P) -> bool,
) -> Result<P, PrepareError>
where
    F: Fn(Option<ConversionOptions>) -> Fut,
    Fut: std::future::Future<Output = Result<P, breez_sdk_spark::SdkError>>,
    S: Fn(u64) -> SFut,
    SFut: std::future::Future<Output = Result<Option<UsdbShortfall>, breez_sdk_spark::SdkError>>,
{
    match pay_from {
        WalletAsset::Bitcoin => {
            prepare_with_usdb_fallback(
                known_amount_sat,
                prepare,
                shortfall_conversion,
                needed_sats,
                has_conversion,
            )
            .await
        }
        WalletAsset::Usdb => prepare(Some(usdb_to_bitcoin_conversion()))
            .await
            .map_err(PrepareError::PayFromUsdb),
    }
}

/// The quote for the conversion funding a send, for Review. `None` when the
/// prepare carries no conversion. Refuses any conversion other than the one
/// the bridge attaches ([`usdb_to_bitcoin_conversion`]): the gui shows its
/// cap and its direction.
fn send_conversion_quote(
    estimate: Option<&ConversionEstimate>,
) -> Result<Option<Box<ConversionQuote>>, String> {
    let Some(estimate) = estimate else {
        return Ok(None);
    };
    let expected = usdb_to_bitcoin_conversion();
    if estimate.options.conversion_type != expected.conversion_type
        || estimate.options.max_slippage_bps != expected.max_slippage_bps
    {
        return Err(format!(
            "prepare returned an unexpected token-conversion leg: {:?}",
            estimate.options
        ));
    }
    let usdb = |amount| AssetAmount {
        asset: WalletAsset::Usdb,
        amount,
    };
    Ok(Some(Box::new(ConversionQuote {
        amount_in: usdb(estimate.amount_in),
        amount_out: AssetAmount {
            asset: WalletAsset::Bitcoin,
            amount: estimate.amount_out,
        },
        // The SDK denominates the fee in the asset converted out of.
        fee: usdb(estimate.fee),
        amount_adjustment: estimate
            .amount_adjustment
            .as_ref()
            .map(|reason| match reason {
                AmountAdjustmentReason::FlooredToMinLimit => AmountAdjustment::FlooredToMinLimit,
                AmountAdjustmentReason::IncreasedToAvoidDust => {
                    AmountAdjustment::IncreasedToAvoidDust
                }
            }),
        max_slippage_bps: CONVERSION_MAX_SLIPPAGE_BPS,
    })))
}

/// Why a prepare paid from a balance can't be offered, past what the SDK
/// itself refuses: a token leg other than the conversion (paying a token
/// invoice spends that token, which neither balance describes), and, paying
/// from USDB, no conversion at all.
fn pay_from_refusal(
    pay_from: WalletAsset,
    token_identifier: Option<&str>,
    has_conversion: bool,
) -> Option<(ErrorKind, &'static str)> {
    if token_identifier.is_some() {
        return Some((ErrorKind::BadRequest, TOKEN_INVOICE_REFUSAL));
    }
    (pay_from == WalletAsset::Usdb && !has_conversion).then_some((
        ErrorKind::Sdk,
        "the SDK prepared no USDB conversion for a payment from USDB",
    ))
}

const NOT_ENOUGH_USDB: &str = "Not enough USDB in this Spark wallet to cover the amount and its \
     fee. Send a smaller amount or pay from Bitcoin.";

const TOKEN_INVOICE_REFUSAL: &str = "This invoice asks to be paid in a token, which can't be paid \
     from Bitcoin or US Dollar.";

const USDB_INVOICE_FROM_BITCOIN: &str = "This invoice asks to be paid in US Dollar (USDB). Pay it \
     from US Dollar.";

const USDB_SEND_WITHOUT_AMOUNT: &str = "Enter how many US Dollars to send.";

const USDB_SEND_NOT_SPARK_ADDRESS: &str = "US Dollar can be sent to a Spark address. Paste the \
     recipient's Spark address.";

const AMOUNTLESS_USDB_INVOICE: &str =
    "This invoice asks to be paid in US Dollar (USDB) but doesn't \
     say how much. Ask the recipient for an invoice with an amount.";

/// Whether an SDK prepare error is the wallet saying the *bitcoin* balance
/// can't cover the send. Two shapes: `SdkError::InsufficientFunds` from the
/// Lightning and Spark paths — since 0.25.0 it names a token when the
/// shortfall is in one, and that case is not ours to fix with more
/// conversion — and `SdkError::SparkError("... Tree service error:
/// insufficient funds")` from the on-chain path, whose leaf selection runs
/// inside the fee quote and only ever means sats.
fn is_insufficient_funds(e: &breez_sdk_spark::SdkError) -> bool {
    match e {
        breez_sdk_spark::SdkError::InsufficientFunds { token_identifier } => {
            token_identifier.is_none()
        }
        other => other
            .to_string()
            .to_lowercase()
            .contains("insufficient funds"),
    }
}

/// How [`prepare_with_usdb_fallback`] failed.
#[derive(Debug)]
enum PrepareError {
    /// The plain prepare failed and USDB was no help (not a funds problem,
    /// no USDB held, or no amount to size a conversion from). Reported as
    /// the SDK's own error, as before.
    Sdk(breez_sdk_spark::SdkError),
    /// The bitcoin balance was short, USDB was there to cover it, and the
    /// prepare *with* the conversion failed — in practice the AMM refusing
    /// the swap. `balance_sats` is what the wallet can still send without
    /// a conversion.
    UsdbConversion {
        balance_sats: u64,
        source: breez_sdk_spark::SdkError,
    },
    /// Paying from USDB, the prepare with the conversion failed: an SDK
    /// error, or the AMM refusing the swap.
    PayFromUsdb(breez_sdk_spark::SdkError),
}

impl PrepareError {
    /// Turn the failure into a bridge response. A conversion failure gets a
    /// sentence the user can act on; the SDK's text is a Flashnet JSON blob
    /// (`{"errorCode":"FSAG-4201",...,"message":"AMM has insufficient
    /// liquidity/reserves: ..."}`) and is reduced to its `message`.
    fn into_response(self, id: u64, what: &str) -> Response {
        match self {
            PrepareError::Sdk(e) => {
                Response::err(id, ErrorKind::Sdk, format!("{what} failed: {e}"))
            }
            PrepareError::UsdbConversion {
                balance_sats,
                source,
            } => Response::err(
                id,
                ErrorKind::BadRequest,
                format!(
                    "This send needs more bitcoin than the Spark wallet holds ({available} sats \
                     available), and converting USDB to bitcoin to cover it failed: {}. Send \
                     less than {available} sats, leaving room for the fee, or try again later.",
                    conversion_failure_reason(&source.to_string()),
                    available = with_thousands(balance_sats),
                ),
            ),
            // The SDK checks the USDB holding before it quotes
            // (`ensure_token_balance_covers` at the pin).
            PrepareError::PayFromUsdb(breez_sdk_spark::SdkError::InsufficientFunds {
                token_identifier: Some(_),
            }) => Response::err(id, ErrorKind::BadRequest, NOT_ENOUGH_USDB),
            PrepareError::PayFromUsdb(source) => Response::err(
                id,
                ErrorKind::BadRequest,
                format!(
                    "Converting USDB to bitcoin to pay this failed: {}. Pay from Bitcoin, or \
                     try again later.",
                    conversion_failure_reason(&source.to_string()),
                ),
            ),
        }
    }
}

/// `11254` → `11,254`, for error copy the user reads.
fn with_thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The human part of a conversion error. Flashnet errors reach us as
/// `Error: {json}`; pull the JSON's `message` out. Anything else is passed
/// through trimmed.
fn conversion_failure_reason(text: &str) -> String {
    let json = text
        .find('{')
        .and_then(|start| text.rfind('}').map(|end| &text[start..=end]));
    json.and_then(|j| serde_json::from_str::<serde_json::Value>(j).ok())
        .and_then(|v| v.get("message")?.as_str().map(str::to_string))
        .unwrap_or_else(|| text.trim().to_string())
}

/// Run a plain sats prepare, then re-run it with a USDB→BTC conversion
/// attached when the bitcoin balance falls short and USDB can make up the
/// difference — see [`usdb_shortfall_conversion`].
///
/// `prepare` performs one SDK prepare with the given conversion options;
/// `shortfall_conversion` is [`usdb_shortfall_conversion`] for a given
/// `needed_sats` (a parameter so the sequencing is testable without an
/// SDK); `needed_sats` reads `amount + fee` off a successful plain prepare.
/// `known_amount_sat` is the requested amount when the caller has it, used
/// for the on-chain path, whose plain prepare fails outright (leaf selection
/// for the exact amount happens inside the fee quote) instead of returning
/// a response to read the shortfall from.
///
/// A prepare that already carries a conversion is returned as is.
async fn prepare_with_usdb_fallback<P, F, Fut, S, SFut>(
    known_amount_sat: Option<u64>,
    prepare: F,
    shortfall_conversion: S,
    needed_sats: impl Fn(&P) -> u64,
    has_conversion: impl Fn(&P) -> bool,
) -> Result<P, PrepareError>
where
    F: Fn(Option<ConversionOptions>) -> Fut,
    Fut: std::future::Future<Output = Result<P, breez_sdk_spark::SdkError>>,
    S: Fn(u64) -> SFut,
    SFut: std::future::Future<Output = Result<Option<UsdbShortfall>, breez_sdk_spark::SdkError>>,
{
    // What the plain prepare tells us the send needs, and what to answer
    // if USDB turns out not to help.
    let (needed, fallback): (u64, Result<P, PrepareError>) = match prepare(None).await {
        Ok(plain) if has_conversion(&plain) => return Ok(plain),
        Ok(plain) => {
            let needed = needed_sats(&plain);
            (needed, Ok(plain))
        }
        Err(e) if is_insufficient_funds(&e) => match known_amount_sat {
            Some(amount) => (amount, Err(PrepareError::Sdk(e))),
            None => return Err(PrepareError::Sdk(e)),
        },
        Err(e) => return Err(PrepareError::Sdk(e)),
    };

    match shortfall_conversion(needed).await {
        Ok(Some(UsdbShortfall {
            balance_sats,
            conversion,
        })) => {
            tracing::info!(
                "bitcoin balance {balance_sats} sats short of {needed}; re-preparing with a USDB \
                 conversion"
            );
            prepare(Some(conversion))
                .await
                .map_err(|source| PrepareError::UsdbConversion {
                    balance_sats,
                    source,
                })
        }
        Ok(None) => fallback,
        Err(e) => {
            // Can't tell whether USDB would help; the plain outcome stands.
            tracing::warn!("could not read balances to check for a USDB shortfall: {e}");
            fallback
        }
    }
}

async fn handle_prepare_send(
    id: u64,
    params: coincube_spark_protocol::PrepareSendParams,
    state: Arc<ServerState>,
) -> Response {
    match prepare_send(
        id,
        params.input,
        params.amount_sat,
        None,
        params.fees_included,
        &state,
    )
    .await
    {
        Ok((prepare, _)) => Response::ok(id, OkPayload::PrepareSend(prepare)),
        Err(response) => response,
    }
}

async fn handle_prepare_send_from(
    id: u64,
    params: PrepareSendFromParams,
    state: Arc<ServerState>,
) -> Response {
    let pay_from = params.pay_from;
    let Some(sdk) = state.sdk.read().await.clone() else {
        return Response::err(
            id,
            ErrorKind::NotConnected,
            "init must succeed before prepare_send_from",
        );
    };
    // A token invoice is decided before preparing anything: a USDB one paid
    // from USDB takes its own path, every other one is refused. A failed
    // parse is left to the prepare, and a token leg it returns is refused
    // there (`pay_from_refusal`), which also covers a BIP-21 URI carrying one.
    if let Ok(InputType::SparkInvoice(details)) = sdk.sdk.parse(params.input.trim()).await {
        if let Some(token_identifier) = details.token_identifier.as_deref() {
            if let Some((kind, reason)) =
                token_invoice_refusal(pay_from, token_identifier, details.amount)
            {
                return Response::err(id, kind, reason);
            }
            return prepare_usdb_transfer(id, params.input, UsdbDestination::Invoice, &sdk, &state)
                .await;
        }
    }
    match prepare_send(
        id,
        params.input,
        params.amount_sat,
        Some(pay_from),
        false,
        &state,
    )
    .await
    {
        Ok((prepare, conversion)) => Response::ok(
            id,
            OkPayload::PrepareSendFrom(PrepareSendFromOk {
                prepare,
                pay_from,
                conversion,
                token_payment: None,
            }),
        ),
        Err(response) => response,
    }
}

/// Why a Spark invoice that requests `token_identifier` can't be paid from
/// `pay_from`, or `None` for the one shape this bridge pays: a USDB invoice
/// that names its amount, paid from USDB ([`prepare_usdb_transfer`]).
///
/// Paid from bitcoin it stays refused. The SDK can fund a token invoice
/// with a `FromBitcoin` conversion (`prepare/spark_invoice.rs` at the pin),
/// but no Review shows a bitcoin-to-USDB conversion on a send yet. An
/// amountless one is refused because the only amount the gui sends is in
/// sats, and the SDK would read it as token base units.
fn token_invoice_refusal(
    pay_from: WalletAsset,
    token_identifier: &str,
    amount: Option<u128>,
) -> Option<(ErrorKind, &'static str)> {
    if token_identifier != sdk_adapter::USDB_MAINNET_TOKEN_IDENTIFIER {
        return Some((ErrorKind::BadRequest, TOKEN_INVOICE_REFUSAL));
    }
    if pay_from == WalletAsset::Bitcoin {
        return Some((ErrorKind::BadRequest, USDB_INVOICE_FROM_BITCOIN));
    }
    if amount.unwrap_or(0) == 0 {
        return Some((ErrorKind::BadRequest, AMOUNTLESS_USDB_INVOICE));
    }
    None
}

/// [`Method::PrepareUsdbSend`]: USDB from the USDB balance to a Spark
/// address, for an amount in USDB base units. Anything but a Spark address is
/// refused before preparing: an invoice names its own amount and goes through
/// [`Method::PrepareSendFrom`], and no other destination can take USDB.
async fn handle_prepare_usdb_send(
    id: u64,
    params: PrepareUsdbSendParams,
    state: Arc<ServerState>,
) -> Response {
    let Some(sdk) = state.sdk.read().await.clone() else {
        return Response::err(
            id,
            ErrorKind::NotConnected,
            "init must succeed before prepare_usdb_send",
        );
    };
    if params.amount == 0 {
        return Response::err(id, ErrorKind::BadRequest, USDB_SEND_WITHOUT_AMOUNT);
    }
    let input = params.input.trim().to_string();
    match sdk.sdk.parse(&input).await {
        Ok(InputType::SparkAddress(_)) => {}
        Ok(_) => return Response::err(id, ErrorKind::BadRequest, USDB_SEND_NOT_SPARK_ADDRESS),
        Err(e) => {
            return Response::err(
                id,
                ErrorKind::BadRequest,
                format!("parse_input failed: {e}"),
            )
        }
    }
    prepare_usdb_transfer(
        id,
        input,
        UsdbDestination::Address {
            amount: params.amount,
        },
        &sdk,
        &state,
    )
    .await
}

/// Where a direct USDB transfer goes, and so where its amount comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UsdbDestination {
    /// A Spark invoice asking for USDB: the invoice's own amount.
    Invoice,
    /// A Spark address: the amount asked for, in USDB base units.
    Address { amount: u128 },
}

impl UsdbDestination {
    /// The SDK's `SendPaymentMethod` variant name a prepare to this
    /// destination must come back as.
    fn method(self) -> &'static str {
        match self {
            Self::Invoice => "SparkInvoice",
            Self::Address { .. } => "SparkAddress",
        }
    }
}

/// Pay USDB straight from the USDB balance: a USDB invoice once
/// [`token_invoice_refusal`] has let it through, or a Spark address
/// ([`handle_prepare_usdb_send`]). The SDK prepares a direct token transfer
/// with no conversion.
///
/// The preview's sats figures are `0` and the real ones travel as
/// [`PrepareSendFromOk::token_payment`], so nothing reads token base units
/// as sats. `has_token_leg` is `Some(true)`: the send drops the
/// idempotency key (`execute_regular_send`), so the gui treats a retry as
/// unguarded.
async fn prepare_usdb_transfer(
    id: u64,
    input: String,
    destination: UsdbDestination,
    sdk: &SdkHandle,
    state: &ServerState,
) -> Response {
    let prepare = match sdk
        .sdk
        .prepare_send_payment(PrepareSendPaymentRequest {
            payment_request: PaymentRequest::Input { input },
            // In token base units; an invoice's own amount when `None`.
            amount: match destination {
                UsdbDestination::Invoice => None,
                UsdbDestination::Address { amount } => Some(amount),
            },
            token_identifier: Some(sdk_adapter::USDB_MAINNET_TOKEN_IDENTIFIER.to_string()),
            conversion_options: None,
            fee_policy: None,
        })
        .await
    {
        Ok(prepare) => prepare,
        Err(breez_sdk_spark::SdkError::InsufficientFunds {
            token_identifier: Some(_),
        }) => return Response::err(id, ErrorKind::BadRequest, NOT_ENOUGH_USDB),
        Err(e) => return Response::err(id, ErrorKind::Sdk, format!("prepare_send failed: {e}")),
    };
    let method_fee = match (&prepare.payment_method, destination) {
        (
            SendPaymentMethod::SparkInvoice {
                fee,
                token_identifier,
                ..
            },
            UsdbDestination::Invoice,
        )
        | (
            SendPaymentMethod::SparkAddress {
                fee,
                token_identifier,
                ..
            },
            UsdbDestination::Address { .. },
        ) => Some((*fee, token_identifier.as_deref())),
        _ => None,
    };
    let token_payment = match usdb_transfer_payment(
        destination,
        prepare.token_identifier.as_deref(),
        prepare.conversion_estimate.is_some(),
        method_fee,
        prepare.amount,
    ) {
        Ok(token_payment) => token_payment,
        Err(reason) => return Response::err(id, ErrorKind::Sdk, reason),
    };

    // The SDK checks the holding when it sends, not here. Check it now so a
    // short balance is refused on the amount screen, not after Confirm. A
    // forced sync, as in `unfundable_conversion_error`: a stale cache that
    // under-reports would refuse a send that would have worked.
    let held = match sdk
        .sdk
        .get_info(GetInfoRequest {
            ensure_synced: Some(true),
        })
        .await
    {
        Ok(info) => usdb_balance(&info.token_balances).map_or(0, |usdb| usdb.balance),
        Err(e) => {
            return Response::err(
                id,
                ErrorKind::Sdk,
                format!("could not read the USDB holding to price this send: {e}"),
            );
        }
    };
    if held
        < token_payment
            .amount
            .amount
            .saturating_add(token_payment.fee.amount)
    {
        return Response::err(id, ErrorKind::BadRequest, NOT_ENOUGH_USDB);
    }

    let handle = Uuid::new_v4().to_string();
    state
        .pending_prepares
        .lock()
        .await
        .insert(handle.clone(), (Instant::now(), prepare));
    Response::ok(
        id,
        OkPayload::PrepareSendFrom(PrepareSendFromOk {
            prepare: PrepareSendOk {
                handle,
                // Token base units travel in `token_payment`, never here.
                amount_sat: 0,
                fee_sat: 0,
                method: destination.method().to_string(),
                cross_chain: None,
                has_token_leg: Some(true),
            },
            pay_from: WalletAsset::Usdb,
            conversion: None,
            token_payment: Some(token_payment),
        }),
    )
}

/// The USDB figures of a prepared direct USDB transfer, or why the prepare
/// isn't one: the SDK must have prepared a transfer of the USDB token to
/// `destination` (`method_fee` is the matching method's fee and token, `None`
/// for any other method), with no conversion, and to an address for exactly
/// the amount asked. The SDK denominates both the amount and the fee in the
/// token's base units when a token is set.
fn usdb_transfer_payment(
    destination: UsdbDestination,
    token_identifier: Option<&str>,
    has_conversion: bool,
    method_fee: Option<(u128, Option<&str>)>,
    amount: u128,
) -> Result<TokenPayment, String> {
    let usdb = Some(sdk_adapter::USDB_MAINNET_TOKEN_IDENTIFIER);
    let method = destination.method();
    if has_conversion {
        return Err(format!(
            "prepare attached a conversion to a USDB {method} payment"
        ));
    }
    if token_identifier != usdb {
        return Err(format!(
            "prepare of a USDB {method} payment moves {token_identifier:?}, not USDB"
        ));
    }
    let fee = match method_fee {
        Some((fee, method_token)) if method_token == usdb => fee,
        Some((_, method_token)) => {
            return Err(format!(
                "prepare of a USDB {method} payment pays {method_token:?}, not USDB"
            ))
        }
        None => {
            return Err(format!(
                "prepare of a USDB payment is not a {method} payment"
            ))
        }
    };
    if let UsdbDestination::Address { amount: asked } = destination {
        if amount != asked {
            return Err(format!(
                "prepare of a USDB {method} payment moves {amount}, not the {asked} asked"
            ));
        }
    }
    let usdb = |amount| AssetAmount {
        asset: WalletAsset::Usdb,
        amount,
    };
    Ok(TokenPayment {
        amount: usdb(amount),
        fee: usdb(fee),
    })
}

/// [`Method::PrepareSend`] (`pay_from: None`) and [`Method::PrepareSendFrom`]:
/// the preview, and the quote for the conversion funding it, if any.
/// `pay_from: None` pays from bitcoin and passes a token leg through, as
/// before pay-from existed; with a balance named, a token leg other than
/// the conversion is refused ([`pay_from_refusal`]).
async fn prepare_send(
    id: u64,
    input: String,
    amount_sat: Option<u64>,
    pay_from_param: Option<WalletAsset>,
    fees_included: bool,
    state: &ServerState,
) -> Result<(PrepareSendOk, Option<Box<ConversionQuote>>), Response> {
    let pay_from = pay_from_param.unwrap_or(WalletAsset::Bitcoin);
    let sdk = match state.sdk.read().await.clone() {
        Some(s) => s,
        None => {
            return Err(Response::err(
                id,
                ErrorKind::NotConnected,
                "init must succeed before prepare_send",
            ));
        }
    };

    // 0.19.0 turned `payment_request` from a bare `String` into an enum, so the
    // regular send path now has to say explicitly that it's handing over raw
    // user input for the SDK to classify (bolt11 / spark address / BIP-21 / …).
    // The cross-chain path uses the other variant — see `prepare_cross_chain`.
    let amount = amount_sat.map(|a| a as u128);
    let speed = selected_onchain_speed();
    let prepare = |conversion_options| {
        sdk.sdk.prepare_send_payment(PrepareSendPaymentRequest {
            payment_request: PaymentRequest::Input {
                input: input.clone(),
            },
            amount,
            token_identifier: None,
            conversion_options,
            fee_policy: fees_included.then_some(breez_sdk_spark::FeePolicy::FeesIncluded),
        })
    };
    // A fees-included send drains the bitcoin balance it names; it never tops
    // itself up from USDB, so it skips the shortfall fallback.
    let prepared = if fees_included {
        prepare(None).await.map_err(PrepareError::Sdk)
    } else {
        prepare_funded(
            pay_from,
            amount_sat,
            prepare,
            |needed| usdb_shortfall_conversion(&sdk, needed),
            |p: &PrepareSendPaymentResponse| {
                clamp_u128_to_u64(p.amount)
                    .saturating_add(fee_and_method(&p.payment_method, &speed).0)
            },
            |p: &PrepareSendPaymentResponse| p.conversion_estimate.is_some(),
        )
        .await
    };

    match prepared {
        Ok(prepare) => {
            // A token→BTC conversion may be attached here by the fallback
            // above. Fail now, in the user's terms, if the wallet can't fund
            // it — see `unfundable_conversion_error`.
            if let Some(response) = unfundable_conversion_error(
                id,
                &sdk,
                prepare.conversion_estimate.as_ref(),
                pay_from,
            )
            .await
            {
                return Err(response);
            }
            if let Some((kind, reason)) = pay_from_param.and_then(|pay_from| {
                pay_from_refusal(
                    pay_from,
                    prepare.token_identifier.as_deref(),
                    prepare.conversion_estimate.is_some(),
                )
            }) {
                return Err(Response::err(id, kind, reason));
            }
            // Only a pay-from preview carries the quote; a plain one is
            // unchanged.
            let conversion = match pay_from_param {
                Some(_) => send_conversion_quote(prepare.conversion_estimate.as_ref())
                    .map_err(|e| Response::err(id, ErrorKind::Sdk, e))?,
                None => None,
            };

            // Extract display-friendly fields before stashing the full
            // struct. `amount` + method-specific fees are u128 in the
            // SDK (Spark tokens can exceed sat precision); we saturate
            // to u64 for display. Bitcoin-side sends are well within
            // u64::MAX.
            let amount_sat = clamp_u128_to_u64(prepare.amount);
            // The tier quoted here is the tier `execute_regular_send` will send
            // at — both read `selected_onchain_speed()`.
            let (fee_sat, method_tag) =
                fee_and_method(&prepare.payment_method, &selected_onchain_speed());
            // Read before the insert below moves `prepare`. Mirrors
            // `execute_regular_send`'s `has_token_leg` — the same condition that
            // makes it drop the idempotency key, which is exactly what the gui
            // needs to know.
            let has_token_leg =
                prepare.token_identifier.is_some() || prepare.conversion_estimate.is_some();

            let handle = Uuid::new_v4().to_string();
            state
                .pending_prepares
                .lock()
                .await
                .insert(handle.clone(), (Instant::now(), prepare));

            Ok((
                PrepareSendOk {
                    handle,
                    amount_sat,
                    fee_sat,
                    method: method_tag.to_string(),
                    // Regular sends carry no cross-chain quote — those come
                    // from `prepare_cross_chain`.
                    cross_chain: None,
                    has_token_leg: Some(has_token_leg),
                },
                conversion,
            ))
        }
        Err(e) => Err(e.into_response(id, "prepare_send")),
    }
}

// ── Cross-chain stablecoin send ────────────────────────────────────────────

/// Whether a route can be funded from the wallet's BTC balance. v1 only offers
/// these: the source asset is always BTC sats (converted at pay time), and
/// sending *from* Stable Balance is deferred. This is also what makes the v1
/// path retry-safe — see [`route_is_retry_safe`].
fn route_accepts_btc(route: &CrossChainRoutePair) -> bool {
    // 0.25.0 renamed `supported_sources: Vec<SourceAsset>` to
    // `accepted_assets` and added `delivery_methods` (the rail — Spark /
    // Lightning / Bitcoin) alongside it; 0.28.0 wrapped each asset with its
    // own limits. The rail is the SDK's concern; funding is what decides
    // retry safety here.
    route
        .accepted_assets
        .iter()
        .any(|a| matches!(a.asset, SparkAsset::Bitcoin))
}

/// Whether a failed send along this route can be blind-retried.
///
/// Retry safety is a property of the *source* leg. A BTC-funded send moves
/// value with a Spark transfer, which honours the `idempotency_key` we pass to
/// `send_payment`. A token-funded send goes through `spark_wallet::transfer_tokens`,
/// which has no idempotency hook — retrying one could pay twice. Since v1 only
/// offers BTC-funded routes this is always true today, but it's computed rather
/// than hardcoded so that adding token sources later can't silently start
/// telling the gui a token send is safe to retry.
fn route_is_retry_safe(route: &CrossChainRoutePair) -> bool {
    route_accepts_btc(route)
}

fn sdk_route_to_protocol(route: &CrossChainRoutePair) -> coincube_spark_protocol::CrossChainRoute {
    coincube_spark_protocol::CrossChainRoute {
        provider: match route.provider {
            breez_sdk_spark::CrossChainProvider::Orchestra => "orchestra".to_string(),
            breez_sdk_spark::CrossChainProvider::Boltz => "boltz".to_string(),
        },
        chain: route.chain.clone(),
        chain_id: route.chain_id.clone(),
        asset: route.asset.clone(),
        contract_address: route.contract_address.clone(),
        decimals: route.decimals,
        btc_source_supported: route_accepts_btc(route),
        btc_limits: route.limits_for(&SparkAsset::Bitcoin).map(|limits| {
            coincube_spark_protocol::CrossChainRouteLimits {
                // Sats fit in u64; anything larger is not a real bound.
                min_sats: limits.min_amount.map(|v| v.min(u64::MAX as u128) as u64),
                max_sats: limits.max_amount.map(|v| v.min(u64::MAX as u128) as u64),
                min_usd_cents: limits.min_usd_cents,
                max_usd_cents: limits.max_usd_cents,
            }
        }),
    }
}

/// The on-chain confirmation tier this build previews **and** sends at.
///
/// Named once, and taken as an argument by everything that depends on it
/// ([`fee_and_method`], [`send_options_for`]), so no code path can quote one
/// tier and execute another.
///
/// That is not hypothetical. `fee_and_method` used to hardcode
/// `fee_quote.speed_medium` while `execute_regular_send` passed `options: None`
/// — and in the pinned SDK 0.19.0, `None` on a Bitcoin-address send resolves to
/// `OnchainConfirmationSpeed::Fast` (`sdk/payments/send/bitcoin_address.rs`:
/// `None => OnchainConfirmationSpeed::Fast, // Default to fast`). The
/// confirmation screen showed the medium fee, the wallet paid the fast one, and
/// the success response reported the medium figure back.
///
/// When the UI grows a Fast/Medium/Slow picker, the user's choice replaces this
/// call at the two call sites and the invariant holds by construction: the tier
/// that priced the send is the tier that is sent.
fn selected_onchain_speed() -> OnchainConfirmationSpeed {
    OnchainConfirmationSpeed::Medium
}

/// Stable name for a confirmation tier, for logs and tests.
///
/// `OnchainConfirmationSpeed` is an SDK type with neither `Debug` nor
/// `PartialEq`, so this is also how the bridge states which tier a send went
/// out at without reaching for a derive it does not own.
fn speed_tag(speed: &OnchainConfirmationSpeed) -> &'static str {
    match speed {
        OnchainConfirmationSpeed::Fast => "fast",
        OnchainConfirmationSpeed::Medium => "medium",
        OnchainConfirmationSpeed::Slow => "slow",
    }
}

/// Total sats for one tier of an on-chain fee quote.
///
/// Saturating, matching the SDK's own `total_fee_sat()`: both halves are
/// externally supplied and must not be able to wrap a displayed total.
fn onchain_fee_for_speed(quote: &SendOnchainFeeQuote, speed: &OnchainConfirmationSpeed) -> u64 {
    let tier = match speed {
        OnchainConfirmationSpeed::Fast => &quote.speed_fast,
        OnchainConfirmationSpeed::Medium => &quote.speed_medium,
        OnchainConfirmationSpeed::Slow => &quote.speed_slow,
    };
    tier.user_fee_sat.saturating_add(tier.l1_broadcast_fee_sat)
}

/// The SDK options a prepared payment must be executed with, for `speed`.
///
/// `Some` only for Bitcoin-address sends: that is the one method whose options
/// carry a confirmation speed, and the SDK rejects
/// `SendPaymentOptions::BitcoinAddress` on any other method with
/// `InvalidInput`. Bolt11, Spark and cross-chain sends therefore keep the
/// `None` they have always been executed with, and their behaviour is unchanged.
fn send_options_for(
    method: &SendPaymentMethod,
    speed: &OnchainConfirmationSpeed,
) -> Option<SendPaymentOptions> {
    match method {
        SendPaymentMethod::BitcoinAddress { .. } => Some(SendPaymentOptions::BitcoinAddress {
            confirmation_speed: speed.clone(),
        }),
        SendPaymentMethod::Bolt11Invoice { .. }
        | SendPaymentMethod::SparkAddress { .. }
        | SendPaymentMethod::SparkInvoice { .. }
        | SendPaymentMethod::CrossChainAddress { .. } => None,
    }
}

/// The sats-denominated fee for a prepared send at `speed`, plus the method tag
/// the gui branches on.
///
/// Single definition on purpose: this used to be written out twice (once when
/// preparing, once when executing), and two copies of a money-formatting match
/// is two chances to disagree about what a fee is.
///
/// `speed` only affects the Bitcoin-address arm; every other method quotes one
/// fee regardless. It is still taken unconditionally so that the preview and
/// the execution are demonstrably reading the same selection.
///
/// **The fee is in sats, and for a cross-chain send that is not the whole
/// story.** Cross-chain's headline fee (`fee_amount`) is denominated in the
/// *destination* asset's base units — USDC/USDT, not sats — so surfacing it
/// through this field would misreport it by orders of magnitude. Only
/// `source_transfer_fee_sats` is genuinely sats, so that is what's reported
/// here; the full breakdown reaches the gui on the prepare response's
/// `CrossChainQuote`, which is where the send panel renders it.
fn fee_and_method(
    method: &SendPaymentMethod,
    speed: &OnchainConfirmationSpeed,
) -> (u64, &'static str) {
    match method {
        SendPaymentMethod::BitcoinAddress { fee_quote, .. } => {
            (onchain_fee_for_speed(fee_quote, speed), "BitcoinAddress")
        }
        SendPaymentMethod::Bolt11Invoice {
            spark_transfer_fee_sats,
            lightning_fee_sats,
            ..
        } => (
            // Saturating for the same reason as `onchain_fee_for_speed`: two
            // SDK-supplied values being combined into a number the user is
            // shown and charged against.
            spark_transfer_fee_sats
                .unwrap_or(0)
                .saturating_add(*lightning_fee_sats),
            "Bolt11Invoice",
        ),
        SendPaymentMethod::SparkAddress { fee, .. } => (clamp_u128_to_u64(*fee), "SparkAddress"),
        SendPaymentMethod::SparkInvoice { fee, .. } => (clamp_u128_to_u64(*fee), "SparkInvoice"),
        SendPaymentMethod::CrossChainAddress {
            source_transfer_fee_sats,
            ..
        } => (*source_transfer_fee_sats, "CrossChainAddress"),
    }
}

fn family_str(family: CrossChainAddressFamily) -> &'static str {
    match family {
        CrossChainAddressFamily::Evm => "evm",
        CrossChainAddressFamily::Solana => "solana",
        CrossChainAddressFamily::Tron => "tron",
    }
}

/// Inverse of [`family_str`]. `None` for anything we didn't emit — the gui only
/// ever echoes back a family string we produced, so an unknown one means the
/// wire contract has drifted and the safe move is to refuse rather than guess a
/// chain family for a money transfer.
fn family_from_str(s: &str) -> Option<CrossChainAddressFamily> {
    match s {
        "evm" => Some(CrossChainAddressFamily::Evm),
        "solana" => Some(CrossChainAddressFamily::Solana),
        "tron" => Some(CrossChainAddressFamily::Tron),
        _ => None,
    }
}

/// Rebuild the SDK's address details from the destination the gui echoed back.
///
/// This is the *whole* reason [`coincube_spark_protocol::PrepareCrossChainParams`]
/// round-trips a [`CrossChainAddress`] rather than a bare address string. The
/// alternative — re-parsing the address here — silently discards the
/// `contract_address` and `chain_id` that only a URI destination carries, and
/// re-resolves routes against a broader destination than the one the user was
/// actually offered a choice from. It survives today only because the SDK's
/// route filter treats an absent contract filter as "match everything", so the
/// broader set is a superset that still contains the chosen route. That's an
/// implementation accident, not a guarantee, and it isn't one worth betting a
/// send on.
fn details_from_protocol(
    destination: &coincube_spark_protocol::CrossChainAddress,
) -> Option<CrossChainAddressDetails> {
    Some(CrossChainAddressDetails {
        address: destination.address.clone(),
        address_family: family_from_str(&destination.family)?,
        contract_address: destination.contract_address.clone(),
        chain_id: destination.chain_id,
        amount: destination.amount,
    })
}

/// Classify a raw destination as a cross-chain address.
///
/// Delegates to the SDK's own `parse`, which already recognises both bare
/// addresses (`0xabc…`, Solana base58, `T…` Tron) and canonical URIs
/// (`ethereum:0xabc…?chain=base&asset=usdc`) and returns
/// [`InputType::CrossChainAddress`]. Going through the SDK rather than
/// `breez_sdk_common`'s standalone `detect_address_family` matters: the SDK
/// re-declares `CrossChainAddressDetails` as its own type (with generated
/// `From` impls to bridge the two), and `CrossChainRouteFilter::Send` wants
/// *that* one. Using the SDK's parser keeps a single type in play and keeps
/// detection consistent with whatever the SDK will accept at prepare time.
///
/// `None` for any non-cross-chain input — a BOLT11 invoice, a Spark address,
/// an on-chain Bitcoin address, or plain garbage.
async fn parse_cross_chain_input(sdk: &SdkHandle, input: &str) -> Option<CrossChainAddressDetails> {
    match sdk.sdk.parse(input.trim()).await {
        Ok(InputType::CrossChainAddress(details)) => Some(details),
        _ => None,
    }
}

async fn handle_get_cross_chain_routes(
    id: u64,
    params: coincube_spark_protocol::GetCrossChainRoutesParams,
    state: Arc<ServerState>,
) -> Response {
    let sdk = match state.sdk.read().await.clone() {
        Some(s) => s,
        None => {
            return Response::err(
                id,
                ErrorKind::NotConnected,
                "init must succeed before get_cross_chain_routes",
            );
        }
    };

    // Not a cross-chain address — an ordinary BOLT11/Spark/on-chain
    // destination. Not an error: this is exactly how the Send panel decides
    // which flow to run, so it must be able to ask about any input.
    let Some(details) = parse_cross_chain_input(&sdk, &params.input).await else {
        return Response::ok(
            id,
            OkPayload::GetCrossChainRoutes(coincube_spark_protocol::CrossChainRoutesOk {
                address: None,
                routes: Vec::new(),
            }),
        );
    };

    let address = coincube_spark_protocol::CrossChainAddress {
        address: details.address.clone(),
        family: family_str(details.address_family).to_string(),
        contract_address: details.contract_address.clone(),
        chain_id: details.chain_id,
        amount: details.amount,
    };

    // Unset delivery method: routes paid from the Spark balance, the only
    // kind the bridge sends.
    let filter = CrossChainRouteFilter::Send {
        address_details: details,
        delivery_method: None,
    };
    match sdk.sdk.get_cross_chain_routes(&filter).await {
        Ok(routes) => {
            // Drop routes we can't fund. Offering a token-only route while the
            // v1 send path always debits BTC would produce a prepare that fails
            // at the SDK — better to never show it.
            let routes: Vec<_> = routes
                .iter()
                .filter(|r| route_accepts_btc(r))
                .map(sdk_route_to_protocol)
                .collect();
            Response::ok(
                id,
                OkPayload::GetCrossChainRoutes(coincube_spark_protocol::CrossChainRoutesOk {
                    address: Some(address),
                    routes,
                }),
            )
        }
        Err(e) => Response::err(id, ErrorKind::Sdk, e.to_string()),
    }
}

async fn handle_prepare_cross_chain(
    id: u64,
    params: coincube_spark_protocol::PrepareCrossChainParams,
    state: Arc<ServerState>,
) -> Response {
    let sdk = match state.sdk.read().await.clone() {
        Some(s) => s,
        None => {
            return Response::err(
                id,
                ErrorKind::NotConnected,
                "init must succeed before prepare_cross_chain",
            );
        }
    };

    if let Some(bps) = params.max_slippage_bps {
        // Bounds-check here rather than letting the SDK reject: a rejected
        // prepare surfaces as an opaque SDK error, whereas the gui's advanced
        // disclosure needs to say *what* is wrong with the number.
        if !(10..=500).contains(&bps) {
            return Response::err(
                id,
                ErrorKind::BadRequest,
                format!("max_slippage_bps must be between 10 and 500, got {bps}"),
            );
        }
    }

    // Re-resolve the route against the SDK's live list rather than trusting the
    // one the gui echoed back. The gui's copy is a stringified snapshot that
    // may be seconds or minutes stale, and a `CrossChainRoutePair` carries
    // provider-internal fields we deliberately don't round-trip. Re-fetching
    // means the prepare always runs against a route the SDK currently offers.
    //
    // The details are *reconstructed*, not re-parsed — see
    // `details_from_protocol`. Re-parsing would drop the URI-only fields and
    // quietly widen the destination the routes are resolved against.
    let Some(details) = details_from_protocol(&params.destination) else {
        return Response::err(
            id,
            ErrorKind::BadRequest,
            "destination is not a recognised cross-chain address",
        );
    };
    // Unset delivery method: routes paid from the Spark balance, the only
    // kind the bridge sends.
    let filter = CrossChainRouteFilter::Send {
        address_details: details,
        delivery_method: None,
    };
    let live_routes = match sdk.sdk.get_cross_chain_routes(&filter).await {
        Ok(routes) => routes,
        Err(e) => return Response::err(id, ErrorKind::Sdk, e.to_string()),
    };
    let Some(route) = live_routes
        .into_iter()
        // Identity only: the published limits may have moved since the
        // gui looked the route up, and the bridge never trusts them anyway.
        .find(|r| sdk_route_to_protocol(r).same_route_as(&params.route))
    else {
        return Response::err(
            id,
            ErrorKind::BadRequest,
            "the selected route is no longer offered — re-fetch routes and retry",
        );
    };
    let retry_safe = route_is_retry_safe(&route);

    // The SDK wants the bare recipient address here, not the URI it may have
    // come from — the URI's extra context has already done its job in the
    // route filter above.
    let address = params.destination.address;
    let (max_slippage_bps, amount_sat) = (params.max_slippage_bps, params.amount_sat);
    let (shortfall, prepared) = prepare_cross_chain_once(
        amount_sat,
        |needed| usdb_shortfall_conversion(&sdk, needed),
        |conversion_options| {
            sdk.sdk.prepare_send_payment(cross_chain_prepare_request(
                address,
                route,
                max_slippage_bps,
                amount_sat,
                conversion_options,
            ))
        },
    )
    .await;

    match prepared {
        Ok(prepare) => {
            // The SDK refuses a top-up the USDB balance can't fund
            // (`ensure_token_balance_covers` at the pin); this re-reads the
            // balance synced, with the copy the other send paths use.
            if shortfall.is_some() {
                if let Some(response) = unfundable_conversion_error(
                    id,
                    &sdk,
                    prepare.conversion_estimate.as_ref(),
                    WalletAsset::Bitcoin,
                )
                .await
                {
                    return response;
                }
            }
            let amount_sat = clamp_u128_to_u64(prepare.amount);
            let SendPaymentMethod::CrossChainAddress {
                route,
                estimated_out,
                fee_amount,
                source_transfer_fee_sats,
                expires_at,
                ..
            } = &prepare.payment_method
            else {
                // The SDK answered a `PaymentRequest::CrossChain` with some
                // other method. Refuse rather than guess: the amounts in the
                // other variants are sats-denominated and would be rendered
                // against a USDC/USDT scale.
                return Response::err(
                    id,
                    ErrorKind::Sdk,
                    "cross-chain prepare returned a non-cross-chain payment method",
                );
            };

            // Copy every field we still need out of the borrow before handing
            // `prepare` to the pending map — the insert moves it.
            let fee_sat = *source_transfer_fee_sats;
            // Unlike the two plain-send paths, a conversion leg here is exactly
            // what was asked for: a v1 cross-chain route funds a stablecoin from
            // BTC. So it is reported, not refused.
            let has_token_leg =
                prepare.token_identifier.is_some() || prepare.conversion_estimate.is_some();
            let quote = coincube_spark_protocol::CrossChainQuote {
                route: sdk_route_to_protocol(route),
                estimated_out: *estimated_out,
                fee_amount: *fee_amount,
                source_transfer_fee_sats: fee_sat,
                expires_at: expires_at.clone(),
                retry_safe,
            };

            // Same pending map as a regular prepare, so `send_payment` routes
            // the handle without needing to know it's cross-chain.
            let handle = Uuid::new_v4().to_string();
            state
                .pending_prepares
                .lock()
                .await
                .insert(handle.clone(), (Instant::now(), prepare));

            Response::ok(
                id,
                OkPayload::PrepareSend(PrepareSendOk {
                    handle,
                    amount_sat,
                    fee_sat,
                    method: "CrossChainAddress".to_string(),
                    cross_chain: Some(Box::new(quote)),
                    has_token_leg: Some(has_token_leg),
                }),
            )
        }
        Err(e) => cross_chain_prepare_failed(id, e, shortfall.as_ref()),
    }
}

/// The one prepare of [`handle_prepare_cross_chain`], with a USDB top-up when
/// the bitcoin balance is below the requested `amount_sat` and USDB is held
/// (#965 F9): the trigger SDK Stable Balance applied here before S2b
/// (`get_conversion_options` at the pin). Returns the top-up attached, for
/// the error copy, and the prepare's result.
///
/// The provider's amount and source fee are known only from its prepare,
/// which has side effects (a Boltz reverse swap each time), so `prepare` is
/// `FnOnce`: there is no second prepare to catch a shortfall in the fees
/// alone. Bitcoin that covers the amount but not the fees gets no top-up, as
/// before S2b. Balances that can't be read mean no top-up, as before this
/// repair.
async fn prepare_cross_chain_once<P, F, Fut, S, SFut>(
    amount_sat: u64,
    shortfall_conversion: S,
    prepare: F,
) -> (Option<UsdbShortfall>, Result<P, breez_sdk_spark::SdkError>)
where
    F: FnOnce(Option<ConversionOptions>) -> Fut,
    Fut: std::future::Future<Output = Result<P, breez_sdk_spark::SdkError>>,
    S: FnOnce(u64) -> SFut,
    SFut: std::future::Future<Output = Result<Option<UsdbShortfall>, breez_sdk_spark::SdkError>>,
{
    let shortfall = match shortfall_conversion(amount_sat).await {
        Ok(shortfall) => shortfall,
        Err(e) => {
            tracing::warn!("could not read the balances for a cross-chain USDB top-up: {e}");
            None
        }
    };
    let prepared = prepare(shortfall.as_ref().map(|s| s.conversion.clone())).await;
    (shortfall, prepared)
}

/// The SDK request for [`handle_prepare_cross_chain`]: sats-denominated, with
/// the USDB top-up `conversion_options` when one is attached. With it, the
/// SDK converts USDB for the provider's amount plus the source transfer fee
/// before sending (`prepare_sats_denominated_with_conversion` at the pin).
fn cross_chain_prepare_request(
    address: String,
    route: CrossChainRoutePair,
    max_slippage_bps: Option<u32>,
    amount_sat: u64,
    conversion_options: Option<ConversionOptions>,
) -> PrepareSendPaymentRequest {
    PrepareSendPaymentRequest {
        payment_request: PaymentRequest::CrossChain {
            address,
            route,
            max_slippage_bps,
            // Leave the overpay pad at the SDK default (15 bps). It only
            // applies to `FeesExcluded` conversion sends and exists to stop the
            // recipient landing *under* the requested amount; there's no user
            // question here worth exposing.
            target_overpay_bps: None,
        },
        amount: Some(u128::from(amount_sat)),
        token_identifier: None,
        conversion_options,
        fee_policy: None,
    }
}

/// The answer to a failed cross-chain prepare. With a USDB top-up attached,
/// the failure is reported as the top-up's, as on the other send paths
/// ([`PrepareError::UsdbConversion`]); without one, as the SDK's error, as
/// before.
fn cross_chain_prepare_failed(
    id: u64,
    e: breez_sdk_spark::SdkError,
    shortfall: Option<&UsdbShortfall>,
) -> Response {
    match shortfall {
        Some(shortfall) => PrepareError::UsdbConversion {
            balance_sats: shortfall.balance_sats,
            source: e,
        }
        .into_response(id, "prepare_cross_chain"),
        None => Response::err(id, ErrorKind::Sdk, e.to_string()),
    }
}

async fn handle_send_payment(
    id: u64,
    params: coincube_spark_protocol::SendPaymentParams,
    state: Arc<ServerState>,
) -> Response {
    let sdk = match state.sdk.read().await.clone() {
        Some(s) => s,
        None => {
            return Response::err(
                id,
                ErrorKind::NotConnected,
                "init must succeed before send_payment",
            );
        }
    };

    // Phase 4e: the same `SendPayment` RPC handles both regular sends
    // and LNURL-pay sends. We look up the handle in `pending_prepares`
    // first; if it's not there, fall through to `pending_lnurl_prepares`
    // and dispatch to `sdk.lnurl_pay` instead of `sdk.send_payment`.
    let handle = params.prepare_handle;

    // Clone the prepare rather than removing it up front. A cross-chain
    // (token-leg) send must stay re-sendable under the *same* handle after a
    // failure: the provider derives its BTC-leg `TransferId` from this prepare's
    // swap id, so re-sending the identical quote dedups at the Spark protocol
    // level instead of paying a second time (see `derive_btc_leg_transfer_id` in
    // the SDK). A regular send has no such retry path, and a success consumes
    // the handle either way. The `PREPARE_TTL` sweep evicts a retained-but-
    // abandoned prepare after five minutes.
    let peeked = {
        let guard = state.pending_prepares.lock().await;
        guard.get(&handle).map(|(_, prepare)| prepare.clone())
    };
    if let Some(prepare) = peeked {
        let retain_on_failure =
            prepare.token_identifier.is_some() || prepare.conversion_estimate.is_some();
        let response = execute_regular_send(id, sdk, prepare, params.idempotency_key).await;
        let succeeded = matches!(
            &response.result,
            coincube_spark_protocol::ResponseResult::Ok(_)
        );
        if succeeded || !retain_on_failure {
            state.pending_prepares.lock().await.remove(&handle);
        }
        return response;
    }

    if let Some((_inserted_at, prepare)) = state.pending_lnurl_prepares.lock().await.remove(&handle)
    {
        return execute_lnurl_send(id, sdk, prepare, params.idempotency_key).await;
    }

    Response::err(
        id,
        ErrorKind::BadRequest,
        format!(
            "no pending prepare for handle {} (consumed, expired, or never existed)",
            handle
        ),
    )
}

async fn execute_regular_send(
    id: u64,
    sdk: SdkHandle,
    prepare: PrepareSendPaymentResponse,
    idempotency_key: Option<String>,
) -> Response {
    // Snapshot for the response so we can surface the final amount/fee
    // even after the SDK consumes the prepare response.
    //
    // One selection drives all three of: the fee reported back to the gui, the
    // options the SDK executes under, and (via `handle_prepare_send`) the fee
    // the confirmation screen showed. They cannot disagree.
    let speed = selected_onchain_speed();
    let (fee_sat, method_tag) = fee_and_method(&prepare.payment_method, &speed);
    // A direct token transfer (`prepare_usdb_transfer`) has
    // its amount and fee in token base units. Report no sats rather than
    // those, as its preview did. A conversion's prepare carries no token
    // identifier: the SDK clears it for `ToBitcoin`.
    let (amount_sat, fee_sat) = if prepare.token_identifier.is_some() {
        (0, 0)
    } else {
        (clamp_u128_to_u64(prepare.amount), fee_sat)
    };
    let options = send_options_for(&prepare.payment_method, &speed);
    if options.is_some() {
        // Which tier the money actually goes out at, next to the fee reported
        // for it. If those two ever disagree again, the log says so.
        tracing::debug!(
            "executing {method_tag} send at {} speed, fee {fee_sat} sat",
            speed_tag(&speed)
        );
    }

    // The SDK rejects an idempotency key on any payment with a token transfer
    // leg — a direct token send or an AMM conversion (see `orchestrate_send`).
    // A cross-chain send funds a stablecoin from BTC, so it always carries a
    // conversion leg; forwarding the gui's key made `send_payment` fail with
    // "Idempotency key is not supported for payments with a token transfer leg".
    // Dropping it costs nothing: with no key the provider derives the BTC-leg
    // `TransferId` deterministically from its own quote/swap id, so the source
    // transfer still dedups at the Spark protocol level (see
    // `derive_btc_leg_transfer_id`). Mirrors the SDK's own `has_token_leg` gate.
    let has_token_leg = prepare.token_identifier.is_some() || prepare.conversion_estimate.is_some();
    let idempotency_key = if has_token_leg { None } else { idempotency_key };

    // On-chain sends carry an **explicit** confirmation speed — the one the fee
    // above was quoted at. Leaving `options: None` here is what made the SDK
    // fall back to `Fast` while the user was looking at the Medium fee. Every
    // other method still passes `None` and keeps the SDK defaults it had
    // (Spark-preferred routing for Bolt11 without a completion timeout, etc.).
    let request = SendPaymentRequest {
        prepare_response: prepare,
        options,
        idempotency_key,
    };

    match sdk.sdk.send_payment(request).await {
        Ok(resp) => Response::ok(
            id,
            OkPayload::SendPayment(SendPaymentOk {
                payment_id: resp.payment.id,
                amount_sat,
                fee_sat,
            }),
        ),
        Err(e) => Response::err(id, ErrorKind::Sdk, format!("send_payment failed: {e}")),
    }
}

async fn execute_lnurl_send(
    id: u64,
    sdk: SdkHandle,
    prepare: PrepareLnurlPayResponse,
    idempotency_key: Option<String>,
) -> Response {
    // The LNURL prepare response carries its own top-level
    // `amount_sats` / `fee_sats` fields (u64, already in sats — no
    // u128 clamping needed here). Snapshot them for the send response.
    let amount_sat = prepare.amount_sats;
    let fee_sat = prepare.fee_sats;

    let request = LnurlPayRequest {
        prepare_response: prepare,
        // Honour the caller's key here too. Dropping it on this path alone
        // would make `SendPaymentParams::idempotency_key` a promise the bridge
        // silently breaks for LNURL sends — the retry would look guarded and
        // wouldn't be.
        idempotency_key,
    };

    match sdk.sdk.lnurl_pay(request).await {
        Ok(resp) => Response::ok(
            id,
            OkPayload::SendPayment(SendPaymentOk {
                payment_id: resp.payment.id,
                amount_sat,
                fee_sat,
            }),
        ),
        Err(e) => Response::err(id, ErrorKind::Sdk, format!("lnurl_pay failed: {e}")),
    }
}

async fn handle_receive_bolt11(
    id: u64,
    params: coincube_spark_protocol::ReceiveBolt11Params,
    state: Arc<ServerState>,
) -> Response {
    let sdk = match state.sdk.read().await.clone() {
        Some(s) => s,
        None => {
            return Response::err(
                id,
                ErrorKind::NotConnected,
                "init must succeed before receive_bolt11",
            );
        }
    };

    let request = ReceivePaymentRequest {
        payment_method: ReceivePaymentMethod::Bolt11Invoice {
            description: params.description,
            amount_sats: params.amount_sat,
            expiry_secs: params.expiry_secs,
            payment_hash: None,
            // 0.28.0: a caller-chosen preimage. Unset lets the SDK pick one.
            preimage: None,
            // 0.25.0: invoices can be issued for another Spark identity.
            // Ours always pay this wallet.
            receiver_identity_public_key: None,
        },
    };

    match sdk.sdk.receive_payment(request).await {
        Ok(resp) => Response::ok(
            id,
            OkPayload::ReceivePayment(ReceivePaymentOk {
                payment_request: resp.payment_request,
                fee_sat: clamp_u128_to_u64(resp.fee),
            }),
        ),
        Err(e) => Response::err(id, ErrorKind::Sdk, format!("receive_bolt11 failed: {e}")),
    }
}

async fn handle_receive_onchain(
    id: u64,
    params: coincube_spark_protocol::ReceiveOnchainParams,
    state: Arc<ServerState>,
) -> Response {
    let sdk = match state.sdk.read().await.clone() {
        Some(s) => s,
        None => {
            return Response::err(
                id,
                ErrorKind::NotConnected,
                "init must succeed before receive_onchain",
            );
        }
    };

    let request = ReceivePaymentRequest {
        payment_method: ReceivePaymentMethod::BitcoinAddress {
            new_address: params.new_address,
        },
    };

    match sdk.sdk.receive_payment(request).await {
        Ok(resp) => Response::ok(
            id,
            OkPayload::ReceivePayment(ReceivePaymentOk {
                payment_request: resp.payment_request,
                fee_sat: clamp_u128_to_u64(resp.fee),
            }),
        ),
        Err(e) => Response::err(id, ErrorKind::Sdk, format!("receive_onchain failed: {e}")),
    }
}

async fn handle_receive_spark(id: u64, state: Arc<ServerState>) -> Response {
    let sdk = match state.sdk.read().await.clone() {
        Some(s) => s,
        None => {
            return Response::err(
                id,
                ErrorKind::NotConnected,
                "init must succeed before receive_spark",
            );
        }
    };

    let request = ReceivePaymentRequest {
        payment_method: ReceivePaymentMethod::SparkAddress,
    };

    match sdk.sdk.receive_payment(request).await {
        Ok(resp) => Response::ok(
            id,
            OkPayload::ReceivePayment(ReceivePaymentOk {
                payment_request: resp.payment_request,
                fee_sat: clamp_u128_to_u64(resp.fee),
            }),
        ),
        Err(e) => Response::err(id, ErrorKind::Sdk, format!("receive_spark failed: {e}")),
    }
}

// ---------------------------------------------------------------------------
// Phase 4e: LNURL-pay support
// ---------------------------------------------------------------------------

async fn handle_parse_input(
    id: u64,
    params: coincube_spark_protocol::ParseInputParams,
    state: Arc<ServerState>,
) -> Response {
    let sdk = match state.sdk.read().await.clone() {
        Some(s) => s,
        None => {
            return Response::err(
                id,
                ErrorKind::NotConnected,
                "init must succeed before parse_input",
            );
        }
    };

    match sdk.sdk.parse(&params.input).await {
        Ok(input_type) => Response::ok(id, OkPayload::ParseInput(input_type_to_ok(input_type))),
        Err(e) => Response::err(id, ErrorKind::Sdk, format!("parse_input failed: {e}")),
    }
}

/// Translate a [`breez_sdk_spark::InputType`] into the protocol's
/// [`ParseInputOk`] shape. Only the fields the gui actually branches
/// on are surfaced — everything else stays inside the SDK type tree
/// and the bridge re-parses on `prepare_lnurl_pay` / `prepare_send`.
fn input_type_to_ok(input: InputType) -> ParseInputOk {
    // Sats-from-millisats helper — BOLT11 invoices carry
    // `amount_msat`, LNURL declares min/max in msats, etc.
    fn msat_to_sat(msat: u64) -> u64 {
        msat / 1000
    }

    match input {
        InputType::Bolt11Invoice(details) => ParseInputOk {
            kind: ParseInputKind::Bolt11Invoice,
            amount_sat: details.amount_msat.map(msat_to_sat),
            lnurl_min_sendable_sat: None,
            lnurl_max_sendable_sat: None,
            lnurl_comment_allowed: 0,
            lnurl_address: None,
            usdb_invoice: false,
        },
        InputType::BitcoinAddress(_details) => ParseInputOk {
            // Plain on-chain addresses don't carry an amount — only
            // BIP21 URIs do. The user must supply one in the Send
            // panel's amount field for the prepare to succeed.
            kind: ParseInputKind::BitcoinAddress,
            amount_sat: None,
            lnurl_min_sendable_sat: None,
            lnurl_max_sendable_sat: None,
            lnurl_comment_allowed: 0,
            lnurl_address: None,
            usdb_invoice: false,
        },
        InputType::Bip21(details) => ParseInputOk {
            kind: ParseInputKind::BitcoinAddress,
            amount_sat: details.amount_sat,
            lnurl_min_sendable_sat: None,
            lnurl_max_sendable_sat: None,
            lnurl_comment_allowed: 0,
            lnurl_address: None,
            usdb_invoice: false,
        },
        InputType::LnurlPay(pay) => ParseInputOk {
            kind: ParseInputKind::LnurlPay,
            amount_sat: None,
            lnurl_min_sendable_sat: Some(msat_to_sat(pay.min_sendable)),
            lnurl_max_sendable_sat: Some(msat_to_sat(pay.max_sendable)),
            lnurl_comment_allowed: pay.comment_allowed,
            lnurl_address: pay.address,
            usdb_invoice: false,
        },
        InputType::LightningAddress(addr) => ParseInputOk {
            kind: ParseInputKind::LightningAddress,
            amount_sat: None,
            lnurl_min_sendable_sat: Some(msat_to_sat(addr.pay_request.min_sendable)),
            lnurl_max_sendable_sat: Some(msat_to_sat(addr.pay_request.max_sendable)),
            lnurl_comment_allowed: addr.pay_request.comment_allowed,
            lnurl_address: Some(addr.address),
            usdb_invoice: false,
        },
        InputType::SparkAddress(_details) => ParseInputOk {
            // Static, identity-bound Spark address — no amount.
            kind: ParseInputKind::SparkAddress,
            amount_sat: None,
            lnurl_min_sendable_sat: None,
            lnurl_max_sendable_sat: None,
            lnurl_comment_allowed: 0,
            lnurl_address: None,
            usdb_invoice: false,
        },
        InputType::SparkInvoice(details) => ParseInputOk {
            kind: ParseInputKind::SparkInvoice,
            // `amount` is sats only for Bitcoin invoices; for token
            // invoices it's token base units, which the sats-typed
            // field can't represent — surface None in that case.
            amount_sat: if details.token_identifier.is_none() {
                details.amount.map(clamp_u128_to_u64)
            } else {
                None
            },
            lnurl_min_sendable_sat: None,
            lnurl_max_sendable_sat: None,
            lnurl_comment_allowed: 0,
            lnurl_address: None,
            usdb_invoice: details.token_identifier.as_deref()
                == Some(sdk_adapter::USDB_MAINNET_TOKEN_IDENTIFIER),
        },
        // Everything else — BOLT12 invoices/offers, LNURL-auth,
        // LNURL-withdraw, silent payment, bare URLs — falls through
        // to `Other`. The gui shows a "not supported yet" error;
        // future phases can break each one out as demand appears.
        _ => ParseInputOk {
            kind: ParseInputKind::Other,
            amount_sat: None,
            lnurl_min_sendable_sat: None,
            lnurl_max_sendable_sat: None,
            lnurl_comment_allowed: 0,
            lnurl_address: None,
            usdb_invoice: false,
        },
    }
}

async fn handle_prepare_lnurl_pay(
    id: u64,
    params: coincube_spark_protocol::PrepareLnurlPayParams,
    state: Arc<ServerState>,
) -> Response {
    let (input, amount_sat, comment) = (params.input, params.amount_sat, params.comment);
    match prepare_lnurl_pay(id, input, amount_sat, comment, None, &state).await {
        Ok((prepare, _)) => Response::ok(id, OkPayload::PrepareSend(prepare)),
        Err(response) => response,
    }
}

async fn handle_prepare_lnurl_pay_from(
    id: u64,
    params: PrepareLnurlPayFromParams,
    state: Arc<ServerState>,
) -> Response {
    let pay_from = params.pay_from;
    let (input, amount_sat, comment) = (params.input, params.amount_sat, params.comment);
    match prepare_lnurl_pay(id, input, amount_sat, comment, Some(pay_from), &state).await {
        Ok((prepare, conversion)) => Response::ok(
            id,
            OkPayload::PrepareSendFrom(PrepareSendFromOk {
                prepare,
                pay_from,
                conversion,
                token_payment: None,
            }),
        ),
        Err(response) => response,
    }
}

/// [`Method::PrepareLnurlPay`] (`pay_from: None`) and
/// [`Method::PrepareLnurlPayFrom`], as [`prepare_send`] is for the regular
/// send.
async fn prepare_lnurl_pay(
    id: u64,
    input: String,
    amount_sat: u64,
    comment: Option<String>,
    pay_from_param: Option<WalletAsset>,
    state: &ServerState,
) -> Result<(PrepareSendOk, Option<Box<ConversionQuote>>), Response> {
    let pay_from = pay_from_param.unwrap_or(WalletAsset::Bitcoin);
    let sdk = match state.sdk.read().await.clone() {
        Some(s) => s,
        None => {
            return Err(Response::err(
                id,
                ErrorKind::NotConnected,
                "init must succeed before prepare_lnurl_pay",
            ));
        }
    };

    // Re-parse the input to recover the SDK's `LnurlPayRequestDetails`.
    // We could stash the parse result from the earlier `parse_input`
    // call and pass it back, but that would tie the protocol to the
    // SDK's internal types. Re-parsing is cheap — it's a local
    // regex/bech32 decode on a string we already know to be valid.
    let pay_request = match sdk.sdk.parse(&input).await {
        Ok(InputType::LnurlPay(details)) => details,
        Ok(InputType::LightningAddress(addr)) => addr.pay_request,
        Ok(other) => {
            return Err(Response::err(
                id,
                ErrorKind::BadRequest,
                format!(
                    "prepare_lnurl_pay called with non-LNURL input (parsed as {:?})",
                    std::mem::discriminant(&other)
                ),
            ));
        }
        Err(e) => {
            return Err(Response::err(
                id,
                ErrorKind::Sdk,
                format!("parse_input failed: {e}"),
            ));
        }
    };

    let prepared = prepare_funded(
        pay_from,
        Some(amount_sat),
        |conversion_options| {
            sdk.sdk.prepare_lnurl_pay(PrepareLnurlPayRequest {
                amount: amount_sat as u128,
                pay_request: pay_request.clone(),
                comment: comment.clone(),
                validate_success_action_url: None,
                token_identifier: None,
                conversion_options,
                fee_policy: None,
            })
        },
        |needed| usdb_shortfall_conversion(&sdk, needed),
        |p: &PrepareLnurlPayResponse| p.amount_sats.saturating_add(p.fee_sats),
        |p: &PrepareLnurlPayResponse| p.conversion_estimate.is_some(),
    )
    .await;

    match prepared {
        Ok(prepare) => {
            // Same conversion attach as the regular send path (the bridge's
            // USDB fallback), and the same refusal when the holding can't
            // fund it. This path has a
            // second failure mode the check also heads off: `execute_lnurl_send`
            // forwards the idempotency key, and the SDK rejects a key on a
            // payment with a token leg outright.
            if let Some(response) = unfundable_conversion_error(
                id,
                &sdk,
                prepare.conversion_estimate.as_ref(),
                pay_from,
            )
            .await
            {
                return Err(response);
            }
            // LNURL has no token leg but the conversion.
            if let Some((kind, reason)) = pay_from_param.and_then(|pay_from| {
                pay_from_refusal(pay_from, None, prepare.conversion_estimate.is_some())
            }) {
                return Err(Response::err(id, kind, reason));
            }
            let conversion = match pay_from_param {
                Some(_) => send_conversion_quote(prepare.conversion_estimate.as_ref())
                    .map_err(|e| Response::err(id, ErrorKind::Sdk, e))?,
                None => None,
            };

            // Preview fields come straight out of the SDK's
            // `PrepareLnurlPayResponse` — it already exposes top-level
            // `amount_sats` and `fee_sats` in u64, so no u128 clamping
            // is needed on this path.
            let amount_sat = prepare.amount_sats;
            let fee_sat = prepare.fee_sats;
            let method = "LnurlPay".to_string();
            let has_token_leg = prepare.conversion_estimate.is_some();

            let handle = Uuid::new_v4().to_string();
            state
                .pending_lnurl_prepares
                .lock()
                .await
                .insert(handle.clone(), (Instant::now(), prepare));

            Ok((
                PrepareSendOk {
                    handle,
                    amount_sat,
                    fee_sat,
                    method,
                    // LNURL-pay is a Lightning send; never cross-chain.
                    cross_chain: None,
                    has_token_leg: Some(has_token_leg),
                },
                conversion,
            ))
        }
        Err(e) => Err(e.into_response(id, "prepare_lnurl_pay")),
    }
}

// ---------------------------------------------------------------------------
// Phase 4f: on-chain claim lifecycle
// ---------------------------------------------------------------------------

async fn handle_list_unclaimed_deposits(id: u64, state: Arc<ServerState>) -> Response {
    let sdk = match state.sdk.read().await.clone() {
        Some(s) => s,
        None => {
            return Response::err(
                id,
                ErrorKind::NotConnected,
                "init must succeed before list_unclaimed_deposits",
            );
        }
    };

    match sdk
        .sdk
        .list_unclaimed_deposits(ListUnclaimedDepositsRequest {})
        .await
    {
        Ok(resp) => {
            let deposits: Vec<DepositInfo> = resp
                .deposits
                .into_iter()
                .map(|d| DepositInfo {
                    txid: d.txid,
                    vout: d.vout,
                    amount_sat: d.amount_sats,
                    is_mature: d.is_mature,
                    // Stringify the SDK's `DepositClaimError` enum
                    // for display. Phase 4g+ can promote to a typed
                    // protocol enum if the gui needs to branch on
                    // specific error reasons.
                    claim_error: d.claim_error.map(|e| format!("{:?}", e)),
                })
                .collect();
            Response::ok(
                id,
                OkPayload::ListUnclaimedDeposits(ListUnclaimedDepositsOk { deposits }),
            )
        }
        Err(e) => Response::err(
            id,
            ErrorKind::Sdk,
            format!("list_unclaimed_deposits failed: {e}"),
        ),
    }
}

async fn handle_claim_deposit(
    id: u64,
    params: coincube_spark_protocol::ClaimDepositParams,
    state: Arc<ServerState>,
) -> Response {
    let sdk = match state.sdk.read().await.clone() {
        Some(s) => s,
        None => {
            return Response::err(
                id,
                ErrorKind::NotConnected,
                "init must succeed before claim_deposit",
            );
        }
    };

    let request = ClaimDepositRequest {
        txid: params.txid,
        vout: params.vout,
        // Cap the claim fee at the network's fastest recommended rate plus a
        // small leeway. NOT `None`: the SDK defaults that to a 1 sat/vbyte cap
        // (`max_deposit_claim_fee`), which fails with `MaxDepositClaimFeeExceeded`
        // the moment the network needs more than 1 sat/vbyte — stranding the
        // deposit until fees happen to drop that low. `NetworkRecommended`
        // adapts to current conditions; the SDK still pays only the rate
        // required to confirm, this is just the ceiling.
        max_fee: Some(MaxFee {
            onchain: MaxOnchainFee::NetworkRecommended {
                leeway_sat_per_vbyte: 5,
            },
            // 0.27.0: an optional share of the deposit on top. None keeps the
            // ceiling what it was: on-chain only.
            proportional_ppm: None,
        }),
    };

    match sdk.sdk.claim_deposit(request).await {
        Ok(resp) => {
            // A deposit claimed at maturity settles synchronously and comes
            // back with the Payment, whose `amount` is the post-fee value.
            // Since 0.25.0 a deposit can also be claimed *before* maturity
            // (when the provider's spread fits under `max_fee`); that
            // transfer settles asynchronously and there is no Payment yet —
            // the gui hears about it through `PaymentSucceeded` /
            // `DepositsChanged` like any other incoming transfer.
            Response::ok(id, OkPayload::ClaimDeposit(claim_deposit_ok(resp.outcome)))
        }
        Err(e) => Response::err(id, ErrorKind::Sdk, format!("claim_deposit failed: {e}")),
    }
}

fn claim_deposit_ok(outcome: ClaimDepositOutcome) -> ClaimDepositOk {
    let (payment_id, amount_sat, deferred) = match outcome {
        ClaimDepositOutcome::Settled { payment } => (
            Some(payment.id),
            Some(clamp_u128_to_u64(payment.amount)),
            None,
        ),
        ClaimDepositOutcome::Submitted => (None, None, None),
        // 0.28.0: nothing was claimed yet, and the SDK keeps trying on its
        // own. Not a failure, so it is an Ok carrying the reason.
        ClaimDepositOutcome::Deferred { reason } => (None, None, Some(claim_deferral(reason))),
    };
    ClaimDepositOk {
        payment_id,
        amount_sat,
        deferred,
    }
}

fn claim_deferral(reason: ClaimDeferredReason) -> ClaimDeferral {
    match reason {
        ClaimDeferredReason::MaxFeeExceeded {
            required_fee_sats,
            max_fee_sats,
        } => ClaimDeferral::MaxFeeExceeded {
            required_fee_sats,
            max_fee_sats,
        },
        ClaimDeferredReason::NoEarlyClaimAvailable => ClaimDeferral::NoEarlyClaimAvailable,
        ClaimDeferredReason::ProviderDeclined { message } => {
            ClaimDeferral::ProviderDeclined { message }
        }
    }
}

async fn handle_get_user_settings(id: u64, state: Arc<ServerState>) -> Response {
    let sdk = match state.sdk.read().await.clone() {
        Some(s) => s,
        None => {
            return Response::err(
                id,
                ErrorKind::NotConnected,
                "init must succeed before get_user_settings",
            );
        }
    };
    let Some(auto_convert) = state.auto_convert.get() else {
        return Response::err(
            id,
            ErrorKind::NotConnected,
            "init must succeed before get_user_settings",
        );
    };

    match sdk.sdk.get_user_settings().await {
        Ok(settings) => {
            let auto_convert = auto_convert.lock().await.state();
            Response::ok(
                id,
                OkPayload::GetUserSettings(user_settings_ok(
                    &auto_convert,
                    settings.spark_private_mode_enabled,
                )),
            )
        }
        Err(e) => Response::err(id, ErrorKind::Sdk, format!("get_user_settings failed: {e}")),
    }
}

/// The legacy settings answer. `stable_balance_active` has meant
/// bridge-driven auto-convert since S2b: the gui built before it reconciles
/// its per-Cube Stable Balance preference against this flag, and so moves
/// that preference onto auto-convert. SDK Stable Balance is never
/// configured, so there is no other state for it to report.
fn user_settings_ok(auto_convert: &AutoConvertState, private_mode: bool) -> GetUserSettingsOk {
    GetUserSettingsOk {
        stable_balance_active: auto_convert.enabled,
        private_mode_enabled: private_mode,
    }
}

/// Legacy alias for [`handle_set_auto_convert`], answered the old way. The
/// gui built before S2b drives auto-convert through it: its Settings switch
/// and its per-Cube preference reconcile.
async fn handle_set_stable_balance(
    id: u64,
    params: SetStableBalanceParams,
    state: Arc<ServerState>,
) -> Response {
    match set_auto_convert(&state, params.enabled, "set_stable_balance").await {
        Ok(_) => Response::ok(id, OkPayload::SetStableBalance {}),
        Err((kind, message)) => Response::err(id, kind, message),
    }
}

async fn handle_get_auto_convert(id: u64, state: Arc<ServerState>) -> Response {
    match state.auto_convert.get() {
        Some(auto_convert) => Response::ok(
            id,
            OkPayload::AutoConvert(auto_convert.lock().await.state()),
        ),
        None => Response::err(
            id,
            ErrorKind::NotConnected,
            "init must succeed before get_auto_convert",
        ),
    }
}

async fn handle_set_auto_convert(
    id: u64,
    params: SetAutoConvertParams,
    state: Arc<ServerState>,
) -> Response {
    match set_auto_convert(&state, params.enabled, "set_auto_convert").await {
        Ok(auto_convert) => Response::ok(id, OkPayload::AutoConvert(auto_convert)),
        Err((kind, message)) => Response::err(id, kind, message),
    }
}

/// Turn auto-convert on or off, and return the state after. The change is
/// saved before it is answered: if it can't be saved, nothing changed and
/// the answer is an error. A change is also announced with
/// [`ProtocolEvent::AutoConvertStateChanged`].
///
/// It first takes the send gate ([`AutoConvert::send_gate`]), so it waits
/// for a conversion already committed to its send. Off is therefore never
/// acknowledged while a conversion can still start.
///
/// Neither direction touches the wallet's funds: off leaves the USDB
/// balance as USDB, and on converts only bitcoin received afterwards.
async fn set_auto_convert(
    state: &ServerState,
    enabled: bool,
    method: &str,
) -> Result<AutoConvertState, (ErrorKind, String)> {
    let Some(auto_convert) = state.auto_convert.get() else {
        return Err((
            ErrorKind::NotConnected,
            format!("init must succeed before {method}"),
        ));
    };
    let gate = auto_convert.lock().await.send_gate();
    let _no_send_in_flight = gate.lock().await;
    let mut auto_convert = auto_convert.lock().await;
    let changed = auto_convert.set_enabled(enabled).map_err(|e| {
        (
            ErrorKind::Sdk,
            format!("could not save the auto-convert setting: {e}"),
        )
    })?;
    let after = auto_convert.state();
    if changed {
        let _ = state
            .event_tx
            .send(Frame::Event(ProtocolEvent::AutoConvertStateChanged {
                state: after.clone(),
            }));
    }
    Ok(after)
}

// ---------------------------------------------------------------------------
// #950 S3b: BTC ⇄ USDB swaps (see `crate::swap`)
// ---------------------------------------------------------------------------

async fn handle_get_conversion_limits(
    id: u64,
    params: GetConversionLimitsParams,
    state: Arc<ServerState>,
) -> Response {
    let Some(sdk) = state.sdk.read().await.clone() else {
        return Response::err(
            id,
            ErrorKind::NotConnected,
            "init must succeed before get_conversion_limits",
        );
    };
    let backend = SdkConverter::new(sdk, Arc::clone(&state.own_spark_address));
    match swap::limits(&backend, params.from).await {
        Ok(limits) => Response::ok(id, OkPayload::GetConversionLimits(limits)),
        Err(e) => Response::err(
            id,
            ErrorKind::Sdk,
            format!("get_conversion_limits failed: {e}"),
        ),
    }
}

async fn handle_prepare_swap(
    id: u64,
    params: PrepareSwapParams,
    state: Arc<ServerState>,
) -> Response {
    let max_slippage_bps = match swap::slippage_cap(params.max_slippage_bps) {
        Ok(cap) => cap,
        Err(e) => return Response::err(id, ErrorKind::BadRequest, e),
    };
    let Some(sdk) = state.sdk.read().await.clone() else {
        return Response::err(
            id,
            ErrorKind::NotConnected,
            "init must succeed before prepare_swap",
        );
    };
    let backend =
        SdkConverter::new(sdk, Arc::clone(&state.own_spark_address)).at_slippage(max_slippage_bps);
    match swap::prepare(&backend, params.amount, max_slippage_bps).await {
        Ok(swap::Prepared {
            swap,
            quote,
            limits,
        }) => {
            let handle = Uuid::new_v4().to_string();
            state
                .pending_swaps
                .lock()
                .await
                .insert(handle.clone(), (Instant::now(), swap));
            Response::ok(
                id,
                OkPayload::PrepareSwap(PrepareSwapOk {
                    handle,
                    quote,
                    limits,
                }),
            )
        }
        Err(refusal) => swap_refusal(id, "prepare_swap", refusal),
    }
}

/// Execute a prepared swap. Every refusal before the SDK is asked to send is
/// `BadRequest`, including the one before `init` (there can be no handle
/// then), so the gui can treat it as definite ([`Method::Swap`]).
async fn handle_swap(id: u64, params: SwapParams, state: Arc<ServerState>) -> Response {
    // Single-use: taken before anything else, so a refused swap must be
    // prepared again.
    let taken = state
        .pending_swaps
        .lock()
        .await
        .remove(&params.prepare_handle);
    let Some((_inserted_at, prepared)) = taken else {
        return Response::err(
            id,
            ErrorKind::BadRequest,
            format!(
                "no pending swap for handle {} (consumed, expired, or never existed). Nothing \
                 was sent.",
                params.prepare_handle
            ),
        );
    };
    let Some(sdk) = state.sdk.read().await.clone() else {
        return Response::err(
            id,
            ErrorKind::BadRequest,
            "init must succeed before swap. Nothing was sent.",
        );
    };
    let backend = SdkConverter::new(sdk, Arc::clone(&state.own_spark_address))
        .at_slippage(prepared.max_slippage_bps());
    let auto_convert = state.auto_convert.get().map(|ac| ac.as_ref());
    let emit = |event| {
        let _ = state.event_tx.send(Frame::Event(event));
    };
    let outcome = swap::execute(&backend, prepared, auto_convert, &emit).await;
    // Passes held during a swap out of USDB run now; otherwise this finds
    // nothing new to do.
    if let Some(wake) = state.wake_auto_convert.get() {
        let _ = wake.send(());
    }
    match outcome {
        Ok((swap_id, status)) => {
            // Followed before the answer is written, so in a rare race a
            // `SwapStatusChanged` can reach the gui just before its `SwapOk`.
            state.swaps.lock().await.track(&swap_id, status);
            Response::ok(id, OkPayload::Swap(SwapOk { swap_id, status }))
        }
        // The SDK's whole text: after an unknown outcome it is the only
        // record the gui logs of what happened.
        Err(swap::Refusal::Sdk(message)) => {
            Response::err(id, ErrorKind::Sdk, format!("swap failed: {message}"))
        }
        Err(refusal) => swap_refusal(id, "swap", refusal),
    }
}

/// A swap refusal as a response. An SDK error keeps only the human part of
/// a Flashnet error ([`conversion_failure_reason`]).
fn swap_refusal(id: u64, what: &str, refusal: swap::Refusal) -> Response {
    match refusal {
        swap::Refusal::BadRequest(message) => Response::err(id, ErrorKind::BadRequest, message),
        swap::Refusal::Sdk(message) => Response::err(
            id,
            ErrorKind::Sdk,
            format!("{what} failed: {}", conversion_failure_reason(&message)),
        ),
    }
}

// ---------------------------------------------------------------------------
// Phase 4g: Breez-hosted LNURL / Lightning Address management
// ---------------------------------------------------------------------------

async fn handle_check_lightning_address_available(
    id: u64,
    params: coincube_spark_protocol::CheckLightningAddressAvailableParams,
    state: Arc<ServerState>,
) -> Response {
    let sdk = match state.sdk.read().await.clone() {
        Some(s) => s,
        None => {
            return Response::err(
                id,
                ErrorKind::NotConnected,
                "init must succeed before check_lightning_address_available",
            );
        }
    };

    match sdk
        .sdk
        .check_lightning_address_available(CheckLightningAddressRequest {
            username: params.username,
        })
        .await
    {
        Ok(available) => Response::ok(
            id,
            OkPayload::CheckLightningAddressAvailable(CheckLightningAddressAvailableOk {
                available,
            }),
        ),
        Err(e) => Response::err(
            id,
            ErrorKind::Sdk,
            format!("check_lightning_address_available failed: {e}"),
        ),
    }
}

async fn handle_register_lightning_address(
    id: u64,
    params: coincube_spark_protocol::RegisterLightningAddressParams,
    state: Arc<ServerState>,
) -> Response {
    let sdk = match state.sdk.read().await.clone() {
        Some(s) => s,
        None => {
            return Response::err(
                id,
                ErrorKind::NotConnected,
                "init must succeed before register_lightning_address",
            );
        }
    };

    match sdk
        .sdk
        .register_lightning_address(RegisterLightningAddressRequest {
            username: params.username,
            description: params.description,
        })
        .await
    {
        Ok(info) => Response::ok(
            id,
            OkPayload::RegisterLightningAddress(RegisterLightningAddressOk {
                info: sdk_address_info_to_protocol(info),
            }),
        ),
        Err(e) => Response::err(
            id,
            ErrorKind::Sdk,
            format!("register_lightning_address failed: {e}"),
        ),
    }
}

async fn handle_get_lightning_address(id: u64, state: Arc<ServerState>) -> Response {
    let sdk = match state.sdk.read().await.clone() {
        Some(s) => s,
        None => {
            return Response::err(
                id,
                ErrorKind::NotConnected,
                "init must succeed before get_lightning_address",
            );
        }
    };

    match sdk.sdk.get_lightning_address().await {
        Ok(info) => Response::ok(
            id,
            OkPayload::GetLightningAddress(GetLightningAddressOk {
                info: info.map(sdk_address_info_to_protocol),
            }),
        ),
        Err(e) => Response::err(
            id,
            ErrorKind::Sdk,
            format!("get_lightning_address failed: {e}"),
        ),
    }
}

async fn handle_delete_lightning_address(id: u64, state: Arc<ServerState>) -> Response {
    let sdk = match state.sdk.read().await.clone() {
        Some(s) => s,
        None => {
            return Response::err(
                id,
                ErrorKind::NotConnected,
                "init must succeed before delete_lightning_address",
            );
        }
    };

    match sdk.sdk.delete_lightning_address().await {
        Ok(()) => Response::ok(id, OkPayload::DeleteLightningAddress {}),
        Err(e) => Response::err(
            id,
            ErrorKind::Sdk,
            format!("delete_lightning_address failed: {e}"),
        ),
    }
}

/// Flatten the SDK's `LightningAddressInfo { lnurl: LnurlInfo { url,
/// bech32 }, .. }` into the protocol's flat shape so the gui doesn't
/// need to know about `LnurlInfo`. The SDK's `description` is
/// non-optional; we preserve its value as-is (callers that didn't
/// supply one got the SDK's `"Pay to <user>@<domain>"` default).
fn sdk_address_info_to_protocol(info: SdkLightningAddressInfo) -> ProtocolLightningAddressInfo {
    ProtocolLightningAddressInfo {
        lightning_address: info.lightning_address,
        username: info.username,
        description: Some(info.description),
        lnurl_url: info.lnurl.url,
        lnurl_bech32: info.lnurl.bech32,
    }
}

/// Saturating cast. Spark token amounts are u128 in the SDK (room for
/// arbitrary precision tokens); sat-denominated amounts fit well within
/// u64 in practice but we clamp defensively so an overflow doesn't
/// panic the bridge mid-request.
fn clamp_u128_to_u64(v: u128) -> u64 {
    if v > u64::MAX as u128 {
        u64::MAX
    } else {
        v as u64
    }
}

/// Phase 4f: walk the pending-prepare maps and drop entries whose
/// insertion timestamp is older than [`PREPARE_TTL`]. Called from the
/// background sweep task in `run()`.
///
/// Logged at debug level when entries are evicted so manual smoke
/// testing can observe the eviction without noise on a quiet bridge.
async fn sweep_expired_prepares(state: &Arc<ServerState>) {
    let now = Instant::now();
    let mut evicted_regular = 0usize;
    let mut evicted_lnurl = 0usize;

    {
        let mut guard = state.pending_prepares.lock().await;
        guard.retain(|_handle, (inserted_at, _prepare)| {
            let keep = now.duration_since(*inserted_at) < PREPARE_TTL;
            if !keep {
                evicted_regular += 1;
            }
            keep
        });
    }
    {
        let mut guard = state.pending_lnurl_prepares.lock().await;
        guard.retain(|_handle, (inserted_at, _prepare)| {
            let keep = now.duration_since(*inserted_at) < PREPARE_TTL;
            if !keep {
                evicted_lnurl += 1;
            }
            keep
        });
    }
    let mut evicted_swaps = 0usize;
    {
        let mut guard = state.pending_swaps.lock().await;
        guard.retain(|_handle, (inserted_at, _swap)| {
            let keep = now.duration_since(*inserted_at) < PREPARE_TTL;
            if !keep {
                evicted_swaps += 1;
            }
            keep
        });
    }

    if evicted_regular > 0 || evicted_lnurl > 0 || evicted_swaps > 0 {
        tracing::debug!(
            "evicted {} expired prepare(s), {} expired lnurl prepare(s), {} expired swap(s)",
            evicted_regular,
            evicted_lnurl,
            evicted_swaps
        );
    }
}

#[cfg(test)]
mod fee_tier_tests {
    use super::*;
    use breez_sdk_spark::{
        BitcoinAddressDetails, BitcoinNetwork, PaymentRequestSource, SendOnchainSpeedFeeQuote,
    };

    /// Distinct per-tier totals so a test can tell which tier was read.
    /// slow = 100, medium = 220, fast = 3_300.
    fn fee_quote() -> SendOnchainFeeQuote {
        let tier = |user: u64, l1: u64| SendOnchainSpeedFeeQuote {
            user_fee_sat: user,
            l1_broadcast_fee_sat: l1,
        };
        SendOnchainFeeQuote {
            id: "quote-1".to_string(),
            expires_at: 0,
            speed_fast: tier(3_000, 300),
            speed_medium: tier(200, 20),
            speed_slow: tier(90, 10),
            is_estimate: false,
        }
    }

    fn bitcoin_address_method() -> SendPaymentMethod {
        SendPaymentMethod::BitcoinAddress {
            address: BitcoinAddressDetails {
                address: "bc1qexample".to_string(),
                network: BitcoinNetwork::Bitcoin,
                source: PaymentRequestSource {
                    bip_21_uri: None,
                    bip_353_address: None,
                },
            },
            fee_quote: fee_quote(),
        }
    }

    fn spark_address_method() -> SendPaymentMethod {
        SendPaymentMethod::SparkAddress {
            address: "sp1example".to_string(),
            fee: 42,
            token_identifier: None,
        }
    }

    /// **The audited mismatch, as a test.** A Bitcoin-address send prepared at
    /// Medium must execute with an explicit Medium option — not `None`, which
    /// the pinned SDK resolves to Fast, charging 3_300 sats for a send quoted
    /// at 220.
    #[test]
    fn a_bitcoin_address_send_executes_at_the_tier_it_was_quoted_at() {
        let method = bitcoin_address_method();
        let speed = selected_onchain_speed();
        assert_eq!(
            speed_tag(&speed),
            "medium",
            "the shipped tier is Medium; if this changes, the fee shown changes with it"
        );

        let (quoted_fee, tag) = fee_and_method(&method, &speed);
        assert_eq!(tag, "BitcoinAddress");
        assert_eq!(quoted_fee, 220, "the preview must be the medium total");

        match send_options_for(&method, &speed) {
            Some(SendPaymentOptions::BitcoinAddress { confirmation_speed }) => {
                assert_eq!(
                    speed_tag(&confirmation_speed),
                    "medium",
                    "execution must name the tier explicitly — `None` means Fast in \
                     the pinned SDK, which is the bug"
                );
            }
            _ => panic!("a Bitcoin-address send must carry an explicit confirmation speed"),
        }
    }

    /// Preview and execution read one selection: for every tier, the fee the gui
    /// is given equals the fee of the tier the SDK is told to use. This is the
    /// property a future Fast/Medium/Slow picker must not be able to break.
    #[test]
    fn the_quoted_fee_and_the_executed_tier_agree_for_every_tier() {
        let method = bitcoin_address_method();
        let expected = [
            (OnchainConfirmationSpeed::Fast, 3_300_u64),
            (OnchainConfirmationSpeed::Medium, 220),
            (OnchainConfirmationSpeed::Slow, 100),
        ];

        for (speed, fee) in expected {
            let tag = speed_tag(&speed);
            let (quoted, _) = fee_and_method(&method, &speed);
            assert_eq!(quoted, fee, "wrong fee quoted for {tag}");

            let Some(SendPaymentOptions::BitcoinAddress { confirmation_speed }) =
                send_options_for(&method, &speed)
            else {
                panic!("a Bitcoin-address send must carry options for {tag}");
            };
            assert_eq!(speed_tag(&confirmation_speed), tag);
            // And the fee that tier will actually be charged at.
            assert_eq!(
                onchain_fee_for_speed(
                    match &method {
                        SendPaymentMethod::BitcoinAddress { fee_quote, .. } => fee_quote,
                        _ => unreachable!(),
                    },
                    &confirmation_speed
                ),
                quoted,
                "the executed tier's fee differs from the quoted fee for {tag}"
            );
        }
    }

    /// Fast and Slow must not be reportable as Medium: the tiers are distinct
    /// values and the lookup is total, so a mixed-up tier shows up as a
    /// different number rather than silently passing.
    #[test]
    fn the_tiers_cannot_be_confused_with_one_another() {
        let quote = fee_quote();
        let fast = onchain_fee_for_speed(&quote, &OnchainConfirmationSpeed::Fast);
        let medium = onchain_fee_for_speed(&quote, &OnchainConfirmationSpeed::Medium);
        let slow = onchain_fee_for_speed(&quote, &OnchainConfirmationSpeed::Slow);

        assert_eq!((fast, medium, slow), (3_300, 220, 100));
        assert_ne!(fast, medium);
        assert_ne!(slow, medium);
        assert!(slow < medium && medium < fast, "tiers must stay ordered");
    }

    /// Fee arithmetic saturates rather than wrapping: both halves come from the
    /// SDK and a wrapped total would be shown to the user as a tiny fee.
    #[test]
    fn tier_totals_saturate_on_absurd_sdk_values() {
        let quote = SendOnchainFeeQuote {
            id: "overflow".to_string(),
            expires_at: 0,
            speed_fast: SendOnchainSpeedFeeQuote {
                user_fee_sat: u64::MAX,
                l1_broadcast_fee_sat: 10,
            },
            speed_medium: SendOnchainSpeedFeeQuote {
                user_fee_sat: u64::MAX,
                l1_broadcast_fee_sat: 1,
            },
            speed_slow: SendOnchainSpeedFeeQuote {
                user_fee_sat: 0,
                l1_broadcast_fee_sat: 0,
            },
            is_estimate: false,
        };
        assert_eq!(
            onchain_fee_for_speed(&quote, &OnchainConfirmationSpeed::Medium),
            u64::MAX
        );
    }

    /// Every other payment method keeps the options it has always been executed
    /// with — `None`. The SDK rejects a Bitcoin-address option elsewhere with
    /// `InvalidInput`, so applying one would break Bolt11 and Spark sends
    /// outright.
    #[test]
    fn non_bitcoin_methods_keep_their_existing_options_and_fees() {
        let speed = selected_onchain_speed();

        let spark = spark_address_method();
        assert!(
            send_options_for(&spark, &speed).is_none(),
            "a Spark send must not be given Bitcoin-address options"
        );
        assert_eq!(fee_and_method(&spark, &speed), (42, "SparkAddress"));

        // The tier must make no difference off the on-chain path.
        for other in [
            OnchainConfirmationSpeed::Fast,
            OnchainConfirmationSpeed::Slow,
        ] {
            assert_eq!(fee_and_method(&spark, &other), (42, "SparkAddress"));
            assert!(send_options_for(&spark, &other).is_none());
        }
    }
}

#[cfg(test)]
mod usdb_fallback_tests {
    use super::*;
    use breez_sdk_spark::SdkError;
    use std::cell::Cell;

    #[test]
    fn shortfall_conversion_only_when_bitcoin_is_short_and_usdb_is_held() {
        // Enough bitcoin: no conversion, whatever the USDB holding.
        assert!(conversion_for_shortfall(2_000_100, 5_000_000, 2_000_100).is_none());
        assert!(conversion_for_shortfall(3_000_000, 0, 2_000_100).is_none());
        // Short, but nothing to convert from.
        assert!(conversion_for_shortfall(11_254, 0, 2_000_100).is_none());
        // Short and USDB held: convert from USDB, capped at the 50 bps the
        // gui shows (not the SDK's 10 bps default). The timeout stays the
        // SDK's.
        let shortfall = conversion_for_shortfall(11_254, 5_000_000, 2_000_100)
            .expect("usdb should fund the shortfall");
        assert_eq!(shortfall.balance_sats, 11_254);
        assert_eq!(
            shortfall.conversion.conversion_type,
            ConversionType::ToBitcoin {
                from_token_identifier: sdk_adapter::USDB_MAINNET_TOKEN_IDENTIFIER.to_string(),
            }
        );
        assert_eq!(shortfall.conversion.max_slippage_bps, Some(50));
        assert_eq!(
            shortfall.conversion.max_slippage_bps,
            Some(CONVERSION_MAX_SLIPPAGE_BPS)
        );
        assert_eq!(shortfall.conversion.completion_timeout_secs, None);
    }

    #[test]
    fn insufficient_funds_is_recognised_in_both_sdk_error_shapes() {
        // Lightning / Spark-address paths.
        assert!(is_insufficient_funds(&SdkError::InsufficientFunds {
            token_identifier: None
        }));
        // A *token* shortfall is a different problem; more USDB conversion
        // is not the answer to "not enough USDB".
        assert!(!is_insufficient_funds(&SdkError::InsufficientFunds {
            token_identifier: Some(sdk_adapter::USDB_MAINNET_TOKEN_IDENTIFIER.to_string())
        }));
        // On-chain path: the tree service error surfaces as a string.
        assert!(is_insufficient_funds(&SdkError::SparkError(
            "Tree service error: insufficient funds".to_string()
        )));
        assert!(!is_insufficient_funds(&SdkError::InvalidInput(
            "Amount is required".to_string()
        )));
        assert!(!is_insufficient_funds(&SdkError::SparkError(
            "Service error: generic error: Token outputs not found".to_string()
        )));
    }

    /// A stand-in prepare: `(had_conversion, amount)`. The plain call
    /// returns `plain`; a call with a conversion returns `converted`.
    /// Records every conversion it was handed.
    struct FakePrepare {
        plain: Result<(bool, u64), SdkError>,
        converted: Result<(bool, u64), SdkError>,
        calls: Cell<u32>,
    }

    impl FakePrepare {
        fn call(
            &self,
            conversion: Option<ConversionOptions>,
        ) -> impl std::future::Future<Output = Result<(bool, u64), SdkError>> + '_ {
            self.calls.set(self.calls.get() + 1);
            let out = if conversion.is_some() {
                self.converted.clone()
            } else {
                self.plain.clone()
            };
            async move { out }
        }
    }

    fn usdb() -> UsdbShortfall {
        conversion_for_shortfall(11_254, 1, 200_000).expect("short with usdb converts")
    }

    /// A fake prepare that funds the send only as `funds` says, recording the
    /// conversion each call asked for.
    struct FundedPrepare {
        bitcoin_covers: bool,
        usdb_covers: bool,
        asked: std::cell::RefCell<Vec<Option<ConversionOptions>>>,
    }

    impl FundedPrepare {
        fn call(
            &self,
            conversion: Option<ConversionOptions>,
        ) -> impl std::future::Future<Output = Result<(bool, u64), SdkError>> + '_ {
            self.asked.borrow_mut().push(conversion.clone());
            let out = match conversion {
                Some(_) if self.usdb_covers => Ok((true, 20_000)),
                Some(_) => Err(SdkError::InsufficientFunds {
                    token_identifier: Some(sdk_adapter::USDB_MAINNET_TOKEN_IDENTIFIER.into()),
                }),
                None if self.bitcoin_covers => Ok((false, 20_000)),
                None => Err(SdkError::InsufficientFunds {
                    token_identifier: None,
                }),
            };
            async move { out }
        }
    }

    /// The coverage matrix (USDB enough / short; bitcoin enough / short).
    /// Paying from USDB attaches the full conversion to the only prepare,
    /// whatever bitcoin holds, and never reads balances for a shortfall.
    /// Paying from bitcoin converts only to cover a shortfall.
    #[tokio::test]
    async fn pay_from_decides_full_or_shortfall_conversion() {
        for bitcoin_covers in [true, false] {
            for usdb_covers in [true, false] {
                let fake = FundedPrepare {
                    bitcoin_covers,
                    usdb_covers,
                    asked: Default::default(),
                };
                let shortfall_checked = Cell::new(false);
                let result = prepare_funded(
                    WalletAsset::Usdb,
                    Some(20_000),
                    |c| fake.call(c),
                    |_| {
                        shortfall_checked.set(true);
                        async { Ok(Some(usdb())) }
                    },
                    |p| p.1,
                    |p| p.0,
                )
                .await;
                assert_eq!(
                    *fake.asked.borrow(),
                    vec![Some(usdb_to_bitcoin_conversion())],
                    "from USDB: one prepare, with the full conversion \
                     (bitcoin covers: {bitcoin_covers}, USDB covers: {usdb_covers})"
                );
                assert!(!shortfall_checked.get());
                if usdb_covers {
                    assert_eq!(result.unwrap(), (true, 20_000));
                } else {
                    assert!(matches!(
                        result,
                        Err(PrepareError::PayFromUsdb(SdkError::InsufficientFunds {
                            token_identifier: Some(_)
                        }))
                    ));
                }

                let fake = FundedPrepare {
                    bitcoin_covers,
                    usdb_covers,
                    asked: Default::default(),
                };
                let result = prepare_funded(
                    WalletAsset::Bitcoin,
                    Some(20_000),
                    |c| fake.call(c),
                    |needed| async move {
                        let balance = if bitcoin_covers { 50_000 } else { 12_000 };
                        Ok(conversion_for_shortfall(balance, 5_000_000, needed))
                    },
                    |p| p.1,
                    |p| p.0,
                )
                .await;
                let asked = fake.asked.borrow().clone();
                if bitcoin_covers {
                    assert_eq!(asked, vec![None], "from bitcoin, covered: no conversion");
                    assert_eq!(result.unwrap(), (false, 20_000));
                } else {
                    assert_eq!(
                        asked,
                        vec![None, Some(usdb_to_bitcoin_conversion())],
                        "from bitcoin, short: the shortfall conversion"
                    );
                    assert_eq!(result.is_ok(), usdb_covers);
                }
            }
        }
    }

    #[test]
    fn every_send_conversion_is_usdb_to_bitcoin_at_the_cap() {
        let options = usdb_to_bitcoin_conversion();
        assert_eq!(options.max_slippage_bps, Some(50));
        assert!(matches!(
            options.conversion_type,
            ConversionType::ToBitcoin { ref from_token_identifier }
                if from_token_identifier == sdk_adapter::USDB_MAINNET_TOKEN_IDENTIFIER
        ));
        assert_eq!(
            conversion_for_shortfall(11_254, 1, 200_000).map(|s| s.conversion),
            Some(options)
        );
    }

    fn estimate(options: ConversionOptions) -> ConversionEstimate {
        ConversionEstimate {
            options,
            amount_in: 12_400_000,
            amount_out: 15_003,
            fee: 62_000,
            amount_adjustment: Some(AmountAdjustmentReason::IncreasedToAvoidDust),
        }
    }

    #[test]
    fn a_send_conversion_quote_names_usdb_in_and_bitcoin_out() {
        assert_eq!(send_conversion_quote(None), Ok(None));
        let quote = send_conversion_quote(Some(&estimate(usdb_to_bitcoin_conversion())))
            .unwrap()
            .expect("a quote");
        assert_eq!(
            *quote,
            ConversionQuote {
                amount_in: AssetAmount {
                    asset: WalletAsset::Usdb,
                    amount: 12_400_000
                },
                amount_out: AssetAmount {
                    asset: WalletAsset::Bitcoin,
                    amount: 15_003
                },
                fee: AssetAmount {
                    asset: WalletAsset::Usdb,
                    amount: 62_000
                },
                amount_adjustment: Some(AmountAdjustment::IncreasedToAvoidDust),
                max_slippage_bps: 50,
            }
        );

        // Not the conversion the bridge attaches: refused, not shown.
        let mut other_cap = usdb_to_bitcoin_conversion();
        other_cap.max_slippage_bps = None;
        assert!(send_conversion_quote(Some(&estimate(other_cap))).is_err());
        let mut from_bitcoin = usdb_to_bitcoin_conversion();
        from_bitcoin.conversion_type = ConversionType::FromBitcoin;
        assert!(send_conversion_quote(Some(&estimate(from_bitcoin))).is_err());
    }

    #[test]
    fn a_pay_from_preview_refuses_token_legs_and_a_usdb_payment_without_conversion() {
        let token = Some("btkn1other");
        for pay_from in [WalletAsset::Bitcoin, WalletAsset::Usdb] {
            for has_conversion in [false, true] {
                assert_eq!(
                    pay_from_refusal(pay_from, token, has_conversion),
                    Some((ErrorKind::BadRequest, TOKEN_INVOICE_REFUSAL))
                );
            }
        }
        assert!(matches!(
            pay_from_refusal(WalletAsset::Usdb, None, false),
            Some((ErrorKind::Sdk, _))
        ));
        assert_eq!(pay_from_refusal(WalletAsset::Usdb, None, true), None);
        assert_eq!(pay_from_refusal(WalletAsset::Bitcoin, None, false), None);
        assert_eq!(pay_from_refusal(WalletAsset::Bitcoin, None, true), None);
    }

    /// A USDB invoice is paid only from USDB and only with its own amount.
    /// Every other token, and USDB from bitcoin, stays refused before
    /// anything is prepared.
    #[test]
    fn a_usdb_invoice_is_paid_from_usdb_and_every_other_token_is_refused() {
        let usdb = sdk_adapter::USDB_MAINNET_TOKEN_IDENTIFIER;
        assert_eq!(
            token_invoice_refusal(WalletAsset::Usdb, usdb, Some(2_500_000)),
            None
        );
        assert_eq!(
            token_invoice_refusal(WalletAsset::Bitcoin, usdb, Some(2_500_000)),
            Some((ErrorKind::BadRequest, USDB_INVOICE_FROM_BITCOIN))
        );
        for amount in [None, Some(0)] {
            assert_eq!(
                token_invoice_refusal(WalletAsset::Usdb, usdb, amount),
                Some((ErrorKind::BadRequest, AMOUNTLESS_USDB_INVOICE))
            );
        }
        for pay_from in [WalletAsset::Bitcoin, WalletAsset::Usdb] {
            assert_eq!(
                token_invoice_refusal(pay_from, "btkn1other", Some(2_500_000)),
                Some((ErrorKind::BadRequest, TOKEN_INVOICE_REFUSAL))
            );
        }
    }

    /// The preview carries the invoice's USDB amount and fee, denominated,
    /// and refuses a prepare that isn't a direct USDB transfer to an invoice.
    #[test]
    fn a_usdb_invoice_preview_carries_usdb_figures() {
        let usdb = Some(sdk_adapter::USDB_MAINNET_TOKEN_IDENTIFIER);
        let invoice = UsdbDestination::Invoice;
        let usdb_amount = |amount| AssetAmount {
            asset: WalletAsset::Usdb,
            amount,
        };
        assert_eq!(
            usdb_transfer_payment(invoice, usdb, false, Some((1_200, usdb)), 2_500_000),
            Ok(TokenPayment {
                amount: usdb_amount(2_500_000),
                fee: usdb_amount(1_200),
            })
        );
        // A conversion, another token, a bitcoin leg or another method.
        assert!(usdb_transfer_payment(invoice, usdb, true, Some((0, usdb)), 2_500_000).is_err());
        assert!(usdb_transfer_payment(
            invoice,
            Some("btkn1other"),
            false,
            Some((0, usdb)),
            2_500_000
        )
        .is_err());
        assert!(usdb_transfer_payment(invoice, None, false, Some((0, None)), 2_500_000).is_err());
        assert!(usdb_transfer_payment(invoice, usdb, false, Some((0, None)), 2_500_000).is_err());
        assert!(usdb_transfer_payment(invoice, usdb, false, None, 2_500_000).is_err());
    }

    /// A USDB send to a Spark address moves exactly the amount asked, in
    /// USDB, and is refused on the same grounds as an invoice otherwise.
    #[test]
    fn a_usdb_address_send_preview_carries_the_amount_asked() {
        let usdb = Some(sdk_adapter::USDB_MAINNET_TOKEN_IDENTIFIER);
        let address = UsdbDestination::Address { amount: 12_500_000 };
        assert_eq!(address.method(), "SparkAddress");
        assert_eq!(UsdbDestination::Invoice.method(), "SparkInvoice");
        let usdb_amount = |amount| AssetAmount {
            asset: WalletAsset::Usdb,
            amount,
        };
        assert_eq!(
            usdb_transfer_payment(address, usdb, false, Some((0, usdb)), 12_500_000),
            Ok(TokenPayment {
                amount: usdb_amount(12_500_000),
                fee: usdb_amount(0),
            })
        );
        // The SDK changed the amount, attached a conversion, moved another
        // token or bitcoin, or prepared another method.
        assert!(usdb_transfer_payment(address, usdb, false, Some((0, usdb)), 12_000_000).is_err());
        assert!(usdb_transfer_payment(address, usdb, true, Some((0, usdb)), 12_500_000).is_err());
        assert!(usdb_transfer_payment(
            address,
            Some("btkn1other"),
            false,
            Some((0, Some("btkn1other"))),
            12_500_000
        )
        .is_err());
        assert!(usdb_transfer_payment(address, None, false, Some((0, None)), 12_500_000).is_err());
        assert!(usdb_transfer_payment(address, usdb, false, None, 12_500_000).is_err());
    }

    #[test]
    fn a_failed_usdb_payment_says_what_to_do() {
        let short = PrepareError::PayFromUsdb(SdkError::InsufficientFunds {
            token_identifier: Some(sdk_adapter::USDB_MAINNET_TOKEN_IDENTIFIER.into()),
        })
        .into_response(7, "prepare_send");
        let coincube_spark_protocol::ResponseResult::Err(err) = short.result else {
            panic!("an error");
        };
        assert_eq!(err.kind, ErrorKind::BadRequest);
        assert_eq!(err.message, NOT_ENOUGH_USDB);

        let amm = PrepareError::PayFromUsdb(SdkError::Generic(
            r#"Error: {"errorCode":"FSAG-4201","message":"AMM has insufficient liquidity"}"#.into(),
        ))
        .into_response(7, "prepare_send");
        let coincube_spark_protocol::ResponseResult::Err(err) = amm.result else {
            panic!("an error");
        };
        assert_eq!(err.kind, ErrorKind::BadRequest);
        assert!(
            err.message.contains("AMM has insufficient liquidity")
                && err.message.contains("Pay from Bitcoin"),
            "{}",
            err.message
        );
    }

    #[tokio::test]
    async fn prepare_already_carrying_a_conversion_is_returned_untouched() {
        let fake = FakePrepare {
            plain: Ok((true, 5_000)),
            converted: Ok((true, 999)),
            calls: Cell::new(0),
        };
        let shortfall_checked = Cell::new(false);
        let result = prepare_with_usdb_fallback(
            Some(5_000),
            |c| fake.call(c),
            |_| {
                shortfall_checked.set(true);
                async { Ok(Some(usdb())) }
            },
            |p| p.1,
            |p| p.0,
        )
        .await;
        assert_eq!(result.unwrap(), (true, 5_000));
        assert_eq!(fake.calls.get(), 1, "no re-prepare");
        assert!(!shortfall_checked.get(), "balances are not even read");
    }

    #[tokio::test]
    async fn plain_prepare_is_kept_when_bitcoin_covers_it() {
        let fake = FakePrepare {
            plain: Ok((false, 5_000)),
            converted: Ok((true, 999)),
            calls: Cell::new(0),
        };
        let asked = Cell::new(0);
        let result = prepare_with_usdb_fallback(
            None,
            |c| fake.call(c),
            |needed| {
                asked.set(needed);
                async { Ok(None) }
            },
            |p| p.1,
            |p| p.0,
        )
        .await;
        assert_eq!(result.unwrap(), (false, 5_000));
        assert_eq!(
            asked.get(),
            5_000,
            "shortfall is checked against amount + fee"
        );
        assert_eq!(fake.calls.get(), 1);
    }

    #[tokio::test]
    async fn plain_prepare_is_re_run_with_usdb_when_bitcoin_is_short() {
        let fake = FakePrepare {
            plain: Ok((false, 5_000)),
            converted: Ok((true, 5_000)),
            calls: Cell::new(0),
        };
        let result = prepare_with_usdb_fallback(
            None,
            |c| fake.call(c),
            |_| async { Ok(Some(usdb())) },
            |p| p.1,
            |p| p.0,
        )
        .await;
        assert_eq!(result.unwrap(), (true, 5_000));
        assert_eq!(fake.calls.get(), 2, "plain, then with the conversion");
    }

    #[tokio::test]
    async fn insufficient_funds_error_is_retried_with_usdb_using_the_known_amount() {
        let fake = FakePrepare {
            plain: Err(SdkError::SparkError(
                "Tree service error: insufficient funds".to_string(),
            )),
            converted: Ok((true, 2_000_000)),
            calls: Cell::new(0),
        };
        let asked = Cell::new(0);
        let result = prepare_with_usdb_fallback(
            Some(2_000_000),
            |c| fake.call(c),
            |needed| {
                asked.set(needed);
                async { Ok(Some(usdb())) }
            },
            |p| p.1,
            |p| p.0,
        )
        .await;
        assert_eq!(result.unwrap(), (true, 2_000_000));
        assert_eq!(asked.get(), 2_000_000);
        assert_eq!(fake.calls.get(), 2);
    }

    #[tokio::test]
    async fn insufficient_funds_error_stands_without_usdb_or_a_known_amount() {
        // No USDB to draw on: the original error comes back.
        let fake = FakePrepare {
            plain: Err(SdkError::InsufficientFunds {
                token_identifier: None,
            }),
            converted: Ok((true, 1)),
            calls: Cell::new(0),
        };
        let result = prepare_with_usdb_fallback(
            Some(1),
            |c| fake.call(c),
            |_| async { Ok(None) },
            |p| p.1,
            |p| p.0,
        )
        .await;
        assert!(matches!(
            result,
            Err(PrepareError::Sdk(SdkError::InsufficientFunds {
                token_identifier: None
            }))
        ));
        assert_eq!(fake.calls.get(), 1);

        // Amount unknown (amountless prepare that failed): nothing to size
        // the conversion from, so no retry.
        let fake = FakePrepare {
            plain: Err(SdkError::InsufficientFunds {
                token_identifier: None,
            }),
            converted: Ok((true, 1)),
            calls: Cell::new(0),
        };
        let checked = Cell::new(false);
        let result = prepare_with_usdb_fallback(
            None,
            |c| fake.call(c),
            |_| {
                checked.set(true);
                async { Ok(Some(usdb())) }
            },
            |p| p.1,
            |p| p.0,
        )
        .await;
        assert!(matches!(
            result,
            Err(PrepareError::Sdk(SdkError::InsufficientFunds {
                token_identifier: None
            }))
        ));
        assert!(!checked.get());
        assert_eq!(fake.calls.get(), 1);
    }

    #[tokio::test]
    async fn other_prepare_errors_and_balance_read_failures_leave_the_plain_outcome() {
        // A non-funds error is never retried.
        let fake = FakePrepare {
            plain: Err(SdkError::InvalidInput("Amount is required".to_string())),
            converted: Ok((true, 1)),
            calls: Cell::new(0),
        };
        let result = prepare_with_usdb_fallback(
            Some(1),
            |c| fake.call(c),
            |_| async { Ok(Some(usdb())) },
            |p| p.1,
            |p| p.0,
        )
        .await;
        assert!(matches!(
            result,
            Err(PrepareError::Sdk(SdkError::InvalidInput(_)))
        ));
        assert_eq!(fake.calls.get(), 1);

        // Balance read failing: the plain prepare is still the answer.
        let fake = FakePrepare {
            plain: Ok((false, 5_000)),
            converted: Ok((true, 5_000)),
            calls: Cell::new(0),
        };
        let result = prepare_with_usdb_fallback(
            None,
            |c| fake.call(c),
            |_| async { Err(SdkError::SparkError("offline".to_string())) },
            |p| p.1,
            |p| p.0,
        )
        .await;
        assert_eq!(result.unwrap(), (false, 5_000));
        assert_eq!(fake.calls.get(), 1);
    }
}

#[cfg(test)]
mod usdb_fallback_error_tests {
    use super::*;
    use breez_sdk_spark::SdkError;
    use std::cell::Cell;

    const FLASHNET_ERROR: &str = r#"Error: {"errorCode":"FSAG-4201","errorCategory":"BUSINESS_LOGIC","message":"FSAG-4201: AMM has insufficient liquidity/reserves: Insufficient liquidity in pool","details":{"liquidity_error":"Insufficient liquidity in pool"},"requestId":"01A08F33-D5CB-7901-BDC6-4E3629D9CAFC","timestamp":"2026-09-11T06:42:14.092591956Z","service":"flashnet-amm-gateway","severity":"Error"}"#;

    #[test]
    fn conversion_failure_reason_pulls_the_message_out_of_flashnet_json() {
        assert_eq!(
            conversion_failure_reason(FLASHNET_ERROR),
            "FSAG-4201: AMM has insufficient liquidity/reserves: Insufficient liquidity in pool"
        );
        // Not JSON: passed through.
        assert_eq!(conversion_failure_reason("  Timeout  "), "Timeout");
        // JSON without a message: passed through rather than blanked.
        assert_eq!(conversion_failure_reason(r#"{"code":1}"#), r#"{"code":1}"#);
    }

    #[tokio::test]
    async fn a_failed_usdb_conversion_prepare_names_the_bitcoin_balance_and_the_reason() {
        // On-chain path: the plain prepare fails on funds, USDB is there,
        // and the AMM refuses the conversion.
        let calls = Cell::new(0);
        let prepare = |conversion: Option<ConversionOptions>| {
            calls.set(calls.get() + 1);
            let out = if conversion.is_some() {
                Err(SdkError::Generic(
                    FLASHNET_ERROR.trim_start_matches("Error: ").to_string(),
                ))
            } else {
                Err(SdkError::SparkError(
                    "Tree service error: insufficient funds".to_string(),
                ))
            };
            async move { out }
        };
        let result: Result<u64, PrepareError> = prepare_with_usdb_fallback(
            Some(200_000),
            prepare,
            |_| async { Ok(conversion_for_shortfall(11_254, 15_177_980_000, 200_000)) },
            |_| 0,
            |_| false,
        )
        .await;
        assert_eq!(calls.get(), 2);
        let Err(err @ PrepareError::UsdbConversion { balance_sats, .. }) = result else {
            panic!("expected a usdb conversion failure");
        };
        assert_eq!(balance_sats, 11_254);

        let response = err.into_response(7, "prepare_send");
        let value = serde_json::to_value(&response).expect("serialize");
        let message = value["err"]["message"].as_str().expect("message");
        assert!(
            message.contains("11,254 sats available"),
            "must name what is still spendable: {message}"
        );
        assert!(
            message.contains("AMM has insufficient liquidity/reserves"),
            "must carry the AMM's reason: {message}"
        );
        assert!(
            !message.contains("errorCode"),
            "must not leak the raw JSON blob: {message}"
        );
        assert_eq!(value["err"]["kind"], serde_json::json!("bad_request"));
    }

    #[test]
    fn with_thousands_groups_digits() {
        assert_eq!(with_thousands(0), "0");
        assert_eq!(with_thousands(999), "999");
        assert_eq!(with_thousands(1_000), "1,000");
        assert_eq!(with_thousands(11_254), "11,254");
        assert_eq!(with_thousands(19_718_473), "19,718,473");
    }

    #[test]
    fn plain_sdk_errors_keep_their_original_shape() {
        let response = PrepareError::Sdk(SdkError::InvalidInput("Amount is required".to_string()))
            .into_response(7, "prepare_send");
        let value = serde_json::to_value(&response).expect("serialize");
        assert_eq!(
            value["err"]["message"],
            serde_json::json!("prepare_send failed: Invalid input: Amount is required")
        );
        assert_eq!(value["err"]["kind"], serde_json::json!("sdk"));
    }
}

#[cfg(test)]
mod cross_chain_tests {
    use super::*;
    use breez_sdk_spark::{CrossChainProvider, SdkError};

    fn route(provider: CrossChainProvider, sources: Vec<SparkAsset>) -> CrossChainRoutePair {
        CrossChainRoutePair {
            provider,
            chain: "base".to_string(),
            chain_id: Some("8453".to_string()),
            asset: "USDC".to_string(),
            contract_address: Some("0xabc".to_string()),
            decimals: 6,
            exact_out_eligible: true,
            accepted_assets: sources
                .into_iter()
                .map(|asset| breez_sdk_spark::CrossChainAcceptedAsset {
                    asset,
                    limits: None,
                })
                .collect(),
            delivery_methods: vec![breez_sdk_spark::DeliveryMethod::Spark],
        }
    }

    fn top_up(balance_sats: u64, held_usdb: u128) -> Option<ConversionOptions> {
        conversion_for_shortfall(balance_sats, held_usdb, 20_000).map(|s| s.conversion)
    }

    /// #965 F9: a cross-chain send whose bitcoin is below the requested
    /// amount, with USDB held, prepares once with a USDB → bitcoin top-up at
    /// the 50 bps cap. The SDK sizes it for the provider's amount and fee.
    #[test]
    fn a_cross_chain_send_short_of_bitcoin_tops_up_from_usdb() {
        let r = route(CrossChainProvider::Boltz, vec![SparkAsset::Bitcoin]);
        let request = cross_chain_prepare_request(
            "0xrecipient".to_string(),
            r.clone(),
            Some(100),
            20_000,
            top_up(19_999, 5_000_000),
        );
        assert_eq!(
            request.conversion_options,
            Some(ConversionOptions {
                conversion_type: ConversionType::ToBitcoin {
                    from_token_identifier: sdk_adapter::USDB_MAINNET_TOKEN_IDENTIFIER.to_string(),
                },
                max_slippage_bps: Some(50),
                completion_timeout_secs: None,
            })
        );
        assert_eq!(request.amount, Some(20_000));
        assert_eq!(request.token_identifier, None);
        assert!(request.fee_policy.is_none());
        assert!(matches!(
            request.payment_request,
            PaymentRequest::CrossChain {
                ref address,
                ref route,
                max_slippage_bps: Some(100),
                target_overpay_bps: None,
            } if address == "0xrecipient" && *route == r
        ));
    }

    /// One prepare, whatever the balances say: with the top-up, sized from
    /// the requested amount, when bitcoin is short and USDB is held; plain
    /// otherwise, and plain when the balances can't be read (#965 F9).
    /// `prepare` is `FnOnce`, so there is no second one.
    #[tokio::test]
    async fn a_cross_chain_send_prepares_once() {
        let asked = std::cell::Cell::new(None);
        let (shortfall, prepared) = prepare_cross_chain_once(
            20_000,
            |needed| {
                asked.set(Some(needed));
                async move { Ok(conversion_for_shortfall(12_000, 5_000_000, needed)) }
            },
            |conversion| async move { Ok::<_, SdkError>(conversion) },
        )
        .await;
        assert_eq!(asked.get(), Some(20_000), "sized from the requested amount");
        let attached = prepared.expect("prepared");
        assert!(attached.is_some());
        assert_eq!(attached, shortfall.map(|s| s.conversion));

        let (shortfall, prepared) = prepare_cross_chain_once(
            20_000,
            |needed| async move { Ok(conversion_for_shortfall(20_000, 5_000_000, needed)) },
            |conversion| async move { Ok::<_, SdkError>(conversion) },
        )
        .await;
        assert!(shortfall.is_none());
        assert_eq!(prepared.expect("prepared"), None);

        let (shortfall, prepared) = prepare_cross_chain_once(
            20_000,
            |_| async { Err(SdkError::Generic("offline".to_string())) },
            |conversion| async move { Ok::<_, SdkError>(conversion) },
        )
        .await;
        assert!(shortfall.is_none());
        assert_eq!(prepared.expect("prepared"), None, "no balances, no top-up");
    }

    /// Bitcoin covering the requested amount, or no USDB held: the plain
    /// prepare, as before. So is bitcoin that covers the amount but not the
    /// provider and source fees (the known limit: those fees are known only
    /// from the provider's prepare).
    #[test]
    fn a_cross_chain_send_with_enough_bitcoin_or_no_usdb_stays_plain() {
        for (balance_sats, held_usdb) in [
            (20_000, 5_000_000),
            (25_000, 5_000_000),
            (19_999, 0),
            (0, 0),
        ] {
            let request = cross_chain_prepare_request(
                "0xrecipient".to_string(),
                route(CrossChainProvider::Orchestra, vec![SparkAsset::Bitcoin]),
                None,
                20_000,
                top_up(balance_sats, held_usdb),
            );
            assert!(
                request.conversion_options.is_none(),
                "{balance_sats} sats, {held_usdb} USDB"
            );
        }
    }

    /// A top-up the USDB balance can't fund is refused at prepare, so no
    /// handle exists to send. The refusal is answered in the user's terms, as
    /// on the other send paths; a plain prepare's error keeps its shape.
    #[test]
    fn a_refused_cross_chain_top_up_is_answered_in_the_users_terms() {
        let shortfall = conversion_for_shortfall(12_000, 1, 20_000).expect("a top-up");
        // `ensure_token_balance_covers` at the pin.
        let refused = SdkError::InvalidInput(
            "Insufficient USDB balance for cross-chain conversion: have 1, need 8200000 \
             (includes conversion and bridge fees)."
                .to_string(),
        );

        let value = serde_json::to_value(cross_chain_prepare_failed(
            7,
            refused.clone(),
            Some(&shortfall),
        ))
        .expect("serialize");
        assert_eq!(value["err"]["kind"], serde_json::json!("bad_request"));
        let message = value["err"]["message"].as_str().expect("a message");
        assert!(message.contains("(12,000 sats available)"), "{message}");
        assert!(
            message.contains("converting USDB to bitcoin to cover it failed"),
            "{message}"
        );

        let value = serde_json::to_value(cross_chain_prepare_failed(7, refused.clone(), None))
            .expect("serialize");
        assert_eq!(value["err"]["kind"], serde_json::json!("sdk"));
        assert_eq!(
            value["err"]["message"],
            serde_json::json!(refused.to_string())
        );
    }

    #[test]
    fn address_families_map_to_stable_wire_strings() {
        // The gui branches on these strings and shows them to the user; they
        // are wire contract, not debug output.
        assert_eq!(family_str(CrossChainAddressFamily::Evm), "evm");
        assert_eq!(family_str(CrossChainAddressFamily::Solana), "solana");
        assert_eq!(family_str(CrossChainAddressFamily::Tron), "tron");
    }

    #[test]
    fn a_btc_fundable_route_is_offered_and_marked_retry_safe() {
        let r = route(CrossChainProvider::Orchestra, vec![SparkAsset::Bitcoin]);
        assert!(route_accepts_btc(&r));
        assert!(route_is_retry_safe(&r));
        let wire = sdk_route_to_protocol(&r);
        assert_eq!(wire.provider, "orchestra");
        assert_eq!(wire.asset, "USDC");
        assert_eq!(wire.decimals, 6);
        assert!(wire.btc_source_supported);
    }

    #[test]
    fn boltz_routes_map_to_their_own_provider_string() {
        let r = route(CrossChainProvider::Boltz, vec![SparkAsset::Bitcoin]);
        assert_eq!(sdk_route_to_protocol(&r).provider, "boltz");
    }

    #[test]
    fn a_token_only_route_is_neither_offered_nor_retry_safe() {
        // v1 funds every send from BTC. A token-only route can't be prepared,
        // and — critically — a token source leg has no idempotency hook, so it
        // must never be reported to the gui as safe to blind-retry.
        let r = route(
            CrossChainProvider::Orchestra,
            vec![SparkAsset::Token {
                token_identifier: "btkn1xyz".to_string(),
            }],
        );
        assert!(!route_accepts_btc(&r));
        assert!(!route_is_retry_safe(&r));
        assert!(!sdk_route_to_protocol(&r).btc_source_supported);
    }

    fn protocol_address(
        contract: Option<&str>,
        chain_id: Option<u64>,
    ) -> coincube_spark_protocol::CrossChainAddress {
        coincube_spark_protocol::CrossChainAddress {
            address: "0x71C7656EC7ab88b098defB751B7401B5f6d8976F".to_string(),
            family: "evm".to_string(),
            contract_address: contract.map(str::to_string),
            chain_id,
            amount: Some(1_000_000),
        }
    }

    #[test]
    fn address_families_survive_a_round_trip_through_the_wire() {
        for family in [
            CrossChainAddressFamily::Evm,
            CrossChainAddressFamily::Solana,
            CrossChainAddressFamily::Tron,
        ] {
            assert_eq!(family_from_str(family_str(family)), Some(family));
        }
    }

    #[test]
    fn an_unknown_family_is_refused_rather_than_guessed() {
        // The gui only ever echoes back a family we emitted. An unknown one
        // means the wire contract drifted — guessing a chain for a money
        // transfer is how funds end up on the wrong network.
        assert_eq!(family_from_str("bitcoin"), None);
        assert_eq!(family_from_str(""), None);
        assert_eq!(family_from_str("EVM"), None);
    }

    /// The point of round-tripping the whole `CrossChainAddress`: a URI
    /// destination carries a contract and chain id that a bare address does
    /// not. Re-parsing the address string on the prepare side would drop them,
    /// and the route would then be re-resolved against a *broader* destination
    /// than the one the user was offered a choice from.
    #[test]
    fn uri_only_details_survive_into_the_sdk_filter() {
        let details = details_from_protocol(&protocol_address(Some("0xA0b8991c"), Some(8453)))
            .expect("known family");
        assert_eq!(details.contract_address.as_deref(), Some("0xA0b8991c"));
        assert_eq!(details.chain_id, Some(8453));
        assert_eq!(details.address_family, CrossChainAddressFamily::Evm);
        assert_eq!(
            details.address,
            "0x71C7656EC7ab88b098defB751B7401B5f6d8976F"
        );
    }

    #[test]
    fn a_bare_address_reconstructs_without_inventing_details() {
        let details = details_from_protocol(&protocol_address(None, None)).expect("known family");
        assert_eq!(details.contract_address, None);
        assert_eq!(details.chain_id, None);
    }

    #[test]
    fn a_destination_with_an_unknown_family_reconstructs_to_nothing() {
        let mut bad = protocol_address(None, None);
        bad.family = "dogecoin".to_string();
        assert!(details_from_protocol(&bad).is_none());
    }

    #[test]
    fn a_route_accepting_both_sources_is_offered_via_its_btc_leg() {
        let r = route(
            CrossChainProvider::Orchestra,
            vec![
                SparkAsset::Token {
                    token_identifier: "btkn1xyz".to_string(),
                },
                SparkAsset::Bitcoin,
            ],
        );
        assert!(route_accepts_btc(&r));
        assert!(route_is_retry_safe(&r));
    }
}

#[cfg(test)]
mod detail_summary_tests {
    use super::*;
    use breez_sdk_spark::{LnurlPayInfo, SparkHtlcDetails, SparkHtlcStatus};

    fn htlc(payment_hash: &str) -> SparkHtlcDetails {
        SparkHtlcDetails {
            payment_hash: payment_hash.into(),
            preimage: Some("preimage-stays-in-the-bridge".into()),
            expiry_time: 0,
            status: SparkHtlcStatus::PreimageShared,
        }
    }

    #[test]
    fn lightning_send_carries_hash_invoice_and_address_but_not_the_preimage() {
        let details = PaymentDetails::Lightning {
            description: None,
            invoice: "lnbc42u1synthetic".into(),
            destination_pubkey: "02synthetic".into(),
            htlc_details: htlc("4c8dsynthetic"),
            lnurl_pay_info: Some(LnurlPayInfo {
                ln_address: Some("shop@example.test".into()),
                comment: None,
                domain: None,
                metadata: None,
                processed_success_action: None,
                raw_success_action: None,
            }),
            lnurl_withdraw_info: None,
            lnurl_receive_metadata: None,
            conversion_info: None,
        };
        let summary = detail_summary(Some(&details));
        assert_eq!(
            summary,
            PaymentDetailSummary {
                payment_hash: Some("4c8dsynthetic".into()),
                invoice: Some("lnbc42u1synthetic".into()),
                lightning_address: Some("shop@example.test".into()),
                tx_id: None,
                conversion: None,
            }
        );
        assert!(!format!("{summary:?}").contains("preimage-stays-in-the-bridge"));
    }

    /// 0.28.0: each claim outcome maps onto the wire, a deferral as an Ok
    /// carrying its reason rather than an error.
    #[test]
    fn claim_outcomes_map_and_a_deferral_is_not_an_error() {
        let submitted = claim_deposit_ok(ClaimDepositOutcome::Submitted);
        assert_eq!(
            (
                submitted.payment_id,
                submitted.amount_sat,
                submitted.deferred
            ),
            (None, None, None)
        );
        let deferred = claim_deposit_ok(ClaimDepositOutcome::Deferred {
            reason: ClaimDeferredReason::MaxFeeExceeded {
                required_fee_sats: 1_200,
                max_fee_sats: 800,
            },
        });
        assert_eq!(deferred.payment_id, None);
        assert_eq!(
            deferred.deferred,
            Some(ClaimDeferral::MaxFeeExceeded {
                required_fee_sats: 1_200,
                max_fee_sats: 800
            })
        );
        assert_eq!(
            claim_deferral(ClaimDeferredReason::NoEarlyClaimAvailable),
            ClaimDeferral::NoEarlyClaimAvailable
        );
        assert_eq!(
            claim_deferral(ClaimDeferredReason::ProviderDeclined {
                message: "busy".into()
            }),
            ClaimDeferral::ProviderDeclined {
                message: "busy".into()
            }
        );
    }

    #[test]
    fn on_chain_legs_carry_their_txid_and_empty_values_are_dropped() {
        assert_eq!(
            detail_summary(Some(&PaymentDetails::Deposit {
                tx_id: "0a9bsynthetic".into(),
                vout: 1,
            }))
            .tx_id
            .as_deref(),
            Some("0a9bsynthetic")
        );
        assert_eq!(
            detail_summary(Some(&PaymentDetails::Withdraw {
                tx_id: String::new(),
                vout: 0,
                destination_address: String::new(),
            })),
            PaymentDetailSummary::default()
        );
        assert_eq!(detail_summary(None), PaymentDetailSummary::default());
    }
}

#[cfg(test)]
mod p0_stub_tests {
    use super::*;
    use coincube_spark_protocol::{
        AssetAmount, ErrorPayload, GetConversionLimitsParams, ListPaymentsParams,
        PrepareLnurlPayFromParams, PrepareSendFromParams, PrepareSwapParams, PrepareUsdbSendParams,
        ResponseResult, SwapAmount, SwapParams, WalletAsset,
    };

    async fn answer(state: &Arc<ServerState>, method: Method) -> ErrorPayload {
        let response = handle_request(Request { id: 7, method }, state.clone()).await;
        assert_eq!(response.id, 7);
        match response.result {
            ResponseResult::Err(err) => err,
            ResponseResult::Ok(ok) => panic!("a P0 stub answered ok: {ok:?}"),
        }
    }

    /// The pay-from methods (S4b) need init, as `list_payments` does: the
    /// state here has no SDK. Nothing is emitted or held for a later send.
    #[tokio::test]
    async fn pay_from_methods_need_init() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let state = Arc::new(ServerState::new(tx));

        let control = answer(
            &state,
            Method::ListPayments(ListPaymentsParams {
                limit: None,
                offset: None,
            }),
        )
        .await;
        assert_eq!(control.kind, ErrorKind::NotConnected);

        for method in [
            Method::PrepareSendFrom(PrepareSendFromParams {
                input: "lnbc1".into(),
                amount_sat: Some(15_000),
                pay_from: WalletAsset::Usdb,
            }),
            Method::PrepareLnurlPayFrom(PrepareLnurlPayFromParams {
                input: "shop@example.test".into(),
                amount_sat: 15_000,
                comment: None,
                pay_from: WalletAsset::Bitcoin,
            }),
            Method::PrepareUsdbSend(PrepareUsdbSendParams {
                input: "spark1".into(),
                amount: 12_500_000,
            }),
        ] {
            let label = format!("{method:?}");
            let err = answer(&state, method).await;
            assert_eq!(err.kind, ErrorKind::NotConnected, "{label}");
        }

        // Nothing was emitted, and nothing was stored for a later send.
        assert!(rx.try_recv().is_err(), "a stub emitted a frame");
        assert!(state.pending_prepares.lock().await.is_empty());
        assert!(state.pending_lnurl_prepares.lock().await.is_empty());
    }

    fn usdb(amount: u128) -> AssetAmount {
        AssetAmount {
            asset: WalletAsset::Usdb,
            amount,
        }
    }

    /// S3b's reads refuse before `init` like every other read, after a
    /// cap outside the protocol's range is refused on its own.
    #[tokio::test]
    async fn swap_reads_need_init_and_a_cap_in_range() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let state = Arc::new(ServerState::new(tx));

        for method in [
            Method::GetConversionLimits(GetConversionLimitsParams {
                from: WalletAsset::Bitcoin,
            }),
            Method::GetConversionLimits(GetConversionLimitsParams {
                from: WalletAsset::Usdb,
            }),
            Method::PrepareSwap(PrepareSwapParams {
                amount: SwapAmount::Exact {
                    amount_in: usdb(1_000_000),
                },
                max_slippage_bps: None,
            }),
            Method::PrepareSwap(PrepareSwapParams {
                amount: SwapAmount::Max {
                    from: WalletAsset::Bitcoin,
                },
                max_slippage_bps: Some(500),
            }),
        ] {
            let label = format!("{method:?}");
            let err = answer(&state, method).await;
            assert_eq!(err.kind, ErrorKind::NotConnected, "{label}");
            assert!(err.message.contains("init must succeed"), "{label}");
        }

        for bps in [0, 9, 501, u32::MAX] {
            let err = answer(
                &state,
                Method::PrepareSwap(PrepareSwapParams {
                    amount: SwapAmount::Exact {
                        amount_in: usdb(1_000_000),
                    },
                    max_slippage_bps: Some(bps),
                }),
            )
            .await;
            assert_eq!(err.kind, ErrorKind::BadRequest, "{bps} bps");
            assert!(
                err.message.contains("between 10 and 500"),
                "{}",
                err.message
            );
        }

        assert!(rx.try_recv().is_err());
        assert!(state.pending_swaps.lock().await.is_empty());
    }

    /// A prepared Spark send, for a test that only needs one to exist.
    fn send_prepare() -> PrepareSendPaymentResponse {
        PrepareSendPaymentResponse {
            payment_method: SendPaymentMethod::SparkAddress {
                address: "spark1someoneelse".into(),
                fee: 0,
                token_identifier: None,
            },
            amount: 1_000,
            token_identifier: None,
            conversion_estimate: None,
            fee_policy: breez_sdk_spark::FeePolicy::FeesExcluded,
        }
    }

    /// A handle `swap` doesn't hold is refused as `BadRequest`, the definite
    /// kind, saying nothing was sent: an unknown one, and a send's, which
    /// `swap` never executes and leaves for `send_payment`.
    #[tokio::test]
    async fn swap_refuses_handles_it_does_not_hold() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let state = Arc::new(ServerState::new(tx));
        state
            .pending_prepares
            .lock()
            .await
            .insert("send-h".into(), (Instant::now(), send_prepare()));

        for handle in ["swap-h", "send-h"] {
            let err = answer(
                &state,
                Method::Swap(SwapParams {
                    prepare_handle: handle.into(),
                }),
            )
            .await;
            assert_eq!(err.kind, ErrorKind::BadRequest, "{handle}");
            assert!(err.message.contains("no pending swap"), "{}", err.message);
            assert!(err.message.contains("Nothing was sent"), "{}", err.message);
        }
        assert!(state.pending_prepares.lock().await.contains_key("send-h"));
        assert!(rx.try_recv().is_err());
    }
}

#[cfg(test)]
mod auto_convert_handler_tests {
    use super::*;
    use coincube_spark_protocol::{AutoConvertBreaker, ResponseResult};

    #[tokio::test]
    async fn receive_only_bridge_refuses_spends_sync_and_address_rotation() {
        let (tx, _) = tokio::sync::mpsc::unbounded_channel();
        let state = Arc::new(ServerState {
            receive_only: true,
            ..ServerState::new(tx)
        });
        for method in [
            Method::SendPayment(coincube_spark_protocol::SendPaymentParams {
                prepare_handle: "test".into(),
                idempotency_key: None,
            }),
            Method::GetInfo(GetInfoParams {
                ensure_synced: Some(true),
            }),
            Method::ReceiveOnchain(coincube_spark_protocol::ReceiveOnchainParams {
                new_address: Some(true),
            }),
        ] {
            let response = handle_request(Request { id: 1, method }, state.clone()).await;
            assert!(
                matches!(response.result, ResponseResult::Err(error) if error.kind == ErrorKind::BadRequest)
            );
        }
    }

    /// A fresh directory for one test's state file, removed on drop.
    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("cc-auto-convert-srv-{}", Uuid::new_v4()));
            std::fs::create_dir_all(&dir).expect("temp dir");
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A server whose `init` loaded auto-convert from `dir`. No SDK: the
    /// switch never needs one.
    fn server(
        dir: &TempDir,
    ) -> (
        Arc<ServerState>,
        tokio::sync::mpsc::UnboundedReceiver<Frame>,
    ) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let state = Arc::new(ServerState::new(tx));
        let _ = state
            .auto_convert
            .set(Arc::new(Mutex::new(AutoConvert::load(&dir.0))));
        (state, rx)
    }

    async fn call(state: &Arc<ServerState>, method: Method) -> ResponseResult {
        handle_request(Request { id: 3, method }, state.clone())
            .await
            .result
    }

    fn on() -> AutoConvertState {
        AutoConvertState {
            enabled: true,
            breaker: AutoConvertBreaker::Ready,
            held_after_swap: false,
        }
    }

    #[tokio::test]
    async fn the_switch_needs_init() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let state = Arc::new(ServerState::new(tx));
        for method in [
            Method::GetAutoConvert,
            Method::SetAutoConvert(SetAutoConvertParams { enabled: true }),
            Method::SetStableBalance(SetStableBalanceParams { enabled: true }),
            Method::GetUserSettings,
        ] {
            let label = format!("{method:?}");
            match call(&state, method).await {
                ResponseResult::Err(e) => assert_eq!(e.kind, ErrorKind::NotConnected, "{label}"),
                ResponseResult::Ok(ok) => panic!("{label} answered {ok:?} before init"),
            }
        }
    }

    #[tokio::test]
    async fn set_and_get_auto_convert_report_the_saved_state() {
        let dir = TempDir::new();
        let (state, mut rx) = server(&dir);

        assert!(matches!(
            call(&state, Method::GetAutoConvert).await,
            ResponseResult::Ok(OkPayload::AutoConvert(s)) if !s.enabled
        ));
        assert!(matches!(
            call(&state, Method::SetAutoConvert(SetAutoConvertParams { enabled: true })).await,
            ResponseResult::Ok(OkPayload::AutoConvert(s)) if s == on()
        ));
        assert!(matches!(
            rx.try_recv(),
            Ok(Frame::Event(ProtocolEvent::AutoConvertStateChanged { state: s })) if s == on()
        ));
        // Saved before it was answered.
        assert_eq!(AutoConvert::load(&dir.0).state(), on());

        // The same again changes nothing and announces nothing.
        assert!(matches!(
            call(&state, Method::SetAutoConvert(SetAutoConvertParams { enabled: true })).await,
            ResponseResult::Ok(OkPayload::AutoConvert(s)) if s == on()
        ));
        assert!(rx.try_recv().is_err());
    }

    /// The gui built before S2b drives auto-convert through the legacy
    /// Stable Balance calls: its reconcile re-applies a recorded "on" as
    /// `set_stable_balance(true)`, which must land on bridge auto-convert.
    #[tokio::test]
    async fn legacy_set_stable_balance_drives_auto_convert() {
        let dir = TempDir::new();
        let (state, mut rx) = server(&dir);

        assert!(matches!(
            call(
                &state,
                Method::SetStableBalance(SetStableBalanceParams { enabled: true })
            )
            .await,
            ResponseResult::Ok(OkPayload::SetStableBalance {})
        ));
        assert_eq!(state.auto_convert.get().unwrap().lock().await.state(), on());
        assert!(matches!(
            rx.try_recv(),
            Ok(Frame::Event(ProtocolEvent::AutoConvertStateChanged { .. }))
        ));

        assert!(matches!(
            call(
                &state,
                Method::SetStableBalance(SetStableBalanceParams { enabled: false })
            )
            .await,
            ResponseResult::Ok(OkPayload::SetStableBalance {})
        ));
        assert!(!AutoConvert::load(&dir.0).state().enabled);
    }

    /// `stable_balance_active` reports bridge auto-convert, whatever the
    /// SDK might once have saved.
    #[test]
    fn legacy_user_settings_report_auto_convert() {
        let paused = AutoConvertState {
            enabled: false,
            breaker: AutoConvertBreaker::Paused {
                failures: 3,
                reason: "Pool has no liquidity".into(),
            },
            held_after_swap: false,
        };
        let ok = user_settings_ok(&on(), true);
        assert!(ok.stable_balance_active && ok.private_mode_enabled);
        let ok = user_settings_ok(&paused, false);
        assert!(!ok.stable_balance_active && !ok.private_mode_enabled);
    }

    /// A switch that can't be saved is refused, not acknowledged, and
    /// nothing changes.
    #[tokio::test]
    async fn an_unsaved_switch_is_an_error() {
        let dir = TempDir::new();
        let blocker = dir.0.join("blocker");
        std::fs::write(&blocker, b"").unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let state = Arc::new(ServerState::new(tx));
        let _ = state
            .auto_convert
            .set(Arc::new(Mutex::new(AutoConvert::load(&blocker))));

        for method in [
            Method::SetAutoConvert(SetAutoConvertParams { enabled: true }),
            Method::SetStableBalance(SetStableBalanceParams { enabled: true }),
        ] {
            match call(&state, method).await {
                ResponseResult::Err(e) => {
                    assert_eq!(e.kind, ErrorKind::Sdk);
                    assert!(e.message.contains("could not save"), "{}", e.message);
                }
                ResponseResult::Ok(ok) => panic!("acknowledged an unsaved switch: {ok:?}"),
            }
        }
        assert!(
            !state
                .auto_convert
                .get()
                .unwrap()
                .lock()
                .await
                .state()
                .enabled
        );
        assert!(rx.try_recv().is_err());
    }

    fn listener(
        dir: &TempDir,
    ) -> (
        BridgeEventListener,
        Arc<Mutex<AutoConvert>>,
        tokio::sync::mpsc::UnboundedReceiver<Frame>,
        tokio::sync::mpsc::UnboundedReceiver<()>,
    ) {
        let (tx, frames) = tokio::sync::mpsc::unbounded_channel();
        let (wake_tx, wakes) = tokio::sync::mpsc::unbounded_channel();
        // Swap status wakes aren't these tests' concern.
        let (swap_wake_tx, _) = tokio::sync::mpsc::unbounded_channel();
        let auto_convert = Arc::new(Mutex::new(AutoConvert::load(&dir.0)));
        let listener = BridgeEventListener {
            tx,
            auto_convert: Arc::clone(&auto_convert),
            wake_auto_convert: wake_tx,
            wake_swap_status: swap_wake_tx,
        };
        (listener, auto_convert, frames, wakes)
    }

    fn received(id: &str, sats: u128) -> breez_sdk_spark::Payment {
        breez_sdk_spark::Payment {
            id: id.to_string(),
            payment_type: breez_sdk_spark::PaymentType::Receive,
            status: breez_sdk_spark::PaymentStatus::Completed,
            amount: sats,
            fees: 0,
            timestamp: 1,
            method: breez_sdk_spark::PaymentMethod::Spark,
            details: None,
            conversion_details: None,
        }
    }

    /// Syncs reach the gui and don't wake the worker: the start-up wake
    /// follows `init` publishing the SDK.
    #[tokio::test]
    async fn syncs_are_forwarded_without_waking_the_worker() {
        let dir = TempDir::new();
        let (listener, _ac, mut frames, mut wakes) = listener(&dir);
        listener.on_event(SdkEvent::Synced).await;
        assert!(wakes.try_recv().is_err());
        assert!(matches!(
            frames.try_recv(),
            Ok(Frame::Event(ProtocolEvent::Synced))
        ));
    }

    /// 0.28.0: a settled payment's late details reach the gui as an update
    /// only: not counted as a receive and not waking the worker.
    #[tokio::test]
    async fn metadata_updates_are_forwarded_as_updates_only() {
        let dir = TempDir::new();
        let (listener, ac, mut frames, mut wakes) = listener(&dir);
        ac.lock().await.set_enabled(true).unwrap();
        listener
            .on_event(SdkEvent::PaymentMetadataUpdated {
                payment: received("late", 4_000),
            })
            .await;
        assert!(wakes.try_recv().is_err());
        assert!(matches!(
            frames.try_recv(),
            Ok(Frame::Event(ProtocolEvent::PaymentUpdated { id })) if id == "late"
        ));
    }

    /// A receive is counted and saved by the listener as it arrives, then
    /// the worker is woken; while off it is neither. The gui gets the
    /// payment event either way.
    #[tokio::test]
    async fn the_listener_counts_a_receive_as_it_arrives() {
        let dir = TempDir::new();
        let (listener, ac, mut frames, mut wakes) = listener(&dir);

        listener
            .on_event(SdkEvent::PaymentSucceeded {
                payment: received("off", 4_000),
            })
            .await;
        assert!(wakes.try_recv().is_err());
        assert!(matches!(
            frames.try_recv(),
            Ok(Frame::Event(ProtocolEvent::PaymentSucceeded { .. }))
        ));

        ac.lock().await.set_enabled(true).unwrap();
        listener
            .on_event(SdkEvent::PaymentSucceeded {
                payment: received("on", 4_000),
            })
            .await;
        assert!(wakes.try_recv().is_ok());
        assert!(matches!(
            frames.try_recv(),
            Ok(Frame::Event(ProtocolEvent::PaymentSucceeded { .. }))
        ));
        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.0.join(auto_convert::STATE_FILE)).unwrap())
                .unwrap();
        assert_eq!(json["pending_sats"], 4_000, "saved on arrival");
    }

    /// A token payment's event names its token, so the gui reads its amount
    /// as token base units, not sats; a bitcoin payment's names none.
    #[tokio::test]
    async fn payment_events_name_the_token_a_token_payment_moved() {
        let dir = TempDir::new();
        let (listener, _ac, mut frames, _wakes) = listener(&dir);

        let mut usdb = received("usdb", 12_500_000);
        usdb.method = breez_sdk_spark::PaymentMethod::Token;
        usdb.details = Some(PaymentDetails::Token {
            metadata: breez_sdk_spark::TokenMetadata {
                identifier: crate::sdk_adapter::USDB_MAINNET_TOKEN_IDENTIFIER.to_string(),
                issuer_public_key: "02issuer".to_string(),
                name: "USDB".to_string(),
                ticker: "USDB".to_string(),
                decimals: 6,
                max_supply: u128::MAX,
                is_freezable: false,
            },
            tx_hash: "tx".to_string(),
            tx_type: breez_sdk_spark::TokenTransactionType::Transfer,
            invoice_details: None,
            conversion_info: None,
        });
        listener
            .on_event(SdkEvent::PaymentSucceeded { payment: usdb })
            .await;
        match frames.try_recv() {
            Ok(Frame::Event(ProtocolEvent::PaymentSucceeded {
                amount_sat, token, ..
            })) => {
                assert_eq!(amount_sat, 12_500_000);
                assert_eq!(
                    token,
                    Some(coincube_spark_protocol::PaymentToken {
                        identifier: crate::sdk_adapter::USDB_MAINNET_TOKEN_IDENTIFIER.to_string(),
                        ticker: "USDB".to_string(),
                        decimals: 6,
                        usdb: true,
                    })
                );
            }
            other => panic!("expected a payment event, got {other:?}"),
        }

        // A token payment the SDK sent without metadata is still a token
        // (never sats), just not one that can be named, and not USDB.
        let mut bare = received("bare", 7_000_000);
        bare.method = breez_sdk_spark::PaymentMethod::Token;
        listener
            .on_event(SdkEvent::PaymentSucceeded { payment: bare })
            .await;
        match frames.try_recv() {
            Ok(Frame::Event(ProtocolEvent::PaymentSucceeded {
                token: Some(token), ..
            })) => assert!(!token.usdb && token.ticker.is_empty()),
            other => panic!("expected a token payment event, got {other:?}"),
        }

        listener
            .on_event(SdkEvent::PaymentSucceeded {
                payment: received("btc", 4_000),
            })
            .await;
        assert!(matches!(
            frames.try_recv(),
            Ok(Frame::Event(ProtocolEvent::PaymentSucceeded {
                token: None,
                ..
            }))
        ));
    }

    /// A converter whose send, once committed, waits to be released.
    #[derive(Default)]
    struct HeldSend {
        entered: tokio::sync::Notify,
        release: tokio::sync::Notify,
        sends: std::sync::atomic::AtomicUsize,
    }

    #[async_trait]
    impl auto_convert::Converter for HeldSend {
        type Prepared = ();
        async fn limits(&self) -> Result<auto_convert::Limits, String> {
            Ok(auto_convert::Limits {
                min_from_sats: Some(1_000),
                min_to_units: None,
            })
        }
        async fn balance_sats(&self) -> Result<u64, String> {
            Ok(100_000)
        }
        async fn quote(&self, usdb_target: u128) -> Result<auto_convert::Quote<()>, String> {
            // 1 USDB (10^6 units) for 1,000 sats.
            Ok(auto_convert::Quote {
                sats_in: usdb_target.div_ceil(1_000),
                prepared: (),
            })
        }
        async fn send(&self, _prepared: ()) -> Result<(), String> {
            self.entered.notify_one();
            self.release.notified().await;
            self.sends.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
    }

    /// Off sent while a conversion is past its final switch check waits for
    /// that send to finish, through the real handler: it is never
    /// acknowledged while that conversion can still start.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn off_waits_for_a_send_already_committed() {
        let dir = TempDir::new();
        let (state, _rx) = server(&dir);
        call(
            &state,
            Method::SetAutoConvert(SetAutoConvertParams { enabled: true }),
        )
        .await;
        let ac = Arc::clone(state.auto_convert.get().unwrap());
        assert!(ac.lock().await.note_receive(&auto_convert::Receipt {
            payment_id: "a".into(),
            sats: 20_000,
        }));

        let held = Arc::new(HeldSend::default());
        let pass = {
            let (ac, held) = (Arc::clone(&ac), Arc::clone(&held));
            tokio::spawn(async move { auto_convert::convert_pending(&ac, &*held, &|_| {}).await })
        };
        held.entered.notified().await;

        let off = {
            let state = Arc::clone(&state);
            tokio::spawn(async move {
                call(
                    &state,
                    Method::SetAutoConvert(SetAutoConvertParams { enabled: false }),
                )
                .await
            })
        };
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(!off.is_finished(), "Off acknowledged mid-send");
        assert!(ac.lock().await.state().enabled);

        held.release.notify_one();
        pass.await.unwrap();
        assert!(matches!(
            off.await.unwrap(),
            ResponseResult::Ok(OkPayload::AutoConvert(s)) if !s.enabled
        ));
        assert_eq!(held.sends.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    /// Migration off SDK Stable Balance, at the handler level: a Cube whose
    /// gui recorded "on" moves to bridge auto-convert through the legacy
    /// call, which needs no SDK, starts no conversion and leaves nothing
    /// pending, so the existing USDB and bitcoin stay as they are; only
    /// bitcoin received afterwards converts. (That the SDK itself converts
    /// nothing back is `mainnet_config_leaves_stable_balance_off_…` plus
    /// `nothing_calls_the_sdk_settings_setter`.)
    #[tokio::test]
    async fn migrating_an_sdk_active_preference_moves_no_funds() {
        let dir = TempDir::new();
        let (state, mut rx) = server(&dir);

        // The gui reconcile: recorded on, bridge reports off, so re-apply.
        let before = match call(&state, Method::GetAutoConvert).await {
            ResponseResult::Ok(OkPayload::AutoConvert(s)) => s,
            other => panic!("{other:?}"),
        };
        assert!(!before.enabled);
        call(
            &state,
            Method::SetStableBalance(SetStableBalanceParams { enabled: true }),
        )
        .await;

        // Only the state event; no conversion was started, nothing is pending.
        assert!(matches!(
            rx.try_recv(),
            Ok(Frame::Event(ProtocolEvent::AutoConvertStateChanged { .. }))
        ));
        assert!(rx.try_recv().is_err());
        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.0.join(auto_convert::STATE_FILE)).unwrap())
                .unwrap();
        assert_eq!(json["enabled"], true);
        assert_eq!(json["pending_sats"], 0);
    }
}

#[cfg(test)]
mod payment_summary_tests {
    use breez_sdk_spark::{
        ConversionInfo, ConversionPurpose, ConversionStatus, Payment, PaymentDetails,
        PaymentMethod, PaymentStatus, PaymentType, SparkHtlcDetails, SparkHtlcStatus,
        TokenMetadata,
    };

    use super::payment_to_summary;

    fn payment(
        id: &str,
        method: PaymentMethod,
        payment_type: PaymentType,
        amount: u128,
        fees: u128,
        details: Option<PaymentDetails>,
    ) -> Payment {
        Payment {
            id: id.to_string(),
            payment_type,
            status: PaymentStatus::Completed,
            amount,
            fees,
            timestamp: 1,
            method,
            details,
            conversion_details: None,
        }
    }

    fn token_metadata(ticker: &str, decimals: u32) -> TokenMetadata {
        TokenMetadata {
            identifier: format!("btkn1{ticker}"),
            issuer_public_key: "02issuer".to_string(),
            name: ticker.to_string(),
            ticker: ticker.to_string(),
            decimals,
            max_supply: u128::MAX,
            is_freezable: false,
        }
    }

    fn htlc() -> SparkHtlcDetails {
        SparkHtlcDetails {
            payment_hash: "hash".to_string(),
            preimage: None,
            expiry_time: 0,
            status: SparkHtlcStatus::PreimageShared,
        }
    }

    fn lightning(memo: &str, amount: u128, conversion: Option<ConversionInfo>) -> Payment {
        payment(
            "ln",
            PaymentMethod::Lightning,
            PaymentType::Send,
            amount,
            21,
            Some(PaymentDetails::Lightning {
                description: Some(memo.to_string()),
                invoice: "lnbc1hold".to_string(),
                destination_pubkey: "02ab".to_string(),
                htlc_details: htlc(),
                lnurl_pay_info: None,
                lnurl_withdraw_info: None,
                lnurl_receive_metadata: None,
                conversion_info: conversion,
            }),
        )
    }

    fn conversion(json: serde_json::Value) -> ConversionInfo {
        serde_json::from_value(json).expect("conversion info")
    }

    /// History reads `amount_sat` as bitcoin. A token payment's SDK amount is
    /// base units, so the summary has to move it onto `token_amount` and zero
    /// the sat fields — including when the SDK omitted the token metadata, and
    /// when the amount does not fit in a `u64` (saturate, don't truncate).
    #[test]
    fn a_token_payment_is_not_reported_as_satoshis() {
        let metadata = token_metadata("USDT", 6);
        let named = payment_to_summary(payment(
            "usdt",
            PaymentMethod::Token,
            PaymentType::Receive,
            1_580_000,
            42,
            Some(PaymentDetails::Token {
                metadata: metadata.clone(),
                tx_hash: "tx".to_string(),
                tx_type: breez_sdk_spark::TokenTransactionType::Transfer,
                invoice_details: None,
                conversion_info: None,
            }),
        ));
        assert_eq!(named.amount_sat, 0);
        assert_eq!(named.fees_sat, 0);
        assert_eq!(named.token_amount, Some(1_580_000));
        assert_eq!(named.token_decimals, Some(6));
        assert_eq!(named.token_ticker.as_deref(), Some("USDT"));
        assert_eq!(named.method, "token");
        assert!(named.description.is_none());

        let bare = payment_to_summary(payment(
            "bare",
            PaymentMethod::Token,
            PaymentType::Receive,
            7_000_000,
            9,
            None,
        ));
        assert_eq!(bare.amount_sat, 0, "base units must not land in amount_sat");
        assert_eq!(bare.fees_sat, 0);
        assert_eq!(bare.token_amount, Some(7_000_000));
        assert_eq!(bare.token_decimals, None);
        assert_eq!(bare.token_ticker, None);

        let zero = payment_to_summary(payment(
            "zero",
            PaymentMethod::Token,
            PaymentType::Receive,
            0,
            0,
            None,
        ));
        assert_eq!(zero.amount_sat, 0);
        assert_eq!(zero.token_amount, Some(0));

        let overflow = payment_to_summary(payment(
            "overflow",
            PaymentMethod::Token,
            PaymentType::Receive,
            u128::from(u64::MAX) + 1,
            u128::from(u64::MAX) + 9,
            Some(PaymentDetails::Token {
                metadata,
                tx_hash: "tx".to_string(),
                tx_type: breez_sdk_spark::TokenTransactionType::Transfer,
                invoice_details: None,
                conversion_info: None,
            }),
        ));
        assert_eq!(overflow.amount_sat, 0);
        assert_eq!(overflow.fees_sat, 0);
        assert_eq!(
            overflow.token_amount,
            Some(u64::MAX),
            "a value past u64::MAX saturates instead of truncating to 1"
        );
    }

    #[test]
    fn a_bitcoin_payment_keeps_its_sats_and_fees() {
        let summary = payment_to_summary(payment(
            "btc",
            PaymentMethod::Spark,
            PaymentType::Receive,
            4_000,
            21,
            None,
        ));
        assert_eq!(summary.amount_sat, 4_000);
        assert_eq!(summary.fees_sat, 21);
        assert_eq!(summary.token_amount, None);
        assert_eq!(summary.token_decimals, None);
        assert_eq!(summary.token_ticker, None);
        assert_eq!(summary.method, "spark");
        assert_eq!(summary.direction, "Receive");
        assert_eq!(summary.status, "Completed");
    }

    /// A Boltz or Orchestra leg is a bitcoin Lightning payment that delivered
    /// a stablecoin elsewhere. History should name that destination and still
    /// count the Spark leg in sats. An AMM conversion has no external chain,
    /// so it keeps the invoice memo.
    #[test]
    fn a_cross_chain_leg_names_its_destination_without_dropping_the_sats() {
        let memo = "Send to TBTC address";
        let boltz = payment_to_summary(lightning(
            memo,
            150_000,
            Some(conversion(serde_json::json!({
                "type": "boltz",
                "swap_id": "swap-1",
                "invoice": "lnbc1hold",
                "invoice_amount_sats": 150_000,
                "max_slippage_bps": 100,
                "chain": "solana",
                "asset": "USDT",
                "recipient_address": "recipient",
                "estimated_out": "99000000",
                "status": "Completed",
                "asset_decimals": 6
            }))),
        ));
        assert_eq!(boltz.amount_sat, 150_000);
        assert_eq!(boltz.fees_sat, 21);
        assert_eq!(boltz.token_amount, None);
        assert_eq!(boltz.description.as_deref(), Some("Sent USDT on Solana"));

        let orchestra = payment_to_summary(lightning(
            memo,
            20_000,
            Some(conversion(serde_json::json!({
                "type": "orchestra",
                "order_id": "order-1",
                "quote_id": "quote-1",
                "chain": "base",
                "asset": "USDC",
                "recipient_address": "0xrecipient",
                "estimated_out": "1000000",
                "status": "Completed",
                "asset_decimals": 6
            }))),
        ));
        assert_eq!(orchestra.amount_sat, 20_000);
        assert_eq!(orchestra.token_amount, None);
        assert_eq!(orchestra.description.as_deref(), Some("Sent USDC on Base"));

        let amm = payment_to_summary(lightning(
            memo,
            4_000,
            Some(ConversionInfo::Amm {
                pool_id: "pool".to_string(),
                conversion_id: "conv".to_string(),
                status: ConversionStatus::Completed,
                fee: None,
                purpose: Some(ConversionPurpose::SelfTransfer),
                amount_adjustment: None,
                degradation: None,
            }),
        ));
        assert_eq!(amm.amount_sat, 4_000);
        assert_eq!(amm.description.as_deref(), Some(memo));

        let plain = payment_to_summary(lightning(memo, 4_000, None));
        assert_eq!(plain.amount_sat, 4_000);
        assert_eq!(plain.description.as_deref(), Some(memo));
        assert_eq!(plain.method, "lightning");
    }
}

#[cfg(test)]
mod usdb_balance_tests {
    use super::*;
    use breez_sdk_spark::TokenMetadata;

    const USDB: &str = sdk_adapter::USDB_MAINNET_TOKEN_IDENTIFIER;
    /// Another token the wallet could hold.
    const OTHER: &str = "btkn1another0issuer0token0for0tests";

    fn token(identifier: &str, balance: u128, decimals: u32, ticker: &str) -> TokenBalance {
        TokenBalance {
            balance,
            token_metadata: TokenMetadata {
                identifier: identifier.to_string(),
                issuer_public_key: "02".repeat(33),
                name: format!("{ticker} token"),
                ticker: ticker.to_string(),
                decimals,
                max_supply: u128::MAX,
                is_freezable: false,
            },
        }
    }

    /// A `token_balances` map, keyed the way the SDK keys it.
    fn balances(
        entries: impl IntoIterator<Item = (&'static str, TokenBalance)>,
    ) -> HashMap<String, TokenBalance> {
        entries
            .into_iter()
            .map(|(key, balance)| (key.to_string(), balance))
            .collect()
    }

    fn usdb(balance: u128) -> TokenBalance {
        token(USDB, balance, 6, "USDB")
    }

    #[test]
    fn a_nonzero_usdb_holding_is_reported_with_its_metadata() {
        assert_eq!(
            usdb_balance(&balances([(USDB, usdb(50_000_000))])),
            Some(UsdbBalanceSnapshot {
                token_identifier: USDB.to_string(),
                balance: 50_000_000,
                decimals: 6,
                ticker: "USDB".to_string(),
            })
        );
    }

    /// The snapshot carries the token's own metadata, not a hard-coded
    /// 6 decimals and "USDB": a holding with other metadata is passed through
    /// as the SDK reports it.
    #[test]
    fn the_snapshot_carries_the_token_metadata_as_reported() {
        assert_eq!(
            usdb_balance(&balances([(USDB, token(USDB, 1_234_567_890, 8, "USDB.e"))])),
            Some(UsdbBalanceSnapshot {
                token_identifier: USDB.to_string(),
                balance: 1_234_567_890,
                decimals: 8,
                ticker: "USDB.e".to_string(),
            })
        );
    }

    #[test]
    fn zero_or_missing_usdb_is_none() {
        assert_eq!(usdb_balance(&HashMap::new()), None);
        assert_eq!(usdb_balance(&balances([(USDB, usdb(0))])), None);
    }

    /// Only the configured USDB token is USDB. Another token that calls
    /// itself "USDB" isn't, and neither is an entry filed under the USDB key
    /// whose metadata names a different token.
    #[test]
    fn only_the_configured_usdb_token_is_usdb() {
        let imitation = token(OTHER, 7_000_000, 6, "USDB");
        assert_eq!(usdb_balance(&balances([(OTHER, imitation.clone())])), None);

        let snapshot = usdb_balance(&balances([(OTHER, imitation), (USDB, usdb(1_250_000))]))
            .expect("USDB is held");
        assert_eq!(
            (snapshot.token_identifier.as_str(), snapshot.balance),
            (USDB, 1_250_000)
        );

        assert_eq!(
            usdb_balance(&balances([(USDB, token(OTHER, 9_000_000, 6, "USDB"))])),
            None
        );
    }

    #[test]
    fn a_balance_above_u64_max_is_not_clamped() {
        for held in [u64::MAX as u128 + 1, u128::MAX] {
            let snapshot = usdb_balance(&balances([(USDB, usdb(held))])).expect("USDB is held");
            assert_eq!(snapshot.balance, held);
        }
    }

    /// The whole `get_info` answer. `balance_sats` stays the SDK's
    /// bitcoin-only figure, the legacy `stable_balance` is unchanged (still
    /// clamped to `u64`), and `usdb_balance` carries the exact figure, as a
    /// decimal string on the wire.
    #[test]
    fn get_info_keeps_bitcoin_and_the_legacy_field_and_adds_exact_usdb() {
        let held = u64::MAX as u128 + 1;
        let ok = get_info_ok(GetInfoResponse {
            identity_pubkey: "03ab".to_string(),
            balance_sats: 25_053,
            recoverable_funds_sats: 0,
            last_sync_error: None,
            last_synced_at: None,
            token_balances: balances([(OTHER, token(OTHER, 3, 0, "XYZ")), (USDB, usdb(held))]),
        });
        assert_eq!(ok.balance_sats, 25_053);
        assert_eq!(ok.identity_pubkey, "03ab");
        let legacy = ok
            .stable_balance
            .as_ref()
            .expect("the legacy field is still reported");
        assert_eq!(
            (legacy.balance, legacy.decimals, legacy.ticker.as_str()),
            (u64::MAX, 6, "USDB")
        );
        let json = serde_json::to_value(&ok).expect("get_info serializes");
        assert_eq!(json["balance_sats"], 25_053);
        assert_eq!(json["stable_balance"]["balance"], u64::MAX);
        assert_eq!(
            json["usdb_balance"],
            serde_json::json!({
                "token_identifier": USDB,
                "balance": "18446744073709551616",
                "decimals": 6,
                "ticker": "USDB",
            })
        );

        // Without USDB, both fields are empty and `usdb_balance` stays off
        // the wire, as before S1b.
        let ok = get_info_ok(GetInfoResponse {
            identity_pubkey: "03ab".to_string(),
            balance_sats: 25_053,
            recoverable_funds_sats: 0,
            last_sync_error: None,
            last_synced_at: None,
            token_balances: balances([(OTHER, token(OTHER, 3, 0, "XYZ"))]),
        });
        assert!(ok.stable_balance.is_none());
        assert!(ok.usdb_balance.is_none());
        let json = serde_json::to_value(&ok).expect("get_info serializes");
        assert!(json.get("usdb_balance").is_none(), "{json}");
        assert_eq!(json["balance_sats"], 25_053);
    }
}
