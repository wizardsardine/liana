use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use iced::futures::SinkExt;
use iced::Subscription;
use nip06::FromMnemonic;
use nostr_sdk::prelude::*;
use serde::{Deserialize, Serialize};

use super::components::trade_card::{TradeRole, TradeStatus};
use super::components::{OrderType, P2POrder, P2PTrade};
use crate::app::view::message::P2PMessage;
use crate::app::{message::Message, view};

const FETCH_INTERVAL_SECS: u64 = 10;
const ORDER_LOOKBACK_SECS: u64 = 48 * 3600; // 48 hours, same as mobile
const MOSTRO_INFO_EVENT_KIND: u16 = 38385;

// ── Key management ──────────────────────────────────────────────────────

/// Identity persisted to disk, keyed by cube name.
#[derive(Serialize, Deserialize)]
struct MostroIdentity {
    mnemonic: String,
    last_trade_index: i64,
}

/// A trade session persisted to disk so we can track the user's own orders.
#[derive(Serialize, Deserialize, Clone)]
pub struct TradeSession {
    pub order_id: String,
    pub trade_index: i64,
    pub kind: String,
    pub fiat_code: String,
    pub fiat_amount: i64,
    pub min_amount: Option<i64>,
    pub max_amount: Option<i64>,
    pub amount: i64,
    pub premium: i64,
    pub payment_method: String,
    pub created_at: i64,
    #[serde(default = "default_role")]
    pub role: String,
    #[serde(default)]
    pub last_dm_action: Option<String>,
}

fn default_role() -> String {
    "creator".to_string()
}

/// Path to the trades file for a given cube name.
fn trades_file_path(cube_name: &str) -> Result<PathBuf, String> {
    let data_dir = dirs::data_dir().ok_or_else(|| "Cannot determine data directory".to_string())?;
    let mostro_dir = data_dir.join("coincube").join("mostro");
    std::fs::create_dir_all(&mostro_dir)
        .map_err(|e| format!("Failed to create mostro data dir: {e}"))?;
    Ok(mostro_dir.join(format!("{cube_name}_trades.json")))
}

fn load_trades(cube_name: &str) -> Vec<TradeSession> {
    let path = match trades_file_path(cube_name) {
        Ok(p) => p,
        Err(_) => return Vec::new(),
    };
    if !path.exists() {
        return Vec::new();
    }
    let data = match std::fs::read(&path) {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };
    serde_json::from_slice(&data).unwrap_or_default()
}

fn save_trades(cube_name: &str, trades: &[TradeSession]) -> Result<(), String> {
    let path = trades_file_path(cube_name)?;
    let bytes = serde_json::to_vec_pretty(trades)
        .map_err(|e| format!("Failed to serialize trades: {e}"))?;
    let dir = path
        .parent()
        .ok_or_else(|| "Trades file has no parent directory".to_string())?;
    let tmp_path = dir.join(format!(".trades_{cube_name}.tmp"));
    let mut tmp_file = std::fs::File::create(&tmp_path)
        .map_err(|e| format!("Failed to create temp trades file: {e}"))?;
    std::io::Write::write_all(&mut tmp_file, &bytes)
        .map_err(|e| format!("Failed to write temp trades file: {e}"))?;
    tmp_file
        .sync_all()
        .map_err(|e| format!("Failed to sync temp trades file: {e}"))?;
    std::fs::rename(&tmp_path, &path)
        .map_err(|e| format!("Failed to rename temp trades file: {e}"))?;
    Ok(())
}

/// Update the last_dm_action for a specific trade on disk.
pub fn update_trade_dm_action(cube_name: &str, order_id: &str, action: &str) {
    let mut trades = load_trades(cube_name);
    if let Some(session) = trades.iter_mut().find(|t| t.order_id == order_id) {
        session.last_dm_action = Some(action.to_string());
        if let Err(e) = save_trades(cube_name, &trades) {
            tracing::warn!("Failed to persist DM action: {e}");
        }
    }
}

fn append_trade(cube_name: &str, session: TradeSession) -> Result<(), String> {
    let mut trades = load_trades(cube_name);
    // Replace existing session for the same order (re-take after cancel uses new keys)
    if let Some(existing) = trades.iter_mut().find(|t| t.order_id == session.order_id) {
        *existing = session;
    } else {
        trades.push(session);
    }
    save_trades(cube_name, &trades)?;
    Ok(())
}

/// Map `mostro_core::order::Status` to UI `TradeStatus`.
fn map_trade_status(status: &mostro_core::order::Status) -> TradeStatus {
    use mostro_core::order::Status;
    match status {
        Status::Pending => TradeStatus::Pending,
        Status::Active => TradeStatus::Active,
        Status::WaitingPayment => TradeStatus::WaitingPayment,
        Status::WaitingBuyerInvoice => TradeStatus::WaitingBuyerInvoice,
        Status::FiatSent => TradeStatus::FiatSent,
        Status::Success | Status::SettledByAdmin | Status::CompletedByAdmin => TradeStatus::Success,
        Status::Canceled | Status::CanceledByAdmin => TradeStatus::Canceled,
        Status::CooperativelyCanceled => TradeStatus::CooperativelyCanceled,
        Status::Dispute => TradeStatus::Dispute,
        Status::Expired => TradeStatus::Expired,
        Status::SettledHoldInvoice | Status::InProgress => TradeStatus::Active,
    }
}

