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

const FETCH_INTERVAL_SECS: u64 = 60; // Fallback polling; primary updates via subscription
const ORDER_LOOKBACK_SECS: u64 = 48 * 3600; // 48 hours, same as mobile
const MOSTRO_INFO_EVENT_KIND: u16 = 38385;

/// Check if the client has at least one connected relay.
async fn has_connected_relay(client: &Client) -> bool {
    client.relays().await.values().any(|r| r.is_connected())
}

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
    /// True when this message was sent by us (chat messages only).
    #[serde(default)]
    pub is_own: bool,
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
    /// Counterparty's trade pubkey (hex) for P2P chat, set when trade becomes active.
    #[serde(default)]
    pub counterparty_trade_pubkey: Option<String>,
    /// Admin/solver's trade pubkey (hex) for dispute chat, set when AdminTookDispute is received.
    #[serde(default)]
    pub admin_trade_pubkey: Option<String>,
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
        let is_dup = session.messages.iter().any(|m| {
            m.timestamp == msg.timestamp
                && m.action == msg.action
                && m.payload_json == msg.payload_json
        });
        if !is_dup {
            session.messages.push(msg);
            session.messages.sort_by_key(|m| m.timestamp);
            if let Err(e) = save_data(cube_name, &data) {
                tracing::warn!("Failed to persist trade message: {e}");
            }
        }
    }
}

/// Update the counterparty's trade pubkey for a given order (persisted to disk).
pub fn set_counterparty_pubkey(cube_name: &str, order_id: &str, pubkey: &str) {
    let mut data = load_data(cube_name);
    if let Some(session) = data.trades.iter_mut().find(|t| t.order_id == order_id) {
        if session.counterparty_trade_pubkey.as_deref() != Some(pubkey) {
            session.counterparty_trade_pubkey = Some(pubkey.to_string());
            if let Err(e) = save_data(cube_name, &data) {
                tracing::warn!("Failed to persist counterparty pubkey: {e}");
            }
        }
    }
}

/// Update the admin/solver's trade pubkey for a given order (persisted to disk).
fn set_admin_pubkey(cube_name: &str, order_id: &str, pubkey: &str) {
    let mut data = load_data(cube_name);
    if let Some(session) = data.trades.iter_mut().find(|t| t.order_id == order_id) {
        if session.admin_trade_pubkey.as_deref() != Some(pubkey) {
            session.admin_trade_pubkey = Some(pubkey.to_string());
            if let Err(e) = save_data(cube_name, &data) {
                tracing::warn!("Failed to persist admin pubkey: {e}");
            }
        }
    }
}

/// Extract the counterparty's trade pubkey from a SmallOrder payload.
/// `our_trade_pubkey_hex` is our own trade pubkey for this session so we can
/// identify which of buyer/seller pubkey is the counterparty's.
fn extract_counterparty_pubkey(
    order: &mostro_core::order::SmallOrder,
    our_trade_pubkey_hex: &str,
) -> Option<String> {
    let buyer = order.buyer_trade_pubkey.as_deref();
    let seller = order.seller_trade_pubkey.as_deref();
    match (buyer, seller) {
        (Some(b), Some(s)) => {
            if b == our_trade_pubkey_hex {
                Some(s.to_string())
            } else {
                Some(b.to_string())
            }
        }
        (Some(b), None) if b != our_trade_pubkey_hex => Some(b.to_string()),
        (None, Some(s)) if s != our_trade_pubkey_hex => Some(s.to_string()),
        _ => None,
    }
}

/// Get the latest protocol DM action for a trade from its message history.
/// Skips non-protocol chat entries (e.g. "SendDm") so that P2P chat
/// messages do not affect protocol state detection.
pub fn latest_dm_action(session: &TradeSession) -> Option<&str> {
    session
        .messages
        .iter()
        .rev()
        .find(|m| m.action != "SendDm")
        .map(|m| m.action.as_str())
}

/// Extract the hold invoice from a trade session's message history.
/// Checks PayInvoice/WaitingSellerToPay and BuyerTookOrder messages since
/// either may carry the PaymentRequest payload depending on order flow.
pub fn extract_hold_invoice(session: &TradeSession) -> Option<String> {
    session
        .messages
        .iter()
        .rev()
        .find(|m| {
            m.action == "PayInvoice"
                || m.action == "WaitingSellerToPay"
                || m.action == "BuyerTookOrder"
        })
        .and_then(|m| {
            // payload_json is the serialized Option<Payload>
            // For PayInvoice/BuyerTookOrder: Some(PaymentRequest(Some(order), invoice_string, Some(amount)))
            let payload: Option<mostro_core::message::Payload> =
                serde_json::from_str(&m.payload_json).ok()?;
            match payload {
                Some(mostro_core::message::Payload::PaymentRequest(_, invoice, _)) => Some(invoice),
                _ => None,
            }
        })
}

/// Find the timestamp of the DM that started the current countdown phase.
/// For WaitingBuyerInvoice status → find the AddInvoice/WaitingBuyerInvoice message
/// For WaitingPayment status → find the PayInvoice/WaitingSellerToPay message
pub fn countdown_start_timestamp(session: &TradeSession) -> Option<u64> {
    let last_action = session
        .messages
        .iter()
        .rev()
        .find(|m| m.action != "SendDm")
        .map(|m| m.action.as_str())?;

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
        .rfind(|m| target_actions.contains(&m.action.as_str()) && m.timestamp > 0)
        .map(|m| m.timestamp)
}

