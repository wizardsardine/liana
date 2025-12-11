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
            CoincubeError::Unsuccessful(e) => write!(f, "{}", e.text),
            CoincubeError::Api(msg) => write!(f, "API error: {}", msg),
            CoincubeError::Parse(msg) => write!(f, "Parse error: {}", msg),
        }
    }
}

impl std::error::Error for CoincubeError {}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveQuoteRequest<'a, T: Serialize> {
    pub quote_id: &'a str,
    pub quote: T,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SaveQuoteResponse {
    pub success: bool,
}

#[derive(Serialize, Deserialize)]
pub struct AuthDetail {
    pub provider: u8,
    pub password: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AccountType {
    // businesses are not supported yet
    Business,
    Individual,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignUpRequest {
    pub account_type: AccountType,
    pub email: String,
    pub legal_name: String,
    pub auth_details: [AuthDetail; 1],
}

#[derive(Serialize)]
pub struct EmailVerificationStatusRequest<'a> {
    pub email: &'a str,
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

#[derive(Serialize)]
pub struct PasswordResetEmailRequest<'a> {
    pub email: &'a str,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: u32,
    pub email: String,
    pub legal_name: String,
    pub email_verified: Option<bool>,
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
}

#[derive(Deserialize)]
pub struct VerifyEmailResponse {
    pub message: String,
    pub email: String,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct LoginResponse {
    pub requires_2fa: bool,
    pub token: String,
    pub refresh_token: String,
    pub user: User,
}

#[derive(Deserialize, Debug, Clone)]
pub struct PasswordResetEmailResponse {
    pub message: String,
}

#[derive(Debug, Deserialize, Clone, PartialEq)]
pub struct Country {
    pub name: &'static str,
    pub code: &'static str,
    pub flag: &'static str,
    pub currency: Currency,
}

impl std::fmt::Display for Country {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.name, self.code)
    }
}

#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
pub struct Currency {
    pub code: &'static str,
    pub name: &'static str,
    pub symbol: &'static str,
}

pub fn get_countries() -> &'static [Country] {
    static COUNTRIES_JSON: &'static str = include_str!("../countries.json");
    static COUNTRIES: std::sync::OnceLock<Vec<Country>> = std::sync::OnceLock::new();

    COUNTRIES
        .get_or_init(|| serde_json::from_str(COUNTRIES_JSON).unwrap())
        .as_slice()
}
