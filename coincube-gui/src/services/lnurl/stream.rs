use std::hash::{Hash, Hasher};
use std::sync::Arc;

use iced::futures::{self, SinkExt, TryStreamExt};
use reqwest_sse::EventSource as _;

use crate::app::breez::BreezClient;

use super::{InvoiceRequestEvent, InvoiceResponse, LnurlMessage};

/// Wrapper around the data needed for the LNURL SSE subscription.
/// Implements `Hash` based only on `token` and `retries` so that
/// Iced re-creates the subscription when those change (reconnect on
/// disconnect), while the `breez_client` Arc is passed through without
/// affecting identity.
struct LnurlStreamData {
    token: String,
    retries: usize,
    breez_client: Arc<BreezClient>,
}

impl Hash for LnurlStreamData {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.token.hash(state);
        self.retries.hash(state);
    }
}

/// Creates a long-lived SSE subscription for LNURL invoice requests.
///
/// When a payer hits the user's Lightning Address callback, the API sends an
/// `lnurl:invoice-request` SSE event. This stream generates a BOLT11 invoice
/// via the Breez Liquid SDK and POSTs it back to the API.
pub fn lnurl_subscription(
    token: String,
    retries: usize,
    breez_client: Arc<BreezClient>,
) -> iced::Subscription<LnurlMessage> {
    iced::Subscription::run_with(
        LnurlStreamData {
            token,
            retries,
            breez_client,
        },
        create_stream,
    )
}

fn create_stream(
    data: &LnurlStreamData,
) -> impl iced::futures::Stream<Item = LnurlMessage> + 'static {
    #[cfg(debug_assertions)]
    let events_base_url = "https://dev-events.coincube.io";
    #[cfg(not(debug_assertions))]
    let events_base_url = env!("EVENTS_API_URL");

    #[cfg(debug_assertions)]
    let api_base_url: &'static str =
        option_env!("COINCUBE_API_URL").unwrap_or("https://dev-api.coincube.io");
    #[cfg(not(debug_assertions))]
    let api_base_url: &'static str = env!("COINCUBE_API_URL");

    let auth = format!("Bearer {}", data.token);
    let sse_url = format!("{}/api/v1/lnurl/stream", events_base_url);
    let breez_client = data.breez_client.clone();
    let retries = data.retries;

    // Attempt to parse parameters for the SSE connection
    let init = match reqwest::Url::parse(&sse_url) {
        Ok(url) => match auth.parse() {
            Ok(a) => {
                let mut req = reqwest::Request::new(reqwest::Method::GET, url);
                req.headers_mut().append("Authorization", a);
                Some((req, auth.clone()))
            }
            Err(err) => {
                log::error!("[LNURL] Unable to start subscription, {}", err);
                None
            }
        },
        Err(err) => {
            log::error!("[LNURL] Unable to start subscription, {:?}", err);
            None
        }
    };

    log::trace!(
        "[LNURL] Starting subscription execution: Attempt #{}",
        data.retries
    );

    iced::stream::channel(
        8,
        move |mut channel: iced::futures::channel::mpsc::Sender<LnurlMessage>| async move {
            // Exponential backoff: 2^retries seconds, capped at 60s.
            // Delays before the stream exits so the subscription recreation
            // (triggered by the retry counter bump) doesn't spin in a tight loop.
            let backoff = std::time::Duration::from_secs((1u64 << retries.min(6)).min(60));

            // Client for outgoing POST requests (invoice responses).
            // 10s timeout leaves headroom within the API's 15s callback window.
            let http = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new());

            if let Some((request, auth_header)) = init {
                // Use connect_timeout only — a full response timeout would kill
                // the long-lived SSE stream. The 60s heartbeat loop handles idle detection.
                let sse_client = reqwest::Client::builder()
                    .connect_timeout(std::time::Duration::from_secs(10))
                    .build()
                    .unwrap_or_else(|_| reqwest::Client::new());
                match sse_client.execute(request).await {
                    Ok(res) => {
                        log::info!("[LNURL] SSE stream connected");
                        let _ = channel.send(LnurlMessage::StreamConnected).await;

                        match res.events().await {
                            Ok(mut source) => loop {
                                let timeout =
                                    tokio::time::sleep(std::time::Duration::from_secs(60));
                                let event = TryStreamExt::try_next(&mut source);

                                futures::pin_mut!(timeout);
                                futures::pin_mut!(event);

                                match futures::future::select(timeout, event).await {
                                    futures::future::Either::Left(_) => {
                                        tokio::time::sleep(backoff).await;
                                        let _ = channel
                                            .send(LnurlMessage::EventSourceDisconnected(
                                                "EventSource heartbeat failure, client is probably offline".to_string(),
                                            ))
                                            .await;
                                        break;
                                    }
                                    futures::future::Either::Right((event, _)) => match event {
                                        Ok(Some(ev)) => {
                                            if ev.event_type == "lnurl:invoice-request" {
                                                match serde_json::from_str::<InvoiceRequestEvent>(
                                                    &ev.data,
                                                ) {
                                                    Ok(req_event) => {
                                                        handle_invoice_request(
                                                            &mut channel,
                                                            &breez_client,
                                                            &http,
                                                            api_base_url,
                                                            &auth_header,
                                                            req_event,
                                                        )
                                                        .await;
                                                    }
                                                    Err(e) => {
                                                        log::warn!(
                                                            "[LNURL] Failed to parse invoice request: {}",
                                                            e
                                                        );
                                                    }
                                                }
                                            } else {
                                                continue;
                                            }
                                        }
                                        Ok(None) => {
                                            log::info!("[LNURL] EventSource exiting safely");
                                            tokio::time::sleep(backoff).await;
                                            let _ = channel
                                                .send(LnurlMessage::EventSourceDisconnected(
                                                    "EventSource stream ended".to_string(),
                                                ))
                                                .await;
                                            break;
                                        }
                                        Err(err) => {
                                            tokio::time::sleep(backoff).await;
                                            let _ = channel
                                                .send(LnurlMessage::EventSourceDisconnected(
                                                    err.to_string(),
                                                ))
                                                .await;
                                            break;
                                        }
                                    },
                                }
                            },
                            Err(err) => {
                                log::error!("[LNURL] Failed to get event source: {}", err);
                                tokio::time::sleep(backoff).await;
                                let _ = channel
                                    .send(LnurlMessage::StreamError(err.to_string()))
                                    .await;
                            }
                        }
                    }
                    Err(err) => {
                        log::error!("[LNURL] EventSource pre-request failed: {}", err);
                        tokio::time::sleep(backoff).await;
                        let _ = channel
                            .send(LnurlMessage::StreamError(err.to_string()))
                            .await;
                    }
                }
            }
        },
    )
}

