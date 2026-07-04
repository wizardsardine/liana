use async_trait::async_trait;
use reqwest::Response;
use serde::Deserialize;

/// Matches `{"success":false,"error":{"code":"...","message":"..."}}` error
/// bodies returned by the coincube-api on non-2xx responses.
#[derive(Debug, Deserialize)]
struct ApiErrorEnvelope {
    error: ApiErrorBody,
}

#[derive(Debug, Deserialize)]
struct ApiErrorBody {
    message: String,
}

/// Information about an unsuccessful response.
#[derive(Debug, Clone)]
pub struct NotSuccessResponseInfo {
    pub status_code: u16,
    pub text: String,
}

impl NotSuccessResponseInfo {
    /// The human-readable error message. If the body is the standard coincube-api
    /// error envelope (`{"success":false,"error":{"code","message"}}`), returns
    /// the parsed `error.message`; otherwise returns the raw body text.
    ///
    /// This is the single place that unwraps the envelope, so every caller
    /// renders a clean message regardless of which endpoint failed. As of the
    /// CC-API-001 backend migration, the `core/*` endpoints (Meld/Mavapay/fiat/
    /// config) now return this envelope where they previously returned plain
    /// text — the fallback keeps any non-envelope body working unchanged.
    pub fn message(&self) -> String {
        serde_json::from_str::<ApiErrorEnvelope>(&self.text)
            .map(|env| env.error.message)
            .unwrap_or_else(|_| self.text.clone())
    }
}

#[async_trait]
pub trait ResponseExt {
    async fn check_success(self) -> Result<Self, NotSuccessResponseInfo>
    where
        Self: Sized;
}

#[async_trait]
impl ResponseExt for Response {
    async fn check_success(self) -> Result<Self, NotSuccessResponseInfo> {
        let status = self.status();
        if !status.is_success() {
            return Err(NotSuccessResponseInfo {
                status_code: status.as_u16(),
                text: self
                    .text()
                    .await
                    .unwrap_or_else(|_| "Failed to read response text".to_string()),
            });
        }
        Ok(self)
    }
}