/// Map a DM action string (Debug format of mostro_core::message::Action) to a TradeStatus.
/// This mirrors the mobile client's `_getStatusFromAction()`.
pub fn dm_action_to_status(action: &str) -> Option<TradeStatus> {
    match action {
        "WaitingSellerToPay" | "PayInvoice" => Some(TradeStatus::WaitingPayment),
        "WaitingBuyerInvoice" | "AddInvoice" => Some(TradeStatus::WaitingBuyerInvoice),
        "BuyerTookOrder" | "HoldInvoicePaymentAccepted" => Some(TradeStatus::Active),
        "FiatSent" | "FiatSentOk" => Some(TradeStatus::FiatSent),
        "Released" | "Release" => Some(TradeStatus::Active),
        "PurchaseCompleted" | "Rate" => Some(TradeStatus::Success),
        "Canceled" | "Cancel" | "AdminCanceled" => Some(TradeStatus::Canceled),
        "CooperativeCancelInitiatedByYou" | "CooperativeCancelInitiatedByPeer" => {
            Some(TradeStatus::CooperativelyCanceled)
        }
        "CooperativeCancelAccepted" => Some(TradeStatus::Canceled),
        "DisputeInitiatedByYou" | "DisputeInitiatedByPeer" | "AdminTookDispute" => {
            Some(TradeStatus::Dispute)
        }
        "AdminSettled" => Some(TradeStatus::Success),
        "PaymentFailed" => Some(TradeStatus::WaitingPayment),
        _ => None,
    }
}

/// Derive per-trade Nostr keys (same derivation path as mostrix).
fn derive_trade_keys(mnemonic: &str, trade_index: i64) -> Result<Keys, String> {
    let account: u32 = 38383; // NOSTR_ORDER_EVENT_KIND
    Keys::from_mnemonic_advanced(
        mnemonic,
        None,
        Some(account),
        Some(trade_index as u32),
        Some(0),
    )
    .map_err(|e| format!("Failed to derive trade keys: {e}"))
}

/// Path to the identity file for a given cube name.
fn identity_file_path(cube_name: &str) -> Result<PathBuf, String> {
    let data_dir = dirs::data_dir().ok_or_else(|| "Cannot determine data directory".to_string())?;
    let mostro_dir = data_dir.join("coincube").join("mostro");
    std::fs::create_dir_all(&mostro_dir)
        .map_err(|e| format!("Failed to create mostro data dir: {e}"))?;
    Ok(mostro_dir.join(format!("{cube_name}.json")))
}

/// Load or create a MostroIdentity from disk.
fn load_or_create_identity(cube_name: &str) -> Result<MostroIdentity, String> {
    let path = identity_file_path(cube_name)?;

    // Try to load existing identity
    if path.exists() {
        let data =
            std::fs::read(&path).map_err(|e| format!("Failed to read identity file: {e}"))?;
        if let Ok(identity) = serde_json::from_slice::<MostroIdentity>(&data) {
            return Ok(identity);
        }
    }

    // Generate new mnemonic and store
    let mnemonic = bip39::Mnemonic::generate(12)
        .map_err(|e| format!("Failed to generate mnemonic: {e}"))?
        .to_string();

    let identity = MostroIdentity {
        mnemonic,
        last_trade_index: 0,
    };

    save_identity(cube_name, &identity)?;
    Ok(identity)
}

/// Persist the updated identity to disk.
fn save_identity(cube_name: &str, identity: &MostroIdentity) -> Result<(), String> {
    let path = identity_file_path(cube_name)?;
    let bytes = serde_json::to_vec_pretty(identity)
        .map_err(|e| format!("Failed to serialize identity: {e}"))?;
    std::fs::write(&path, bytes).map_err(|e| format!("Failed to write identity file: {e}"))?;
    Ok(())
}

/// Info fetched from the Mostro info event (kind 38385): order limits and accepted currencies.
#[derive(Default, Clone)]
struct MostroNodeInfo {
    min_order_amount: Option<i64>,
    max_order_amount: Option<i64>,
    fiat_currencies: Vec<String>,
}

/// Fetch the Mostro instance info event and parse order limits + accepted currencies from tags.
async fn fetch_mostro_info(client: &Client, mostro_pubkey: PublicKey) -> MostroNodeInfo {
    let filter = Filter::new()
        .author(mostro_pubkey)
        .kind(Kind::Custom(MOSTRO_INFO_EVENT_KIND))
        .limit(1);

    let mut info = MostroNodeInfo::default();
    if let Ok(events) = client.fetch_events(filter, Duration::from_secs(5)).await {
        if let Some(event) = events.iter().next() {
            for tag in event.tags.iter() {
                let t = tag.clone().to_vec();
                if t.len() < 2 {
                    continue;
                }
                match t[0].as_str() {
                    "max_order_amount" => {
                        info.max_order_amount = t[1].parse::<i64>().ok();
                    }
                    "min_order_amount" => {
                        info.min_order_amount = t[1].parse::<i64>().ok();
                    }
                    "fiat_currencies_accepted" => {
                        info.fiat_currencies = t[1]
                            .split(',')
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect();
                    }
                    _ => {}
                }
            }
        }
    }
    info
}

fn format_sats(sats: i64) -> String {
    // Format with thousand separators
    let s = sats.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}

/// Convert a CantDoReason to a user-friendly error message, with optional limits context.
fn cant_do_description(
    reason: &mostro_core::error::CantDoReason,
    limits: &MostroNodeInfo,
) -> String {
    use mostro_core::error::CantDoReason;
    match reason {
        CantDoReason::InvalidAmount => "Invalid amount — check your order values".into(),
        CantDoReason::OutOfRangeFiatAmount => {
            match (&limits.min_order_amount, &limits.max_order_amount) {
                (Some(min), Some(max)) => format!(
                    "Fiat amount is out of range — Mostro allows {} to {} sats equivalent",
                    format_sats(*min),
                    format_sats(*max),
                ),
                _ => "Fiat amount is out of the acceptable range".into(),
            }
        }
        CantDoReason::OutOfRangeSatsAmount => {
            match (&limits.min_order_amount, &limits.max_order_amount) {
                (Some(min), Some(max)) => format!(
                    "Amount out of range — Mostro allows {} to {} sats",
                    format_sats(*min),
                    format_sats(*max),
                ),
                _ => "Amount is too large — try a smaller fiat amount".into(),
            }
        }
        CantDoReason::InvalidFiatCurrency => {
            "Currency not supported — try specifying a fixed sats amount".into()
        }
        CantDoReason::InvalidParameters => "Invalid parameters — check your order details".into(),
        CantDoReason::PendingOrderExists => {
            "You already have a pending order — complete or cancel it first".into()
        }
        CantDoReason::TooManyRequests => "Too many requests — please wait and try again".into(),
        CantDoReason::InvalidInvoice => "Invalid lightning invoice".into(),
        other => format!("Order rejected: {other:?}"),
    }
}

