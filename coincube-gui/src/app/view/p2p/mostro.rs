use std::collections::{BTreeMap, HashMap, HashSet};
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

/// All Mostro state persisted to a single file per cube.
#[derive(Serialize, Deserialize, Default)]
struct MostroData {
    last_trade_index: i64,
    #[serde(default)]
    trades: Vec<TradeSession>,
}

/// A single DM message from Mostro, persisted for state reconstruction.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TradeMessage {
    pub timestamp: u64,
    pub action: String,
    pub payload_json: String,
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
    pub messages: Vec<TradeMessage>,
}

fn default_role() -> String {
    "creator".to_string()
}

/// Sanitize a cube name so it is safe to use as a filename component.
/// Replaces any character that isn't alphanumeric, hyphen, or underscore with '_',
/// and rejects empty / dot-only results to prevent path traversal.
fn safe_filename(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() || sanitized.trim_matches('.').is_empty() {
        "default".to_string()
    } else {
        sanitized
    }
}

/// Path to the single Mostro data file for a given cube name.
fn data_file_path(cube_name: &str) -> Result<PathBuf, String> {
    Ok(super::mostro_dir()?.join(format!("{}_mostro.json", safe_filename(cube_name))))
}

fn load_data(cube_name: &str) -> MostroData {
    let path = match data_file_path(cube_name) {
        Ok(p) => p,
        Err(_) => return MostroData::default(),
    };
    if !path.exists() {
        return MostroData::default();
    }
    let data = match std::fs::read(&path) {
        Ok(d) => d,
        Err(_) => return MostroData::default(),
    };
    serde_json::from_slice(&data).unwrap_or_default()
}

fn save_data(cube_name: &str, data: &MostroData) -> Result<(), String> {
    let path = data_file_path(cube_name)?;
    let bytes = serde_json::to_vec_pretty(data)
        .map_err(|e| format!("Failed to serialize mostro data: {e}"))?;
    let dir = path
        .parent()
        .ok_or_else(|| "Data file has no parent directory".to_string())?;
    let tmp_path = dir.join(format!(".mostro_{}.tmp", safe_filename(cube_name)));
    let mut tmp_file =
        std::fs::File::create(&tmp_path).map_err(|e| format!("Failed to create temp file: {e}"))?;
    std::io::Write::write_all(&mut tmp_file, &bytes)
        .map_err(|e| format!("Failed to write temp file: {e}"))?;
    tmp_file
        .sync_all()
        .map_err(|e| format!("Failed to sync temp file: {e}"))?;
    std::fs::rename(&tmp_path, &path).map_err(|e| format!("Failed to rename temp file: {e}"))?;
    Ok(())
}

/// Append a DM message to a trade's message history on disk.
/// Deduplicates by (timestamp, action) to avoid storing the same message twice.
pub fn append_trade_message(cube_name: &str, order_id: &str, msg: TradeMessage) {
    let mut data = load_data(cube_name);
    if let Some(session) = data.trades.iter_mut().find(|t| t.order_id == order_id) {
        let is_dup = session
            .messages
            .iter()
            .any(|m| m.timestamp == msg.timestamp && m.action == msg.action);
        if !is_dup {
            session.messages.push(msg);
            session.messages.sort_by_key(|m| m.timestamp);
            if let Err(e) = save_data(cube_name, &data) {
                tracing::warn!("Failed to persist trade message: {e}");
            }
        }
    }
}

/// Get the latest DM action for a trade from its message history.
pub fn latest_dm_action(session: &TradeSession) -> Option<&str> {
    session.messages.last().map(|m| m.action.as_str())
}

/// Find the timestamp of the DM that started the current countdown phase.
/// For WaitingBuyerInvoice status → find the AddInvoice/WaitingBuyerInvoice message
/// For WaitingPayment status → find the PayInvoice/WaitingSellerToPay message
pub fn countdown_start_timestamp(session: &TradeSession) -> Option<u64> {
    let last_action = session.messages.last().map(|m| m.action.as_str())?;

    let target_actions: &[&str] = match last_action {
        "AddInvoice" | "WaitingBuyerInvoice" => &["AddInvoice", "WaitingBuyerInvoice"],
        "PayInvoice" | "WaitingSellerToPay" => &["PayInvoice", "WaitingSellerToPay"],
        _ => return None,
    };

    // Find the earliest message with a matching action (the one that started this phase)
    session
        .messages
        .iter()
        .rev()
        .filter(|m| target_actions.contains(&m.action.as_str()) && m.timestamp > 0)
        .last()
        .map(|m| m.timestamp)
}

