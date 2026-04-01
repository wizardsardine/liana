use std::sync::Arc;
use std::time::Duration;

use iced::futures::SinkExt;
use tokio::sync::RwLock;

use crate::services::connect::client::auth::AccessTokenResponse;

use super::connect_v1::{
    realtime_service_client::RealtimeServiceClient, stream_envelope::Body, ClientAck, ClientHello,
    DevicePlatform, Pong, StreamEnvelope,
};
use super::interceptor::AuthInterceptor;
use super::ConnectStreamMessage;

/// Configuration for the realtime gRPC stream.
#[derive(Debug, Clone)]
pub struct ConnectStreamConfig {
    pub grpc_url: String,
    pub tokens: Arc<RwLock<AccessTokenResponse>>,
    pub device_id: String,
    pub user_agent: String,
    pub vault_ids: Vec<String>,
    pub last_seen_seq: i64,
}

/// Creates a persistent gRPC realtime stream following the Iced `stream::channel` pattern.
///
/// Automatically reconnects with exponential backoff on disconnection.
/// Sends `ClientHello` on connect, `ClientAck` for session events, and `Pong` for pings.
pub fn connect_stream(
    data: &ConnectStreamConfig,
) -> impl iced::futures::Stream<Item = ConnectStreamMessage> + 'static {
    let config = data.clone();

    iced::stream::channel(
        64,
        |mut channel: iced::futures::channel::mpsc::Sender<ConnectStreamMessage>| async move {
            let mut backoff = Duration::from_secs(1);
            let max_backoff = Duration::from_secs(30);
            let mut last_seen_seq = config.last_seen_seq;

            loop {
                match super::create_channel(&config.grpc_url).await {
                    Ok(grpc_channel) => {
                        let interceptor = AuthInterceptor::new(config.tokens.clone());
                        let mut client =
                            RealtimeServiceClient::with_interceptor(grpc_channel, interceptor);

                        // Set up bidirectional stream
                        let (tx, rx) = tokio::sync::mpsc::channel::<StreamEnvelope>(64);
                        let hello = StreamEnvelope {
                            body: Some(Body::ClientHello(ClientHello {
                                device_id: config.device_id.clone(),
                                platform: DevicePlatform::Desktop as i32,
                                user_agent: config.user_agent.clone(),
                                subscribe_vault_ids: config.vault_ids.clone(),
                                last_seen_event_seq: last_seen_seq,
                            })),
                        };
                        if tx.send(hello).await.is_err() {
                            break;
                        }

                        let outbound = tokio_stream::wrappers::ReceiverStream::new(rx);
                        match client.connect(outbound).await {
                            Ok(response) => {
                                let _ = channel.send(ConnectStreamMessage::Connected).await;
                                backoff = Duration::from_secs(1); // Reset on success

                                let mut inbound = response.into_inner();
                                loop {
                                    match inbound.message().await {
                                        Ok(Some(envelope)) => match envelope.body {
                                            Some(Body::SessionEvent(event)) => {
                                                last_seen_seq = event.event_seq;
                                                // Acknowledge receipt
                                                let _ = tx
                                                    .send(StreamEnvelope {
                                                        body: Some(Body::ClientAck(ClientAck {
                                                            event_seq: event.event_seq,
                                                        })),
                                                    })
                                                    .await;
                                                let _ = channel
                                                    .send(ConnectStreamMessage::SessionEvent(event))
                                                    .await;
                                            }
                                            Some(Body::Ping(_)) => {
                                                let _ = tx
                                                    .send(StreamEnvelope {
                                                        body: Some(Body::Pong(Pong {
                                                            ts_unix_ms: chrono::Utc::now()
                                                                .timestamp_millis(),
                                                        })),
                                                    })
                                                    .await;
                                            }
                                            Some(Body::Error(err)) => {
                                                let _ = channel
                                                    .send(ConnectStreamMessage::Error(format!(
                                                        "{}: {}",
                                                        err.code, err.message
                                                    )))
                                                    .await;
                                            }
                                            _ => {}
                                        },
                                        Ok(None) => {
                                            // Stream ended gracefully
                                            break;
                                        }
                                        Err(e) => {
                                            log::error!("[CONNECT GRPC] Stream error: {}", e);
                                            break;
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                let _ = channel
                                    .send(ConnectStreamMessage::Error(e.to_string()))
                                    .await;
                            }
                        }
                    }
                    Err(e) => {
                        let _ = channel
                            .send(ConnectStreamMessage::Error(e.to_string()))
                            .await;
                    }
                }

                // Disconnected — reconnect with exponential backoff
                let _ = channel
                    .send(ConnectStreamMessage::Disconnected(
                        "Stream disconnected, reconnecting...".into(),
                    ))
                    .await;
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(max_backoff);
            }
        },
    )
}