// ── Reusable Mostro messaging helper ─────────────────────────────────────

/// Send a gift-wrapped message to Mostro and wait for a response.
/// Returns the parsed response message from Mostro.
async fn send_mostro_message(
    mnemonic: &str,
    trade_index: i64,
    mostro_pubkey_hex: &str,
    relay_urls: &[String],
    message: mostro_core::message::Message,
) -> Result<(mostro_core::message::Message, MostroNodeInfo), String> {
    let mostro_pubkey = PublicKey::from_hex(mostro_pubkey_hex)
        .map_err(|e| format!("Invalid Mostro pubkey: {e}"))?;

    let trade_keys = derive_trade_keys(mnemonic, trade_index)?;

    // Build gift wrap content: (Message, Option<Signature>) with None signature
    let content: (mostro_core::message::Message, Option<Signature>) = (message, None);
    let content_json =
        serde_json::to_string(&content).map_err(|e| format!("Failed to serialize content: {e}"))?;

    // Create rumor (unsigned event)
    let rumor = EventBuilder::text_note(content_json).build(trade_keys.public_key());

    // Create gift wrap event
    let gift_wrap_event = EventBuilder::gift_wrap(&trade_keys, &mostro_pubkey, rumor, [])
        .await
        .map_err(|e| format!("Failed to create gift wrap: {e}"))?;

    // Connect to relays
    let client = Client::new(trade_keys.clone());
    for relay_url in relay_urls {
        client
            .add_relay(relay_url.as_str())
            .await
            .map_err(|e| format!("Failed to add relay {relay_url}: {e}"))?;
    }
    client.connect().await;

    // Fetch Mostro instance info (used for contextual error messages)
    let limits = fetch_mostro_info(&client, mostro_pubkey).await;

    // Subscribe BEFORE sending to avoid missing the response
    let mut notifications = client.notifications();
    let sub_filter = Filter::new()
        .pubkey(trade_keys.public_key())
        .kind(nostr_sdk::Kind::GiftWrap)
        .limit(0);
    let opts =
        SubscribeAutoCloseOptions::default().exit_policy(ReqExitPolicy::WaitForEventsAfterEOSE(4));
    client
        .subscribe(sub_filter, Some(opts))
        .await
        .map_err(|e| format!("Failed to subscribe: {e}"))?;

    // Send the gift wrap event
    client
        .send_event(&gift_wrap_event)
        .await
        .map_err(|e| format!("Failed to send message: {e}"))?;

    // Wait for response with 15s timeout
    let response_event = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            match notifications.recv().await {
                Ok(RelayPoolNotification::Event { event, .. }) => {
                    return Ok(*event);
                }
                Ok(_) => continue,
                Err(e) => {
                    return Err(format!("Error receiving notification: {e}"));
                }
            }
        }
    })
    .await
    .map_err(|_| "Timeout waiting for Mostro response (15s)".to_string())?
    .map_err(|e| format!("Error waiting for response: {e}"))?;

    let _ = client.disconnect().await;

    // Unwrap gift wrap response
    let unwrapped = nip59::extract_rumor(&trade_keys, &response_event)
        .await
        .map_err(|e| format!("Failed to unwrap response: {e}"))?;

    if unwrapped.rumor.pubkey != mostro_pubkey {
        return Err(format!(
            "Response sender mismatch: expected {mostro_pubkey}, got {}",
            unwrapped.rumor.pubkey
        ));
    }

    let (response_message, _): (mostro_core::message::Message, Option<String>) =
        serde_json::from_str(&unwrapped.rumor.content)
            .map_err(|e| format!("Failed to parse response: {e}"))?;

    Ok((response_message, limits))
}

// ── Order submission ────────────────────────────────────────────────────

/// Data extracted from the create order form.
pub struct OrderFormData {
    pub kind: mostro_core::order::Kind,
    pub fiat_code: String,
    pub fiat_amount: i64,
    pub min_amount: Option<i64>,
    pub max_amount: Option<i64>,
    pub amount: i64,
    pub premium: i64,
    pub payment_method: String,
    pub cube_name: String,
    pub buyer_invoice: Option<String>,
    pub expiry_days: u32,
    pub mostro_pubkey_hex: String,
    pub relay_urls: Vec<String>,
}

/// Result returned to the UI after order submission.
#[derive(Debug, Clone)]
pub enum OrderSubmitResponse {
    Success { order_id: String },
}

