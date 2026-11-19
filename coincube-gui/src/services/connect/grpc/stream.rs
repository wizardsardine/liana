use std::sync::Arc;
use std::time::Duration;

use iced::futures::{SinkExt, StreamExt};
use tokio::sync::RwLock;

use crate::services::connect::client::auth::AccessTokenResponse;

use super::connect_v1::{
    realtime_service_client::RealtimeServiceClient, stream_envelope::Body, ClientAck, ClientHello,
    DevicePlatform, Ping, Pong, StreamEnvelope,
};
use super::interceptor::AuthInterceptor;
use super::trouble::{self, Refusal, Trouble};
use super::ConnectStreamMessage;

#[cfg(test)]
#[path = "stream_tests.rs"]
mod transport_tests;

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
/// Sends `ClientHello` on connect, heartbeats while idle, `ClientAck` for session
/// events, and `Pong` for server pings.
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
                let refused = match run_connect_attempt(
                    &config,
                    &mut channel,
                    &mut last_seen_seq,
                    &mut backoff,
                    &mut attempt,
                )
                .await
                {
                    StreamAttemptOutcome::Superseded => return,
                    StreamAttemptOutcome::Refused(refusal) => Some(refusal),
                    StreamAttemptOutcome::Disconnected => None,
                };

                // No `Disconnected` after a refusal: the status stays on it
                // instead of flipping back to "reconnecting".
                match refused {
                    // Removed from the account: never retried. The App signs
                    // out, which drops this subscription.
                    Some(Refusal::DeviceRevoked) => {
                        std::future::pending::<()>().await;
                    }
                    // The App renews the registration, which normally
                    // restarts this subscription under the new id. Re-check
                    // in case the renewal kept the same id.
                    Some(Refusal::DeviceUnknown) => {
                        tokio::time::sleep(UNKNOWN_RECHECK).await;
                        continue;
                    }
                    // A server without reasons maps a failed device lookup
                    // (a DB blip included) to the same denial, so re-check
                    // the same id slowly; such a blip must still heal
                    // without minting a new device id.
                    Some(Refusal::Unexplained) => {
                        tokio::time::sleep(REJECTED_RECHECK).await;
                        continue;
                    }
                    None => {}
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

type ConnectEventSender = iced::futures::channel::mpsc::Sender<ConnectStreamMessage>;

enum StreamAttemptOutcome {
    Disconnected,
    Refused(Refusal),
    Superseded,
}

async fn run_connect_attempt(
    config: &ConnectStreamConfig,
    channel: &mut ConnectEventSender,
    last_seen_seq: &mut i64,
    backoff: &mut Duration,
    attempt: &mut u64,
) -> StreamAttemptOutcome {
    let grpc_channel =
        tokio::time::timeout(CONNECT_TIMEOUT, super::create_channel(&config.grpc_url))
            .await
            .map_err(|_| {
                log::warn!("[CONNECT GRPC] Transport connection timed out");
                Trouble::Unreachable
            })
            .and_then(|result| {
                result.map_err(|e| {
                    log::warn!("[CONNECT GRPC] Transport error: {}", e);
                    trouble::trouble(&e)
                })
            });
    match grpc_channel {
        Ok(grpc_channel) => {
            let access_token = config.tokens.read().await.access_token.clone();
            let interceptor = AuthInterceptor::new(&access_token);
            let mut client = RealtimeServiceClient::with_interceptor(grpc_channel, interceptor);

            // Set up bidirectional stream
            let (tx, rx) = tokio::sync::mpsc::channel::<StreamEnvelope>(64);
            let hello = StreamEnvelope {
                body: Some(Body::ClientHello(ClientHello {
                    device_id: config.device_id.clone(),
                    platform: DevicePlatform::Desktop as i32,
                    user_agent: config.user_agent.clone(),
                    subscribe_vault_ids: config.vault_ids.clone(),
                    last_seen_event_seq: *last_seen_seq,
                })),
            };
            let outbound = outbound_stream(hello, rx);
            let response = tokio::time::timeout(CONNECT_TIMEOUT, client.connect(outbound))
                .await
                .unwrap_or_else(|_| {
                    Err(tonic::Status::deadline_exceeded(
                        "Connect opening handshake timed out",
                    ))
                });
            match response {
                Ok(response) => {
                    log::info!("[CONNECT GRPC] Stream connected");
                    if let Err(e) = channel.send(ConnectStreamMessage::Connected).await {
                        log::warn!("[CONNECT GRPC] Failed to forward Connected event: {}", e);
                    }
                    // Reset on success: backoff so the next
                    // disconnect retries fast, and `attempt`
                    // so the warn-every-16 log below flags a
                    // wedged loop of *consecutive* failures
                    // rather than cumulative lifetime flaps.
                    *backoff = Duration::from_secs(1);
                    *attempt = 0;

                    return read_connect_stream(response.into_inner(), &tx, channel, last_seen_seq)
                        .await;
                }
                Err(e) => return report_open_attempt_failure(channel, &e).await,
            }
        }
        Err(trouble) => {
            if let Err(send_err) = channel.send(ConnectStreamMessage::Trouble(trouble)).await {
                log::warn!(
                    "[CONNECT GRPC] Failed to forward channel trouble: {}",
                    send_err
                );
            }
        }
    }
    StreamAttemptOutcome::Disconnected
}

async fn report_open_attempt_failure(
    channel: &mut ConnectEventSender,
    e: &tonic::Status,
) -> StreamAttemptOutcome {
    if e.code() == tonic::Code::Aborted && e.message().contains("superseded") {
        log::info!(
            "[CONNECT GRPC] Connect attempt superseded by newer connection; stopping stale stream"
        );
        return StreamAttemptOutcome::Superseded;
    }
    if let Some(refusal) = trouble::refusal(e) {
        refuse_device(channel, e, refusal).await;
        return StreamAttemptOutcome::Refused(refusal);
    } else {
        log::warn!("[CONNECT GRPC] Connect attempt failed: {}", e);
        report_failure(channel, e).await;
    }
    StreamAttemptOutcome::Disconnected
}

async fn read_connect_stream(
    mut inbound: tonic::Streaming<StreamEnvelope>,
    tx: &tokio::sync::mpsc::Sender<StreamEnvelope>,
    channel: &mut ConnectEventSender,
    last_seen_seq: &mut i64,
) -> StreamAttemptOutcome {
    loop {
        let message = tokio::time::timeout(HEARTBEAT_TIMEOUT, inbound.message())
            .await
            .unwrap_or_else(|_| {
                Err(tonic::Status::deadline_exceeded(
                    "Connect live updates stopped responding",
                ))
            });
        match message {
            Ok(Some(envelope)) => {
                if !forward_inbound_envelope(envelope, tx, channel, last_seen_seq).await {
                    return StreamAttemptOutcome::Disconnected;
                }
            }
            Ok(None) => {
                // Stream ended gracefully
                return StreamAttemptOutcome::Disconnected;
            }
            Err(e) => {
                if e.code() == tonic::Code::Aborted && e.message().contains("superseded") {
                    log::info!(
                        "[CONNECT GRPC] Stream superseded by newer connection; stopping stale stream"
                    );
                    return StreamAttemptOutcome::Superseded;
                }
                if let Some(refusal) = trouble::refusal(&e) {
                    // Refused mid-stream.
                    refuse_device(channel, &e, refusal).await;
                    return StreamAttemptOutcome::Refused(refusal);
                }
                log::error!("[CONNECT GRPC] Stream error: {}", e);
                report_failure(channel, &e).await;
                return StreamAttemptOutcome::Disconnected;
            }
        }
    }
}

async fn forward_inbound_envelope(
    envelope: StreamEnvelope,
    tx: &tokio::sync::mpsc::Sender<StreamEnvelope>,
    channel: &mut ConnectEventSender,
    last_seen_seq: &mut i64,
) -> bool {
    match envelope.body {
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
                return false;
            }
            *last_seen_seq = event.event_seq;
            if let Err(e) = channel
                .send(ConnectStreamMessage::SessionEvent(event))
                .await
            {
                log::warn!("[CONNECT GRPC] Failed to forward SessionEvent: {}", e);
            }
        }
        Some(Body::Ping(_)) => {
            if let Err(e) = tx
                .send(StreamEnvelope {
                    body: Some(Body::Pong(Pong {
                        ts_unix_ms: chrono::Utc::now().timestamp_millis(),
                    })),
                })
                .await
            {
                log::warn!(
                    "[CONNECT GRPC] Outbound channel closed while sending Pong: {}; reconnecting",
                    e
                );
                return false;
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
                log::warn!("[CONNECT GRPC] Failed to forward Error event: {}", e);
            }
        }
        Some(Body::DuressActivated(event)) => {
            // Phase 7b: remote duress activation. Convert the
            // protobuf timestamp and forward; the app locks the
            // cryptic screen WITHOUT wiping.
            let unlock_at = event
                .duress_unlock_at
                .and_then(|ts| chrono::DateTime::from_timestamp(ts.seconds, ts.nanos as u32));
            if let Err(e) = channel
                .send(ConnectStreamMessage::DuressActivated {
                    unlock_at,
                    source: event.source,
                })
                .await
            {
                log::warn!("[CONNECT GRPC] Failed to forward DuressActivated: {}", e);
            }
        }
        Some(Body::DuressCleared(_)) => {
            if let Err(e) = channel.send(ConnectStreamMessage::DuressCleared).await {
                log::warn!("[CONNECT GRPC] Failed to forward DuressCleared: {}", e);
            }
        }
        Some(Body::DuressDisabled(event)) => {
            // Issue 2: duress turned off account-wide.
            // Forward the account id so the app can
            // disarm only a matching local enrollment.
            if let Err(e) = channel
                .send(ConnectStreamMessage::DuressDisabled {
                    account_id: event.account_id,
                })
                .await
            {
                log::warn!("[CONNECT GRPC] Failed to forward DuressDisabled: {}", e);
            }
        }
        _ => {}
    }
    true
}

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(90);

