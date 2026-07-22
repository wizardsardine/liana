use super::currency::Currency;

use async_trait::async_trait;

use crate::services::http::NotSuccessResponseInfo;

#[derive(Debug, Clone)]
pub struct GetPriceResult {
    pub value: f64,
    pub updated_at: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct ListCurrenciesResult {
    pub currencies: Vec<Currency>,
}

#[derive(Debug, Clone)]
pub enum PriceApiError {
    RequestFailed(String),
    NotSuccessResponse(NotSuccessResponseInfo),
    CannotParseResponse(String),
    CannotParseData(String),
}

impl std::fmt::Display for PriceApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RequestFailed(e) => write!(f, "Request failed: {}", e),
            Self::NotSuccessResponse(info) => {
                write!(
                    f,
                    "Not success response ({}): {}",
                    info.status_code,
                    info.message()
                )
            }
            Self::CannotParseResponse(e) => write!(f, "Cannot parse response: {}", e),
            Self::CannotParseData(e) => write!(f, "Cannot parse data: {}", e),
        }
    }
}

#[async_trait]
pub trait PriceApi {
    async fn get_price(&self, currency: Currency) -> Result<GetPriceResult, PriceApiError>;

    async fn list_currencies(&self) -> Result<ListCurrenciesResult, PriceApiError>;
}

#[cfg(test)]
mod tests {
    use super::PriceApiError;
    use crate::services::http::NotSuccessResponseInfo;

    #[test]
    fn display_renders_actionable_error_messages() {
        assert_eq!(
            PriceApiError::RequestFailed("timeout".to_string()).to_string(),
            "Request failed: timeout"
        );
        assert_eq!(
            PriceApiError::CannotParseResponse("bad json".to_string()).to_string(),
            "Cannot parse response: bad json"
        );
        assert_eq!(
            PriceApiError::CannotParseData("price".to_string()).to_string(),
            "Cannot parse data: price"
        );
    }

    #[test]
    fn display_unwraps_coincube_error_envelope() {
        let err = PriceApiError::NotSuccessResponse(NotSuccessResponseInfo {
            status_code: 429,
            text: r#"{"success":false,"error":{"code":"rate_limited","message":"slow down"}}"#
                .to_string(),
        });

        assert_eq!(err.to_string(), "Not success response (429): slow down");
    }
}
