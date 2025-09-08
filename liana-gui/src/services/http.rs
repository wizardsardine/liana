use async_trait::async_trait;
use reqwest::Response;

/// Information about an unsuccessful response.
#[derive(Debug, Clone)]
pub struct NotSuccessResponseInfo {
    pub status_code: u16,
    pub text: String,
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
