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
            Self::NotSuccessResponse(info) => write!(f, "Not success response: {:?}", info),
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
