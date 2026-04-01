use std::collections::{BTreeMap, HashMap, HashSet};
use std::convert::TryFrom;
use std::path::PathBuf;
use std::time::Duration;

use fs4::fs_std::FileExt;

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

fn load_data(cube_name: &str) -> Result<MostroData, String> {
    let path = data_file_path(cube_name)?;
    if !path.exists() {
        return Ok(MostroData::default());
    }
    let data = std::fs::read(&path)
        .map_err(|e| format!("Failed to read mostro data at {}: {e}", path.display()))?;
    serde_json::from_slice(&data)
        .map_err(|e| format!("Failed to parse mostro data at {}: {e}", path.display()))
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

/// Acquire an exclusive file lock, load data, run `f`, save if `f` succeeds, then release.
///
/// The lock file lives at `{data_file}.lock` and is held for the entire load-modify-save
/// cycle so that concurrent callers (subscription stream vs UI thread) cannot interleave
/// their reads and writes.  The lock is released automatically when the `File` handle drops.
fn with_locked_data<F, T>(cube_name: &str, f: F) -> Result<T, String>
where
    F: FnOnce(&mut MostroData) -> Result<T, String>,
{
    let path = data_file_path(cube_name)?;
    let lock_path = path.with_extension("lock");
    let lock_file = std::fs::File::create(&lock_path)
        .map_err(|e| format!("Failed to create lock file: {e}"))?;
    lock_file
        .lock_exclusive()
        .map_err(|e| format!("Failed to acquire lock: {e}"))?;

    let mut data = load_data(cube_name)?;
    let result = f(&mut data);

    if result.is_ok() {
        save_data(cube_name, &data)?;
    }

    // Lock released on drop of `lock_file`.
    result
}

/// Append a DM message to a trade's message history on disk.
/// Deduplicates by (timestamp, action) to avoid storing the same message twice.
pub fn append_trade_message(cube_name: &str, order_id: &str, msg: TradeMessage) {
    let result = with_locked_data(cube_name, |data| {
        if let Some(session) = data.trades.iter_mut().find(|t| t.order_id == order_id) {
            let is_dup = session.messages.iter().any(|m| {
                m.timestamp == msg.timestamp
                    && m.action == msg.action
                    && m.payload_json == msg.payload_json
            });
            if !is_dup {
                session.messages.push(msg.clone());
                session.messages.sort_by_key(|m| m.timestamp);
            }
        }
        Ok(())
    });
    if let Err(e) = result {
        tracing::warn!("Failed to persist trade message: {e}");
    }
}

/// Update the counterparty's trade pubkey for a given order (persisted to disk).
pub fn set_counterparty_pubkey(cube_name: &str, order_id: &str, pubkey: &str) {
    let result = with_locked_data(cube_name, |data| {
        if let Some(session) = data.trades.iter_mut().find(|t| t.order_id == order_id) {
            if session.counterparty_trade_pubkey.as_deref() != Some(pubkey) {
                session.counterparty_trade_pubkey = Some(pubkey.to_string());
            }
        }
        Ok(())
    });
    if let Err(e) = result {
        tracing::warn!("Failed to persist counterparty pubkey: {e}");
    }
}

/// Update the admin/solver's trade pubkey for a given order (persisted to disk).
fn set_admin_pubkey(cube_name: &str, order_id: &str, pubkey: &str) {
    let result = with_locked_data(cube_name, |data| {
        if let Some(session) = data.trades.iter_mut().find(|t| t.order_id == order_id) {
            if session.admin_trade_pubkey.as_deref() != Some(pubkey) {
                session.admin_trade_pubkey = Some(pubkey.to_string());
            }
        }
        Ok(())
    });
    if let Err(e) = result {
        tracing::warn!("Failed to persist admin pubkey: {e}");
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

/// Non-protocol chat actions that should be skipped when detecting trade state.
const CHAT_ACTIONS: &[&str] = &["SendDm", "AdminDm", "chat", "dispute_chat"];

fn is_chat_action(action: &str) -> bool {
    CHAT_ACTIONS.contains(&action)
}

/// Extract the counterparty's trade pubkey from a payload (Order or PaymentRequest).
fn counterparty_from_payload(
    payload: &Option<mostro_core::message::Payload>,
    our_trade_hex: &str,
) -> Option<String> {
    let order = match payload {
        Some(mostro_core::message::Payload::Order(ref o)) => Some(o),
        Some(mostro_core::message::Payload::PaymentRequest(Some(ref o), _, _)) => Some(o),
        _ => None,
    }?;
    extract_counterparty_pubkey(order, our_trade_hex)
}

/// Get the latest protocol DM action for a trade from its message history.
/// Skips non-protocol chat entries so that chat messages do not affect
/// protocol state detection.
pub fn latest_dm_action(session: &TradeSession) -> Option<&str> {
    session
        .messages
        .iter()
        .rev()
        .find(|m| !is_chat_action(&m.action))
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
        .filter(|m| {
            m.action == "PayInvoice"
                || m.action == "WaitingSellerToPay"
                || m.action == "BuyerTookOrder"
        })
        .find_map(|m| {
            // payload_json is the serialized Option<Payload>
            // For PayInvoice/BuyerTookOrder: Some(PaymentRequest(Some(order), invoice_string, Some(amount)))
            let payload: Option<mostro_core::message::Payload> =
                match serde_json::from_str(&m.payload_json) {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::debug!("Failed to deserialize hold-invoice payload: {e}");
                        return None;
                    }
                };
            match payload {
                Some(mostro_core::message::Payload::PaymentRequest(_, invoice, _)) => Some(invoice),
                _ => None,
            }
        })
}

/// Find the timestamp of the DM that started the current countdown phase.
pub fn countdown_start_timestamp(session: &TradeSession) -> Option<u64> {
    let last_action = session
        .messages
        .iter()
        .rev()
        .find(|m| !is_chat_action(&m.action))
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

    let request_id = rand::random::<u64>();
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
    // Acquire the data lock before merging so that any in-flight subscription
    // stream events cannot interleave with this write.
    let lock_path = data_file_path(cube_name)?.with_extension("lock");
    let lock_file = std::fs::File::create(&lock_path)
        .map_err(|e| format!("Failed to create lock file: {e}"))?;
    lock_file
        .lock_exclusive()
        .map_err(|e| format!("Failed to acquire lock: {e}"))?;

    // Read current on-disk state under the lock — local writes that happened
    // while the restore was in-flight must not be silently overwritten.
    let mut data = load_data(cube_name)?;

    // Never move last_trade_index backwards, and reject out-of-range values
    // (must fit in u32 for derive_trade_keys).
    if last_trade_index > data.last_trade_index && last_trade_index <= i64::from(u32::MAX) {
        data.last_trade_index = last_trade_index;
    } else if last_trade_index > i64::from(u32::MAX) {
        tracing::warn!(
            "Restore: ignoring out-of-range last_trade_index {}",
            last_trade_index
        );
    }

    // Merge restored sessions: only add sessions for orders not already tracked
    // locally (local state may have newer DM messages / status).
    let existing_ids: HashSet<String> = data.trades.iter().map(|t| t.order_id.clone()).collect();
    for session in sessions {
        if !existing_ids.contains(&session.order_id) {
            data.trades.push(session);
        }
    }

    save_data(cube_name, &data)?;
    // Lock released on drop of lock_file.

    tracing::info!(
        "Restore: recovered {} trades, last_trade_index={}",
        count,
        last_trade_index
    );

    Ok(count)
}

fn append_trade(cube_name: &str, session: TradeSession) -> Result<(), String> {
    with_locked_data(cube_name, |data| {
        // Replace existing session for the same order (re-take after cancel uses new keys)
        if let Some(existing) = data
            .trades
            .iter_mut()
            .find(|t| t.order_id == session.order_id)
        {
            *existing = session.clone();
        } else {
            data.trades.push(session.clone());
        }
        Ok(())
    })
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
        // Known actions that don't represent status transitions
        "NewOrder" | "CantDo" | "Peer" | "RateUser" | "Orders" | "LastTradeIndex" => None,
        _ => {
            tracing::warn!("Unknown DM action: {action}");
            None
        }
    }
}

/// Derive per-trade Nostr keys (same derivation path as mostrix).
fn derive_trade_keys(mnemonic: &str, trade_index: i64) -> Result<Keys, String> {
    let account: u32 = 38383; // NOSTR_ORDER_EVENT_KIND
    Keys::from_mnemonic_advanced(
        mnemonic,
        None,
        Some(account),
        Some(
            u32::try_from(trade_index)
                .map_err(|_| format!("trade_index {trade_index} exceeds u32::MAX"))?,
        ),
        Some(0),
    )
    .map_err(|e| format!("Failed to derive trade keys: {e}"))
}

/// Info fetched from the Mostro info event (kind 38385): order limits and accepted currencies.
#[derive(Default, Clone)]
struct MostroNodeInfo {
    min_order_amount: Option<u64>,
    max_order_amount: Option<u64>,
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
                        info.max_order_amount = t[1].parse::<u64>().ok();
                    }
                    "min_order_amount" => {
                        info.min_order_amount = t[1].parse::<u64>().ok();
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
                    format_with_separators(*min),
                    format_with_separators(*max),
                ),
                _ => "Fiat amount is out of the acceptable range".into(),
            }
        }
        CantDoReason::OutOfRangeSatsAmount => {
            match (&limits.min_order_amount, &limits.max_order_amount) {
                (Some(min), Some(max)) => format!(
                    "Amount out of range — Mostro allows {} to {} sats",
                    format_with_separators(*min),
                    format_with_separators(*max),
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
    if idx < 0 || idx > i64::from(u32::MAX) {
        return Err("Server returned invalid last_trade_index".to_string());
    }

    with_locked_data(cube_name, |data| {
        tracing::info!(
            "Trade index sync: local={}, server={}",
            data.last_trade_index,
            idx
        );
        data.last_trade_index = data.last_trade_index.max(idx);
        Ok(data.last_trade_index)
    })
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
        // Critical section 1: atomically read the next trade index.
        // The lock is released before any .await so we never hold it across a yield point.
        let next_idx = with_locked_data(&form.cube_name, |data| Ok(data.last_trade_index + 1))?;

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
        let request_id = rand::random::<u64>();
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
                    next_idx - 1
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

        // Critical section 2: atomically advance trade index AND persist session.
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
            counterparty_trade_pubkey: None,
            admin_trade_pubkey: None,
        };
        with_locked_data(&form.cube_name, |data| {
            if next_idx > data.last_trade_index {
                data.last_trade_index = next_idx;
            }
            if let Some(existing) = data
                .trades
                .iter_mut()
                .find(|t| t.order_id == session.order_id)
            {
                *existing = session.clone();
            } else {
                data.trades.push(session.clone());
            }
            Ok(())
        })?;

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
    pub fiat_code: Option<String>,
    pub fiat_amount: Option<i64>,
    pub payment_method: Option<String>,
    pub premium: Option<i64>,
    pub sats_amount: Option<i64>,
    pub min_amount: Option<i64>,
    pub max_amount: Option<i64>,
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
        // Critical section 1: atomically read the next trade index.
        // The lock is released before any .await so we never hold it across a yield point.
        let next_idx = with_locked_data(&data.cube_name, |mdata| Ok(mdata.last_trade_index + 1))?;

        let request_id = rand::random::<u64>();

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
                    next_idx - 1
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

        // Critical section 2: atomically commit the incremented trade index.
        // Determine our role's order kind: if we take a sell, we're buying
        let our_kind = match data.order_type {
            OrderType::Sell => "buy",
            OrderType::Buy => "sell",
        };

        // Extract counterparty trade pubkey from the response payload
        let our_trade_hex = derive_trade_keys(&data.mnemonic, next_idx)
            .map(|k| k.public_key().to_hex())
            .unwrap_or_default();
        let counterparty_trade_pubkey = counterparty_from_payload(&inner.payload, &our_trade_hex);

        // Critical section 2: atomically advance trade index AND persist session.
        let session = TradeSession {
            order_id: data.order_id.clone(),
            trade_index: next_idx,
            kind: our_kind.to_string(),
            fiat_code: data.fiat_code.clone().unwrap_or_default(),
            fiat_amount: data.fiat_amount.unwrap_or_else(|| data.amount.unwrap_or(0)),
            min_amount: data.min_amount,
            max_amount: data.max_amount,
            amount: data.sats_amount.unwrap_or(0),
            premium: data.premium.unwrap_or(0),
            payment_method: data.payment_method.clone().unwrap_or_default(),
            created_at: chrono::Utc::now().timestamp(),
            role: "taker".to_string(),
            messages: Vec::new(),
            counterparty_trade_pubkey,
            admin_trade_pubkey: None,
        };
        with_locked_data(&data.cube_name, |mdata| {
            if next_idx > mdata.last_trade_index {
                mdata.last_trade_index = next_idx;
            }
            if let Some(existing) = mdata
                .trades
                .iter_mut()
                .find(|t| t.order_id == session.order_id)
            {
                *existing = session.clone();
            } else {
                mdata.trades.push(session.clone());
            }
            Ok(())
        })?;

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
    let sessions = load_data(&data.cube_name).unwrap_or_default().trades;
    let session = sessions
        .iter()
        .find(|s| s.order_id == data.order_id)
        .ok_or_else(|| format!("No trade session found for order {}", data.order_id))?;

    let order_uuid =
        uuid::Uuid::parse_str(&data.order_id).map_err(|e| format!("Invalid order ID: {e}"))?;

    let request_id = rand::random::<u64>();
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
    let sessions = load_data(&data.cube_name).unwrap_or_default().trades;
    let session = sessions
        .iter()
        .find(|s| s.order_id == data.order_id)
        .ok_or_else(|| format!("No trade session found for order {}", data.order_id))?;

    let order_uuid =
        uuid::Uuid::parse_str(&data.order_id).map_err(|e| format!("Invalid order ID: {e}"))?;

    let request_id = rand::random::<u64>();

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

/// Who the encrypted chat message is addressed to.
enum ChatTarget {
    Peer,
    Admin,
}

/// Send an encrypted P2P chat message via NIP-59 gift wrap.
/// The target determines which shared key (peer or admin) is used.
async fn send_encrypted_chat(data: &TradeActionData, target: ChatTarget) -> Result<(), String> {
    let text = data.invoice.as_deref().ok_or("Chat text is required")?;
    if text.trim().is_empty() {
        return Err("Empty message".to_string());
    }

    let sessions = load_data(&data.cube_name).unwrap_or_default().trades;
    let session = sessions
        .iter()
        .find(|s| s.order_id == data.order_id)
        .ok_or_else(|| format!("No trade session found for order {}", data.order_id))?;

    let (recipient_hex, label) = match target {
        ChatTarget::Peer => (
            session
                .counterparty_trade_pubkey
                .as_deref()
                .ok_or("No counterparty trade pubkey — chat not yet possible")?,
            "chat",
        ),
        ChatTarget::Admin => (
            session
                .admin_trade_pubkey
                .as_deref()
                .ok_or("No admin trade pubkey — dispute chat not yet possible")?,
            "dispute chat",
        ),
    };
    let recipient_pubkey =
        PublicKey::from_hex(recipient_hex).map_err(|e| format!("Invalid {label} pubkey: {e}"))?;

    let trade_keys = derive_trade_keys(&data.mnemonic, session.trade_index)?;
    let shared_keys = derive_shared_chat_keys(&trade_keys, &recipient_pubkey)?;
    let shared_pubkey = shared_keys.public_key();

    let inner_event = EventBuilder::text_note(text)
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
    .map_err(|e| format!("Failed to encrypt {label} message: {e}"))?;

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
        .map_err(|e| format!("Failed to send {label} message: {e}"))?;

    let _ = client.disconnect().await;

    tracing::info!("P2P {} message sent for order {}", label, data.order_id);
    Ok(())
}

/// Send a chat message to the counterparty.
pub async fn send_chat_message(data: TradeActionData) -> Result<(), String> {
    send_encrypted_chat(&data, ChatTarget::Peer).await
}

/// Send a dispute chat message to the admin/solver.
pub async fn send_admin_chat_message(data: TradeActionData) -> Result<(), String> {
    send_encrypted_chat(&data, ChatTarget::Admin).await
}

/// Get all trade messages for a given order from disk.
pub fn get_trade_messages(cube_name: &str, order_id: &str) -> Vec<TradeMessage> {
    let data = load_data(cube_name).unwrap_or_default();
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
    let data = load_data(cube_name).unwrap_or_default();
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

// ── Encrypted image attachments ─────────────────────────────────────────

const BLOSSOM_SERVERS: &[&str] = &[
    "https://blossom.primal.net",
    "https://blossom.band",
    "https://nostr.media",
];

/// Metadata for an encrypted image attachment, compatible with the Mostro mobile client.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ImageMetadata {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub blossom_url: String,
    pub nonce: String,
    pub mime_type: String,
    pub original_size: u64,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub filename: Option<String>,
    pub encrypted_size: u64,
}

/// Metadata for an encrypted file attachment, compatible with the Mostro mobile client.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FileMetadata {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub file_type: String,
    pub blossom_url: String,
    pub nonce: String,
    pub mime_type: String,
    pub original_size: u64,
    pub filename: String,
    pub encrypted_size: u64,
}

/// Parsed attachment metadata from a chat message.
#[derive(Debug, Clone)]
pub enum AttachmentMeta {
    Image(ImageMetadata),
    File(FileMetadata),
}

impl AttachmentMeta {
    pub fn blossom_url(&self) -> &str {
        match self {
            Self::Image(m) => &m.blossom_url,
            Self::File(m) => &m.blossom_url,
        }
    }

    pub fn filename(&self) -> &str {
        match self {
            Self::Image(m) => m.filename.as_deref().unwrap_or("image"),
            Self::File(m) => &m.filename,
        }
    }
}

/// Extract the inner text content from a chat payload JSON.
fn extract_payload_text(payload_json: &str) -> Option<String> {
    if let Ok(Some(mostro_core::message::Payload::TextMessage(t))) =
        serde_json::from_str::<Option<mostro_core::message::Payload>>(payload_json)
    {
        Some(t)
    } else {
        serde_json::from_str::<String>(payload_json).ok()
    }
}

/// Try to parse a chat message payload as an encrypted attachment (image or file).
pub fn parse_attachment_metadata(payload_json: &str) -> Option<AttachmentMeta> {
    let text = extract_payload_text(payload_json)?;
    // Try image first
    if let Ok(meta) = serde_json::from_str::<ImageMetadata>(&text) {
        if meta.msg_type == "image_encrypted" {
            return Some(AttachmentMeta::Image(meta));
        }
    }
    // Try file
    if let Ok(meta) = serde_json::from_str::<FileMetadata>(&text) {
        if meta.msg_type == "file_encrypted" {
            return Some(AttachmentMeta::File(meta));
        }
    }
    None
}

/// Backwards-compatible alias — returns Some only for images.
pub fn parse_image_metadata(payload_json: &str) -> Option<ImageMetadata> {
    match parse_attachment_metadata(payload_json)? {
        AttachmentMeta::Image(m) => Some(m),
        AttachmentMeta::File(_) => None,
    }
}

/// Derive the raw 32-byte ECDH shared key for ChaCha20-Poly1305 encryption.
fn derive_encryption_key(
    our_keys: &Keys,
    counterparty_pubkey: &PublicKey,
) -> Result<[u8; 32], String> {
    nostr_sdk::util::generate_shared_key(our_keys.secret_key(), counterparty_pubkey)
        .map_err(|e| format!("Failed to compute shared key: {e}"))
}

/// Encrypt image bytes with ChaCha20-Poly1305.
/// Returns blob in wire format: [nonce 12B][ciphertext][auth_tag 16B].
fn encrypt_image_blob(key: &[u8; 32], plaintext: &[u8]) -> Result<(Vec<u8>, [u8; 12]), String> {
    use chacha20poly1305::{aead::Aead, ChaCha20Poly1305, KeyInit, Nonce};

    let cipher = ChaCha20Poly1305::new(key.into());
    let nonce_bytes: [u8; 12] = rand::random();
    let nonce = Nonce::from(nonce_bytes);

    let ciphertext = cipher
        .encrypt(&nonce, plaintext)
        .map_err(|e| format!("Encryption failed: {e}"))?;

    // Wire format: [nonce][ciphertext+tag] (chacha20poly1305 appends the 16B tag to ciphertext)
    let mut blob = Vec::with_capacity(12 + ciphertext.len());
    blob.extend_from_slice(&nonce_bytes);
    blob.extend_from_slice(&ciphertext);

    Ok((blob, nonce_bytes))
}

/// Decrypt an image blob (wire format: [nonce 12B][ciphertext+tag]).
pub fn decrypt_image_blob(key: &[u8; 32], blob: &[u8]) -> Result<Vec<u8>, String> {
    use chacha20poly1305::{aead::Aead, ChaCha20Poly1305, KeyInit, Nonce};

    if blob.len() < 28 {
        return Err("Blob too small for ChaCha20-Poly1305".to_string());
    }

    let nonce = Nonce::from_slice(&blob[..12]);
    let ciphertext_and_tag = &blob[12..];

    let cipher = ChaCha20Poly1305::new(key.into());
    cipher
        .decrypt(nonce, ciphertext_and_tag)
        .map_err(|e| format!("Decryption failed: {e}"))
}

/// Upload an encrypted blob to a Blossom server with Nostr auth.
async fn upload_to_blossom(encrypted_blob: &[u8], trade_keys: &Keys) -> Result<String, String> {
    use base64::Engine as _;
    use sha2::{Digest, Sha256};

    let hash_hex = {
        let mut hasher = Sha256::new();
        hasher.update(encrypted_blob);
        hex::encode(hasher.finalize())
    };

    let timestamp = chrono::Utc::now().timestamp();
    let expiration = (timestamp + 3600).to_string();

    // Create kind-24242 Blossom auth event
    let auth_event = EventBuilder::new(nostr_sdk::Kind::Custom(24242), "Upload image")
        .tag(Tag::custom(
            nostr_sdk::TagKind::SingleLetter(nostr_sdk::SingleLetterTag::lowercase(
                nostr_sdk::Alphabet::T,
            )),
            ["upload"],
        ))
        .tag(Tag::custom(
            nostr_sdk::TagKind::SingleLetter(nostr_sdk::SingleLetterTag::lowercase(
                nostr_sdk::Alphabet::X,
            )),
            [hash_hex.as_str()],
        ))
        .tag(Tag::custom(
            nostr_sdk::TagKind::Custom("expiration".into()),
            [expiration.as_str()],
        ))
        .sign_with_keys(trade_keys)
        .map_err(|e| format!("Failed to sign Blossom auth: {e}"))?;

    let auth_json = auth_event.as_json();
    let auth_base64 = base64::engine::general_purpose::STANDARD.encode(auth_json.as_bytes());

    let http_client = reqwest::Client::new();

    for server in BLOSSOM_SERVERS {
        let url = format!("{}/upload", server);
        tracing::debug!("Uploading to Blossom: {url}");

        match http_client
            .put(&url)
            .header("Content-Type", "application/octet-stream")
            .header("Authorization", format!("Nostr {auth_base64}"))
            .body(encrypted_blob.to_vec())
            .timeout(Duration::from_secs(120))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                let blob_url = format!("{}/{}", server, hash_hex);
                tracing::info!("Blossom upload successful: {blob_url}");
                return Ok(blob_url);
            }
            Ok(resp) => {
                tracing::warn!("Blossom upload to {server} failed: {}", resp.status());
            }
            Err(e) => {
                tracing::warn!("Blossom upload to {server} error: {e}");
            }
        }
    }

    Err("All Blossom servers failed".to_string())
}

