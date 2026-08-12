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
    CheckLightningAddressRequest, ClaimDepositRequest, ConversionEstimate, ConversionType,
    CrossChainAddressDetails, CrossChainAddressFamily, CrossChainRouteFilter, CrossChainRoutePair,
    EventListener, GetInfoRequest, InputType, LightningAddressInfo as SdkLightningAddressInfo,
    ListPaymentsRequest, ListUnclaimedDepositsRequest, LnurlPayRequest, MaxFee,
    OnchainConfirmationSpeed, PaymentDetails, PaymentRequest, PrepareLnurlPayRequest,
    PrepareLnurlPayResponse, PrepareSendPaymentRequest, PrepareSendPaymentResponse,
    ReceivePaymentMethod, ReceivePaymentRequest, RegisterLightningAddressRequest, SdkEvent,
    SendOnchainFeeQuote, SendPaymentMethod, SendPaymentOptions, SendPaymentRequest, SourceAsset,
    StableBalanceActiveLabel, UpdateUserSettingsRequest,
};
use coincube_spark_protocol::{
    CheckLightningAddressAvailableOk, ClaimDepositOk, DepositInfo, ErrorKind,
    Event as ProtocolEvent, Frame, GetInfoOk, GetLightningAddressOk, GetUserSettingsOk,
    LightningAddressInfo as ProtocolLightningAddressInfo, ListPaymentsOk, ListUnclaimedDepositsOk,
    Method, OkPayload, ParseInputKind, ParseInputOk, PaymentSummary, PrepareSendOk,
    ReceivePaymentOk, RegisterLightningAddressOk, Request, Response, SendPaymentOk,
    SetStableBalanceParams, StableBalanceSnapshot,
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
        Ok(info) => {
            let stable_balance = info
                .token_balances
                .get(crate::sdk_adapter::USDB_MAINNET_TOKEN_IDENTIFIER)
                .filter(|tb| tb.balance > 0)
                .map(|tb| StableBalanceSnapshot {
                    balance: clamp_u128_to_u64(tb.balance),
                    decimals: tb.token_metadata.decimals,
                    ticker: tb.token_metadata.ticker.clone(),
                });
            Response::ok(
                id,
                OkPayload::GetInfo(GetInfoOk {
                    balance_sats: info.balance_sats,
                    identity_pubkey: info.identity_pubkey,
                    stable_balance,
                }),
            )
        }
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
    }
}

// ---------------------------------------------------------------------------
// Phase 4c write-path handlers
// ---------------------------------------------------------------------------

/// Rejects a prepare that came back carrying a token-conversion leg the wallet
/// cannot actually fund. `None` means the prepare is fine to hand to the gui.
///
/// **Why a plain sats send can grow a conversion leg at all.** Both
/// [`handle_prepare_send`] and [`handle_prepare_lnurl_pay`] pass
/// `conversion_options: None` and `token_identifier: None`, so any conversion on
/// the response was auto-attached by the SDK. Stable Balance does that: when it
/// is active and the sat balance is below `amount + fee`,
/// `stable_balance::get_conversion_options` silently fills in a
/// `ToBitcoin { <stable token> }` conversion so the shortfall can be covered by
/// swapping Stable Balance back to bitcoin (SDK 0.19.0,
/// `crates/breez-sdk/core/src/stable_balance/mod.rs`). That is a *feature*, and
/// this function deliberately lets it through when it can work.
///
/// **Why it needs a guard.** The SDK validates that auto-attached conversion
/// against the AMM pool, not against the wallet's own token balance. A wallet
/// with Stable Balance on and no stable token prepares cleanly, shows a fee, and
/// then dies inside Flashnet at send time with
/// `Wallet: Service error: generic error: Token outputs not found` — an
/// insufficient-funds condition wearing an unreadable disguise, arriving after
/// the user has already pressed Confirm and Send. Checking the holding here
/// turns it into a plain-language refusal on the amount screen instead.
///
/// The balance read forces a sync: a stale cache that under-reports the holding
/// would reject a send that would have worked, which is the worse mistake of the
/// two. It only runs on the rare prepare that actually carries a conversion.
async fn unfundable_conversion_error(
    id: u64,
    sdk: &SdkHandle,
    estimate: Option<&ConversionEstimate>,
) -> Option<Response> {
    let estimate = estimate?;
    let ConversionType::ToBitcoin {
        from_token_identifier,
    } = &estimate.options.conversion_type
    else {
        // `FromBitcoin` cannot legitimately appear on these paths — the gui
        // never requests one, and Stable Balance only ever auto-attaches
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
        "Not enough bitcoin in this Spark wallet to cover the amount and its fee. \
         Stable Balance is on, so the wallet tried to make up the difference by \
         converting Stable Balance back to bitcoin — but the Stable Balance holding \
         is too small to cover it. Send a smaller amount, top up the Spark bitcoin \
         balance, or turn Stable Balance off in Spark settings.",
    ))
}

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

    // 0.19.0 turned `payment_request` from a bare `String` into an enum, so the
    // regular send path now has to say explicitly that it's handing over raw
    // user input for the SDK to classify (bolt11 / spark address / BIP-21 / …).
    // The cross-chain path uses the other variant — see `prepare_cross_chain`.
    let request = PrepareSendPaymentRequest {
        payment_request: PaymentRequest::Input {
            input: params.input,
        },
        amount: params.amount_sat.map(|a| a as u128),
        token_identifier: None,
        conversion_options: None,
        fee_policy: None,
    };

    match sdk.sdk.prepare_send_payment(request).await {
        Ok(prepare) => {
            // Stable Balance can auto-attach a token→BTC conversion here even
            // though we asked for none. Fail now, in the user's terms, if the
            // wallet can't fund it — see `unfundable_conversion_error`.
            if let Some(response) =
                unfundable_conversion_error(id, &sdk, prepare.conversion_estimate.as_ref()).await
            {
                return response;
            }

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

            Response::ok(
                id,
                OkPayload::PrepareSend(PrepareSendOk {
                    handle,
                    amount_sat,
                    fee_sat,
                    method: method_tag.to_string(),
                    // Regular sends carry no cross-chain quote — those come
                    // from `prepare_cross_chain`.
                    cross_chain: None,
                    has_token_leg: Some(has_token_leg),
                }),
            )
        }
        Err(e) => Response::err(id, ErrorKind::Sdk, format!("prepare_send failed: {e}")),
    }
}