/// Restore trades from Mostro using the protocol-level restore flow:
/// 1. Send RestoreSession → get order IDs + trade indices
/// 2. Send Orders request → get full order details
/// 3. Send LastTradeIndex → get the correct last index
///    Returns the number of trades recovered.
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
            let is_creator = match d.kind.as_ref() {
                Some(mostro_core::order::Kind::Buy) => d.buyer_trade_pubkey.as_deref() == Some(pk),
                Some(mostro_core::order::Kind::Sell) => {
                    d.seller_trade_pubkey.as_deref() == Some(pk)
                }
                None => false,
            };
            if is_creator {
                "creator"
            } else {
                "taker"
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
                is_own: false,
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
            counterparty_trade_pubkey: details.and_then(|d| {
                // During restore, try to pick up the counterparty pubkey from the order
                let our_hex = derive_trade_keys(mnemonic, *trade_index)
                    .map(|k| k.public_key().to_hex())
                    .unwrap_or_default();
                extract_counterparty_pubkey(d, &our_hex)
            }),
            admin_trade_pubkey: None, // admin pubkey is only set via DM, not restorable
        });
    }

    let count = sessions.len();
    let data = MostroData {
        last_trade_index,
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
    if !has_connected_relay(client).await {
        return info;
    }
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
    client.wait_for_connection(Duration::from_secs(10)).await;

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

// ── Trade index synchronization ─────────────────────────────────────────

/// Query the Mostro server for the authoritative last_trade_index and update local storage.
/// Returns the synced index.
async fn sync_last_trade_index(
    cube_name: &str,
    mnemonic: &str,
    mostro_pubkey_hex: &str,
    relay_urls: &[String],
) -> Result<i64, String> {
    tracing::info!("Syncing last_trade_index from Mostro server");
    let temp_trade_index: i64 = 1;
    let last_index_kind = mostro_core::message::MessageKind::new(
        None,
        None,
        None,
        mostro_core::message::Action::LastTradeIndex,
        None,
    );
    let last_index_msg = mostro_core::message::Message::Restore(last_index_kind);
    let (resp, _) = send_mostro_message(
        mnemonic,
        temp_trade_index,
        mostro_pubkey_hex,
        relay_urls,
        last_index_msg,
    )
    .await?;

    let inner = resp.get_inner_message_kind();
    let idx = inner.trade_index();
    if idx <= 0 {
        return Err("Server returned invalid last_trade_index".to_string());
    }

    let mut data = load_data(cube_name);
    tracing::info!(
        "Trade index sync: local={}, server={}",
        data.last_trade_index,
        idx
    );
    data.last_trade_index = idx;
    save_data(cube_name, &data)?;

    Ok(idx)
}

/// Check if a CantDo payload is an InvalidTradeIndex error.
fn is_invalid_trade_index(inner: &mostro_core::message::MessageKind) -> bool {
    matches!(
        &inner.payload,
        Some(mostro_core::message::Payload::CantDo(Some(
            mostro_core::error::CantDoReason::InvalidTradeIndex
        )))
    )
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
/// On InvalidTradeIndex, automatically re-syncs from the server and retries once.
pub async fn submit_order(form: OrderFormData) -> Result<OrderSubmitResponse, String> {
    for attempt in 0..2u8 {
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

        // Build SmallOrder (clone fields consumed by SmallOrder::new)
        let small_order = mostro_core::order::SmallOrder::new(
            None,
            Some(form.kind),
            Some(mostro_core::order::Status::Pending),
            form.amount,
            form.fiat_code.clone(),
            form.min_amount,
            form.max_amount,
            form.fiat_amount,
            form.payment_method.clone(),
            form.premium,
            None,
            None,
            form.buyer_invoice.clone(),
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
            "Order sent via gift wrap, trade_index={}, request_id={}, attempt={}",
            next_idx,
            request_id,
            attempt
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
            // On InvalidTradeIndex, re-sync from server and retry once
            if is_invalid_trade_index(inner) && attempt == 0 {
                tracing::warn!(
                    "InvalidTradeIndex (local={}), syncing from server and retrying",
                    data.last_trade_index
                );
                sync_last_trade_index(
                    &form.cube_name,
                    &form.mnemonic,
                    &form.mostro_pubkey_hex,
                    &form.relay_urls,
                )
                .await?;
                continue;
            }
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
            fiat_code: form.fiat_code.clone(),
            fiat_amount: form.fiat_amount,
            min_amount: form.min_amount,
            max_amount: form.max_amount,
            amount: form.amount,
            premium: form.premium,
            payment_method: form.payment_method.clone(),
            created_at: chrono::Utc::now().timestamp(),
            role: "creator".to_string(),
            messages: Vec::new(),
            counterparty_trade_pubkey: None, // set later when someone takes the order
            admin_trade_pubkey: None,
        };
        if let Err(e) = append_trade(&form.cube_name, session) {
            tracing::warn!("Failed to persist trade session: {e}");
        }

        return Ok(OrderSubmitResponse::Success { order_id });
    }

    Err("Order failed after trade index sync retry".to_string())
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
/// On InvalidTradeIndex, automatically re-syncs from the server and retries once.
pub async fn take_order(data: TakeOrderData) -> Result<TakeOrderResponse, String> {
    let order_uuid =
        uuid::Uuid::parse_str(&data.order_id).map_err(|e| format!("Invalid order ID: {e}"))?;

    // Determine action: TakeSell if we're buying (order is sell), TakeBuy if we're selling
    let action = match data.order_type {
        OrderType::Sell => mostro_core::message::Action::TakeSell,
        OrderType::Buy => mostro_core::message::Action::TakeBuy,
    };

    for attempt in 0..2u8 {
        let mut mdata = load_data(&data.cube_name);
        let next_idx = mdata.last_trade_index + 1;

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
            "Taking order {}, action={:?}, trade_index={}, attempt={}",
            data.order_id,
            action,
            next_idx,
            attempt
        );

        let message = mostro_core::message::Message::new_order(
            Some(order_uuid),
            Some(request_id),
            Some(next_idx),
            action.clone(),
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
            // On InvalidTradeIndex, re-sync from server and retry once
            if is_invalid_trade_index(inner) && attempt == 0 {
                tracing::warn!(
                    "InvalidTradeIndex on take_order (local={}), syncing from server and retrying",
                    mdata.last_trade_index
                );
                sync_last_trade_index(
                    &data.cube_name,
                    &data.mnemonic,
                    &data.mostro_pubkey_hex,
                    &data.relay_urls,
                )
                .await?;
                continue;
            }
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

        // Extract counterparty trade pubkey from the response payload
        let our_trade_hex = derive_trade_keys(&data.mnemonic, next_idx)
            .map(|k| k.public_key().to_hex())
            .unwrap_or_default();
        let counterparty_trade_pubkey = match &inner.payload {
            Some(mostro_core::message::Payload::Order(order)) => {
                extract_counterparty_pubkey(order, &our_trade_hex)
            }
            Some(mostro_core::message::Payload::PaymentRequest(Some(order), _, _)) => {
                extract_counterparty_pubkey(order, &our_trade_hex)
            }
            _ => None,
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
            counterparty_trade_pubkey,
            admin_trade_pubkey: None,
        };
        if let Err(e) = append_trade(&data.cube_name, session) {
            tracing::warn!("Failed to persist take-order session: {e}");
        }

        // Check response payload: PaymentRequest means seller must pay a hold invoice
        return match &inner.payload {
            Some(mostro_core::message::Payload::PaymentRequest(order, invoice, amount)) => {
                tracing::info!(
                    "Received PaymentRequest for order {} — seller must pay invoice",
                    data.order_id
                );
                // Use explicit amount if present, otherwise fall back to order.amount
                let amount_sats =
                    amount.or_else(|| order.as_ref().map(|o| o.amount).filter(|&a| a > 0));
                Ok(TakeOrderResponse::PaymentRequired {
                    order_id: data.order_id,
                    trade_index: next_idx,
                    invoice: invoice.clone(),
                    amount_sats,
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
        };
    }

    Err("Take order failed after trade index sync retry".to_string())
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
            None, invoice, None,
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

// ── Chat ────────────────────────────────────────────────────────────────

/// Compute the ECDH shared key between our trade key and the counterparty's
/// trade pubkey.  Both parties derive the same shared key.  The shared key's
/// public part is used as the p-tag recipient in chat gift wraps so that
/// neither party's real trade pubkey is leaked to relay operators.
fn derive_shared_chat_keys(
    our_keys: &Keys,
    counterparty_pubkey: &PublicKey,
) -> Result<Keys, String> {
    let shared_bytes =
        nostr_sdk::util::generate_shared_key(our_keys.secret_key(), counterparty_pubkey)
            .map_err(|e| format!("Failed to compute shared key: {e}"))?;
    let shared_secret = nostr_sdk::SecretKey::from_slice(&shared_bytes)
        .map_err(|e| format!("Invalid shared secret: {e}"))?;
    Ok(Keys::new(shared_secret))
}

/// Send a chat message directly to the counterparty via P2P NIP-59 gift wrap.
/// Messages are encrypted to the ECDH shared key (not the counterparty's trade
/// pubkey) per the Mostro chat protocol.
pub async fn send_chat_message(data: TradeActionData) -> Result<(), String> {
    let text = data.invoice.clone().ok_or("Chat text is required")?;
    if text.trim().is_empty() {
        return Err("Empty message".to_string());
    }

    let sessions = load_data(&data.cube_name).trades;
    let session = sessions
        .iter()
        .find(|s| s.order_id == data.order_id)
        .ok_or_else(|| format!("No trade session found for order {}", data.order_id))?;

    let counterparty_hex = session
        .counterparty_trade_pubkey
        .as_deref()
        .ok_or("No counterparty trade pubkey available — chat not yet possible")?;
    let counterparty_pubkey = PublicKey::from_hex(counterparty_hex)
        .map_err(|e| format!("Invalid counterparty pubkey: {e}"))?;

    let trade_keys = derive_trade_keys(&data.mnemonic, session.trade_index)?;
    let shared_keys = derive_shared_chat_keys(&trade_keys, &counterparty_pubkey)?;
    let shared_pubkey = shared_keys.public_key();

    // 1. Create a signed kind-1 text note (signed by our trade key)
    let inner_event = EventBuilder::text_note(&text)
        .sign_with_keys(&trade_keys)
        .map_err(|e| format!("Failed to sign inner event: {e}"))?;
    let inner_json = inner_event.as_json();

    // 2. NIP-44 encrypt with an ephemeral key → shared pubkey
    let ephemeral_keys = Keys::generate();
    let encrypted = nostr_sdk::prelude::nip44::encrypt(
        ephemeral_keys.secret_key(),
        &shared_pubkey,
        &inner_json,
        nostr_sdk::prelude::nip44::Version::V2,
    )
    .map_err(|e| format!("Failed to encrypt chat message: {e}"))?;

    // 3. Build kind-1059 gift wrap, p-tag = shared pubkey
    let gift_wrap = EventBuilder::new(nostr_sdk::Kind::GiftWrap, &encrypted)
        .tag(Tag::public_key(shared_pubkey))
        .custom_created_at(Timestamp::tweaked(
            nostr_sdk::prelude::nip59::RANGE_RANDOM_TIMESTAMP_TWEAK,
        ))
        .sign_with_keys(&ephemeral_keys)
        .map_err(|e| format!("Failed to sign gift wrap: {e}"))?;

    // 4. Send to relays (fire-and-forget)
    let client = Client::new(trade_keys.clone());
    for relay_url in &data.relay_urls {
        client
            .add_relay(relay_url.as_str())
            .await
            .map_err(|e| format!("Failed to add relay {relay_url}: {e}"))?;
    }
    client.connect().await;
    client.wait_for_connection(Duration::from_secs(10)).await;

    client
        .send_event(&gift_wrap)
        .await
        .map_err(|e| format!("Failed to send chat message: {e}"))?;

    let _ = client.disconnect().await;

    // Persist outbound message so get_trade_messages returns it with is_own = true
    let mut store = load_data(&data.cube_name);
    if let Some(session) = store
        .trades
        .iter_mut()
        .find(|s| s.order_id == data.order_id)
    {
        session.messages.push(TradeMessage {
            timestamp: chrono::Utc::now().timestamp() as u64,
            action: "chat".to_string(),
            payload_json: text,
            is_own: true,
        });
        let _ = save_data(&data.cube_name, &store);
    }

    tracing::info!("P2P chat message sent for order {}", data.order_id);
    Ok(())
}

/// Send a dispute chat message to the admin/solver via P2P NIP-59 gift wrap.
/// Same encryption as peer chat but uses the admin's shared key.
pub async fn send_admin_chat_message(data: TradeActionData) -> Result<(), String> {
    let text = data.invoice.clone().ok_or("Chat text is required")?;
    if text.trim().is_empty() {
        return Err("Empty message".to_string());
    }

    let sessions = load_data(&data.cube_name).trades;
    let session = sessions
        .iter()
        .find(|s| s.order_id == data.order_id)
        .ok_or_else(|| format!("No trade session found for order {}", data.order_id))?;

    let admin_hex = session
        .admin_trade_pubkey
        .as_deref()
        .ok_or("No admin trade pubkey available — dispute chat not yet possible")?;
    let admin_pubkey =
        PublicKey::from_hex(admin_hex).map_err(|e| format!("Invalid admin pubkey: {e}"))?;

    let trade_keys = derive_trade_keys(&data.mnemonic, session.trade_index)?;
    let shared_keys = derive_shared_chat_keys(&trade_keys, &admin_pubkey)?;
    let shared_pubkey = shared_keys.public_key();

    let inner_event = EventBuilder::text_note(&text)
        .sign_with_keys(&trade_keys)
        .map_err(|e| format!("Failed to sign inner event: {e}"))?;
    let inner_json = inner_event.as_json();

    let ephemeral_keys = Keys::generate();
    let encrypted = nostr_sdk::prelude::nip44::encrypt(
        ephemeral_keys.secret_key(),
        &shared_pubkey,
        &inner_json,
        nostr_sdk::prelude::nip44::Version::V2,
    )
    .map_err(|e| format!("Failed to encrypt dispute chat message: {e}"))?;

    let gift_wrap = EventBuilder::new(nostr_sdk::Kind::GiftWrap, &encrypted)
        .tag(Tag::public_key(shared_pubkey))
        .custom_created_at(Timestamp::tweaked(
            nostr_sdk::prelude::nip59::RANGE_RANDOM_TIMESTAMP_TWEAK,
        ))
        .sign_with_keys(&ephemeral_keys)
        .map_err(|e| format!("Failed to sign gift wrap: {e}"))?;

    let client = Client::new(trade_keys.clone());
    for relay_url in &data.relay_urls {
        client
            .add_relay(relay_url.as_str())
            .await
            .map_err(|e| format!("Failed to add relay {relay_url}: {e}"))?;
    }
    client.connect().await;
    client.wait_for_connection(Duration::from_secs(10)).await;

    client
        .send_event(&gift_wrap)
        .await
        .map_err(|e| format!("Failed to send dispute chat message: {e}"))?;

    let _ = client.disconnect().await;

    // Persist outbound dispute message so get_trade_messages returns it with is_own = true
    let mut store = load_data(&data.cube_name);
    if let Some(session) = store
        .trades
        .iter_mut()
        .find(|s| s.order_id == data.order_id)
    {
        session.messages.push(TradeMessage {
            timestamp: chrono::Utc::now().timestamp() as u64,
            action: "dispute_chat".to_string(),
            payload_json: text,
            is_own: true,
        });
        let _ = save_data(&data.cube_name, &store);
    }

    tracing::info!("Dispute chat message sent for order {}", data.order_id);
    Ok(())
}

/// Get all trade messages for a given order from disk.
pub fn get_trade_messages(cube_name: &str, order_id: &str) -> Vec<TradeMessage> {
    let data = load_data(cube_name);
    data.trades
        .iter()
        .find(|t| t.order_id == order_id)
        .map(|t| t.messages.clone())
        .unwrap_or_default()
}

/// Chat identity info for the user information panel.
pub struct ChatIdentityInfo {
    pub counterparty_pubkey: Option<String>,
    pub counterparty_nickname: Option<String>,
    pub our_trade_pubkey: Option<String>,
    pub our_nickname: Option<String>,
    pub shared_key: Option<String>,
}

// ── Deterministic nickname generation ──

const ADJECTIVES: &[&str] = &[
    "shadowy",
    "orange",
    "nonCustodial",
    "trustless",
    "unbanked",
    "atomic",
    "magic",
    "hidden",
    "incognito",
    "anonymous",
    "encrypted",
    "ghostly",
    "silent",
    "masked",
    "stealthy",
    "free",
    "nostalgic",
    "ephemeral",
    "sovereign",
    "unstoppable",
    "private",
    "censorshipResistant",
    "hush",
    "defiant",
    "subversive",
    "fiery",
    "subzero",
    "burning",
    "cosmic",
    "mighty",
    "whispering",
    "cyber",
    "rusty",
    "nihilistic",
    "dark",
    "wicked",
    "spicy",
    "noKYC",
    "discreet",
    "loose",
    "boosted",
    "starving",
    "hungry",
    "orwellian",
    "bullish",
    "bearish",
]; // 46 items

const NOUNS: &[&str] = &[
    "wizard",
    "pirate",
    "zap",
    "node",
    "invoice",
    "nipster",
    "nomad",
    "sats",
    "bull",
    "bear",
    "whale",
    "frog",
    "gorilla",
    "nostrich",
    "halFinney",
    "hodlonaut",
    "satoshi",
    "nakamoto",
    "samurai",
    "sparrow",
    "crusader",
    "tinkerer",
    "nostr",
    "pleb",
    "warrior",
    "ecdsa",
    "monkey",
    "wolf",
    "renegade",
    "minotaur",
    "phoenix",
    "dragon",
    "fiatjaf",
    "roasbeef",
    "berlin",
    "tokyo",
    "buenosAires",
    "caracas",
    "havana",
    "miami",
    "prague",
    "amsterdam",
    "lugano",
    "seoul",
    "bitcoinBeach",
    "carnivore",
    "ape",
    "honeyBadger",
    "lnp2pBot",
    "lunaticoin",
    "jorgeValenzuela",
    "javyBastard",
    "loreOrtiz",
    "manuFerrari",
    "pablof7z",
    "btcAndres",
    "laCrypta",
    "niftynei",
    "gloriaZhao",
    "stupidrisks",
    "dolcheVillarreal",
    "furszy",
    "sergi",
    "jarolRod",
    "pieterWuille",
    "edwardSnowden",
    "libreriaDeSatoshi",
    "alexGladstein",
    "bitkoYinowsky",
    "alfreMancera",
    "faixaPreta",
    "laVecinaDeArriba",
    "mempool",
    "pana",
    "chamo",
    "catire",
    "arepa",
    "cachapa",
    "tequeño",
    "hallaca",
    "roraima",
    "canaima",
    "turpial",
    "araguaney",
    "cunaguaro",
    "chiguire",
    "mamarracho",
    "cambur",
]; // 90 items

/// Compute `big_hex_number % modulus` without needing a BigInt library.
/// Processes the hex string digit-by-digit using modular arithmetic.
fn hex_mod(hex: &str, modulus: usize) -> usize {
    let mut result: usize = 0;
    for ch in hex.chars() {
        let digit = match ch.to_digit(16) {
            Some(d) => d as usize,
            None => continue,
        };
        result = (result * 16 + digit) % modulus;
    }
    result
}

/// Compute `big_hex_number / divisor % modulus` without needing a BigInt library.
/// First divides the big number by `divisor`, then takes modulo.
fn hex_div_mod(hex: &str, divisor: usize, modulus: usize) -> usize {
    // To compute (N / divisor) % modulus, we process hex digits and track
    // both the running quotient-mod and the running remainder of the division.
    let mut remainder: usize = 0;
    let mut quotient_mod: usize = 0;
    for ch in hex.chars() {
        let digit = match ch.to_digit(16) {
            Some(d) => d as usize,
            None => continue,
        };
        // N_so_far = old_N * 16 + digit
        // N_so_far / divisor = (old_N * 16 + digit) / divisor
        let combined = remainder * 16 + digit;
        let q_digit = combined / divisor;
        remainder = combined % divisor;
        quotient_mod = (quotient_mod * 16 + q_digit) % modulus;
    }
    quotient_mod
}

/// Generate a deterministic human-readable nickname from a hex public key.
/// Matches the Mostro mobile app's `deterministicHandleFromHexKey` exactly:
/// parses the full hex key as a BigInt, picks adjective and noun via modulo.
pub fn nickname_from_pubkey(hex_key: &str) -> String {
    if hex_key.is_empty() {
        return "unknown".to_string();
    }
    let clean: String = hex_key.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if clean.is_empty() {
        return "unknown".to_string();
    }
    let adj_idx = hex_mod(&clean, ADJECTIVES.len());
    let noun_idx = hex_div_mod(&clean, ADJECTIVES.len(), NOUNS.len());
    format!("{}-{}", ADJECTIVES[adj_idx], NOUNS[noun_idx])
}

pub fn get_chat_identity_info(cube_name: &str, mnemonic: &str, order_id: &str) -> ChatIdentityInfo {
    let data = load_data(cube_name);
    let session = data.trades.iter().find(|t| t.order_id == order_id);
    let Some(session) = session else {
        return ChatIdentityInfo {
            counterparty_pubkey: None,
            counterparty_nickname: None,
            our_trade_pubkey: None,
            our_nickname: None,
            shared_key: None,
        };
    };
    let our_trade_pubkey = derive_trade_keys(mnemonic, session.trade_index)
        .ok()
        .map(|k| k.public_key().to_hex());
    let our_nickname = our_trade_pubkey.as_deref().map(nickname_from_pubkey);
    let counterparty_nickname = session
        .counterparty_trade_pubkey
        .as_deref()
        .map(nickname_from_pubkey);
    let shared_key = session
        .counterparty_trade_pubkey
        .as_deref()
        .and_then(|cp_hex| PublicKey::from_hex(cp_hex).ok())
        .and_then(|cp_pk| {
            derive_trade_keys(mnemonic, session.trade_index)
                .ok()
                .and_then(|keys| derive_shared_chat_keys(&keys, &cp_pk).ok())
        })
        .map(|sk| sk.public_key().to_hex());
    ChatIdentityInfo {
        counterparty_pubkey: session.counterparty_trade_pubkey.clone(),
        counterparty_nickname,
        our_trade_pubkey,
        our_nickname,
        shared_key,
    }
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

/// Process a single kind-38383 order event and update the in-memory cache.
/// Returns true if the cache was modified (caller should emit updated order list).
fn process_order_event(
    event: &nostr_sdk::Event,
    order_cache: &mut BTreeMap<String, (u64, mostro_core::order::SmallOrder, RatingInfo)>,
) -> bool {
    let (order, rating) = order_from_tags(&event.tags);
    let order_id = match &order.id {
        Some(id) => id.to_string(),
        None => return false,
    };

    let created_at = event.created_at.as_u64();

    // Non-pending orders: remove from cache (order was taken/canceled/etc)
    if order.status != Some(mostro_core::order::Status::Pending) {
        return order_cache.remove(&order_id).is_some();
    }

    // Only update if this event is newer
    if let Some((existing_ts, _, _)) = order_cache.get(&order_id) {
        if *existing_ts >= created_at {
            return false;
        }
    }
    order_cache.insert(order_id, (created_at, order, rating));
    true
}

/// Build a Vec<P2POrder> from the in-memory order cache.
fn orders_from_cache(
    order_cache: &BTreeMap<String, (u64, mostro_core::order::SmallOrder, RatingInfo)>,
    cube_name: &str,
) -> Vec<P2POrder> {
    let sessions = load_data(cube_name).trades;
    let my_order_ids: HashSet<&str> = sessions
        .iter()
        .filter(|s| s.role == "creator")
        .map(|s| s.order_id.as_str())
        .collect();

    let mut orders: Vec<P2POrder> = order_cache
        .values()
        .filter_map(|(created_at, order, rating)| {
            let mut p2p = small_order_to_p2p_order(order, rating, *created_at)?;
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

    if !has_connected_relay(client).await {
        return sessions_to_fallback_trades(&sessions);
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
                counterparty_pubkey: session.counterparty_trade_pubkey.clone(),
                admin_pubkey: session.admin_trade_pubkey.clone(),
                hold_invoice: extract_hold_invoice(session),
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
        counterparty_pubkey: session.counterparty_trade_pubkey.clone(),
        admin_pubkey: session.admin_trade_pubkey.clone(),
        hold_invoice: extract_hold_invoice(session),
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
        256,
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
            client.wait_for_connection(Duration::from_secs(10)).await;

            // Track seen DM event IDs to avoid reprocessing
            // (gift-wrap timestamps are randomized per NIP-59, so we can't use timestamps)
            let mut seen_dm_event_ids: HashSet<nostr_sdk::EventId> = HashSet::new();
            // Track which trade pubkeys we're already subscribed to for DMs
            let mut subscribed_trade_pubkeys: Vec<PublicKey> = Vec::new();

            // Initial fetch of node info
            let info = fetch_mostro_info(&client, mostro_pk).await;
            {
                let msg = Message::View(view::Message::P2P(P2PMessage::MostroNodeInfoReceived {
                    currencies: info.fiat_currencies,
                    min_order_sats: info.min_order_amount,
                    max_order_sats: info.max_order_amount,
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

            // Subscribe to order book events (kind 38383) for real-time updates
            let order_since =
                Timestamp::from(chrono::Utc::now().timestamp() as u64 - ORDER_LOOKBACK_SECS);
            let order_filter = Filter::new()
                .author(mostro_pk)
                .kind(Kind::Custom(38383))
                .since(order_since);
            if let Err(e) = client.subscribe(order_filter, None).await {
                tracing::warn!("Failed to subscribe to order events: {e}");
            }

            // In-memory cache of the latest order events, keyed by order UUID
            let mut order_cache: BTreeMap<
                String,
                (u64, mostro_core::order::SmallOrder, RatingInfo),
            > = BTreeMap::new();

            // Initial subscription + historical DM catch-up (silent)
            update_dm_subscriptions(
                &client,
                &cube_name,
                &mnemonic,
                &mut subscribed_trade_pubkeys,
            )
            .await;

            process_dm_notifications(
                &client,
                &cube_name,
                &mnemonic,
                mostro_pk,
                &mut seen_dm_event_ids,
                &mut output,
                true, // silent on first run
            )
            .await;

            // Initial order fetch — populates cache + emits to UI
            {
                let since =
                    Timestamp::from(chrono::Utc::now().timestamp() as u64 - ORDER_LOOKBACK_SECS);
                let filter = Filter::new()
                    .author(mostro_pk)
                    .kind(Kind::Custom(38383))
                    .since(since);
                if has_connected_relay(&client).await {
                    if let Ok(events) = client.fetch_events(filter, Duration::from_secs(15)).await {
                        for event in events.iter() {
                            process_order_event(event, &mut order_cache);
                        }
                    }
                }
            }
            let orders = orders_from_cache(&order_cache, &cube_name);
            let _ = output
                .send(Message::View(view::Message::P2P(
                    P2PMessage::MostroOrdersReceived(orders),
                )))
                .await;
            let trades = fetch_user_trades(&client, &cube_name, mostro_pk).await;
            let _ = output
                .send(Message::View(view::Message::P2P(
                    P2PMessage::MostroTradesReceived(trades),
                )))
                .await;

            // Real-time loop: listen for order + DM notifications, periodic trade fetch
            let mut notifications = client.notifications();
            loop {
                let deadline =
                    tokio::time::Instant::now() + Duration::from_secs(FETCH_INTERVAL_SECS);
                // Process real-time notifications until the periodic timer fires
                loop {
                    tokio::select! {
                        result = notifications.recv() => {
                            if let Ok(RelayPoolNotification::Event { event, .. }) = result {
                                // Real-time order book updates (kind 38383)
                                if event.kind == nostr_sdk::Kind::Custom(38383) {
                                    if process_order_event(&event, &mut order_cache) {
                                        let orders = orders_from_cache(&order_cache, &cube_name);
                                        let _ = output
                                            .send(Message::View(view::Message::P2P(
                                                P2PMessage::MostroOrdersReceived(orders),
                                            )))
                                            .await;
                                    }
                                }
                                // Real-time DM notifications (kind 1059 gift wrap)
                                else if event.kind == nostr_sdk::Kind::GiftWrap
                                    && !seen_dm_event_ids.contains(&event.id)
                                {
                                    let new_cp = process_dm_event(
                                        &event,
                                        &cube_name,
                                        &mnemonic,
                                        mostro_pk,
                                        &mut seen_dm_event_ids,
                                        &mut output,
                                    )
                                    .await;
                                    // New counterparty discovered — immediately subscribe
                                    // to the shared chat key for real-time P2P messages.
                                    if new_cp {
                                        update_dm_subscriptions(
                                            &client,
                                            &cube_name,
                                            &mnemonic,
                                            &mut subscribed_trade_pubkeys,
                                        )
                                        .await;
                                    }
                                }
                            }
                        }
                        _ = tokio::time::sleep_until(deadline) => {
                            break;
                        }
                    }
                }

                // Reconnect if all relays dropped
                if !has_connected_relay(&client).await {
                    tracing::warn!("No connected relays, attempting reconnect");
                    client.connect().await;
                    client.wait_for_connection(Duration::from_secs(10)).await;
                }

                // Periodic: check for new subscriptions + fetch orders/trades
                update_dm_subscriptions(
                    &client,
                    &cube_name,
                    &mnemonic,
                    &mut subscribed_trade_pubkeys,
                )
                .await;

                // Fallback: refresh order cache from a full fetch
                if has_connected_relay(&client).await {
                    let since = Timestamp::from(
                        chrono::Utc::now().timestamp() as u64 - ORDER_LOOKBACK_SECS,
                    );
                    let filter = Filter::new()
                        .author(mostro_pk)
                        .kind(Kind::Custom(38383))
                        .since(since);
                    if let Ok(events) = client.fetch_events(filter, Duration::from_secs(15)).await {
                        for event in events.iter() {
                            process_order_event(event, &mut order_cache);
                        }
                    }
                }
                let orders = orders_from_cache(&order_cache, &cube_name);
                let _ = output
                    .send(Message::View(view::Message::P2P(
                        P2PMessage::MostroOrdersReceived(orders),
                    )))
                    .await;
                let trades = fetch_user_trades(&client, &cube_name, mostro_pk).await;
                let _ = output
                    .send(Message::View(view::Message::P2P(
                        P2PMessage::MostroTradesReceived(trades),
                    )))
                    .await;
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
            // Also subscribe to the ECDH shared key pubkey for P2P chat
            if let Some(ref cp_hex) = session.counterparty_trade_pubkey {
                if let Ok(cp_pk) = PublicKey::from_hex(cp_hex) {
                    if let Ok(shared) = derive_shared_chat_keys(&keys, &cp_pk) {
                        let spk = shared.public_key();
                        if !subscribed_pubkeys.contains(&spk) {
                            new_pubkeys.push(spk);
                        }
                    }
                }
            }
            // Subscribe to ECDH shared key for dispute chat with admin
            if let Some(ref admin_hex) = session.admin_trade_pubkey {
                if let Ok(admin_pk) = PublicKey::from_hex(admin_hex) {
                    if let Ok(shared) = derive_shared_chat_keys(&keys, &admin_pk) {
                        let spk = shared.public_key();
                        if !subscribed_pubkeys.contains(&spk) {
                            new_pubkeys.push(spk);
                        }
                    }
                }
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

    // Fetch gift-wrap events for both trade pubkeys AND shared chat pubkeys
    let mut all_pubkeys: Vec<PublicKey> =
        session_keys.iter().map(|(_, k)| k.public_key()).collect();
    for (session, keys) in &session_keys {
        if let Some(ref cp_hex) = session.counterparty_trade_pubkey {
            if let Ok(cp_pk) = PublicKey::from_hex(cp_hex) {
                if let Ok(shared) = derive_shared_chat_keys(keys, &cp_pk) {
                    all_pubkeys.push(shared.public_key());
                }
            }
        }
        // Include admin shared key for dispute chat
        if let Some(ref admin_hex) = session.admin_trade_pubkey {
            if let Ok(admin_pk) = PublicKey::from_hex(admin_hex) {
                if let Ok(shared) = derive_shared_chat_keys(keys, &admin_pk) {
                    all_pubkeys.push(shared.public_key());
                }
            }
        }
    }
    if !has_connected_relay(client).await {
        return;
    }
    let since = Timestamp::from(chrono::Utc::now().timestamp() as u64 - ORDER_LOOKBACK_SECS);
    let filter = Filter::new()
        .pubkeys(all_pubkeys)
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
            let our_trade_hex = keys.public_key().to_hex();

            // Compute shared chat pubkey (if counterparty is known)
            let shared_chat_hex = session
                .counterparty_trade_pubkey
                .as_deref()
                .and_then(|cp_hex| PublicKey::from_hex(cp_hex).ok())
                .and_then(|cp_pk| derive_shared_chat_keys(keys, &cp_pk).ok())
                .map(|sk| sk.public_key().to_hex());

            // Compute admin shared chat pubkey (if admin is known)
            let admin_shared_chat_hex = session
                .admin_trade_pubkey
                .as_deref()
                .and_then(|admin_hex| PublicKey::from_hex(admin_hex).ok())
                .and_then(|admin_pk| derive_shared_chat_keys(keys, &admin_pk).ok())
                .map(|sk| sk.public_key().to_hex());

            // Match p-tag against our trade pubkey, shared chat pubkey, or admin shared pubkey
            let p_tag_value = event.tags.iter().find_map(|tag| {
                let t = tag.clone().to_vec();
                if t.len() >= 2 && t[0] == "p" {
                    Some(t[1].clone())
                } else {
                    None
                }
            });
            let is_mostro_dm = p_tag_value.as_deref() == Some(&our_trade_hex);
            let is_chat_dm = shared_chat_hex
                .as_deref()
                .is_some_and(|sh| p_tag_value.as_deref() == Some(sh));
            let is_admin_dm = admin_shared_chat_hex
                .as_deref()
                .is_some_and(|sh| p_tag_value.as_deref() == Some(sh));
            if !is_mostro_dm && !is_chat_dm && !is_admin_dm {
                continue;
            }

            // Try NIP-59 unwrap (3-layer: gift wrap → seal → rumor)
            if let Ok(unwrapped) = nip59::extract_rumor(keys, event).await {
                if unwrapped.rumor.pubkey == mostro_pubkey {
                    // ── Mostro protocol message ──
                    if let Ok((msg, _)) = serde_json::from_str::<(
                        mostro_core::message::Message,
                        Option<String>,
                    )>(&unwrapped.rumor.content)
                    {
                        let inner = msg.get_inner_message_kind();
                        let action = format!("{:?}", inner.action);
                        let payload_json =
                            serde_json::to_string(&inner.payload).unwrap_or_default();

                        seen_event_ids.insert(event.id);

                        if inner.action == mostro_core::message::Action::CantDo {
                            tracing::debug!(
                                "Skipping CantDo DM for order {} (not a state transition)",
                                session.order_id
                            );
                            continue;
                        }

                        // Extract counterparty trade pubkey from Order or PaymentRequest payloads
                        let cp_order = match &inner.payload {
                            Some(mostro_core::message::Payload::Order(ref order)) => Some(order),
                            Some(mostro_core::message::Payload::PaymentRequest(
                                Some(ref order),
                                _,
                                _,
                            )) => Some(order),
                            _ => None,
                        };
                        if let Some(order) = cp_order {
                            if let Some(cp_pk) = extract_counterparty_pubkey(order, &our_trade_hex)
                            {
                                set_counterparty_pubkey(cube_name, &session.order_id, &cp_pk);
                            }
                        }

                        // Extract admin pubkey from AdminTookDispute Peer payload
                        if inner.action == mostro_core::message::Action::AdminTookDispute {
                            if let Some(mostro_core::message::Payload::Peer(ref peer)) =
                                inner.payload
                            {
                                set_admin_pubkey(cube_name, &session.order_id, &peer.pubkey);
                            }
                        }

                        let rumor_ts = unwrapped.rumor.created_at.as_u64();

                        append_trade_message(
                            cube_name,
                            &session.order_id,
                            TradeMessage {
                                timestamp: rumor_ts,
                                action: action.clone(),
                                payload_json: payload_json.clone(),
                                is_own: false,
                            },
                        );

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
                    continue;
                }
            }

            // ── P2P chat message (2-layer: gift wrap → signed event) ──
            // Decrypt using the ECDH shared key (per Mostro chat protocol).
            let shared_keys = session
                .counterparty_trade_pubkey
                .as_deref()
                .and_then(|cp_hex| PublicKey::from_hex(cp_hex).ok())
                .and_then(|cp_pk| derive_shared_chat_keys(keys, &cp_pk).ok());
            if let Some(ref shared) = shared_keys {
                if let Ok(decrypted) = nostr_sdk::prelude::nip44::decrypt(
                    shared.secret_key(),
                    &event.pubkey,
                    &event.content,
                ) {
                    if let Ok(inner_event) = nostr_sdk::Event::from_json(&decrypted) {
                        // Verify sender is our known counterparty (inner event is signed by trade key)
                        let sender_hex = inner_event.pubkey.to_hex();
                        let is_counterparty =
                            session.counterparty_trade_pubkey.as_deref() == Some(&sender_hex);
                        if !is_counterparty {
                            continue;
                        }

                        seen_event_ids.insert(event.id);

                        let chat_text = inner_event.content.clone();
                        let payload_json = serde_json::to_string(&Some(
                            mostro_core::message::Payload::TextMessage(chat_text),
                        ))
                        .unwrap_or_default();
                        let rumor_ts = inner_event.created_at.as_u64();

                        append_trade_message(
                            cube_name,
                            &session.order_id,
                            TradeMessage {
                                timestamp: rumor_ts,
                                action: "SendDm".to_string(),
                                payload_json: payload_json.clone(),
                                is_own: false,
                            },
                        );

                        if !silent {
                            let update_msg =
                                Message::View(view::Message::P2P(P2PMessage::TradeUpdate {
                                    order_id: session.order_id.clone(),
                                    action: "SendDm".to_string(),
                                    payload_json,
                                }));
                            let _ = output.send(update_msg).await;
                        }
                    }
                } // if let Ok(decrypted)
            } // if let Some(shared)

            // ── Dispute chat message from admin (2-layer, admin shared key) ──
            let admin_shared_keys = session
                .admin_trade_pubkey
                .as_deref()
                .and_then(|admin_hex| PublicKey::from_hex(admin_hex).ok())
                .and_then(|admin_pk| derive_shared_chat_keys(keys, &admin_pk).ok());
            if let Some(ref admin_shared) = admin_shared_keys {
                if let Ok(decrypted) = nostr_sdk::prelude::nip44::decrypt(
                    admin_shared.secret_key(),
                    &event.pubkey,
                    &event.content,
                ) {
                    if let Ok(inner_event) = nostr_sdk::Event::from_json(&decrypted) {
                        let sender_hex = inner_event.pubkey.to_hex();
                        let is_admin = session.admin_trade_pubkey.as_deref() == Some(&sender_hex);
                        if !is_admin {
                            continue;
                        }

                        seen_event_ids.insert(event.id);

                        let chat_text = inner_event.content.clone();
                        let payload_json = serde_json::to_string(&Some(
                            mostro_core::message::Payload::TextMessage(chat_text),
                        ))
                        .unwrap_or_default();
                        let rumor_ts = inner_event.created_at.as_u64();

                        append_trade_message(
                            cube_name,
                            &session.order_id,
                            TradeMessage {
                                timestamp: rumor_ts,
                                action: "AdminDm".to_string(),
                                payload_json: payload_json.clone(),
                                is_own: false,
                            },
                        );

                        if !silent {
                            let update_msg =
                                Message::View(view::Message::P2P(P2PMessage::TradeUpdate {
                                    order_id: session.order_id.clone(),
                                    action: "AdminDm".to_string(),
                                    payload_json,
                                }));
                            let _ = output.send(update_msg).await;
                        }
                    }
                }
            } // if let Some(admin_shared)
        }
    }
}

/// Process a single incoming DM event in real-time (never silent).
/// Returns true if a new counterparty pubkey was discovered (caller should
/// update DM subscriptions to pick up the shared chat key).
async fn process_dm_event(
    event: &nostr_sdk::Event,
    cube_name: &str,
    mnemonic: &str,
    mostro_pubkey: PublicKey,
    seen_event_ids: &mut HashSet<nostr_sdk::EventId>,
    output: &mut iced::futures::channel::mpsc::Sender<Message>,
) -> bool {
    let sessions = load_data(cube_name).trades;
    let session_keys: Vec<(TradeSession, Keys)> = sessions
        .iter()
        .filter_map(|s| {
            derive_trade_keys(mnemonic, s.trade_index)
                .ok()
                .map(|k| (s.clone(), k))
        })
        .collect();

    let mut new_counterparty = false;

    for (session, keys) in &session_keys {
        let our_trade_hex = keys.public_key().to_hex();

        let shared_chat_hex = session
            .counterparty_trade_pubkey
            .as_deref()
            .and_then(|cp_hex| PublicKey::from_hex(cp_hex).ok())
            .and_then(|cp_pk| derive_shared_chat_keys(keys, &cp_pk).ok())
            .map(|sk| sk.public_key().to_hex());

        let admin_shared_chat_hex = session
            .admin_trade_pubkey
            .as_deref()
            .and_then(|admin_hex| PublicKey::from_hex(admin_hex).ok())
            .and_then(|admin_pk| derive_shared_chat_keys(keys, &admin_pk).ok())
            .map(|sk| sk.public_key().to_hex());

        let p_tag_value = event.tags.iter().find_map(|tag| {
            let t = tag.clone().to_vec();
            if t.len() >= 2 && t[0] == "p" {
                Some(t[1].clone())
            } else {
                None
            }
        });
        let is_mostro_dm = p_tag_value.as_deref() == Some(&our_trade_hex);
        let is_chat_dm = shared_chat_hex
            .as_deref()
            .is_some_and(|sh| p_tag_value.as_deref() == Some(sh));
        let is_admin_dm = admin_shared_chat_hex
            .as_deref()
            .is_some_and(|sh| p_tag_value.as_deref() == Some(sh));
        if !is_mostro_dm && !is_chat_dm && !is_admin_dm {
            continue;
        }

        // Try NIP-59 unwrap (3-layer: gift wrap → seal → rumor)
        if let Ok(unwrapped) = nip59::extract_rumor(keys, event).await {
            if unwrapped.rumor.pubkey == mostro_pubkey {
                if let Ok((msg, _)) = serde_json::from_str::<(
                    mostro_core::message::Message,
                    Option<String>,
                )>(&unwrapped.rumor.content)
                {
                    let inner = msg.get_inner_message_kind();
                    let action = format!("{:?}", inner.action);
                    let payload_json = serde_json::to_string(&inner.payload).unwrap_or_default();

                    seen_event_ids.insert(event.id);

                    if inner.action == mostro_core::message::Action::CantDo {
                        return false;
                    }

                    let cp_order = match &inner.payload {
                        Some(mostro_core::message::Payload::Order(ref order)) => Some(order),
                        Some(mostro_core::message::Payload::PaymentRequest(
                            Some(ref order),
                            _,
                            _,
                        )) => Some(order),
                        _ => None,
                    };
                    if let Some(order) = cp_order {
                        if let Some(cp_pk) = extract_counterparty_pubkey(order, &our_trade_hex) {
                            // Check if this is actually new
                            if session.counterparty_trade_pubkey.as_deref() != Some(&cp_pk) {
                                new_counterparty = true;
                            }
                            set_counterparty_pubkey(cube_name, &session.order_id, &cp_pk);
                        }
                    }

                    // Extract admin pubkey from AdminTookDispute Peer payload
                    if inner.action == mostro_core::message::Action::AdminTookDispute {
                        if let Some(mostro_core::message::Payload::Peer(ref peer)) = inner.payload {
                            if session.admin_trade_pubkey.as_deref() != Some(&peer.pubkey) {
                                new_counterparty = true; // triggers subscription update
                            }
                            set_admin_pubkey(cube_name, &session.order_id, &peer.pubkey);
                        }
                    }

                    let rumor_ts = unwrapped.rumor.created_at.as_u64();
                    append_trade_message(
                        cube_name,
                        &session.order_id,
                        TradeMessage {
                            timestamp: rumor_ts,
                            action: action.clone(),
                            payload_json: payload_json.clone(),
                            is_own: false,
                        },
                    );

                    let update_msg = Message::View(view::Message::P2P(P2PMessage::TradeUpdate {
                        order_id: session.order_id.clone(),
                        action,
                        payload_json,
                    }));
                    let _ = output.send(update_msg).await;
                }
                return new_counterparty;
            }
        }

        // P2P chat message (2-layer: gift wrap → signed event)
        let shared_keys = session
            .counterparty_trade_pubkey
            .as_deref()
            .and_then(|cp_hex| PublicKey::from_hex(cp_hex).ok())
            .and_then(|cp_pk| derive_shared_chat_keys(keys, &cp_pk).ok());
        if let Some(ref shared) = shared_keys {
            if let Ok(decrypted) = nostr_sdk::prelude::nip44::decrypt(
                shared.secret_key(),
                &event.pubkey,
                &event.content,
            ) {
                if let Ok(inner_event) = nostr_sdk::Event::from_json(&decrypted) {
                    let sender_hex = inner_event.pubkey.to_hex();
                    let is_counterparty =
                        session.counterparty_trade_pubkey.as_deref() == Some(&sender_hex);
                    if !is_counterparty {
                        continue;
                    }

                    seen_event_ids.insert(event.id);

                    let chat_text = inner_event.content.clone();
                    let payload_json = serde_json::to_string(&Some(
                        mostro_core::message::Payload::TextMessage(chat_text),
                    ))
                    .unwrap_or_default();
                    let rumor_ts = inner_event.created_at.as_u64();

                    append_trade_message(
                        cube_name,
                        &session.order_id,
                        TradeMessage {
                            timestamp: rumor_ts,
                            action: "SendDm".to_string(),
                            payload_json: payload_json.clone(),
                            is_own: false,
                        },
                    );

                    let update_msg = Message::View(view::Message::P2P(P2PMessage::TradeUpdate {
                        order_id: session.order_id.clone(),
                        action: "SendDm".to_string(),
                        payload_json,
                    }));
                    let _ = output.send(update_msg).await;
                    return false;
                }
            }
        }

        // Dispute chat message from admin (same 2-layer structure, different shared key)
        let admin_shared_keys = session
            .admin_trade_pubkey
            .as_deref()
            .and_then(|admin_hex| PublicKey::from_hex(admin_hex).ok())
            .and_then(|admin_pk| derive_shared_chat_keys(keys, &admin_pk).ok());
        if let Some(ref admin_shared) = admin_shared_keys {
            if let Ok(decrypted) = nostr_sdk::prelude::nip44::decrypt(
                admin_shared.secret_key(),
                &event.pubkey,
                &event.content,
            ) {
                if let Ok(inner_event) = nostr_sdk::Event::from_json(&decrypted) {
                    let sender_hex = inner_event.pubkey.to_hex();
                    let is_admin = session.admin_trade_pubkey.as_deref() == Some(&sender_hex);
                    if !is_admin {
                        continue;
                    }

                    seen_event_ids.insert(event.id);

                    let chat_text = inner_event.content.clone();
                    let payload_json = serde_json::to_string(&Some(
                        mostro_core::message::Payload::TextMessage(chat_text),
                    ))
                    .unwrap_or_default();
                    let rumor_ts = inner_event.created_at.as_u64();

                    append_trade_message(
                        cube_name,
                        &session.order_id,
                        TradeMessage {
                            timestamp: rumor_ts,
                            action: "AdminDm".to_string(),
                            payload_json: payload_json.clone(),
                            is_own: false,
                        },
                    );

                    let update_msg = Message::View(view::Message::P2P(P2PMessage::TradeUpdate {
                        order_id: session.order_id.clone(),
                        action: "AdminDm".to_string(),
                        payload_json,
                    }));
                    let _ = output.send(update_msg).await;
                    return false;
                }
            }
        }
    }
    false
}