/// Submit an order to Mostro via NIP-59 gift wrap.
pub async fn submit_order(form: OrderFormData) -> Result<OrderSubmitResponse, String> {
    // Load or create identity
    let mut identity = load_or_create_identity(&form.cube_name)?;
    let next_idx = identity.last_trade_index + 1;

    // Expiration based on user-chosen days (0 = no expiration)
    let expires_at = if form.expiry_days > 0 {
        let now = chrono::Utc::now();
        let exp = now + chrono::Duration::days(form.expiry_days as i64);
        Some(exp.timestamp())
    } else {
        None
    };

    // Save copies before they're moved into SmallOrder
    let saved_fiat_code = form.fiat_code.clone();
    let saved_payment_method = form.payment_method.clone();

    // Build SmallOrder
    let small_order = mostro_core::order::SmallOrder::new(
        None,
        Some(form.kind),
        Some(mostro_core::order::Status::Pending),
        form.amount,
        form.fiat_code,
        form.min_amount,
        form.max_amount,
        form.fiat_amount,
        form.payment_method,
        form.premium,
        None,
        None,
        form.buyer_invoice,
        Some(0),
        expires_at,
    );

    // Create Mostro message
    let request_id = uuid::Uuid::new_v4().as_u128() as u64;
    let order_content = mostro_core::message::Payload::Order(small_order);
    let message = mostro_core::message::Message::new_order(
        None,
        Some(request_id),
        Some(next_idx),
        mostro_core::message::Action::NewOrder,
        Some(order_content),
    );

    tracing::info!(
        "Order sent via gift wrap, trade_index={}, request_id={}",
        next_idx,
        request_id
    );

    let (response_message, limits) = send_mostro_message(
        &identity.mnemonic,
        next_idx,
        &form.mostro_pubkey_hex,
        &form.relay_urls,
        message,
    )
    .await?;

    let inner = response_message.get_inner_message_kind();

    // Check for CantDo action (error response)
    if inner.action == mostro_core::message::Action::CantDo {
        let reason = match &inner.payload {
            Some(mostro_core::message::Payload::CantDo(Some(reason))) => {
                cant_do_description(reason, &limits)
            }
            Some(mostro_core::message::Payload::CantDo(None)) => {
                "Order rejected by Mostro".to_string()
            }
            _ => "Order rejected by Mostro".to_string(),
        };
        return Err(reason);
    }

    // Extract order ID from response
    let order_id = match &inner.payload {
        Some(mostro_core::message::Payload::Order(order)) => order
            .id
            .map(|id| id.to_string())
            .unwrap_or_else(|| "unknown".to_string()),
        _ => "unknown".to_string(),
    };

    tracing::info!("Order created successfully, order_id={}", order_id);

    // Update trade index in keyring
    identity.last_trade_index = next_idx;
    save_identity(&form.cube_name, &identity)?;

    // Persist trade session to disk (best-effort — order already succeeded)
    let session = TradeSession {
        order_id: order_id.clone(),
        trade_index: next_idx,
        kind: match form.kind {
            mostro_core::order::Kind::Buy => "buy".to_string(),
            mostro_core::order::Kind::Sell => "sell".to_string(),
        },
        fiat_code: saved_fiat_code,
        fiat_amount: form.fiat_amount,
        min_amount: form.min_amount,
        max_amount: form.max_amount,
        amount: form.amount,
        premium: form.premium,
        payment_method: saved_payment_method,
        created_at: chrono::Utc::now().timestamp(),
        role: "creator".to_string(),
        last_dm_action: None,
    };
    if let Err(e) = append_trade(&form.cube_name, session) {
        tracing::warn!("Failed to persist trade session: {e}");
    }

    Ok(OrderSubmitResponse::Success { order_id })
}

// ── Take order ──────────────────────────────────────────────────────────

/// Data needed to take an existing order from the order book.
pub struct TakeOrderData {
    pub order_id: String,
    pub order_type: OrderType,
    pub cube_name: String,
    pub amount: Option<i64>,
    pub lightning_invoice: Option<String>,
    pub mostro_pubkey_hex: String,
    pub relay_urls: Vec<String>,
}

/// Result returned to the UI after taking an order.
#[derive(Debug, Clone)]
pub enum TakeOrderResponse {
    /// Order taken successfully (buyer taking a sell order).
    Success {
        order_id: String,
        trade_index: i64,
        status: String,
    },
    /// Seller must pay this hold invoice to lock sats (seller taking a buy order).
    PaymentRequired {
        order_id: String,
        trade_index: i64,
        invoice: String,
        amount_sats: Option<i64>,
    },
}

/// Take an existing order from the order book.
pub async fn take_order(data: TakeOrderData) -> Result<TakeOrderResponse, String> {
    let mut identity = load_or_create_identity(&data.cube_name)?;
    let next_idx = identity.last_trade_index + 1;

    let order_uuid =
        uuid::Uuid::parse_str(&data.order_id).map_err(|e| format!("Invalid order ID: {e}"))?;

    // Determine action: TakeSell if we're buying (order is sell), TakeBuy if we're selling
    let action = match data.order_type {
        OrderType::Sell => mostro_core::message::Action::TakeSell,
        OrderType::Buy => mostro_core::message::Action::TakeBuy,
    };

    let request_id = uuid::Uuid::new_v4().as_u128() as u64;

    // Build payload per mostro protocol:
    // TakeSell (buyer): PaymentRequest(None, invoice, amount) or Amount(amount)
    // TakeBuy (seller): Amount(amount) or None
    let payload = match action {
        mostro_core::message::Action::TakeSell => {
            // User is buyer
            match (&data.lightning_invoice, data.amount) {
                (Some(inv), Some(amt)) => Some(mostro_core::message::Payload::PaymentRequest(
                    None,
                    inv.clone(),
                    Some(amt),
                )),
                (Some(inv), None) => Some(mostro_core::message::Payload::PaymentRequest(
                    None,
                    inv.clone(),
                    None,
                )),
                (None, Some(amt)) => Some(mostro_core::message::Payload::Amount(amt)),
                (None, None) => None,
            }
        }
        mostro_core::message::Action::TakeBuy => {
            // User is seller
            data.amount.map(mostro_core::message::Payload::Amount)
        }
        _ => None,
    };

    tracing::info!(
        "Taking order {}, action={:?}, trade_index={}",
        data.order_id,
        action,
        next_idx
    );

    let message = mostro_core::message::Message::new_order(
        Some(order_uuid),
        Some(request_id),
        Some(next_idx),
        action,
        payload,
    );

    let (response_message, limits) = send_mostro_message(
        &identity.mnemonic,
        next_idx,
        &data.mostro_pubkey_hex,
        &data.relay_urls,
        message,
    )
    .await?;

    let inner = response_message.get_inner_message_kind();

    if inner.action == mostro_core::message::Action::CantDo {
        let reason = match &inner.payload {
            Some(mostro_core::message::Payload::CantDo(Some(reason))) => {
                cant_do_description(reason, &limits)
            }
            _ => "Failed to take order".to_string(),
        };
        return Err(reason);
    }

    // Update trade index
    identity.last_trade_index = next_idx;
    save_identity(&data.cube_name, &identity)?;

    // Determine our role's order kind: if we take a sell, we're buying
    let our_kind = match data.order_type {
        OrderType::Sell => "buy",
        OrderType::Buy => "sell",
    };

    let session = TradeSession {
        order_id: data.order_id.clone(),
        trade_index: next_idx,
        kind: our_kind.to_string(),
        fiat_code: String::new(),
        fiat_amount: data.amount.unwrap_or(0),
        min_amount: None,
        max_amount: None,
        amount: 0,
        premium: 0,
        payment_method: String::new(),
        created_at: chrono::Utc::now().timestamp(),
        role: "taker".to_string(),
        last_dm_action: None,
    };
    if let Err(e) = append_trade(&data.cube_name, session) {
        tracing::warn!("Failed to persist take-order session: {e}");
    }

    // Check response payload: PaymentRequest means seller must pay a hold invoice
    match &inner.payload {
        Some(mostro_core::message::Payload::PaymentRequest(_order, invoice, amount)) => {
            tracing::info!(
                "Received PaymentRequest for order {} — seller must pay invoice",
                data.order_id
            );
            Ok(TakeOrderResponse::PaymentRequired {
                order_id: data.order_id,
                trade_index: next_idx,
                invoice: invoice.clone(),
                amount_sats: *amount,
            })
        }
        _ => {
            let status = format!("{:?}", inner.action);
            Ok(TakeOrderResponse::Success {
                order_id: data.order_id,
                trade_index: next_idx,
                status,
            })
        }
    }
}

