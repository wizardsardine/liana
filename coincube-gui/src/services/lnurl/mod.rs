pub mod stream;

use serde::{Deserialize, Serialize};

/// SSE event received when a payer hits the user's Lightning Address callback.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InvoiceRequestEvent {
    pub request_id: String,
    pub username: String,
    pub amount_msats: u64,
    pub description_hash: String,
    /// Raw preimage of `description_hash`: the LNURL metadata JSON for
    /// standard requests, or the serialized nostr zap request for
    /// NIP-57 zaps. Present on API versions that support Spark routing
    /// (Phase 5+) and absent on older servers — the SSE handler falls
    /// back to Liquid (which commits to the hash directly) whenever
    /// this field is missing.
    #[serde(default)]
    pub description: Option<String>,
}

/// Request body for POST /api/v1/lnurl/invoice-response
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InvoiceResponse {
    pub request_id: String,
    pub payment_request: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment_hash: Option<String>,
}

#[derive(Debug, Clone)]
pub enum LnurlMessage {
    StreamConnected,
    InvoiceRequest(InvoiceRequestEvent),
    InvoiceGenerated { request_id: String },
    InvoiceError { request_id: String, error: String },
    EventSourceDisconnected(String),
    StreamError(String),
}
