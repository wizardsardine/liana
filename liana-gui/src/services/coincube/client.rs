use reqwest::{Client, Method};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct CoincubeClient {
    client: Client,
    base_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveQuoteRequest {
    pub quote_id: String,
    pub hash: String,
    pub user_id: Option<String>,
    pub amount: u64,
    pub source_currency: String,
    pub target_currency: String,
    pub payment_currency: Option<String>,
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

#[derive(Debug)]
pub enum CoincubeError {
    Network(String),
    Api(String),
    Parse(String),
}

impl std::fmt::Display for CoincubeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CoincubeError::Network(msg) => write!(f, "Network error: {}", msg),
            CoincubeError::Api(msg) => write!(f, "API error: {}", msg),
            CoincubeError::Parse(msg) => write!(f, "Parse error: {}", msg),
        }
    }
}

impl std::error::Error for CoincubeError {}

impl CoincubeClient {
    pub fn new(base_url: String) -> Self {
        Self {
            client: Client::new(),
            base_url,
        }
    }

    /// Save a Mavapay quote to coincube-api
    pub async fn save_quote(
        &self,
        request: SaveQuoteRequest,
    ) -> Result<SaveQuoteResponse, CoincubeError> {
        let url = format!("{}/api/v1/mavapay/quotes", self.base_url);

        #[cfg(debug_assertions)]
        {
            let request_json = serde_json::to_string_pretty(&request).unwrap_or_default();
            tracing::info!("[COINCUBE] Saving quote with request:\n{}", request_json);
        }

        let response = self
            .client
            .request(Method::POST, &url)
            .json(&request)
            .send()
            .await
            .map_err(|e| CoincubeError::Network(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(CoincubeError::Api(format!(
                "HTTP {}: {}",
                status, error_text
            )));
        }

        let save_response: SaveQuoteResponse = response
            .json()
            .await
            .map_err(|e| CoincubeError::Parse(e.to_string()))?;

        #[cfg(debug_assertions)]
        {
            tracing::info!("[COINCUBE] Quote saved successfully: {:?}", save_response);
        }

        Ok(save_response)
    }

    /// Check payment link status via coincube-api (proxies to Mavapay)
    pub async fn check_payment_link_status(
        &self,
        order_id: &str,
    ) -> Result<PaymentLinkStatusResponse, CoincubeError> {
        let url = format!(
            "{}/api/v1/mavapay/paymentlinks/{}/status",
            self.base_url, order_id
        );

        #[cfg(debug_assertions)]
        {
            tracing::info!("[COINCUBE] Checking payment link status for: {}", order_id);
        }

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| CoincubeError::Network(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(CoincubeError::Api(format!(
                "HTTP {}: {}",
                status, error_text
            )));
        }

        let status_response: PaymentLinkStatusResponse = response
            .json()
            .await
            .map_err(|e| CoincubeError::Parse(e.to_string()))?;

        #[cfg(debug_assertions)]
        {
            tracing::info!("[COINCUBE] Payment link status: {:?}", status_response);
        }

        Ok(status_response)
    }

    /// Build the quote display URL
    pub fn get_quote_display_url(&self, quote_id: &str) -> String {
        format!("{}/api/v1/mavapay/quotes/{}", self.base_url, quote_id)
    }
}
