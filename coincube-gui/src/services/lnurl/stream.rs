use std::hash::{Hash, Hasher};
use std::sync::Arc;

use iced::futures::{self, SinkExt, TryStreamExt};
use reqwest_sse::EventSource as _;

use crate::app::breez_liquid::BreezClient;
use crate::app::wallets::{SparkBackend, WalletKind};

use super::{InvoiceRequestEvent, InvoiceResponse, LnurlMessage};

/// BOLT11 BOLT-11 encoded description field hard limit: tagged field
/// values are length-prefixed with 10 bits, so the upper bound is
/// 1023 bytes. Most payer wallets reject invoices larger than this
/// anyway, and the LUD-06 spec caps metadata at ~639 bytes for
/// reliable cross-wallet compatibility. We use 639 as the Spark-side
/// max; longer descriptions (commonly NIP-57 zap requests) fall back
/// to Liquid so the invoice still commits via description_hash.
const BOLT11_MAX_DESCRIPTION_BYTES: usize = 639;

/// Wrapper around the data needed for the LNURL SSE subscription.
/// Implements `Hash` based only on `token` and `retries` so that
/// Iced re-creates the subscription when those change (reconnect on
/// disconnect), while the backend Arcs are passed through without
/// affecting identity.
///
/// Phase 5: both backends are held so `handle_invoice_request` can
/// route per-request based on the cube's `default_lightning_backend`
/// preference and the incoming event's description length.
struct LnurlStreamData {
    token: String,
    retries: usize,
    breez_client: Arc<BreezClient>,
    spark_backend: Option<Arc<SparkBackend>>,
    preferred: WalletKind,
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
    spark_backend: Option<Arc<SparkBackend>>,
    preferred: WalletKind,
) -> iced::Subscription<LnurlMessage> {
    iced::Subscription::run_with(
        LnurlStreamData {
            token,
            retries,
            breez_client,
            spark_backend,
            preferred,
        },
        create_stream,
    )
}

fn create_stream(
    data: &LnurlStreamData,
) -> impl iced::futures::Stream<Item = LnurlMessage> + 'static {
    let api_base_url = crate::services::coincube_api_base_url();

    let auth = format!("Bearer {}", data.token);
    let sse_url = format!("{}/api/v1/lnurl/stream", api_base_url);
    let breez_client = data.breez_client.clone();
    let spark_backend = data.spark_backend.clone();
    let preferred = data.preferred;
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

                // Debug builds only: log a curl template for the request so it
                // can be reproduced from a terminal. The Authorization header
                // is redacted — replace `<REDACTED_AUTH>` with the real bearer
                // value before running the command.
                #[cfg(debug_assertions)]
                {
                    log::warn!(
                        "[LNURL] DEBUG curl reproduction: curl -i -N -H 'Accept: text/event-stream' -H 'Authorization: <REDACTED_AUTH>' '{}'",
                        request.url()
                    );
                }

                match sse_client.execute(request).await {
                    Ok(res) => {
                        let status = res.status();
                        if status == reqwest::StatusCode::UNAUTHORIZED {
                            log::warn!("[LNURL] SSE stream returned 401 Unauthorized");
                            tokio::time::sleep(backoff).await;
                            let _ = channel
                                .send(LnurlMessage::StreamError(
                                    "Unauthorized – token may have expired".to_string(),
                                ))
                                .await;
                            return;
                        }
                        if !status.is_success() {
                            log::error!("[LNURL] SSE stream returned {}", status);
                            tokio::time::sleep(backoff).await;
                            let _ = channel
                                .send(LnurlMessage::StreamError(format!(
                                    "Unexpected status: {}",
                                    status
                                )))
                                .await;
                            return;
                        }
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
                                                            spark_backend.as_ref(),
                                                            preferred,
                                                            &http,
                                                            &api_base_url,
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
/// 1. Generates a BOLT11 invoice via the routed backend
/// 2. POSTs the invoice back to the API
///
/// Routing rules (Phase 5):
/// - `preferred == Spark` AND `spark_backend.is_some()` AND the event
///   carries a `description` AND the description fits in
///   [`BOLT11_MAX_DESCRIPTION_BYTES`] → Spark. The invoice's `d` tag
///   holds the raw metadata string; the payer's wallet must verify
///   that SHA256(description) matches the callback's metadata hash,
///   which it does by construction (the API computes them from the
///   same source).
/// - Otherwise → Liquid, which commits via `description_hash`
///   directly and handles zap requests (NIP-57) that exceed the
///   639-byte cap.
#[allow(clippy::too_many_arguments)]
async fn handle_invoice_request(
    channel: &mut iced::futures::channel::mpsc::Sender<LnurlMessage>,
    breez_client: &Arc<BreezClient>,
    spark_backend: Option<&Arc<SparkBackend>>,
    preferred: WalletKind,
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

    // Decide which backend mints the invoice. Spark only runs when
    // all preconditions hold: explicit preference, bridge available,
    // API sent a description preimage, the preimage fits the BOLT11
    // description cap, AND SHA256(description) matches the
    // description_hash the API advertised to the payer. The hash
    // check prevents a divergence between the invoice's `d` tag and
    // what the payer's wallet expects — if they differ, the payer
    // rejects the invoice.
    let use_spark = matches!(preferred, WalletKind::Spark)
        && spark_backend.is_some()
        && event.description.as_deref().is_some_and(|d| {
            if d.len() > BOLT11_MAX_DESCRIPTION_BYTES {
                return false;
            }
            use sha2::{Digest, Sha256};
            let hash = Sha256::digest(d.as_bytes());
            let hash_hex: String = hash.iter().map(|b| format!("{:02x}", b)).collect();
            if hash_hex != event.description_hash {
                log::warn!(
                    "[LNURL] description hash mismatch for request {}: \
                     expected {}, got {} — falling back to Liquid",
                    request_id,
                    event.description_hash,
                    hash_hex,
                );
                return false;
            }
            true
        });

    let invoice_result = if use_spark {
        let spark = spark_backend.expect("checked above");
        let description = event.description.clone().expect("checked above");
        log::info!(
            "[LNURL] Routing request {} via Spark (desc_len={})",
            request_id,
            description.len()
        );
        spark
            .receive_bolt11(Some(amount_sat), description, None)
            .await
            .map(|ok| ok.payment_request)
            .map_err(|e| e.to_string())
    } else {
        log::info!("[LNURL] Routing request {} via Liquid", request_id);
        breez_client
            .receive_lnurl_invoice(amount_sat, event.description_hash.clone())
            .await
            .map(|resp| resp.destination)
            .map_err(|e| e.to_string())
    };

    match invoice_result {
        Ok(payment_request) => {
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
        Err(error) => {
            log::error!(
                "[LNURL] Backend failed to generate invoice for request {}: {}",
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