/// Restore trades from Mostro using the protocol-level restore flow:
/// 1. Send RestoreSession → get order IDs + trade indices
/// 2. Send Orders request → get full order details
/// 3. Send LastTradeIndex → get the correct last index
/// Returns the number of trades recovered.
pub async fn restore_trades(
    cube_name: &str,
    mnemonic: &str,
    mostro_pubkey_hex: &str,
    relay_urls: &[String],
) -> Result<usize, String> {
    // Use temp trade key at index 1 for all restore requests (same as mobile)
    let temp_trade_index: i64 = 1;

    tracing::info!("Restore: sending RestoreSession request");

    // Step 1: Request restore data from Mostro
    let restore_msg = mostro_core::message::Message::new_restore(None);
    let (restore_response, _) = send_mostro_message(
        mnemonic,
        temp_trade_index,
        mostro_pubkey_hex,
        relay_urls,
        restore_msg,
    )
    .await?;

    let restore_inner = restore_response.get_inner_message_kind();

    // CantDo means no orders found for this user
    if restore_inner.action == mostro_core::message::Action::CantDo {
        tracing::info!("Restore: Mostro returned CantDo — no orders found");
        return Ok(0);
    }

    // Extract order list + trade indices from RestoreData payload
    let restore_info = match &restore_inner.payload {
        Some(mostro_core::message::Payload::RestoreData(info)) => info.clone(),
        _ => {
            return Err(format!(
                "Restore: unexpected response payload: {:?}",
                restore_inner.action
            ));
        }
    };

    if restore_info.restore_orders.is_empty() {
        tracing::info!("Restore: no orders in restore data");
        return Ok(0);
    }

    tracing::info!(
        "Restore: received {} orders, {} disputes",
        restore_info.restore_orders.len(),
        restore_info.restore_disputes.len(),
    );

    // Build order_id → trade_index map
    let mut order_map: HashMap<String, i64> = HashMap::new();
    for order_info in &restore_info.restore_orders {
        order_map.insert(order_info.order_id.to_string(), order_info.trade_index);
    }
    // Include disputed orders too
    for dispute_info in &restore_info.restore_disputes {
        order_map.insert(dispute_info.order_id.to_string(), dispute_info.trade_index);
    }

    // Step 2: Request full order details
    let order_uuids: Vec<uuid::Uuid> = order_map
        .keys()
        .filter_map(|id| uuid::Uuid::parse_str(id).ok())
        .collect();

    tracing::info!(
        "Restore: requesting details for {} orders",
        order_uuids.len()
    );

    let request_id = uuid::Uuid::new_v4().as_u128() as u64;
    let orders_msg = mostro_core::message::Message::new_order(
        None,
        Some(request_id),
        Some(temp_trade_index),
        mostro_core::message::Action::Orders,
        Some(mostro_core::message::Payload::Ids(order_uuids)),
    );
    let orders_response = send_mostro_message(
        mnemonic,
        temp_trade_index,
        mostro_pubkey_hex,
        relay_urls,
        orders_msg,
    )
    .await;

    // Parse order details (best-effort — restore can still work without full details)
    let mut order_details: HashMap<String, mostro_core::order::SmallOrder> = HashMap::new();
    if let Ok((resp, _)) = &orders_response {
        let inner = resp.get_inner_message_kind();
        if let Some(mostro_core::message::Payload::Orders(orders)) = &inner.payload {
            for order in orders {
                if let Some(id) = &order.id {
                    order_details.insert(id.to_string(), order.clone());
                }
            }
            tracing::info!(
                "Restore: received details for {} orders",
                order_details.len()
            );
        }
    } else {
        tracing::warn!("Restore: failed to fetch order details, continuing with partial data");
    }

    // Step 3: Request last trade index
    tracing::info!("Restore: requesting last trade index");
    let last_index_kind = mostro_core::message::MessageKind::new(
        None,
        None,
        None,
        mostro_core::message::Action::LastTradeIndex,
        None,
    );
    let last_index_msg = mostro_core::message::Message::Restore(last_index_kind);
    let last_trade_index = match send_mostro_message(
        mnemonic,
        temp_trade_index,
        mostro_pubkey_hex,
        relay_urls,
        last_index_msg,
    )
    .await
    {
        Ok((resp, _)) => {
            let inner = resp.get_inner_message_kind();
            let idx = inner.trade_index();
            tracing::info!("Restore: last trade index from Mostro = {}", idx);
            idx
        }
        Err(e) => {
            // Fall back to highest index from restored orders
            let fallback = order_map.values().copied().max().unwrap_or(0);
            tracing::warn!(
                "Restore: failed to get last trade index ({e}), using fallback={fallback}"
            );
            fallback
        }
    };

    // Step 4: Build sessions from restore data + order details
    let mut sessions: Vec<TradeSession> = Vec::new();

    for (order_id, trade_index) in &order_map {
        let details = order_details.get(order_id);

        let kind = details
            .and_then(|d| {
                d.kind.as_ref().map(|k| match k {
                    mostro_core::order::Kind::Buy => "buy".to_string(),
                    mostro_core::order::Kind::Sell => "sell".to_string(),
                })
            })
            .unwrap_or_else(|| "unknown".to_string());

        // Try to determine role from order's trade pubkeys
        let trade_keys = derive_trade_keys(mnemonic, *trade_index).ok();
        let our_pubkey = trade_keys.as_ref().map(|k| k.public_key().to_hex());
        let role = if let (Some(d), Some(ref pk)) = (details, &our_pubkey) {
            if d.buyer_trade_pubkey.as_deref() == Some(pk) {
                "taker" // We're the buyer, likely took a sell order
            } else if d.seller_trade_pubkey.as_deref() == Some(pk) {
                "taker" // We're the seller, likely took a buy order
            } else {
                "creator"
            }
        } else {
            "unknown"
        };

        // Get status from restore info
        let status_str = restore_info
            .restore_orders
            .iter()
            .find(|o| o.order_id.to_string() == *order_id)
            .map(|o| o.status.clone());

        // Build initial message from the status if available
        let messages = if let Some(status) = &status_str {
            vec![TradeMessage {
                timestamp: chrono::Utc::now().timestamp() as u64,
                action: status.clone(),
                payload_json: String::new(),
            }]
        } else {
            Vec::new()
        };

        sessions.push(TradeSession {
            order_id: order_id.clone(),
            trade_index: *trade_index,
            kind,
            fiat_code: details.map(|d| d.fiat_code.clone()).unwrap_or_default(),
            fiat_amount: details.map(|d| d.fiat_amount).unwrap_or(0),
            min_amount: details.and_then(|d| d.min_amount),
            max_amount: details.and_then(|d| d.max_amount),
            amount: details.map(|d| d.amount).unwrap_or(0),
            premium: details.map(|d| d.premium).unwrap_or(0),
            payment_method: details
                .map(|d| d.payment_method.clone())
                .unwrap_or_default(),
            created_at: details
                .and_then(|d| d.created_at)
                .unwrap_or_else(|| chrono::Utc::now().timestamp()),
            role: role.to_string(),
            messages,
        });
    }

    let count = sessions.len();
    let data = MostroData {
        last_trade_index: last_trade_index,
        trades: sessions,
    };
    save_data(cube_name, &data)?;

    tracing::info!(
        "Restore: recovered {} trades, last_trade_index={}",
        count,
        last_trade_index
    );

    Ok(count)
}