// ── Trade actions ───────────────────────────────────────────────────────

/// Data needed for trade actions (submit invoice, fiat sent, etc.).
pub struct TradeActionData {
    pub order_id: String,
    pub cube_name: String,
    pub invoice: Option<String>,
    pub mostro_pubkey_hex: String,
    pub relay_urls: Vec<String>,
}

/// Result returned to the UI after a trade action.
#[derive(Debug, Clone)]
pub enum TradeActionResponse {
    Success { new_status: String },
}

/// Submit a lightning invoice for a trade (buyer action when WaitingBuyerInvoice).
pub async fn submit_invoice(data: TradeActionData) -> Result<TradeActionResponse, String> {
    trade_action(data, mostro_core::message::Action::AddInvoice).await
}

/// Confirm fiat has been sent (buyer action).
pub async fn confirm_fiat_sent(data: TradeActionData) -> Result<TradeActionResponse, String> {
    trade_action(data, mostro_core::message::Action::FiatSent).await
}

/// Confirm fiat has been received and release sats (seller action).
pub async fn confirm_fiat_received(data: TradeActionData) -> Result<TradeActionResponse, String> {
    trade_action(data, mostro_core::message::Action::Release).await
}

/// Cancel a trade.
pub async fn cancel_trade(data: TradeActionData) -> Result<TradeActionResponse, String> {
    trade_action(data, mostro_core::message::Action::Cancel).await
}

/// Open a dispute on a trade.
pub async fn open_dispute(data: TradeActionData) -> Result<TradeActionResponse, String> {
    trade_action(data, mostro_core::message::Action::Dispute).await
}

/// Generic trade action helper.
async fn trade_action(
    data: TradeActionData,
    action: mostro_core::message::Action,
) -> Result<TradeActionResponse, String> {
    let sessions = load_trades(&data.cube_name);
    let session = sessions
        .iter()
        .find(|s| s.order_id == data.order_id)
        .ok_or_else(|| format!("No trade session found for order {}", data.order_id))?;

    let identity = load_or_create_identity(&data.cube_name)?;

    let order_uuid =
        uuid::Uuid::parse_str(&data.order_id).map_err(|e| format!("Invalid order ID: {e}"))?;

    let request_id = uuid::Uuid::new_v4().as_u128() as u64;

    let payload = if action == mostro_core::message::Action::AddInvoice {
        let invoice = data
            .invoice
            .ok_or("Invoice is required for AddInvoice action")?;
        let small_order = mostro_core::order::SmallOrder::new(
            Some(order_uuid),
            None,
            None,
            0,
            String::new(),
            None,
            None,
            0,
            String::new(),
            0,
            None,
            None,
            Some(invoice),
            Some(0),
            None,
        );
        Some(mostro_core::message::Payload::Order(small_order))
    } else {
        None
    };

    let action_label = format!("{:?}", action);

    tracing::info!(
        "Trade action {} for order {}, trade_index={}",
        action_label,
        data.order_id,
        session.trade_index
    );

    let message = mostro_core::message::Message::new_order(
        Some(order_uuid),
        Some(request_id),
        Some(session.trade_index),
        action,
        payload,
    );

    let (response_message, limits) = send_mostro_message(
        &identity.mnemonic,
        session.trade_index,
        &data.mostro_pubkey_hex,
        &data.relay_urls,
        message,
    )
    .await?;

    let inner = response_message.get_inner_message_kind();

    if inner.action == mostro_core::message::Action::CantDo {
        let reason = match &inner.payload {
            Some(mostro_core::message::Payload::CantDo(Some(reason))) => {
                cant_do_description(reason, &limits)
            }
            _ => format!("Action {} rejected by Mostro", action_label),
        };
        return Err(reason);
    }

    let new_status = format!("{:?}", inner.action);
    Ok(TradeActionResponse::Success { new_status })
}

// ── Order fetching (existing code) ──────────────────────────────────────

/// Rating data parsed from the event's `rating` tag.
#[derive(Default)]
struct RatingInfo {
    total_rating: Option<f32>,
    total_reviews: Option<u32>,
    days: Option<u32>,
}

/// Parse the rating tag value into RatingInfo.
/// Format: `["rating",{"total_rating":3.0,"total_reviews":1,"days":14}]`
fn parse_rating_tag(json_str: &str) -> RatingInfo {
    let mut info = RatingInfo::default();
    if let Ok(arr) = serde_json::from_str::<serde_json::Value>(json_str) {
        if let Some(data) = arr.get(1) {
            info.total_rating = data["total_rating"].as_f64().map(|f| f as f32);
            info.total_reviews = data["total_reviews"].as_u64().map(|n| n as u32);
            info.days = data["days"].as_u64().map(|n| n as u32);
        }
    }
    info
}