/// Download an encrypted blob from a Blossom URL.
pub async fn download_from_blossom(url: &str) -> Result<Vec<u8>, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| format!("Download failed: {e}"))?;
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("Download failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("Download failed: HTTP {}", resp.status()));
    }

    resp.bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|e| format!("Failed to read response: {e}"))
}

/// Data for sending an image attachment.
pub struct AttachmentData {
    pub file_path: std::path::PathBuf,
    pub order_id: String,
    pub cube_name: String,
    pub mnemonic: String,
    pub mostro_pubkey_hex: String,
    pub relay_urls: Vec<String>,
}

/// Send an encrypted image attachment via P2P chat.
/// Returns the JSON metadata string on success (for persisting as a TradeMessage).
pub async fn send_image_attachment(data: AttachmentData) -> Result<String, String> {
    // 1. Read and decode image
    let img_bytes =
        std::fs::read(&data.file_path).map_err(|e| format!("Failed to read image: {e}"))?;

    let img = ::image::load_from_memory(&img_bytes)
        .map_err(|e| format!("Failed to decode image: {e}"))?;

    // 2. Resize if too large (cap at 1920px longest side)
    let img = if img.width().max(img.height()) > 1920 {
        img.resize(1920, 1920, ::image::imageops::FilterType::Lanczos3)
    } else {
        img
    };

    let width = img.width();
    let height = img.height();

    // 3. Re-encode as JPEG
    let mut jpeg_buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut jpeg_buf, ::image::ImageFormat::Jpeg)
        .map_err(|e| format!("Failed to encode JPEG: {e}"))?;
    let jpeg_bytes = jpeg_buf.into_inner();
    let original_size = jpeg_bytes.len() as u64;

    // 4. Get encryption key
    let sessions = load_data(&data.cube_name).unwrap_or_default().trades;
    let session = sessions
        .iter()
        .find(|s| s.order_id == data.order_id)
        .ok_or_else(|| format!("No trade session for order {}", data.order_id))?;

    let cp_hex = session
        .counterparty_trade_pubkey
        .as_deref()
        .ok_or("No counterparty pubkey — cannot send attachment")?;
    let cp_pk =
        PublicKey::from_hex(cp_hex).map_err(|e| format!("Invalid counterparty pubkey: {e}"))?;
    let trade_keys = derive_trade_keys(&data.mnemonic, session.trade_index)?;
    let encryption_key = derive_encryption_key(&trade_keys, &cp_pk)?;

    // 5. Encrypt
    let (encrypted_blob, nonce_bytes) = encrypt_image_blob(&encryption_key, &jpeg_bytes)?;
    let encrypted_size = encrypted_blob.len() as u64;

    // 6. Upload to Blossom
    let blossom_url = upload_to_blossom(&encrypted_blob, &trade_keys).await?;

    // 7. Build metadata JSON
    let filename = data
        .file_path
        .file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("image_{}.jpg", chrono::Utc::now().timestamp()));

    let metadata = ImageMetadata {
        msg_type: "image_encrypted".to_string(),
        blossom_url,
        nonce: hex::encode(nonce_bytes),
        mime_type: "image/jpeg".to_string(),
        original_size,
        width,
        height,
        filename: Some(filename),
        encrypted_size,
    };
    let metadata_json = serde_json::to_string(&metadata)
        .map_err(|e| format!("Failed to serialize metadata: {e}"))?;

    // 8. Send metadata as chat message (reuse existing chat send infrastructure)
    let chat_data = TradeActionData {
        order_id: data.order_id,
        cube_name: data.cube_name,
        mnemonic: data.mnemonic,
        invoice: Some(metadata_json.clone()),
        mostro_pubkey_hex: data.mostro_pubkey_hex,
        relay_urls: data.relay_urls,
    };
    send_encrypted_chat(&chat_data, ChatTarget::Peer).await?;

    Ok(metadata_json)
}