/// Handles an incoming LNURL invoice request:
/// 1. Generates a BOLT11 invoice via Breez SDK
/// 2. POSTs the invoice back to the API
async fn handle_invoice_request(
    channel: &mut iced::futures::channel::mpsc::Sender<LnurlMessage>,
    breez_client: &Arc<BreezClient>,
    http: &reqwest::Client,
    api_base_url: &str,
    auth_header: &str,
    event: InvoiceRequestEvent,
) {
    let request_id = event.request_id.clone();

    log::info!(
        "[LNURL] Invoice request received: id={}, user={}, amount_msats={}",
        request_id,
        event.username,
        event.amount_msats
    );

    let _ = channel
        .send(LnurlMessage::InvoiceRequest(event.clone()))
        .await;

    if !event.amount_msats.is_multiple_of(1000) {
        let error = format!(
            "amount_msats {} is not a whole satoshi multiple",
            event.amount_msats
        );
        log::warn!(
            "[LNURL] Rejecting invoice request {}: {}",
            request_id,
            error
        );
        let _ = channel
            .send(LnurlMessage::InvoiceError { request_id, error })
            .await;
        return;
    }
    let amount_sat = event.amount_msats / 1000;

    // Generate BOLT11 invoice via Breez Liquid SDK
    let invoice_result = breez_client
        .receive_lnurl_invoice(amount_sat, event.description_hash)
        .await;

    match invoice_result {
        Ok(response) => {
            let payment_request = response.destination;

            log::info!(
                "[LNURL] Invoice generated for request {}: {}...",
                request_id,
                &payment_request[..payment_request.len().min(30)]
            );

            // POST the invoice back to the API
            let invoice_response = InvoiceResponse {
                request_id: request_id.clone(),
                payment_request,
                payment_hash: None,
            };

            let url = format!("{}/api/v1/lnurl/invoice-response", api_base_url);
            let post_result = http
                .post(&url)
                .header("Authorization", auth_header)
                .json(&invoice_response)
                .send()
                .await;

            match post_result {
                Ok(res) if res.status().is_success() => {
                    log::info!("[LNURL] Invoice delivered for request {}", request_id);
                    let _ = channel
                        .send(LnurlMessage::InvoiceGenerated { request_id })
                        .await;
                }
                Ok(res) => {
                    let status = res.status();
                    let error = format!("API returned {}", status);
                    log::warn!(
                        "[LNURL] Failed to deliver invoice for request {}: {}",
                        request_id,
                        error
                    );
                    let _ = channel
                        .send(LnurlMessage::InvoiceError { request_id, error })
                        .await;
                }
                Err(err) => {
                    let error = err.to_string();
                    log::error!(
                        "[LNURL] Failed to POST invoice for request {}: {}",
                        request_id,
                        error
                    );
                    let _ = channel
                        .send(LnurlMessage::InvoiceError { request_id, error })
                        .await;
                }
            }
        }
        Err(err) => {
            let error = err.to_string();
            log::error!(
                "[LNURL] Breez SDK failed to generate invoice for request {}: {}",
                request_id,
                error
            );
            // Don't POST back — the 15s API timeout will trigger "offline" for the payer
            let _ = channel
                .send(LnurlMessage::InvoiceError { request_id, error })
                .await;
        }
    }
}