/// Parse Nostr event tags into a SmallOrder + RatingInfo.
fn order_from_tags(tags: &Tags) -> (mostro_core::order::SmallOrder, RatingInfo) {
    let mut order = mostro_core::order::SmallOrder::default();
    let mut rating = RatingInfo::default();

    for tag in tags.iter() {
        let t = tag.clone().to_vec();
        if t.is_empty() {
            continue;
        }

        let key = t[0].as_str();
        let values = &t[1..];
        let v = values.first().map(|s| s.as_str()).unwrap_or_default();

        match key {
            "d" => {
                order.id = uuid::Uuid::parse_str(v).ok();
            }
            "k" => {
                order.kind = v.parse::<mostro_core::order::Kind>().ok();
            }
            "f" => {
                order.fiat_code = v.to_string();
            }
            "s" => {
                order.status = v
                    .parse::<mostro_core::order::Status>()
                    .ok()
                    .or(Some(mostro_core::order::Status::Pending));
            }
            "amt" => {
                order.amount = v.parse::<i64>().unwrap_or(0);
            }
            "fa" => {
                if v.contains('.') {
                    continue;
                }
                if let Some(max_str) = values.get(1) {
                    order.min_amount = v.parse::<i64>().ok();
                    order.max_amount = max_str.parse::<i64>().ok();
                } else {
                    order.fiat_amount = v.parse::<i64>().unwrap_or(0);
                }
            }
            "pm" => {
                order.payment_method = values.join(",");
            }
            "premium" => {
                order.premium = v.parse::<i64>().unwrap_or(0);
            }
            "rating" => {
                rating = parse_rating_tag(v);
            }
            _ => {}
        }
    }

    (order, rating)
}

/// Convert a SmallOrder + rating + event timestamp into a P2POrder for display.
fn small_order_to_p2p_order(
    order: &mostro_core::order::SmallOrder,
    rating: &RatingInfo,
    event_created_at: u64,
) -> Option<P2POrder> {
    let kind = order.kind.as_ref()?;
    let order_type = match kind {
        mostro_core::order::Kind::Buy => OrderType::Buy,
        mostro_core::order::Kind::Sell => OrderType::Sell,
    };

    let is_range = order.min_amount.is_some() && order.max_amount.is_some();
    let fiat_amount = if is_range {
        0.0
    } else {
        order.fiat_amount as f64
    };

    Some(P2POrder {
        id: order.id.map(|u| u.to_string()).unwrap_or_default(),
        order_type,
        fiat_amount,
        fiat_currency: order.fiat_code.clone(),
        min_amount: order.min_amount.map(|v| v as f64),
        max_amount: order.max_amount.map(|v| v as f64),
        sats_amount: if order.amount > 0 {
            Some(order.amount as u64)
        } else {
            None
        },
        premium_percent: Some(order.premium as f64),
        payment_methods: order
            .payment_method
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        seller_rating: rating.total_rating,
        seller_reviews: rating.total_reviews,
        seller_days_old: rating.days,
        created_at: String::new(),
        created_at_ts: event_created_at,
        time_ago: format_time_ago(event_created_at),
        is_mine: false,
    })
}

/// Format a unix timestamp into a relative time string like "5 min ago".
fn format_time_ago(timestamp: u64) -> String {
    let now = chrono::Utc::now().timestamp() as u64;
    let diff = now.saturating_sub(timestamp);

    if diff < 60 {
        "just now".to_string()
    } else if diff < 3600 {
        let mins = diff / 60;
        format!("{} min ago", mins)
    } else if diff < 86400 {
        let hours = diff / 3600;
        format!("{}h ago", hours)
    } else {
        let days = diff / 86400;
        format!("{}d ago", days)
    }
}

/// Fetch orders from the Mostro relay. Returns parsed pending orders.
async fn fetch_mostro_orders(
    client: &Client,
    mostro_pubkey: PublicKey,
    cube_name: &str,
) -> Vec<P2POrder> {
    let since = Timestamp::from(chrono::Utc::now().timestamp() as u64 - ORDER_LOOKBACK_SECS);
    let filter = Filter::new()
        .author(mostro_pubkey)
        .kind(Kind::Custom(38383))
        .since(since);

    let events = match client.fetch_events(filter, Duration::from_secs(15)).await {
        Ok(events) => events,
        Err(e) => {
            tracing::error!("Failed to fetch Mostro events: {}", e);
            return Vec::new();
        }
    };

    // Deduplicate by order ID, keeping the latest event per UUID.
    let mut latest_events: BTreeMap<String, (u64, mostro_core::order::SmallOrder, RatingInfo)> =
        BTreeMap::new();

    for event in events.iter() {
        let (order, rating) = order_from_tags(&event.tags);

        // Only keep pending orders
        if order.status != Some(mostro_core::order::Status::Pending) {
            continue;
        }

        let order_id = match &order.id {
            Some(id) => id.to_string(),
            None => continue,
        };

        let created_at = event.created_at.as_u64();
        match latest_events.get(&order_id) {
            Some((existing_ts, _, _)) if *existing_ts >= created_at => {}
            _ => {
                latest_events.insert(order_id, (created_at, order, rating));
            }
        }
    }

    let sessions = load_trades(cube_name);
    let my_order_ids: std::collections::HashSet<&str> = sessions
        .iter()
        .filter(|s| s.role == "creator")
        .map(|s| s.order_id.as_str())
        .collect();

    let mut orders: Vec<P2POrder> = latest_events
        .into_values()
        .filter_map(|(created_at, order, rating)| {
            let mut p2p = small_order_to_p2p_order(&order, &rating, created_at)?;
            p2p.is_mine = my_order_ids.contains(p2p.id.as_str());
            Some(p2p)
        })
        .collect();
    orders.sort_by(|a, b| b.created_at_ts.cmp(&a.created_at_ts));
    orders
}