fn append_trade(cube_name: &str, session: TradeSession) -> Result<(), String> {
    let mut data = load_data(cube_name);
    // Replace existing session for the same order (re-take after cancel uses new keys)
    if let Some(existing) = data
        .trades
        .iter_mut()
        .find(|t| t.order_id == session.order_id)
    {
        *existing = session;
    } else {
        data.trades.push(session);
    }
    save_data(cube_name, &data)?;
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
        "BuyerTookOrder" | "HoldInvoicePaymentAccepted" | "BuyerInvoiceAccepted" => {
            Some(TradeStatus::Active)
        }
        "FiatSent" | "FiatSentOk" => Some(TradeStatus::FiatSent),
        "Released" | "Release" => Some(TradeStatus::SettledHoldInvoice),
        "PurchaseCompleted" | "Rate" | "RateReceived" | "HoldInvoicePaymentSettled" => {
            Some(TradeStatus::Success)
        }
        "Canceled" | "Cancel" | "AdminCanceled" | "HoldInvoicePaymentCanceled" => {
            Some(TradeStatus::Canceled)
        }
        "CooperativeCancelInitiatedByYou" | "CooperativeCancelInitiatedByPeer" => {
            Some(TradeStatus::CooperativelyCanceled)
        }
        "CooperativeCancelAccepted" => Some(TradeStatus::Canceled),
        "DisputeInitiatedByYou" | "DisputeInitiatedByPeer" | "AdminTookDispute" => {
            Some(TradeStatus::Dispute)
        }
        "AdminSettled" => Some(TradeStatus::Success),
        "PaymentFailed" => Some(TradeStatus::PaymentFailed),
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

use super::components::format_with_separators;

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
                    format_with_separators(*min as u64),
                    format_with_separators(*max as u64),
                ),
                _ => "Fiat amount is out of the acceptable range".into(),
            }
        }
        CantDoReason::OutOfRangeSatsAmount => {
            match (&limits.min_order_amount, &limits.max_order_amount) {
                (Some(min), Some(max)) => format!(
                    "Amount out of range — Mostro allows {} to {} sats",
                    format_with_separators(*min as u64),
                    format_with_separators(*max as u64),
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
        CantDoReason::InvalidOrderStatus => {
            "Action not allowed — order is not in the expected status".into()
        }
        CantDoReason::IsNotYourOrder => "This order does not belong to you".into(),
        CantDoReason::NotAllowedByStatus => "Action not allowed at this stage of the trade".into(),
        CantDoReason::NotFound => "Order not found on Mostro".into(),
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
    let master_keys = derive_trade_keys(mnemonic, 0)?;

    // Build gift wrap content with trade-key signature for identity proof
    let msg_json =
        serde_json::to_string(&message).map_err(|e| format!("Failed to serialize message: {e}"))?;
    let msg_hash = bitcoin_hashes::sha256::Hash::hash(msg_json.as_bytes());
    let secp_msg = nostr_sdk::secp256k1::Message::from_digest(msg_hash.to_byte_array());
    let sig = trade_keys.sign_schnorr(&secp_msg);
    let content: (mostro_core::message::Message, Option<Signature>) = (message, Some(sig));
    let content_json =
        serde_json::to_string(&content).map_err(|e| format!("Failed to serialize content: {e}"))?;

    // Rumor uses trade_keys pubkey (per-trade identity)
    let rumor = EventBuilder::text_note(content_json).build(trade_keys.public_key());

    // Gift wrap uses master_keys for the seal layer, so Mostro can link
    // all trades from the same user (enables protocol-level restore)
    let gift_wrap_event = EventBuilder::gift_wrap(&master_keys, &mostro_pubkey, rumor, [])
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
    pub mnemonic: String,
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
    let mut data = load_data(&form.cube_name);
    let next_idx = data.last_trade_index + 1;

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
        &form.mnemonic,
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

    // Update trade index
    data.last_trade_index = next_idx;
    save_data(&form.cube_name, &data)?;

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
        messages: Vec::new(),
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
    pub mnemonic: String,
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
    let mut mdata = load_data(&data.cube_name);
    let next_idx = mdata.last_trade_index + 1;

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
        &data.mnemonic,
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
    mdata.last_trade_index = next_idx;
    save_data(&data.cube_name, &mdata)?;

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
        messages: Vec::new(),
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
    pub mnemonic: String,
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

/// Rate the counterparty after a successful trade.
pub async fn rate_counterparty(
    data: TradeActionData,
    rating: u8,
) -> Result<TradeActionResponse, String> {
    let sessions = load_data(&data.cube_name).trades;
    let session = sessions
        .iter()
        .find(|s| s.order_id == data.order_id)
        .ok_or_else(|| format!("No trade session found for order {}", data.order_id))?;

    let order_uuid =
        uuid::Uuid::parse_str(&data.order_id).map_err(|e| format!("Invalid order ID: {e}"))?;

    let request_id = uuid::Uuid::new_v4().as_u128() as u64;
    let payload = Some(mostro_core::message::Payload::RatingUser(rating));

    let message = mostro_core::message::Message::new_order(
        Some(order_uuid),
        Some(request_id),
        Some(session.trade_index),
        mostro_core::message::Action::RateUser,
        payload,
    );

    let (response_message, limits) = send_mostro_message(
        &data.mnemonic,
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
            _ => "Rating rejected by Mostro".to_string(),
        };
        return Err(reason);
    }

    let new_status = format!("{:?}", inner.action);
    Ok(TradeActionResponse::Success { new_status })
}

/// Generic trade action helper.
async fn trade_action(
    data: TradeActionData,
    action: mostro_core::message::Action,
) -> Result<TradeActionResponse, String> {
    let sessions = load_data(&data.cube_name).trades;
    let session = sessions
        .iter()
        .find(|s| s.order_id == data.order_id)
        .ok_or_else(|| format!("No trade session found for order {}", data.order_id))?;

    let order_uuid =
        uuid::Uuid::parse_str(&data.order_id).map_err(|e| format!("Invalid order ID: {e}"))?;

    let request_id = uuid::Uuid::new_v4().as_u128() as u64;

    let payload = if action == mostro_core::message::Action::AddInvoice {
        let invoice = data
            .invoice
            .ok_or("Invoice is required for AddInvoice action")?;
        Some(mostro_core::message::Payload::PaymentRequest(
            None,
            invoice,
            Some(0),
        ))
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
        &data.mnemonic,
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

    let sessions = load_data(cube_name).trades;
    let my_order_ids: HashSet<&str> = sessions
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
    let sessions = load_data(cube_name).trades;
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
            let status = latest_dm_action(session)
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
                created_at_ts: *event_ts as i64,
                created_at: String::new(),
                time_ago: format_time_ago(*event_ts),
                last_dm_action: latest_dm_action(session).map(String::from),
                countdown_start_ts: countdown_start_timestamp(session),
            });
        } else {
            // Not found on relay — fallback from saved session
            trades.push(trade_from_session(session));
        }
    }

    // Sort newest first
    trades.sort_by_key(|t| std::cmp::Reverse(t.created_at_ts));
    trades
}

