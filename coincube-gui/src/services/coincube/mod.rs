use serde::{Deserialize, Serialize};

pub mod client;
pub use client::CoincubeClient;

#[derive(Debug)]
pub enum CoincubeError {
    Network(reqwest::Error),
    Unsuccessful(crate::services::http::NotSuccessResponseInfo),
    Api(String),
    Parse(serde_json::Error),
    SseError(reqwest_sse::error::EventSourceError),
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

impl From<reqwest_sse::error::EventSourceError> for CoincubeError {
    fn from(e: reqwest_sse::error::EventSourceError) -> Self {
        Self::SseError(e)
    }
}

impl std::fmt::Display for CoincubeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CoincubeError::Network(msg) => write!(f, "Network error: {:?}", msg),
            CoincubeError::Unsuccessful(e) => write!(f, "{}", e.text),
            CoincubeError::Api(msg) => write!(f, "API error: {}", msg),
            CoincubeError::Parse(msg) => write!(f, "Parse error: {}", msg),
            CoincubeError::SseError(e) => write!(f, "SSE Error: {}", e),
        }
    }
}

impl std::error::Error for CoincubeError {}

#[derive(Debug, Clone, Deserialize)]
pub struct DownloadStats {
    pub total: u32,
    pub breakdown: std::collections::HashMap<String, u32>,
    pub last_updated: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TodayStats {
    pub count: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TimeseriesPoint {
    pub date: String,
    pub count: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TimeseriesResponse {
    pub points: Vec<TimeseriesPoint>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatsPeriod {
    Day,
    Week,
    Month,
    Year,
}

impl StatsPeriod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
            Self::Year => "year",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Day => "Day",
            Self::Week => "Week",
            Self::Month => "Month",
            Self::Year => "Year",
        }
    }
}

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

#[derive(Serialize)]
pub struct OtpRequest {
    pub email: String,
}

#[derive(Serialize)]
pub struct OtpVerifyRequest {
    pub email: String,
    pub otp: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshTokenRequest<'a> {
    pub refresh_token: &'a str,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: u32,
    pub email: String,
    pub email_verified: Option<bool>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct LoginResponse {
    pub requires_2fa: bool,
    pub token: String,
    pub refresh_token: String,
    pub user: User,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlanTier {
    Free,
    Pro,
    Legacy,
}

impl std::fmt::Display for PlanTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlanTier::Free => write!(f, "Free"),
            PlanTier::Pro => write!(f, "Pro"),
            PlanTier::Legacy => write!(f, "Legacy"),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectPlan {
    pub tier: PlanTier,
    pub paid_until: Option<String>,
    pub status: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifiedDevice {
    pub id: u32,
    pub device_name: Option<String>,
    pub created_at: String,
    pub last_used_at: Option<String>,
    #[serde(default)]
    pub is_current: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginActivity {
    pub id: u32,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
    pub created_at: String,
    pub success: bool,
}

/// Generic wrapper for API responses: `{ "success": true, "data": T }`
#[derive(Debug, Clone, Deserialize)]
pub struct ApiResponse<T> {
    pub data: T,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LightningAddress {
    pub lightning_address: Option<String>,
    pub bolt12_offer: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CheckUsernameResponse {
    pub available: bool,
    pub username: String,
    /// Set when the API returns an error (e.g. reserved/invalid username)
    #[serde(default)]
    pub error_message: Option<String>,
}

/// Error response shape: `{ "success": false, "error": { "code": "...", "message": "..." } }`
#[derive(Debug, Clone, Deserialize)]
pub struct ApiErrorResponse {
    pub error: ApiErrorDetail,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiErrorDetail {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimLightningAddressRequest {
    pub username: String,
    pub bolt12_offer: String,
}

pub fn get_countries() -> &'static [Country] {
    static COUNTRIES_JSON: &str = include_str!("../countries.json");
    static COUNTRIES: std::sync::OnceLock<Vec<Country>> = std::sync::OnceLock::new();

    COUNTRIES
        .get_or_init(|| serde_json::from_str(COUNTRIES_JSON).unwrap())
        .as_slice()
}
