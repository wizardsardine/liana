use super::ServiceProvider;
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt;

const MELD_API_BASE_URL: &str = "https://api-sb.meld.io/crypto/session/widget";

// TODO: Move to .env and use in build.rs for compile-time env
fn meld_auth_header() -> Option<String> {
    if cfg!(debug_assertions) {
        std::env::var("MELD_API_KEY").ok().and_then(|v| {
            if v.is_empty() { None } else { Some(v.trim().to_string()) }
        })
    } else {
        option_env!("MELD_API_KEY").map(|s| s.trim().to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeldSessionRequest {
    #[serde(rename = "sessionData")]
    pub session_data: SessionData,
    #[serde(rename = "sessionType")]
    pub session_type: String,
    #[serde(rename = "externalCustomerId")]
    pub external_customer_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionData {
    #[serde(rename = "walletAddress")]
    pub wallet_address: String,
    #[serde(rename = "countryCode")]
    pub country_code: String,
    #[serde(rename = "sourceCurrencyCode")]
    pub source_currency_code: String,
    #[serde(rename = "sourceAmount")]
    pub source_amount: String,
    #[serde(rename = "destinationCurrencyCode")]
    pub destination_currency_code: String,
    #[serde(rename = "serviceProvider")]
    pub service_provider: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeldSessionResponse {
    #[serde(rename = "customerId")]
    pub customer_id: Option<String>,
    #[serde(rename = "externalCustomerId")]
    pub external_customer_id: Option<String>,
    #[serde(rename = "externalSessionId")]
    pub external_session_id: Option<String>,
    pub id: String,
    pub token: Option<String>,
    #[serde(rename = "widgetUrl")]
    pub widget_url: String,
}

#[derive(Debug)]
pub enum MeldError {
    Network(reqwest::Error),
    Serialization(serde_json::Error),
    Api(String),
}

impl fmt::Display for MeldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MeldError::Network(e) => write!(f, "Network error: {}", e),
            MeldError::Serialization(e) => write!(f, "Serialization error: {}", e),
            MeldError::Api(msg) => fmt::Display::fmt(msg, f),
        }
    }
}

impl Error for MeldError {}

impl From<reqwest::Error> for MeldError {
    fn from(error: reqwest::Error) -> Self {
        MeldError::Network(error)
    }
}

impl From<serde_json::Error> for MeldError {
    fn from(error: serde_json::Error) -> Self {
        MeldError::Serialization(error)
    }
}

#[derive(Debug, Clone)]
pub struct MeldClient {
    client: reqwest::Client,
}

impl MeldClient {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }

    pub async fn create_widget_session(
        &self,
        wallet_address: String,
        country_code: String,
        source_amount: String,
        service_provider: ServiceProvider,
        network: liana::miniscript::bitcoin::Network,
    ) -> Result<MeldSessionResponse, MeldError> {
        // For now, always use "BTC" as shown in your working example
        // TODO: Investigate why BTC_TESTNET might be causing issues
        let destination_currency = "BTC";

        // Debug logging to see what we're sending
        tracing::info!(
            "Creating Meld session with network: {:?}, currency: {}",
            network,
            destination_currency
        );

        // Generate unique customer ID for each request to ensure fresh sessions
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| MeldError::Api(format!("System time error: {}", e)))?
            .as_secs();
        let unique_customer_id = format!("liana_user_{}", timestamp);

        let request = MeldSessionRequest {
            session_data: SessionData {
                wallet_address,
                country_code,
                source_currency_code: "USD".to_string(),
                source_amount,
                destination_currency_code: destination_currency.to_string(),
                service_provider: service_provider.as_str().to_string(),
            },
            session_type: "BUY".to_string(),
            external_customer_id: unique_customer_id,
        };

        // Security: Only log sensitive request details in debug builds
        #[cfg(debug_assertions)]
        {
            tracing::info!("Sending request to: {}", MELD_API_BASE_URL);
            tracing::info!(
                "Request body: {}",
                serde_json::to_string_pretty(&request).unwrap_or_default()
            );
        }
        #[cfg(not(debug_assertions))]
        {
            tracing::info!("[MELD] Widget session request initiated");
        }

        // Resolve Authorization header from env (.env at runtime, or compile-time env)
        let auth = match meld_auth_header() {
            Some(h) => h,
            None => {
                tracing::error!("Meld API key not configured: set MELD_API_KEY in .env or build env");
                return Err(MeldError::Api("Meld API key not configured".to_string()));
            }
        };

        let response = self
            .client
            .post(MELD_API_BASE_URL)
            .header("Authorization", format!("BASIC {}", auth))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await?;

        if response.status().is_success() {
            let session_response: MeldSessionResponse = response.json().await?;
            #[cfg(debug_assertions)]
            tracing::info!("Meld API response: {:?}", session_response);
            #[cfg(not(debug_assertions))]
            tracing::info!("[MELD] Widget session created successfully");

            Ok(session_response)
        } else {
            #[derive(Deserialize, Debug)]
            struct MeldErrorMessageExtract {
                message: String,
            }

            let status = response.status();
            let error_text = response.json::<MeldErrorMessageExtract>().await.ok();

            tracing::error!("Meld API error: HTTP {}: {:?}", status, error_text);
            Err(MeldError::Api(
                error_text
                    .map(|e| e.message)
                    .unwrap_or("Unknown Meld API Error".to_string()),
            ))
        }
    }
}

impl Default for MeldClient {
    fn default() -> Self {
        Self::new()
    }
}