/// Build fallback trades from saved sessions (e.g. when relay is unreachable).
fn sessions_to_fallback_trades(sessions: &[TradeSession]) -> Vec<P2PTrade> {
    let mut trades: Vec<P2PTrade> = sessions.iter().map(trade_from_session).collect();
    trades.sort_by_key(|t| std::cmp::Reverse(t.created_at_ts));
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
    let status = latest_dm_action(session)
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
        created_at_ts: session.created_at,
        created_at: String::new(),
        time_ago: format_time_ago(session.created_at as u64),
        last_dm_action: latest_dm_action(session).map(String::from),
        countdown_start_ts: countdown_start_timestamp(session),
    }
}

/// Iced subscription that periodically fetches orders and trades from the Mostro relay.
/// When any of the three values change, iced detects a hash change and restarts the subscription.
pub fn mostro_subscription(
    cube_name: String,
    mnemonic: String,
    active_pubkey: String,
    relays: Vec<String>,
) -> Subscription<Message> {
    Subscription::run_with((cube_name, mnemonic, active_pubkey, relays), mostro_stream)
}

fn mostro_stream(
    params: &(String, String, String, Vec<String>),
) -> impl iced::futures::Stream<Item = Message> + 'static {
    let cube_name = params.0.clone();
    let mnemonic = params.1.clone();
    let pubkey_hex = params.2.clone();
    let relay_urls = params.3.clone();
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

            // Track seen DM event IDs to avoid reprocessing
            // (gift-wrap timestamps are randomized per NIP-59, so we can't use timestamps)
            let mut seen_dm_event_ids: HashSet<nostr_sdk::EventId> = HashSet::new();
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

            // Auto-restore: if we have a mnemonic but no local trades, scan relay for DMs
            let data = load_data(&cube_name);
            if data.trades.is_empty() && !mnemonic.is_empty() {
                tracing::info!("No local trades found — attempting DM-scan restore");
                match restore_trades(&cube_name, &mnemonic, &pubkey_hex, &relay_urls).await {
                    Ok(0) => tracing::info!("Restore: no trades found on relay"),
                    Ok(n) => {
                        tracing::info!("Restore: recovered {} trades from relay", n);
                        let msg = Message::View(view::Message::ShowSuccess(format!(
                            "Recovered {} trade(s) from relay",
                            n
                        )));
                        let _ = output.send(msg).await;
                    }
                    Err(e) => tracing::warn!("Restore failed: {e}"),
                }
            }

            loop {
                // Subscribe to gift-wrap DMs for any new trade pubkeys
                update_dm_subscriptions(
                    &client,
                    &cube_name,
                    &mnemonic,
                    &mut subscribed_trade_pubkeys,
                )
                .await;

                // Process DMs: first run is silent (state recovery only), subsequent runs send toasts
                process_dm_notifications(
                    &client,
                    &cube_name,
                    &mnemonic,
                    mostro_pk,
                    &mut seen_dm_event_ids,
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
    mnemonic: &str,
    subscribed_pubkeys: &mut Vec<PublicKey>,
) {
    let sessions = load_data(cube_name).trades;
    if sessions.is_empty() {
        return;
    }

    let mut new_pubkeys: Vec<PublicKey> = Vec::new();
    for session in &sessions {
        if let Ok(keys) = derive_trade_keys(mnemonic, session.trade_index) {
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
    mnemonic: &str,
    mostro_pubkey: PublicKey,
    seen_event_ids: &mut HashSet<nostr_sdk::EventId>,
    output: &mut iced::futures::channel::mpsc::Sender<Message>,
    silent: bool,
) {
    let sessions = load_data(cube_name).trades;
    if sessions.is_empty() {
        return;
    }

    let mut session_keys: Vec<(TradeSession, Keys)> = Vec::new();
    for session in &sessions {
        if let Ok(keys) = derive_trade_keys(mnemonic, session.trade_index) {
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
        // Skip already-processed events (use event ID, not timestamp,
        // because NIP-59 gift-wrap timestamps are randomized)
        if seen_event_ids.contains(&event.id) {
            continue;
        }

        for (session, keys) in &session_keys {
            let is_for_us = event.tags.iter().any(|tag| {
                let t = tag.clone().to_vec();
                t.len() >= 2 && t[0] == "p" && t[1] == keys.public_key().to_hex()
            });
            if !is_for_us {
                continue;
            }

            if let Ok(unwrapped) = nip59::extract_rumor(keys, event).await {
                // Verify the DM came from the expected Mostro node
                if unwrapped.rumor.pubkey != mostro_pubkey {
                    continue;
                }

                if let Ok((msg, _)) = serde_json::from_str::<(
                    mostro_core::message::Message,
                    Option<String>,
                )>(&unwrapped.rumor.content)
                {
                    let inner = msg.get_inner_message_kind();
                    let action = format!("{:?}", inner.action);
                    let payload_json = serde_json::to_string(&inner.payload).unwrap_or_default();

                    seen_event_ids.insert(event.id);

                    // Skip error responses (CantDo) — they are not state transitions
                    // and would pollute last_dm_action, hiding action buttons
                    if inner.action == mostro_core::message::Action::CantDo {
                        tracing::debug!(
                            "Skipping CantDo DM for order {} (not a state transition)",
                            session.order_id
                        );
                        continue;
                    }

                    // Use the rumor's timestamp for ordering (not the gift-wrap's randomized ts)
                    let rumor_ts = unwrapped.rumor.created_at.as_u64();

                    // Persist the full message to disk
                    append_trade_message(
                        cube_name,
                        &session.order_id,
                        TradeMessage {
                            timestamp: rumor_ts,
                            action: action.clone(),
                            payload_json: payload_json.clone(),
                        },
                    );

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