/// Fetch the user's trades by loading saved sessions and querying the relay for live status.
async fn fetch_user_trades(
    client: &Client,
    cube_name: &str,
    mostro_pubkey: PublicKey,
) -> Vec<P2PTrade> {
    let sessions = load_trades(cube_name);
    if sessions.is_empty() {
        return Vec::new();
    }

    let order_ids: Vec<String> = sessions.iter().map(|s| s.order_id.clone()).collect();
    let filter = Filter::new()
        .author(mostro_pubkey)
        .kind(Kind::Custom(38383))
        .identifiers(order_ids);

    let events = match client.fetch_events(filter, Duration::from_secs(15)).await {
        Ok(events) => events,
        Err(e) => {
            tracing::error!("Failed to fetch trade events: {e}");
            return sessions_to_fallback_trades(&sessions);
        }
    };

    // Deduplicate events by order ID, keeping the latest
    let mut latest_events: BTreeMap<String, (u64, mostro_core::order::SmallOrder)> =
        BTreeMap::new();
    for event in events.iter() {
        let (order, _) = order_from_tags(&event.tags);
        let order_id = match &order.id {
            Some(id) => id.to_string(),
            None => continue,
        };
        let created_at = event.created_at.as_u64();
        match latest_events.get(&order_id) {
            Some((existing_ts, _)) if *existing_ts >= created_at => {}
            _ => {
                latest_events.insert(order_id, (created_at, order));
            }
        }
    }

    // Build P2PTrade for each session
    let mut trades: Vec<P2PTrade> = Vec::new();
    for session in &sessions {
        if let Some((event_ts, order)) = latest_events.get(&session.order_id) {
            // Live data from relay, but DM action takes priority
            let relay_status = order
                .status
                .as_ref()
                .map(map_trade_status)
                .unwrap_or(TradeStatus::Pending);
            let status = session
                .last_dm_action
                .as_deref()
                .and_then(dm_action_to_status)
                .unwrap_or(relay_status);
            let order_type = match session.kind.as_str() {
                "buy" => OrderType::Buy,
                _ => OrderType::Sell,
            };
            let fiat_amount = if order.fiat_amount > 0 {
                order.fiat_amount as f64
            } else {
                session.fiat_amount as f64
            };
            trades.push(P2PTrade {
                id: session.order_id.clone(),
                order_type,
                status,
                role: role_from_session(session),
                fiat_amount,
                fiat_currency: order.fiat_code.clone(),
                sats_amount: if order.amount > 0 {
                    Some(order.amount as u64)
                } else {
                    None
                },
                premium_percent: Some(order.premium as f64),
                payment_method: order.payment_method.clone(),
                counterparty_rating: None,
                created_at: String::new(),
                time_ago: format_time_ago(*event_ts),
                last_dm_action: session.last_dm_action.clone(),
            });
        } else {
            // Not found on relay — fallback from saved session
            trades.push(trade_from_session(session));
        }
    }

    // Sort newest first
    trades.sort_by(|a, b| b.time_ago.cmp(&a.time_ago));
    trades
}

/// Build fallback trades from saved sessions (e.g. when relay is unreachable).
fn sessions_to_fallback_trades(sessions: &[TradeSession]) -> Vec<P2PTrade> {
    let mut trades: Vec<P2PTrade> = sessions.iter().map(trade_from_session).collect();
    trades.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    trades
}

fn role_from_session(session: &TradeSession) -> TradeRole {
    match session.role.as_str() {
        "taker" => TradeRole::Taker,
        _ => TradeRole::Creator,
    }
}

/// Build a P2PTrade from a saved session, using DM action for status if available.
fn trade_from_session(session: &TradeSession) -> P2PTrade {
    let order_type = match session.kind.as_str() {
        "buy" => OrderType::Buy,
        _ => OrderType::Sell,
    };
    let fiat_amount = if session.fiat_amount > 0 {
        session.fiat_amount as f64
    } else {
        // Range order — show min as the display amount
        session.min_amount.unwrap_or(0) as f64
    };
    let status = session
        .last_dm_action
        .as_deref()
        .and_then(dm_action_to_status)
        .unwrap_or(TradeStatus::Pending);
    P2PTrade {
        id: session.order_id.clone(),
        order_type,
        status,
        role: role_from_session(session),
        fiat_amount,
        fiat_currency: session.fiat_code.clone(),
        sats_amount: if session.amount > 0 {
            Some(session.amount as u64)
        } else {
            None
        },
        premium_percent: Some(session.premium as f64),
        payment_method: session.payment_method.clone(),
        counterparty_rating: None,
        created_at: String::new(),
        time_ago: format_time_ago(session.created_at as u64),
        last_dm_action: session.last_dm_action.clone(),
    }
}

/// Iced subscription that periodically fetches orders and trades from the Mostro relay.
/// When any of the three values change, iced detects a hash change and restarts the subscription.
pub fn mostro_subscription(
    cube_name: String,
    active_pubkey: String,
    relays: Vec<String>,
) -> Subscription<Message> {
    Subscription::run_with((cube_name, active_pubkey, relays), mostro_stream)
}