/// The server waits for an event or a client ping before sending its opening
/// response. An up-to-date, quiet account can otherwise wait forever in
/// `client.connect`. Send Hello first, then an immediate Ping and regular idle
/// heartbeats. Keep these on the request body: a gRPC request deadline would
/// also terminate a healthy, long-lived stream.
fn outbound_stream(
    hello: StreamEnvelope,
    rx: tokio::sync::mpsc::Receiver<StreamEnvelope>,
) -> impl iced::futures::Stream<Item = StreamEnvelope> + Send + 'static {
    let mut interval = tokio::time::interval(HEARTBEAT_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let heartbeats = iced::futures::stream::unfold(interval, |mut interval| async move {
        interval.tick().await;
        Some((
            StreamEnvelope {
                body: Some(Body::Ping(Ping {
                    ts_unix_ms: chrono::Utc::now().timestamp_millis(),
                })),
            },
            interval,
        ))
    });
    iced::futures::stream::once(async move { hello }).chain(iced::futures::stream::select(
        tokio_stream::wrappers::ReceiverStream::new(rx),
        heartbeats,
    ))
}

/// How often an unexplained refusal (a server without reasons) is
/// re-checked. Long enough not to load the API with ids that are really gone,
/// short enough that a server-side blip misread as "not found" heals on its
/// own.
const REJECTED_RECHECK: Duration = Duration::from_secs(5 * 60);

