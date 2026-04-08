use iced::futures::{self, SinkExt, TryStreamExt};
use reqwest_sse::EventSource as _;
use serde::Deserialize;

use super::api::{MavapayCurrency, TransactionStatus};
use super::MavapayMessage;

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

/// Creates an SSE stream that monitors transaction status updates.
pub fn transaction_stream(
    data: &(String, String),
) -> impl iced::futures::Stream<Item = MavapayMessage> + 'static {
    #[cfg(debug_assertions)]
    let base_url = "https://dev-events.coincube.io";
    #[cfg(not(debug_assertions))]
    let base_url = env!("EVENTS_API_URL");

    let (order_id, user_jwt) = data;
    let auth = format!("Bearer {}", user_jwt);
    let url = format!("{}/api/v1/mavapay/stream/transactions", base_url);

    // Attempt to parse parameters
    let init = match reqwest::Url::parse(&url) {
        Ok(url) => match auth.parse() {
            Ok(a) => {
                let mut req = reqwest::Request::new(reqwest::Method::GET, url);
                req.headers_mut().append("Authorization", a);
                Some((req, order_id.clone()))
            }
            Err(err) => {
                log::error!("[MAVAPAY] Unable to start subscription, {}", err);
                None
            }
        },
        Err(err) => {
            log::error!("[MAVAPAY] Unable to start subscription, {:?}", err);
            None
        }
    };

    log::trace!(
        "[MAVAPAY] Starting subscription execution for order: {}",
        order_id
    );

    iced::stream::channel(
        32,
        |mut channel: iced::futures::channel::mpsc::Sender<MavapayMessage>| async move {
            if let Some((request, order_id)) = init {
                // Send request
                match reqwest::Client::new().execute(request).await {
                    // Query event source
                    Ok(res) => {
                        log::trace!("[MAVAPAY] EventSource pre-request was successful");
                        let _ = channel.send(MavapayMessage::StreamConnected).await;

                        match res.events().await {
                            Ok(mut source) => loop {
                                let timeout =
                                    tokio::time::sleep(std::time::Duration::from_secs(60));
                                let event = TryStreamExt::try_next(&mut source);

                                futures::pin_mut!(timeout);
                                futures::pin_mut!(event);

                                match futures::future::select(timeout, event).await {
                                    futures::future::Either::Left(_) => {
                                        let _ = channel
                                        .send(MavapayMessage::EventSourceDisconnected(
                                            "EventSource heartbeat failure, client is probably offline".to_string(),
                                        ))
                                        .await;

                                        break;
                                    }
                                    futures::future::Either::Right((event, _)) => match event {
                                        Ok(Some(ev)) => {
                                            match ev.event_type.as_str() {
                                                "transactionUpdate" => {
                                                    match serde_json::from_str::<TransactionUpdate>(
                                                        &ev.data,
                                                    ) {
                                                        Ok(update)
                                                            if update.order_id == order_id =>
                                                        {
                                                            log::info!(
                                                        "[MAVAPAY] Update for order {}: status={:?}",
                                                        update.order_id,
                                                        update.status
                                                    );
                                                            let _ = channel
                                                            .send(
                                                                MavapayMessage::TransactionUpdated(
                                                                    update,
                                                                ),
                                                            )
                                                            .await;
                                                        }
                                                        Ok(_) => continue, // Different order, ignore
                                                        Err(e) => {
                                                            log::warn!(
                                                            "[MAVAPAY] Failed to parse update: {}",
                                                            e
                                                        );
                                                            break;
                                                        }
                                                    }
                                                }
                                                "connected" | "ping" => continue,
                                                _type => {
                                                    log::warn!("[MAVAPAY] Ignored event: {:?}", ev);
                                                    continue;
                                                }
                                            }
                                        }
                                        Ok(None) => {
                                            log::info!("[MAVAPAY] EventSource exiting safely");
                                            break;
                                        }
                                        Err(err) => {
                                            let _ = channel
                                                .send(MavapayMessage::EventSourceDisconnected(
                                                    err.to_string(),
                                                ))
                                                .await;

                                            break;
                                        }
                                    },
                                }
                            },
                            Err(err) => {
                                log::error!("[MAVAPAY] Failed to get event source: {}", err);
                                let _ = channel
                                    .send(MavapayMessage::StreamError(err.to_string()))
                                    .await;
                            }
                        }
                    }
                    Err(err) => {
                        log::error!("[MAVAPAY] EventSource pre-request failed: {}", err);
                        let _ = channel
                            .send(MavapayMessage::StreamError(err.to_string()))
                            .await;
                    }
                };
            };
        },
    )
}