// ── Cross-chain stablecoin send ────────────────────────────────────────────

/// Whether a route can be funded from the wallet's BTC balance. v1 only offers
/// these: the source asset is always BTC sats (converted at pay time), and
/// sending *from* Stable Balance is deferred. This is also what makes the v1
/// path retry-safe — see [`route_is_retry_safe`].
fn route_accepts_btc(route: &CrossChainRoutePair) -> bool {
    route
        .supported_sources
        .iter()
        .any(|s| matches!(s, SourceAsset::Bitcoin))
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

    let filter = CrossChainRouteFilter::Send {
        address_details: details,
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
    let filter = CrossChainRouteFilter::Send {
        address_details: details,
    };
    let live_routes = match sdk.sdk.get_cross_chain_routes(&filter).await {
        Ok(routes) => routes,
        Err(e) => return Response::err(id, ErrorKind::Sdk, e.to_string()),
    };
    let Some(route) = live_routes
        .into_iter()
        .find(|r| sdk_route_to_protocol(r) == params.route)
    else {
        return Response::err(
            id,
            ErrorKind::BadRequest,
            "the selected route is no longer offered — re-fetch routes and retry",
        );
    };
    let retry_safe = route_is_retry_safe(&route);

    let request = PrepareSendPaymentRequest {
        payment_request: PaymentRequest::CrossChain {
            // The SDK wants the bare recipient address here, not the URI it may
            // have come from — the URI's extra context has already done its job
            // in the route filter above.
            address: params.destination.address,
            route,
            max_slippage_bps: params.max_slippage_bps,
            // Leave the overpay pad at the SDK default (15 bps). It only
            // applies to `FeesExcluded` conversion sends and exists to stop the
            // recipient landing *under* the requested amount; there's no user
            // question here worth exposing.
            target_overpay_bps: None,
        },
        amount: Some(params.amount_sat as u128),
        token_identifier: None,
        conversion_options: None,
        fee_policy: None,
    };

    match sdk.sdk.prepare_send_payment(request).await {
        Ok(prepare) => {
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
        Err(e) => Response::err(id, ErrorKind::Sdk, e.to_string()),
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
    let amount_sat = clamp_u128_to_u64(prepare.amount);
    let (fee_sat, method_tag) = fee_and_method(&prepare.payment_method, &speed);
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
        InputType::SparkAddress(_details) => ParseInputOk {
            // Static, identity-bound Spark address — no amount.
            kind: ParseInputKind::SparkAddress,
            amount_sat: None,
            lnurl_min_sendable_sat: None,
            lnurl_max_sendable_sat: None,
            lnurl_comment_allowed: 0,
            lnurl_address: None,
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
            // Same Stable Balance auto-attach as the regular send path, and the
            // same refusal when the holding can't fund it. This path has a
            // second failure mode the check also heads off: `execute_lnurl_send`
            // forwards the idempotency key, and the SDK rejects a key on a
            // payment with a token leg outright.
            if let Some(response) =
                unfundable_conversion_error(id, &sdk, prepare.conversion_estimate.as_ref()).await
            {
                return response;
            }

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

            Response::ok(
                id,
                OkPayload::PrepareSend(PrepareSendOk {
                    handle,
                    amount_sat,
                    fee_sat,
                    method,
                    // LNURL-pay is a Lightning send; never cross-chain.
                    cross_chain: None,
                    has_token_leg: Some(has_token_leg),
                }),
            )
        }
        Err(e) => Response::err(id, ErrorKind::Sdk, format!("prepare_lnurl_pay failed: {e}")),
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
        max_fee: Some(MaxFee::NetworkRecommended {
            leeway_sat_per_vbyte: 5,
        }),
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
mod cross_chain_tests {
    use super::*;
    use breez_sdk_spark::CrossChainProvider;

    fn route(provider: CrossChainProvider, sources: Vec<SourceAsset>) -> CrossChainRoutePair {
        CrossChainRoutePair {
            provider,
            chain: "base".to_string(),
            chain_id: Some("8453".to_string()),
            asset: "USDC".to_string(),
            contract_address: Some("0xabc".to_string()),
            decimals: 6,
            exact_out_eligible: true,
            supported_sources: sources,
        }
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
        let r = route(CrossChainProvider::Orchestra, vec![SourceAsset::Bitcoin]);
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
        let r = route(CrossChainProvider::Boltz, vec![SourceAsset::Bitcoin]);
        assert_eq!(sdk_route_to_protocol(&r).provider, "boltz");
    }

    #[test]
    fn a_token_only_route_is_neither_offered_nor_retry_safe() {
        // v1 funds every send from BTC. A token-only route can't be prepared,
        // and — critically — a token source leg has no idempotency hook, so it
        // must never be reported to the gui as safe to blind-retry.
        let r = route(
            CrossChainProvider::Orchestra,
            vec![SourceAsset::Token {
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
                SourceAsset::Token {
                    token_identifier: "btkn1xyz".to_string(),
                },
                SourceAsset::Bitcoin,
            ],
        );
        assert!(route_accepts_btc(&r));
        assert!(route_is_retry_safe(&r));
    }
}