/// How long an unknown device id waits before the same id is tried again,
/// in case the App's renewal returned it unchanged.
const UNKNOWN_RECHECK: Duration = Duration::from_secs(60);

/// Report a refusal. The caller sends no `Disconnected` after it.
async fn refuse_device(
    channel: &mut iced::futures::channel::mpsc::Sender<ConnectStreamMessage>,
    status: &tonic::Status,
    refusal: Refusal,
) {
    log::warn!("[CONNECT GRPC] Device refused ({:?}): {}", refusal, status);
    if let Err(e) = channel.send(ConnectStreamMessage::Refused(refusal)).await {
        log::warn!("[CONNECT GRPC] Failed to forward the refusal: {}", e);
    }
}

/// Report a failure that isn't a refusal. An expired sign-in stays an
/// `Error` so the App can tell it apart; the REST client refreshes the
/// shared token and the next attempt uses it. Anything else is trouble:
/// offline or Connect unreachable.
async fn report_failure(
    channel: &mut iced::futures::channel::mpsc::Sender<ConnectStreamMessage>,
    status: &tonic::Status,
) {
    let message = if status.code() == tonic::Code::Unauthenticated {
        ConnectStreamMessage::Error(format!("Connect session expired ({})", status.message()))
    } else {
        ConnectStreamMessage::Trouble(trouble::status_trouble(status))
    };
    if let Err(e) = channel.send(message).await {
        log::warn!("[CONNECT GRPC] Failed to forward the failure: {}", e);
    }
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

    #[tokio::test(start_paused = true)]
    async fn outbound_sends_hello_then_pings_even_without_wallet_events() {
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        let outbound = outbound_stream(
            StreamEnvelope {
                body: Some(Body::ClientHello(ClientHello::default())),
            },
            rx,
        );
        iced::futures::pin_mut!(outbound);
        assert!(matches!(
            outbound.next().await.unwrap().body,
            Some(Body::ClientHello(_))
        ));
        let start = tokio::time::Instant::now();
        assert!(matches!(
            outbound.next().await.unwrap().body,
            Some(Body::Ping(_))
        ));
        assert_eq!(start.elapsed(), Duration::ZERO);

        tx.send(StreamEnvelope {
            body: Some(Body::ClientAck(ClientAck { event_seq: 42 })),
        })
        .await
        .unwrap();
        assert!(matches!(
            outbound.next().await.unwrap().body,
            Some(Body::ClientAck(ClientAck { event_seq: 42 }))
        ));
        assert!(matches!(
            outbound.next().await.unwrap().body,
            Some(Body::Ping(_))
        ));
        assert_eq!(start.elapsed(), HEARTBEAT_INTERVAL);
    }

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