/// Maximum file size for attachments (25 MB, matching mobile).
const MAX_ATTACHMENT_SIZE: u64 = 25 * 1024 * 1024;

/// Image extensions that get the image pipeline (resize + re-encode).
const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "gif", "webp"];

/// Determine the file_type category from a MIME type (matching mobile's categories).
fn file_type_from_mime(mime: &str) -> &'static str {
    if mime.starts_with("image/") {
        "image"
    } else if mime.starts_with("video/") {
        "video"
    } else {
        "document"
    }
}

/// Guess MIME type from file extension.
fn mime_from_extension(ext: &str) -> &'static str {
    match ext.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "avi" => "video/x-msvideo",
        "pdf" => "application/pdf",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        _ => "application/octet-stream",
    }
}

/// Send an encrypted file attachment (non-image) via P2P chat.
pub async fn send_file_attachment(data: AttachmentData) -> Result<String, String> {
    let file_bytes =
        std::fs::read(&data.file_path).map_err(|e| format!("Failed to read file: {e}"))?;

    if file_bytes.len() as u64 > MAX_ATTACHMENT_SIZE {
        return Err(format!(
            "File too large ({:.1} MB). Maximum is 25 MB.",
            file_bytes.len() as f64 / (1024.0 * 1024.0)
        ));
    }

    let original_size = file_bytes.len() as u64;
    let filename = data
        .file_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file")
        .to_string();
    let ext = data
        .file_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    let mime_type = mime_from_extension(ext).to_string();
    let file_type = file_type_from_mime(&mime_type).to_string();

    // Get encryption key
    let sessions = load_data(&data.cube_name).unwrap_or_default().trades;
    let session = sessions
        .iter()
        .find(|s| s.order_id == data.order_id)
        .ok_or_else(|| format!("No trade session for order {}", data.order_id))?;

    let cp_hex = session
        .counterparty_trade_pubkey
        .as_deref()
        .ok_or("No counterparty pubkey — cannot send attachment")?;
    let cp_pk =
        PublicKey::from_hex(cp_hex).map_err(|e| format!("Invalid counterparty pubkey: {e}"))?;
    let trade_keys = derive_trade_keys(&data.mnemonic, session.trade_index)?;
    let encryption_key = derive_encryption_key(&trade_keys, &cp_pk)?;

    // Encrypt
    let (encrypted_blob, nonce_bytes) = encrypt_image_blob(&encryption_key, &file_bytes)?;
    let encrypted_size = encrypted_blob.len() as u64;

    // Upload
    let blossom_url = upload_to_blossom(&encrypted_blob, &trade_keys).await?;

    // Build metadata
    let metadata = FileMetadata {
        msg_type: "file_encrypted".to_string(),
        file_type,
        blossom_url,
        nonce: hex::encode(nonce_bytes),
        mime_type,
        original_size,
        filename,
        encrypted_size,
    };
    let metadata_json = serde_json::to_string(&metadata)
        .map_err(|e| format!("Failed to serialize metadata: {e}"))?;

    // Send as chat message
    let chat_data = TradeActionData {
        order_id: data.order_id,
        cube_name: data.cube_name,
        mnemonic: data.mnemonic,
        invoice: Some(metadata_json.clone()),
        mostro_pubkey_hex: data.mostro_pubkey_hex,
        relay_urls: data.relay_urls,
    };
    send_encrypted_chat(&chat_data, ChatTarget::Peer).await?;

    Ok(metadata_json)
}

