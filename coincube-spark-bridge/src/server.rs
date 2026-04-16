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
    ClaimDepositRequest, EventListener, GetInfoRequest, InputType, ListPaymentsRequest,
    ListUnclaimedDepositsRequest, LnurlPayRequest, PaymentDetails, PrepareLnurlPayRequest,
    PrepareLnurlPayResponse, PrepareSendPaymentRequest, PrepareSendPaymentResponse,
    ReceivePaymentMethod, ReceivePaymentRequest, SdkEvent, SendPaymentMethod, SendPaymentRequest,
    StableBalanceActiveLabel, UpdateUserSettingsRequest,
};
use coincube_spark_protocol::{
    ClaimDepositOk, DepositInfo, ErrorKind, Event as ProtocolEvent, Frame, GetInfoOk,
    GetUserSettingsOk, ListPaymentsOk, ListUnclaimedDepositsOk, Method, OkPayload, ParseInputKind,
    ParseInputOk, PaymentSummary, PrepareSendOk, ReceivePaymentOk, Request, Response,
    SendPaymentOk, SetStableBalanceParams,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;

use crate::sdk_adapter::{self, SdkHandle};

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
pub async fn run() -> anyhow::Result<()> {
    // Single writer task: serializes all stdout writes so responses and
    // events never interleave mid-line. We talk to it over an unbounded
    // channel so request handlers never block on IO.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Frame>();
    // `ServerState` holds a clone of the same sender so the event
    // listener registered in `handle_init` can push `Frame::Event`s
    // onto the same stdout stream the response handlers use.
    let state = Arc::new(ServerState::new(tx.clone()));

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
    /// Clone of the outbound frame sender. Stored here so `handle_init`
    /// can hand a copy to the Spark SDK event listener — the listener
    /// pushes `Frame::Event`s on this channel the same way request
    /// handlers push `Frame::Response`s, so stdout stays interleave-safe.
    event_tx: tokio::sync::mpsc::UnboundedSender<Frame>,
}

impl ServerState {
    fn new(event_tx: tokio::sync::mpsc::UnboundedSender<Frame>) -> Self {
        Self {
            sdk: RwLock::new(None),
            init_lock: Mutex::new(()),
            pending_prepares: Mutex::new(HashMap::new()),
            pending_lnurl_prepares: Mutex::new(HashMap::new()),
            event_tx,
        }
    }
}

async fn handle_request(request: Request, state: Arc<ServerState>) -> Response {
    let id = request.id;
    match request.method {
        Method::Init(params) => handle_init(id, params, state).await,
        Method::GetInfo(params) => handle_get_info(id, params, state).await,
        Method::ListPayments(params) => handle_list_payments(id, params, state).await,
        Method::ParseInput(params) => handle_parse_input(id, params, state).await,
        Method::PrepareSend(params) => handle_prepare_send(id, params, state).await,
        Method::PrepareLnurlPay(params) => handle_prepare_lnurl_pay(id, params, state).await,
        Method::SendPayment(params) => handle_send_payment(id, params, state).await,
        Method::ReceiveBolt11(params) => handle_receive_bolt11(id, params, state).await,
        Method::ReceiveOnchain(params) => handle_receive_onchain(id, params, state).await,
        Method::ListUnclaimedDeposits => handle_list_unclaimed_deposits(id, state).await,
        Method::ClaimDeposit(params) => handle_claim_deposit(id, params, state).await,
        Method::GetUserSettings => handle_get_user_settings(id, state).await,
        Method::SetStableBalance(params) => handle_set_stable_balance(id, params, state).await,
        Method::Shutdown => {
            // Handled inline in the read loop — this branch exists so the
            // match is exhaustive.
            Response::ok(id, OkPayload::Shutdown {})
        }
    }
}

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

    match sdk_adapter::connect_mainnet(
        params.api_key,
        params.mnemonic,
        params.mnemonic_passphrase,
        params.storage_dir,
    )
    .await
    {
        Ok(handle) => {
            // Register an event listener before making the handle
            // visible to other request handlers. `add_event_listener`
            // returns a listener id string (not a Result) that we could
            // use for `remove_event_listener` if we wanted to rotate
            // listeners — Phase 4d just holds it for the process
            // lifetime, so we drop the id.
            let listener = BridgeEventListener {
                tx: state.event_tx.clone(),
            };
            let _listener_id = handle.sdk.add_event_listener(Box::new(listener)).await;
            *state.sdk.write().await = Some(handle);
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
}

#[async_trait]
impl EventListener for BridgeEventListener {
    async fn on_event(&self, event: SdkEvent) {
        let protocol_event = match event {
            SdkEvent::Synced => Some(ProtocolEvent::Synced),
            SdkEvent::PaymentSucceeded { payment } => Some(ProtocolEvent::PaymentSucceeded {
                amount_sat: payment.amount as i64,
                bolt11: extract_bolt11(&payment),
                id: payment.id,
            }),
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
            // Optimization + lightning-address-changed remain
            // swallowed until a panel needs them.
            SdkEvent::Optimization { .. } | SdkEvent::LightningAddressChanged { .. } => None,
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
        Ok(info) => Response::ok(
            id,
            OkPayload::GetInfo(GetInfoOk {
                balance_sats: info.balance_sats,
                identity_pubkey: info.identity_pubkey,
            }),
        ),
        Err(e) => Response::err(id, ErrorKind::Sdk, format!("get_info failed: {e}")),
    }
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
fn payment_to_summary(p: breez_sdk_spark::Payment) -> PaymentSummary {
    let description = match &p.details {
        Some(PaymentDetails::Lightning { description, .. }) => description.clone(),
        _ => None,
    };
    PaymentSummary {
        id: p.id,
        amount_sat: clamp_u128_to_u64(p.amount) as i64,
        fees_sat: clamp_u128_to_u64(p.fees),
        timestamp: p.timestamp,
        status: format!("{:?}", p.status),
        direction: format!("{:?}", p.payment_type),
        method: format!("{}", p.method),
        description,
    }
}

// ---------------------------------------------------------------------------
// Phase 4c write-path handlers
// ---------------------------------------------------------------------------

async fn handle_prepare_send(
    id: u64,
    params: coincube_spark_protocol::PrepareSendParams,
    state: Arc<ServerState>,
) -> Response {
    let sdk = match state.sdk.read().await.clone() {
        Some(s) => s,
        None => {
            return Response::err(
                id,
                ErrorKind::NotConnected,
                "init must succeed before prepare_send",
            );
        }
    };

    let request = PrepareSendPaymentRequest {
        payment_request: params.input,
        amount: params.amount_sat.map(|a| a as u128),
        token_identifier: None,
        conversion_options: None,
        fee_policy: None,
    };

    match sdk.sdk.prepare_send_payment(request).await {
        Ok(prepare) => {
            // Extract display-friendly fields before stashing the full
            // struct. `amount` + method-specific fees are u128 in the
            // SDK (Spark tokens can exceed sat precision); we saturate
            // to u64 for display. Bitcoin-side sends are well within
            // u64::MAX.
            let amount_sat = clamp_u128_to_u64(prepare.amount);
            let (fee_sat, method_tag) = match &prepare.payment_method {
                SendPaymentMethod::BitcoinAddress { fee_quote, .. } => {
                    // Default to the medium-speed quote for display —
                    // the gui can surface all three tiers in Phase 4d.
                    let fee = fee_quote.speed_medium.user_fee_sat
                        + fee_quote.speed_medium.l1_broadcast_fee_sat;
                    (fee, "BitcoinAddress")
                }
                SendPaymentMethod::Bolt11Invoice {
                    spark_transfer_fee_sats,
                    lightning_fee_sats,
                    ..
                } => (
                    spark_transfer_fee_sats.unwrap_or(0) + lightning_fee_sats,
                    "Bolt11Invoice",
                ),
                SendPaymentMethod::SparkAddress { fee, .. } => {
                    (clamp_u128_to_u64(*fee), "SparkAddress")
                }
                SendPaymentMethod::SparkInvoice { fee, .. } => {
                    (clamp_u128_to_u64(*fee), "SparkInvoice")
                }
            };

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
                    method: method_tag.to_string(),
                }),
            )
        }
        Err(e) => Response::err(id, ErrorKind::Sdk, format!("prepare_send failed: {e}")),
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

    if let Some((_inserted_at, prepare)) =
        state.pending_prepares.lock().await.remove(&handle)
    {
        return execute_regular_send(id, sdk, prepare).await;
    }

    if let Some((_inserted_at, prepare)) =
        state.pending_lnurl_prepares.lock().await.remove(&handle)
    {
        return execute_lnurl_send(id, sdk, prepare).await;
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
) -> Response {
    // Snapshot for the response so we can surface the final amount/fee
    // even after the SDK consumes the prepare response.
    let amount_sat = clamp_u128_to_u64(prepare.amount);
    let fee_sat = match &prepare.payment_method {
        SendPaymentMethod::BitcoinAddress { fee_quote, .. } => {
            fee_quote.speed_medium.user_fee_sat + fee_quote.speed_medium.l1_broadcast_fee_sat
        }
        SendPaymentMethod::Bolt11Invoice {
            spark_transfer_fee_sats,
            lightning_fee_sats,
            ..
        } => spark_transfer_fee_sats.unwrap_or(0) + lightning_fee_sats,
        SendPaymentMethod::SparkAddress { fee, .. } => clamp_u128_to_u64(*fee),
        SendPaymentMethod::SparkInvoice { fee, .. } => clamp_u128_to_u64(*fee),
    };

    // Phase 4c ships the default send options (Medium speed for
    // on-chain, Spark-preferred routing for Bolt11 without a completion
    // timeout). User-configurable options (fee tier picker) land in
    // Phase 4f when the UI has the real controls to expose them.
    let request = SendPaymentRequest {
        prepare_response: prepare,
        options: None,
        idempotency_key: None,
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
) -> Response {
    // The LNURL prepare response carries its own top-level
    // `amount_sats` / `fee_sats` fields (u64, already in sats — no
    // u128 clamping needed here). Snapshot them for the send response.
    let amount_sat = prepare.amount_sats;
    let fee_sat = prepare.fee_sats;

    let request = LnurlPayRequest {
        prepare_response: prepare,
        idempotency_key: None,
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
        },
        InputType::Bip21(details) => ParseInputOk {
            kind: ParseInputKind::BitcoinAddress,
            amount_sat: details.amount_sat,
            lnurl_min_sendable_sat: None,
            lnurl_max_sendable_sat: None,
            lnurl_comment_allowed: 0,
            lnurl_address: None,
        },
        InputType::LnurlPay(pay) => ParseInputOk {
            kind: ParseInputKind::LnurlPay,
            amount_sat: None,
            lnurl_min_sendable_sat: Some(msat_to_sat(pay.min_sendable)),
            lnurl_max_sendable_sat: Some(msat_to_sat(pay.max_sendable)),
            lnurl_comment_allowed: pay.comment_allowed,
            lnurl_address: pay.address,
        },
        InputType::LightningAddress(addr) => ParseInputOk {
            kind: ParseInputKind::LightningAddress,
            amount_sat: None,
            lnurl_min_sendable_sat: Some(msat_to_sat(addr.pay_request.min_sendable)),
            lnurl_max_sendable_sat: Some(msat_to_sat(addr.pay_request.max_sendable)),
            lnurl_comment_allowed: addr.pay_request.comment_allowed,
            lnurl_address: Some(addr.address),
        },
        // Everything else — BOLT12 invoices/offers, LNURL-auth,
        // LNURL-withdraw, silent payment, Spark-native types, bare
        // URLs — falls through to `Other`. The gui shows a "not
        // supported yet" error; future phases can break each one out
        // as demand appears.
        _ => ParseInputOk {
            kind: ParseInputKind::Other,
            amount_sat: None,
            lnurl_min_sendable_sat: None,
            lnurl_max_sendable_sat: None,
            lnurl_comment_allowed: 0,
            lnurl_address: None,
        },
    }
}

async fn handle_prepare_lnurl_pay(
    id: u64,
    params: coincube_spark_protocol::PrepareLnurlPayParams,
    state: Arc<ServerState>,
) -> Response {
    let sdk = match state.sdk.read().await.clone() {
        Some(s) => s,
        None => {
            return Response::err(
                id,
                ErrorKind::NotConnected,
                "init must succeed before prepare_lnurl_pay",
            );
        }
    };

    // Re-parse the input to recover the SDK's `LnurlPayRequestDetails`.
    // We could stash the parse result from the earlier `parse_input`
    // call and pass it back, but that would tie the protocol to the
    // SDK's internal types. Re-parsing is cheap — it's a local
    // regex/bech32 decode on a string we already know to be valid.
    let pay_request = match sdk.sdk.parse(&params.input).await {
        Ok(InputType::LnurlPay(details)) => details,
        Ok(InputType::LightningAddress(addr)) => addr.pay_request,
        Ok(other) => {
            return Response::err(
                id,
                ErrorKind::BadRequest,
                format!(
                    "prepare_lnurl_pay called with non-LNURL input (parsed as {:?})",
                    std::mem::discriminant(&other)
                ),
            );
        }
        Err(e) => {
            return Response::err(id, ErrorKind::Sdk, format!("parse_input failed: {e}"));
        }
    };

    let request = PrepareLnurlPayRequest {
        amount: params.amount_sat as u128,
        pay_request,
        comment: params.comment,
        validate_success_action_url: None,
        token_identifier: None,
        conversion_options: None,
        fee_policy: None,
    };

    match sdk.sdk.prepare_lnurl_pay(request).await {
        Ok(prepare) => {
            // Preview fields come straight out of the SDK's
            // `PrepareLnurlPayResponse` — it already exposes top-level
            // `amount_sats` and `fee_sats` in u64, so no u128 clamping
            // is needed on this path.
            let amount_sat = prepare.amount_sats;
            let fee_sat = prepare.fee_sats;
            let method = "LnurlPay".to_string();

            let handle = Uuid::new_v4().to_string();
            state
                .pending_lnurl_prepares
                .lock()
                .await
                .insert(handle.clone(), (Instant::now(), prepare));

            Response::ok(
                id,
                OkPayload::PrepareSend(PrepareSendOk {
                    handle,
                    amount_sat,
                    fee_sat,
                    method,
                }),
            )
        }
        Err(e) => Response::err(
            id,
            ErrorKind::Sdk,
            format!("prepare_lnurl_pay failed: {e}"),
        ),
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
        // Phase 4f uses the SDK default fee policy (None → SDK picks
        // a network-recommended rate). A user-facing fee tier picker
        // for claims is a Phase 4g+ polish item.
        max_fee: None,
    };

    match sdk.sdk.claim_deposit(request).await {
        Ok(resp) => {
            // The SDK's claim returns a Payment whose `amount` reflects
            // the post-fee deposited value. Surface that to the gui so
            // the success toast can show the actual claimed amount.
            let amount_sat = clamp_u128_to_u64(resp.payment.amount);
            Response::ok(
                id,
                OkPayload::ClaimDeposit(ClaimDepositOk {
                    payment_id: resp.payment.id,
                    amount_sat,
                }),
            )
        }
        Err(e) => Response::err(id, ErrorKind::Sdk, format!("claim_deposit failed: {e}")),
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

    match sdk.sdk.get_user_settings().await {
        Ok(settings) => Response::ok(
            id,
            OkPayload::GetUserSettings(GetUserSettingsOk {
                // An active label of `Some(_)` means Stable Balance is
                // currently on. We don't surface the label itself —
                // the gui only cares about the boolean.
                stable_balance_active: settings.stable_balance_active_label.is_some(),
                private_mode_enabled: settings.spark_private_mode_enabled,
            }),
        ),
        Err(e) => Response::err(id, ErrorKind::Sdk, format!("get_user_settings failed: {e}")),
    }
}

async fn handle_set_stable_balance(
    id: u64,
    params: SetStableBalanceParams,
    state: Arc<ServerState>,
) -> Response {
    let sdk = match state.sdk.read().await.clone() {
        Some(s) => s,
        None => {
            return Response::err(
                id,
                ErrorKind::NotConnected,
                "init must succeed before set_stable_balance",
            );
        }
    };

    let active_label = if params.enabled {
        StableBalanceActiveLabel::Set {
            label: crate::sdk_adapter::STABLE_BALANCE_LABEL.to_string(),
        }
    } else {
        StableBalanceActiveLabel::Unset
    };

    let request = UpdateUserSettingsRequest {
        spark_private_mode_enabled: None,
        stable_balance_active_label: Some(active_label),
    };

    match sdk.sdk.update_user_settings(request).await {
        Ok(()) => Response::ok(id, OkPayload::SetStableBalance {}),
        Err(e) => Response::err(
            id,
            ErrorKind::Sdk,
            format!("update_user_settings failed: {e}"),
        ),
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

/// Phase 4f: walk both pending-prepare maps and drop entries whose
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

    if evicted_regular > 0 || evicted_lnurl > 0 {
        tracing::debug!(
            "evicted {} expired prepare(s), {} expired lnurl prepare(s)",
            evicted_regular,
            evicted_lnurl
        );
    }
}
