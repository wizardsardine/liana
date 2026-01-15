use iced::futures::{Stream, StreamExt};
use serde::Deserialize;

use super::api::{MavapayCurrency, TransactionStatus};
use crate::services::sse::{sse_stream, SseConfig, SseStreamEvent};

/// Transaction update event from the SSE stream
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionUpdate {
    pub amount: u64,
    pub created_at: String,
    pub currency: MavapayCurrency,
    pub event_type: String,
    pub order_hash: String,
    pub order_id: String,
    pub status: TransactionStatus,
    pub transaction_id: u64,
}

/// SSE event received from the Mavapay transaction stream
#[derive(Debug, Clone)]
pub enum TransactionStreamEvent {
    TransactionUpdated(TransactionUpdate),
    Connected,
    Ping,
    Error(String),
    Disconnected,
}

/// Configuration for the transaction stream
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TransactionStreamConfig {
    pub base_url: String,
    pub auth_token: String,
    pub order_id: String,
}

/// Creates an SSE stream that monitors transaction status updates.
pub fn transaction_stream(
    config: TransactionStreamConfig,
) -> impl Stream<Item = TransactionStreamEvent> {
    let url = format!("{}/api/v1/mavapay/stream/transactions", config.base_url);
    let sse_config = SseConfig::new(url).with_bearer_token(&config.auth_token);

    log::info!(
        "[MAVAPAY SSE] Connecting to stream for order: {}",
        config.order_id
    );

    sse_stream(sse_config).filter_map(move |event| {
        let order_id = config.order_id.clone();
        async move {
            match event {
                SseStreamEvent::Connected => {
                    log::info!("[MAVAPAY SSE] Connected");
                    Some(TransactionStreamEvent::Connected)
                }
                SseStreamEvent::Event(sse_event) => {
                    let event_type = sse_event.event_type.as_deref();
                    let data = sse_event.data.as_deref();

                    match (event_type, data) {
                        (Some("ping"), _) => Some(TransactionStreamEvent::Ping),
                        (Some("transactionUpdate"), Some(data)) => {
                            match serde_json::from_str::<TransactionUpdate>(data) {
                                Ok(update) if update.order_id == order_id => {
                                    log::info!(
                                        "[MAVAPAY SSE] Update for order {}: status={:?}",
                                        update.order_id,
                                        update.status
                                    );
                                    Some(TransactionStreamEvent::TransactionUpdated(update))
                                }
                                Ok(_) => None, // Different order, ignore
                                Err(e) => {
                                    log::warn!("[MAVAPAY SSE] Failed to parse update: {}", e);
                                    None
                                }
                            }
                        }
                        _ => None,
                    }
                }
                SseStreamEvent::Error(e) => {
                    log::error!("[MAVAPAY SSE] Error: {}", e);
                    Some(TransactionStreamEvent::Error(e))
                }
                SseStreamEvent::Disconnected => {
                    log::info!("[MAVAPAY SSE] Disconnected");
                    Some(TransactionStreamEvent::Disconnected)
                }
            }
        }
    })
}
