use serde::{Deserialize, Serialize};

pub mod client;
pub use client::CoincubeClient;

#[derive(Debug)]
pub enum CoincubeError {
    Network(reqwest::Error),
    Unsuccessful(crate::services::http::NotSuccessResponseInfo),
    Api(String),
    Parse(serde_json::Error),
}

impl From<serde_json::Error> for CoincubeError {
    fn from(v: serde_json::Error) -> Self {
        Self::Parse(v)
    }
}

impl From<crate::services::http::NotSuccessResponseInfo> for CoincubeError {
    fn from(v: crate::services::http::NotSuccessResponseInfo) -> Self {
        Self::Unsuccessful(v)
    }
}

impl From<reqwest::Error> for CoincubeError {
    fn from(e: reqwest::Error) -> Self {
        Self::Network(e)
    }
}

impl std::fmt::Display for CoincubeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CoincubeError::Network(msg) => write!(f, "Network error: {}", msg),
            CoincubeError::Unsuccessful(e) => write!(f, "Unsuccessful HTTP response: {:?}", e),
            CoincubeError::Api(msg) => write!(f, "API error: {}", msg),
            CoincubeError::Parse(msg) => write!(f, "Parse error: {}", msg),
        }
    }
}

impl std::error::Error for CoincubeError {}

#[derive(Debug, Clone, Serialize)]
pub struct SaveQuoteRequest {
    pub quote_id: String,
    pub hash: String,
    pub user_id: Option<String>,
    pub amount: u64,
    pub source_currency: crate::services::mavapay::MavapayUnitCurrency,
    pub target_currency: crate::services::mavapay::MavapayUnitCurrency,
    pub exchange_rate: f64,
    pub usd_to_target_currency_rate: f64,
    pub transaction_fees_in_source_currency: u64,
    pub transaction_fees_in_target_currency: u64,
    pub amount_in_source_currency: u64,
    pub amount_in_target_currency: u64,
    pub total_amount_in_source_currency: u64,
    pub total_amount_in_target_currency: Option<u64>,
    pub bank_account_number: Option<String>,
    pub bank_account_name: Option<String>,
    pub bank_code: Option<String>,
    pub bank_name: Option<String>,
    pub payment_method: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SaveQuoteResponse {
    pub success: bool,
    pub quote_id: String,
    pub id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PaymentLinkStatusResponse {
    pub status: String,
    pub order_id: String,
}

#[derive(Serialize, Deserialize)]
pub struct AuthDetail {
    pub provider: u8,
    pub password: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignUpRequest {
    // TODO: is support for businesses even planned?
    pub account_type: &'static str,
    pub email: String,
    pub first_name: String,
    pub last_name: String,
    pub auth_details: [AuthDetail; 1],
}

#[derive(Serialize)]
pub struct EmailVerificationStatusRequest {
    pub email: String,
}

#[derive(Serialize)]
pub struct ResendVerificationEmailRequest {
    pub email: String,
}

#[derive(Serialize)]
pub struct LoginRequest {
    pub provider: u8, // 1 for email provider
    pub email: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: u32,
    pub email: String,
    pub first_name: String,
    pub last_name: String,
    pub email_verified: bool,
    pub needs_2fa_setup: bool,
}

#[derive(Deserialize)]
pub struct SignUpResponse {
    pub status: String,
    pub data: User,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailVerificationStatusResponse {
    pub email: String,
    pub email_verified: bool,
    pub message: String,
}

#[derive(Deserialize)]
pub struct VerifyEmailResponse {
    pub message: String,
    pub email: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct LoginResponse {
    pub requires_2fa: bool,
    pub token: String, // JWT token for authenticated requests
    pub user: User,    // User data when login is successful
}