/// Send an attachment — routes to image or file pipeline based on extension.
pub async fn send_attachment(data: AttachmentData) -> Result<String, String> {
    let is_image = data
        .file_path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| IMAGE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false);

    if is_image {
        send_image_attachment(data).await
    } else {
        send_file_attachment(data).await
    }
}

/// Download and decrypt an image from a Blossom URL.
pub async fn download_and_decrypt_image(
    blossom_url: String,
    order_id: String,
    cube_name: String,
    mnemonic: String,
) -> Result<(String, String, Vec<u8>), String> {
    // Get encryption key from session
    let sessions = load_data(&cube_name).unwrap_or_default().trades;
    let session = sessions
        .iter()
        .find(|s| s.order_id == order_id)
        .ok_or_else(|| format!("No trade session for order {}", order_id))?;

    let cp_hex = session
        .counterparty_trade_pubkey
        .as_deref()
        .ok_or("No counterparty pubkey")?;
    let cp_pk =
        PublicKey::from_hex(cp_hex).map_err(|e| format!("Invalid counterparty pubkey: {e}"))?;
    let trade_keys = derive_trade_keys(&mnemonic, session.trade_index)?;
    let encryption_key = derive_encryption_key(&trade_keys, &cp_pk)?;

    // Download
    let blob = download_from_blossom(&blossom_url).await?;

    // Decrypt
    let decrypted = decrypt_image_blob(&encryption_key, &blob)?;

    Ok((order_id, blossom_url, decrypted))
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
    let sessions = load_data(cube_name).unwrap_or_default().trades;
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
    let sessions = load_data(cube_name).unwrap_or_default().trades;
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
                min_amount: order.min_amount.map(|v| v as f64),
                max_amount: order.max_amount.map(|v| v as f64),
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
        min_amount: session.min_amount.map(|v| v as f64),
        max_amount: session.max_amount.map(|v| v as f64),
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
            let mut relay_failures = 0usize;
            for url in &relay_urls {
                if let Err(e) = client.add_relay(url.as_str()).await {
                    tracing::warn!("Failed to add relay {url}: {e}");
                    relay_failures += 1;
                }
            }
            if relay_failures == relay_urls.len() {
                let _ = output
                    .send(Message::View(view::Message::P2P(P2PMessage::StreamError(
                        "Failed to connect to any Mostro relay".to_string(),
                    ))))
                    .await;
            }
            client.connect().await;
            client.wait_for_connection(Duration::from_secs(10)).await;

            if !has_connected_relay(&client).await {
                let _ = output
                    .send(Message::View(view::Message::P2P(P2PMessage::StreamError(
                        "Failed to connect to any Mostro relay".to_string(),
                    ))))
                    .await;
            }

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
                if output.send(msg).await.is_err() {
                    tracing::warn!(
                        "Failed to send P2P update to UI — channel may be full or closed"
                    );
                }
            }

            // Auto-restore: if we have a mnemonic but no local trades, scan relay for DMs
            let data = match load_data(&cube_name) {
                Ok(d) => d,
                Err(e) => {
                    tracing::error!("Failed to load P2P data: {e}");
                    MostroData::default()
                }
            };
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
                        if output.send(msg).await.is_err() {
                            tracing::warn!(
                                "Failed to send P2P update to UI — channel may be full or closed"
                            );
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Restore failed: {e}");
                        let _ = output
                            .send(Message::View(view::Message::P2P(P2PMessage::StreamError(
                                format!("Failed to restore trades: {e}"),
                            ))))
                            .await;
                    }
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
            if output
                .send(Message::View(view::Message::P2P(
                    P2PMessage::MostroOrdersReceived(orders),
                )))
                .await
                .is_err()
            {
                tracing::warn!("Failed to send P2P update to UI — channel may be full or closed");
            }
            let trades = fetch_user_trades(&client, &cube_name, mostro_pk).await;
            if output
                .send(Message::View(view::Message::P2P(
                    P2PMessage::MostroTradesReceived(trades),
                )))
                .await
                .is_err()
            {
                tracing::warn!("Failed to send P2P update to UI — channel may be full or closed");
            }

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
                                        if output
                                            .send(Message::View(view::Message::P2P(
                                                P2PMessage::MostroOrdersReceived(orders),
                                            )))
                                            .await
                                            .is_err()
                                        {
                                            tracing::warn!("Failed to send P2P update to UI — channel may be full or closed");
                                        }
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
                if output
                    .send(Message::View(view::Message::P2P(
                        P2PMessage::MostroOrdersReceived(orders),
                    )))
                    .await
                    .is_err()
                {
                    tracing::warn!(
                        "Failed to send P2P update to UI — channel may be full or closed"
                    );
                }
                let trades = fetch_user_trades(&client, &cube_name, mostro_pk).await;
                if output
                    .send(Message::View(view::Message::P2P(
                        P2PMessage::MostroTradesReceived(trades),
                    )))
                    .await
                    .is_err()
                {
                    tracing::warn!(
                        "Failed to send P2P update to UI — channel may be full or closed"
                    );
                }
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
    let session_keys = load_session_keys(cube_name, mnemonic);
    if session_keys.is_empty() {
        return;
    }

    let mut new_pubkeys: Vec<PublicKey> = Vec::new();
    for (session, keys) in &session_keys {
        let pk = keys.public_key();
        if !subscribed_pubkeys.contains(&pk) {
            new_pubkeys.push(pk);
        }
        // Also subscribe to ECDH shared keys for P2P chat and dispute chat
        for party_hex in [
            session.counterparty_trade_pubkey.as_deref(),
            session.admin_trade_pubkey.as_deref(),
        ]
        .iter()
        .copied()
        .flatten()
        {
            if let Some(hex) = shared_chat_hex(keys, Some(party_hex)) {
                if let Ok(spk) = PublicKey::from_hex(&hex) {
                    if !subscribed_pubkeys.contains(&spk) {
                        new_pubkeys.push(spk);
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
        tracing::warn!("Failed to subscribe for trade DMs: {e}");
        return;
    }

    subscribed_pubkeys.extend(new_pubkeys);
}

// ── DM processing helpers ───────────────────────────────────────────────

/// Extract the first `p` tag value from a Nostr event.
fn get_p_tag(event: &nostr_sdk::Event) -> Option<String> {
    event.tags.iter().find_map(|tag| {
        let t = tag.clone().to_vec();
        if t.len() >= 2 && t[0] == "p" {
            Some(t[1].clone())
        } else {
            None
        }
    })
}

/// Derive the ECDH shared chat public key hex for a given trade pubkey hex.
fn shared_chat_hex(keys: &Keys, trade_pubkey_hex: Option<&str>) -> Option<String> {
    let pk = PublicKey::from_hex(trade_pubkey_hex?).ok()?;
    derive_shared_chat_keys(keys, &pk)
        .ok()
        .map(|sk| sk.public_key().to_hex())
}

/// Serialize a chat text into the Mostro payload JSON format.
fn serialize_chat_payload(text: String) -> String {
    serde_json::to_string(&Some(mostro_core::message::Payload::TextMessage(text))).unwrap_or_else(
        |e| {
            tracing::warn!("Failed to serialize chat payload: {e}");
            String::new()
        },
    )
}

/// Persist a trade message to disk and optionally send a UI update.
async fn persist_and_notify(
    cube_name: &str,
    order_id: &str,
    action: String,
    payload_json: String,
    timestamp: u64,
    output: &mut iced::futures::channel::mpsc::Sender<Message>,
    silent: bool,
) {
    append_trade_message(
        cube_name,
        order_id,
        TradeMessage {
            timestamp,
            action: action.clone(),
            payload_json: payload_json.clone(),
            is_own: false,
        },
    );
    if !silent {
        let msg = Message::View(view::Message::P2P(P2PMessage::TradeUpdate {
            order_id: order_id.to_string(),
            action,
            payload_json,
        }));
        let _ = output.send(msg).await;
    }
}

/// Result of processing a single DM event against one session.
enum DmResult {
    /// No match for this session — try the next one.
    NoMatch,
    /// Matched and processed; bool = new counterparty discovered.
    Handled(bool),
    /// CantDo received — skip without further processing.
    CantDo,
}

/// Core logic for processing a single gift-wrap event against a single trade session.
/// Shared by both the batch (`process_dm_notifications`) and real-time (`process_dm_event`) paths.
#[allow(clippy::too_many_arguments)]
async fn process_event_for_session(
    event: &nostr_sdk::Event,
    session: &TradeSession,
    keys: &Keys,
    cube_name: &str,
    mostro_pubkey: PublicKey,
    seen_event_ids: &mut HashSet<nostr_sdk::EventId>,
    output: &mut iced::futures::channel::mpsc::Sender<Message>,
    silent: bool,
) -> DmResult {
    let our_trade_hex = keys.public_key().to_hex();
    let cp_shared_hex = shared_chat_hex(keys, session.counterparty_trade_pubkey.as_deref());
    let admin_shared_hex = shared_chat_hex(keys, session.admin_trade_pubkey.as_deref());

    // Match p-tag against our trade pubkey, shared chat pubkey, or admin shared pubkey
    let p_tag = get_p_tag(event);
    let is_mostro_dm = p_tag.as_deref() == Some(&our_trade_hex);
    let is_chat_dm = cp_shared_hex
        .as_deref()
        .is_some_and(|sh| p_tag.as_deref() == Some(sh));
    let is_admin_dm = admin_shared_hex
        .as_deref()
        .is_some_and(|sh| p_tag.as_deref() == Some(sh));
    if !is_mostro_dm && !is_chat_dm && !is_admin_dm {
        return DmResult::NoMatch;
    }

    // ── Mostro protocol message (NIP-59: gift wrap → seal → rumor) ──
    if let Ok(unwrapped) = nip59::extract_rumor(keys, event).await {
        if unwrapped.rumor.pubkey == mostro_pubkey {
            let mut new_cp = false;
            if let Ok((msg, _)) = serde_json::from_str::<(
                mostro_core::message::Message,
                Option<String>,
            )>(&unwrapped.rumor.content)
            {
                let inner = msg.get_inner_message_kind();
                let action = format!("{:?}", inner.action);
                let payload_json = match serde_json::to_string(&inner.payload) {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!("Failed to serialize DM payload: {e}");
                        String::new()
                    }
                };

                seen_event_ids.insert(event.id);

                if inner.action == mostro_core::message::Action::CantDo {
                    tracing::debug!(
                        "Skipping CantDo DM for order {} (not a state transition)",
                        session.order_id
                    );
                    return DmResult::CantDo;
                }

                // Extract counterparty trade pubkey from Order or PaymentRequest payloads
                if let Some(cp_pk) = counterparty_from_payload(&inner.payload, &our_trade_hex) {
                    if session.counterparty_trade_pubkey.as_deref() != Some(&cp_pk) {
                        new_cp = true;
                    }
                    set_counterparty_pubkey(cube_name, &session.order_id, &cp_pk);
                }

                // Extract admin pubkey from AdminTookDispute Peer payload
                if inner.action == mostro_core::message::Action::AdminTookDispute {
                    if let Some(mostro_core::message::Payload::Peer(ref peer)) = inner.payload {
                        if session.admin_trade_pubkey.as_deref() != Some(&peer.pubkey) {
                            new_cp = true;
                        }
                        set_admin_pubkey(cube_name, &session.order_id, &peer.pubkey);
                    }
                }

                let rumor_ts = unwrapped.rumor.created_at.as_u64();
                persist_and_notify(
                    cube_name,
                    &session.order_id,
                    action,
                    payload_json,
                    rumor_ts,
                    output,
                    silent,
                )
                .await;
            }
            return DmResult::Handled(new_cp);
        }
    }

    // ── P2P chat message (2-layer: gift wrap → signed event via ECDH shared key) ──
    if let Some(chat_msg) = try_decrypt_chat(
        event,
        keys,
        session.counterparty_trade_pubkey.as_deref(),
        session.counterparty_trade_pubkey.as_deref(),
    ) {
        seen_event_ids.insert(event.id);
        persist_and_notify(
            cube_name,
            &session.order_id,
            "SendDm".to_string(),
            serialize_chat_payload(chat_msg.text),
            chat_msg.timestamp,
            output,
            silent,
        )
        .await;
        return DmResult::Handled(false);
    }

    // ── Dispute chat message from admin (same 2-layer, different shared key) ──
    if let Some(chat_msg) = try_decrypt_chat(
        event,
        keys,
        session.admin_trade_pubkey.as_deref(),
        session.admin_trade_pubkey.as_deref(),
    ) {
        seen_event_ids.insert(event.id);
        persist_and_notify(
            cube_name,
            &session.order_id,
            "AdminDm".to_string(),
            serialize_chat_payload(chat_msg.text),
            chat_msg.timestamp,
            output,
            silent,
        )
        .await;
        return DmResult::Handled(false);
    }

    DmResult::NoMatch
}

/// Decrypted chat message content.
struct DecryptedChat {
    text: String,
    timestamp: u64,
}

/// Try to decrypt a 2-layer chat message using the ECDH shared key with the given party.
/// Returns `Some` if decryption succeeds and the sender matches the expected pubkey.
fn try_decrypt_chat(
    event: &nostr_sdk::Event,
    keys: &Keys,
    party_trade_hex: Option<&str>,
    expected_sender_hex: Option<&str>,
) -> Option<DecryptedChat> {
    let party_pk = PublicKey::from_hex(party_trade_hex?).ok()?;
    let shared = derive_shared_chat_keys(keys, &party_pk).ok()?;
    let decrypted =
        nostr_sdk::prelude::nip44::decrypt(shared.secret_key(), &event.pubkey, &event.content)
            .ok()?;
    let inner_event = nostr_sdk::Event::from_json(&decrypted).ok()?;
    let sender_hex = inner_event.pubkey.to_hex();
    if expected_sender_hex != Some(&sender_hex) {
        return None;
    }
    Some(DecryptedChat {
        text: inner_event.content.clone(),
        timestamp: inner_event.created_at.as_u64(),
    })
}

/// Load sessions and derive trade keys, returning session-key pairs.
fn load_session_keys(cube_name: &str, mnemonic: &str) -> Vec<(TradeSession, Keys)> {
    let data = match load_data(cube_name) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("Failed to load session data for {cube_name}: {e}");
            return Vec::new();
        }
    };
    data.trades
        .iter()
        .filter_map(|s| {
            derive_trade_keys(mnemonic, s.trade_index)
                .ok()
                .map(|k| (s.clone(), k))
        })
        .collect()
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
    let session_keys = load_session_keys(cube_name, mnemonic);
    if session_keys.is_empty() {
        return;
    }

    // Fetch gift-wrap events for both trade pubkeys AND shared chat pubkeys
    let mut all_pubkeys: Vec<PublicKey> =
        session_keys.iter().map(|(_, k)| k.public_key()).collect();
    for (session, keys) in &session_keys {
        for party_hex in [
            session.counterparty_trade_pubkey.as_deref(),
            session.admin_trade_pubkey.as_deref(),
        ]
        .iter()
        .copied()
        .flatten()
        {
            if let Some(hex) = shared_chat_hex(keys, Some(party_hex)) {
                if let Ok(pk) = PublicKey::from_hex(&hex) {
                    all_pubkeys.push(pk);
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
        if seen_event_ids.contains(&event.id) {
            continue;
        }
        for (session, keys) in &session_keys {
            match process_event_for_session(
                event,
                session,
                keys,
                cube_name,
                mostro_pubkey,
                seen_event_ids,
                output,
                silent,
            )
            .await
            {
                DmResult::Handled(_) | DmResult::CantDo => break,
                DmResult::NoMatch => continue,
            }
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
    let session_keys = load_session_keys(cube_name, mnemonic);

    for (session, keys) in &session_keys {
        match process_event_for_session(
            event,
            session,
            keys,
            cube_name,
            mostro_pubkey,
            seen_event_ids,
            output,
            false, // never silent in real-time
        )
        .await
        {
            DmResult::Handled(new_cp) => return new_cp,
            DmResult::CantDo => return false,
            DmResult::NoMatch => continue,
        }
    }
    false
}
