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
            let mut attempt: u64 = 0;

            loop {
                match super::create_channel(&config.grpc_url).await {
                    Ok(grpc_channel) => {
                        let access_token = config.tokens.read().await.access_token.clone();
                        let interceptor = AuthInterceptor::new(&access_token);
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
                                log::info!("[CONNECT GRPC] Stream connected");
                                if let Err(e) = channel.send(ConnectStreamMessage::Connected).await
                                {
                                    log::warn!(
                                        "[CONNECT GRPC] Failed to forward Connected event: {}",
                                        e
                                    );
                                }
                                backoff = Duration::from_secs(1); // Reset on success
                                // Track *consecutive* failures so the
                                // warn-every-16 log below flags a wedged
                                // loop, not cumulative lifetime flaps.
                                attempt = 0;

                                let mut inbound = response.into_inner();
                                loop {
                                    match inbound.message().await {
                                        Ok(Some(envelope)) => match envelope.body {
                                            Some(Body::SessionEvent(event)) => {
                                                if let Err(e) = tx
                                                    .send(StreamEnvelope {
                                                        body: Some(Body::ClientAck(ClientAck {
                                                            event_seq: event.event_seq,
                                                        })),
                                                    })
                                                    .await
                                                {
                                                    log::warn!(
                                                        "[CONNECT GRPC] Outbound channel closed while sending ClientAck (seq {}): {}; reconnecting",
                                                        event.event_seq,
                                                        e
                                                    );
                                                    break;
                                                }
                                                last_seen_seq = event.event_seq;
                                                if let Err(e) = channel
                                                    .send(ConnectStreamMessage::SessionEvent(event))
                                                    .await
                                                {
                                                    log::warn!(
                                                        "[CONNECT GRPC] Failed to forward SessionEvent: {}",
                                                        e
                                                    );
                                                }
                                            }
                                            Some(Body::Ping(_)) => {
                                                if let Err(e) = tx
                                                    .send(StreamEnvelope {
                                                        body: Some(Body::Pong(Pong {
                                                            ts_unix_ms: chrono::Utc::now()
                                                                .timestamp_millis(),
                                                        })),
                                                    })
                                                    .await
                                                {
                                                    log::warn!(
                                                        "[CONNECT GRPC] Outbound channel closed while sending Pong: {}; reconnecting",
                                                        e
                                                    );
                                                    break;
                                                }
                                            }
                                            Some(Body::Error(err)) => {
                                                if let Err(e) = channel
                                                    .send(ConnectStreamMessage::Error(format!(
                                                        "{}: {}",
                                                        err.code, err.message
                                                    )))
                                                    .await
                                                {
                                                    log::warn!(
                                                        "[CONNECT GRPC] Failed to forward Error event: {}",
                                                        e
                                                    );
                                                }
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
                                // Distinguish auth failures so the App
                                // can prompt re-login rather than
                                // tight-looping reconnects against the
                                // same expired bearer. The shared
                                // `Arc<RwLock<AccessTokenResponse>>`
                                // gets refreshed automatically by the
                                // REST `BackendClient` on its next
                                // call — but if that path doesn't fire
                                // soon enough, the stream stays stuck
                                // in this branch.
                                let msg = if e.code() == tonic::Code::Unauthenticated {
                                    format!(
                                        "Connect session expired. Sign in again to resume \
                                         real-time updates ({}).",
                                        e.message(),
                                    )
                                } else {
                                    e.to_string()
                                };
                                if let Err(send_err) =
                                    channel.send(ConnectStreamMessage::Error(msg)).await
                                {
                                    log::warn!(
                                        "[CONNECT GRPC] Failed to forward connect Error: {}",
                                        send_err
                                    );
                                }
                            }
                        }
                    }
                    Err(e) => {
                        if let Err(send_err) = channel
                            .send(ConnectStreamMessage::Error(e.to_string()))
                            .await
                        {
                            log::warn!(
                                "[CONNECT GRPC] Failed to forward channel Error: {}",
                                send_err
                            );
                        }
                    }
                }

                // Disconnected — reconnect with exponential backoff + jitter.
                if let Err(e) = channel
                    .send(ConnectStreamMessage::Disconnected(
                        "Stream disconnected, reconnecting...".into(),
                    ))
                    .await
                {
                    log::warn!("[CONNECT GRPC] Failed to forward Disconnected event: {}", e);
                }
                // Jitter: scale the nominal backoff by a random factor in
                // [0.5, 1.0). Without this every desktop in a fleet
                // would reconnect at exactly the same offset after an
                // API restart (a thundering herd against the gRPC
                // gateway). The half-open window is deliberate — we
                // never want a *longer* sleep than the configured cap,
                // only a shorter one.
                let jitter_factor = 0.5 + (rand::random::<f64>() * 0.5);
                let sleep_for = backoff.mul_f64(jitter_factor);
                attempt = attempt.saturating_add(1);
                // Log at warn every 16 attempts so operators can spot
                // a wedged retry loop in production logs without
                // drowning normal flap noise. The first 15 attempts
                // log at debug.
                if attempt.is_multiple_of(16) {
                    log::warn!(
                        "[CONNECT GRPC] Reconnect attempt #{} (sleep {:?}, nominal {:?})",
                        attempt,
                        sleep_for,
                        backoff,
                    );
                } else {
                    log::debug!(
                        "[CONNECT GRPC] Reconnecting in {:?} (jittered from {:?})",
                        sleep_for,
                        backoff,
                    );
                }
                tokio::time::sleep(sleep_for).await;
                backoff = (backoff * 2).min(max_backoff);
            }
        },
    )
}

/// Pure jitter helper exposed for unit testing the backoff math. Given
/// a nominal backoff `d` returns a value in `[d/2, d)` — never longer
/// than `d`, so callers can rely on the configured cap as a true upper
/// bound on the sleep duration.
#[cfg(test)]
fn jittered(d: Duration) -> Duration {
    let factor = 0.5 + (rand::random::<f64>() * 0.5);
    d.mul_f64(factor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jitter_stays_within_half_open_window() {
        let nominal = Duration::from_secs(8);
        for _ in 0..1000 {
            let s = jittered(nominal);
            assert!(
                s >= nominal / 2,
                "jittered sleep {:?} fell below the nominal/2 floor",
                s
            );
            assert!(
                s < nominal,
                "jittered sleep {:?} exceeded the nominal ceiling",
                s
            );
        }
    }

    #[test]
    fn backoff_cap_holds_under_repeated_doubling() {
        // Mirror the loop body: cap at 30s, double each iteration.
        let mut backoff = Duration::from_secs(1);
        let max = Duration::from_secs(30);
        for _ in 0..20 {
            backoff = (backoff * 2).min(max);
            assert!(backoff <= max);
        }
        assert_eq!(backoff, max);
    }
}