fn mostro_stream(
    params: &(String, String, Vec<String>),
) -> impl iced::futures::Stream<Item = Message> + 'static {
    let cube_name = params.0.clone();
    let pubkey_hex = params.1.clone();
    let relay_urls = params.2.clone();
    iced::stream::channel(
        32,
        move |mut output: iced::futures::channel::mpsc::Sender<Message>| async move {
            let mostro_pk = match PublicKey::from_hex(&pubkey_hex) {
                Ok(pk) => pk,
                Err(e) => {
                    tracing::error!("Invalid Mostro pubkey: {e}");
                    return;
                }
            };

            // Create a single persistent client for the stream lifetime
            let client = Client::new(Keys::generate());
            for url in &relay_urls {
                let _ = client.add_relay(url.as_str()).await;
            }
            client.connect().await;

            // Track last seen DM timestamps per order to avoid reprocessing
            let mut last_dm_ts: std::collections::HashMap<String, u64> =
                std::collections::HashMap::new();
            // Track which trade pubkeys we're already subscribed to for DMs
            let mut subscribed_trade_pubkeys: Vec<PublicKey> = Vec::new();
            // First iteration: replay historical DMs silently (persist state, no toasts)
            let mut first_run = true;

            // Initial fetch of node info
            let info = fetch_mostro_info(&client, mostro_pk).await;
            if !info.fiat_currencies.is_empty() {
                let msg = Message::View(view::Message::P2P(P2PMessage::MostroNodeInfoReceived {
                    currencies: info.fiat_currencies,
                }));
                let _ = output.send(msg).await;
            }

            loop {
                // Subscribe to gift-wrap DMs for any new trade pubkeys
                update_dm_subscriptions(&client, &cube_name, &mut subscribed_trade_pubkeys).await;

                // Process DMs: first run is silent (state recovery only), subsequent runs send toasts
                process_dm_notifications(
                    &client,
                    &cube_name,
                    &mut last_dm_ts,
                    &mut output,
                    first_run,
                )
                .await;

                // Fetch orders and trades using the persistent client
                let orders = fetch_mostro_orders(&client, mostro_pk, &cube_name).await;
                let msg =
                    Message::View(view::Message::P2P(P2PMessage::MostroOrdersReceived(orders)));
                let _ = output.send(msg).await;

                let trades = fetch_user_trades(&client, &cube_name, mostro_pk).await;
                let msg =
                    Message::View(view::Message::P2P(P2PMessage::MostroTradesReceived(trades)));
                let _ = output.send(msg).await;

                first_run = false;
                tokio::time::sleep(Duration::from_secs(FETCH_INTERVAL_SECS)).await;
            }
        },
    )
}

/// Subscribe to gift-wrap DMs for any new trade pubkeys not yet subscribed.
/// Uses ORDER_LOOKBACK_SECS (48h) so DMs survive app restarts and cache clears.
async fn update_dm_subscriptions(
    client: &Client,
    cube_name: &str,
    subscribed_pubkeys: &mut Vec<PublicKey>,
) {
    let sessions = load_trades(cube_name);
    if sessions.is_empty() {
        return;
    }

    let identity = match load_or_create_identity(cube_name) {
        Ok(id) => id,
        Err(_) => return,
    };

    let mut new_pubkeys: Vec<PublicKey> = Vec::new();
    for session in &sessions {
        if let Ok(keys) = derive_trade_keys(&identity.mnemonic, session.trade_index) {
            let pk = keys.public_key();
            if !subscribed_pubkeys.contains(&pk) {
                new_pubkeys.push(pk);
            }
        }
    }

    if new_pubkeys.is_empty() {
        return;
    }

    let since = Timestamp::from(chrono::Utc::now().timestamp() as u64 - ORDER_LOOKBACK_SECS);
    let filter = Filter::new()
        .pubkeys(new_pubkeys.clone())
        .kind(nostr_sdk::Kind::GiftWrap)
        .since(since);

    if let Err(e) = client.subscribe(filter, None).await {
        tracing::debug!("Failed to subscribe for trade DMs: {e}");
        return;
    }

    subscribed_pubkeys.extend(new_pubkeys);
}

/// Process any pending DM notifications from the persistent subscription.
/// When `silent` is true, DM actions are persisted to disk but no TradeUpdate messages
/// are sent to the UI (no toasts). Used on first run for state recovery.
async fn process_dm_notifications(
    client: &Client,
    cube_name: &str,
    last_dm_ts: &mut std::collections::HashMap<String, u64>,
    output: &mut iced::futures::channel::mpsc::Sender<Message>,
    silent: bool,
) {
    let sessions = load_trades(cube_name);
    if sessions.is_empty() {
        return;
    }

    let identity = match load_or_create_identity(cube_name) {
        Ok(id) => id,
        Err(_) => return,
    };

    let mut session_keys: Vec<(TradeSession, Keys)> = Vec::new();
    for session in &sessions {
        if let Ok(keys) = derive_trade_keys(&identity.mnemonic, session.trade_index) {
            session_keys.push((session.clone(), keys));
        }
    }
    if session_keys.is_empty() {
        return;
    }

    // Fetch gift-wrap events using the same lookback as orders (48h) so DMs survive restarts
    let trade_pubkeys: Vec<PublicKey> = session_keys.iter().map(|(_, k)| k.public_key()).collect();
    let since = Timestamp::from(chrono::Utc::now().timestamp() as u64 - ORDER_LOOKBACK_SECS);
    let filter = Filter::new()
        .pubkeys(trade_pubkeys)
        .kind(nostr_sdk::Kind::GiftWrap)
        .since(since);

    let events = match client.fetch_events(filter, Duration::from_secs(5)).await {
        Ok(events) => events,
        Err(e) => {
            tracing::debug!("DM fetch failed: {e}");
            return;
        }
    };

    for event in events.iter() {
        let event_ts = event.created_at.as_u64();

        for (session, keys) in &session_keys {
            if let Some(&last_ts) = last_dm_ts.get(&session.order_id) {
                if event_ts <= last_ts {
                    continue;
                }
            }

            let is_for_us = event.tags.iter().any(|tag| {
                let t = tag.clone().to_vec();
                t.len() >= 2 && t[0] == "p" && t[1] == keys.public_key().to_hex()
            });
            if !is_for_us {
                continue;
            }

            if let Ok(unwrapped) = nip59::extract_rumor(keys, event).await {
                if let Ok((msg, _)) = serde_json::from_str::<(
                    mostro_core::message::Message,
                    Option<String>,
                )>(&unwrapped.rumor.content)
                {
                    let inner = msg.get_inner_message_kind();
                    let action = format!("{:?}", inner.action);
                    let payload_json = serde_json::to_string(&inner.payload).unwrap_or_default();

                    last_dm_ts.insert(session.order_id.clone(), event_ts);

                    // Persist the DM action to disk
                    update_trade_dm_action(cube_name, &session.order_id, &action);

                    // Only send UI updates (toasts) for new DMs, not historical replay
                    if !silent {
                        let update_msg =
                            Message::View(view::Message::P2P(P2PMessage::TradeUpdate {
                                order_id: session.order_id.clone(),
                                action,
                                payload_json,
                            }));
                        let _ = output.send(update_msg).await;
                    }
                }
            }
        }
    }
}
