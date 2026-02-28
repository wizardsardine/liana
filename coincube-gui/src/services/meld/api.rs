use crate::services::coincube;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(untagged)]
pub enum MeldApiResult<T> {
    Error {
        code: String,
        message: String,
        errors: Vec<String>,
    },
    Success(T),
    #[serde(skip)]
    Other(coincube::CoincubeError),
}

impl<T> From<coincube::CoincubeError> for MeldApiResult<T> {
    fn from(e: coincube::CoincubeError) -> Self {
        Self::Other(e)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeldCountry {
    pub country_code: String,
    pub name: String,
    pub flag_url: Option<String>,
    pub regions: Option<Vec<MeldRegion>>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MeldRegion {
    pub region_code: String,
    pub name: String,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CurrencyLimit {
    pub currency_code: String,
    pub default_amount: f64,
    pub minimum_amount: f64,
    pub maximum_amount: f64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TransactionType {
    CryptoPurchase,
    CryptoSell,
    CryptoTransfer,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TransactionStatus {
    Pending,
    Settling,
    Settled,
    Error,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum SessionType {
    Buy,
    Sell,
    Transfer,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetQuotesRequest<'a> {
    pub session_type: SessionType,
    pub source_amount: f64,
    pub source_currency: &'a str,
    pub destination_currency: &'a str,
    pub country_code: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_code: Option<&'a str>,
    pub wallet_address: Option<&'a str>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Quote {
    pub transaction_type: TransactionType,

    pub wallet_address: Option<String>,
    pub source_amount: f32,
    pub destination_amount: f32,

    pub exchange_rate: Option<f32>,
    pub total_fee: f32,

    pub source_currency_code: String,
    pub destination_currency_code: String,

    pub service_provider: String,
    pub customer_score: f32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetQuoteResponse {
    pub quotes: Vec<Quote>,
    pub message: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionRequest<'a> {
    pub session_type: SessionType,
    pub quote_provider: &'a str,
    pub source_amount: f32,
    pub source_currency: &'a str,
    pub destination_currency: &'a str,
    pub country_code: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_code: Option<&'a str>,
    pub wallet_address: Option<&'a str>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionResponse {
    pub session_id: String,
    pub widget_url: String,
    pub service_provider_widget_url: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MeldEventType {
    TransactionCryptoPending,
    TransactionCryptoTransferring,
    TransactionCryptoComplete,
    TransactionCryptoFailed,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MeldEvent {
    pub event_type: MeldEventType,
    pub event_id: String, // for idempotency
    pub meld_session_id: String,
    pub transaction_type: TransactionType,
    pub status: TransactionStatus,
    #[serde(flatten)]
    pub extras: serde_json::Value,
}
