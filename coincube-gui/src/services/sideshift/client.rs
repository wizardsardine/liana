use reqwest::Client;

use crate::services::http::ResponseExt;

use super::types::{
    FixedShiftRequest, QuoteRequest, ShiftQuote, ShiftResponse, ShiftStatus, VariableShiftRequest,
};

const SIDESHIFT_BASE_URL: &str = "https://sideshift.ai/api/v2";
const USDT_COIN: &str = "usdt";
const LIQUID_NETWORK: &str = "liquid";

#[derive(Debug, Clone)]
pub struct SideshiftClient {
    client: Client,
}

impl Default for SideshiftClient {
    fn default() -> Self {
        Self::new()
    }
}

impl SideshiftClient {
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .https_only(true)
                .build()
                .expect("failed to build SideShift HTTP client"),
        }
    }

    /// Request a quote for a fixed shift.
    ///
    /// For **receive** (external → Liquid USDt): `deposit_network` is the
    /// external network slug, `settle_amount_usdt` is the optional exact
    /// amount the user wants to receive.
    ///
    /// For **send** (Liquid USDt → external): pass `None` for
    /// `settle_amount_usdt`; the caller controls amounts separately.
    pub async fn get_quote(
        &self,
        deposit_network: &str,
        settle_network: &str,
        settle_amount_usdt: Option<&str>,
        deposit_amount_usdt: Option<&str>,
        affiliate_id: &str,
    ) -> Result<ShiftQuote, String> {
        let url = format!("{}/quotes", SIDESHIFT_BASE_URL);
        let body = QuoteRequest {
            deposit_coin: USDT_COIN.to_string(),
            deposit_network: deposit_network.to_string(),
            settle_coin: USDT_COIN.to_string(),
            settle_network: settle_network.to_string(),
            deposit_amount: deposit_amount_usdt.map(str::to_string),
            settle_amount: settle_amount_usdt.map(str::to_string),
            affiliate_id: affiliate_id.to_string(),
        };

        let response = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("SideShift request failed: {}", e))?
            .check_success()
            .await
            .map_err(|e| format!("SideShift error {}: {}", e.status_code, e.text))?;

        response
            .json::<ShiftQuote>()
            .await
            .map_err(|e| format!("Failed to parse SideShift quote: {}", e))
    }

    /// Create a fixed-rate shift using a previously obtained quote ID.
    /// `settle_address` is the Liquid USDt address (for receive) or the
    /// external recipient address (for send).
    pub async fn create_fixed_shift(
        &self,
        quote_id: &str,
        settle_address: &str,
        affiliate_id: &str,
    ) -> Result<ShiftResponse, String> {
        let url = format!("{}/shifts/fixed", SIDESHIFT_BASE_URL);
        let body = FixedShiftRequest {
            quote_id: quote_id.to_string(),
            settle_address: settle_address.to_string(),
            affiliate_id: affiliate_id.to_string(),
        };

        let response = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("SideShift request failed: {}", e))?
            .check_success()
            .await
            .map_err(|e| format!("SideShift error {}: {}", e.status_code, e.text))?;

        response
            .json::<ShiftResponse>()
            .await
            .map_err(|e| format!("Failed to parse SideShift shift: {}", e))
    }

    /// Create a variable-rate receive shift (external → Liquid USDt).
    pub async fn create_variable_receive_shift(
        &self,
        deposit_network: &str,
        liquid_settle_address: &str,
        affiliate_id: &str,
    ) -> Result<ShiftResponse, String> {
        self.create_variable_shift(
            deposit_network,
            LIQUID_NETWORK,
            liquid_settle_address,
            affiliate_id,
        )
        .await
    }

    /// Create a variable-rate send shift (Liquid USDt → external).
    pub async fn create_variable_send_shift(
        &self,
        settle_network: &str,
        external_settle_address: &str,
        affiliate_id: &str,
    ) -> Result<ShiftResponse, String> {
        self.create_variable_shift(
            LIQUID_NETWORK,
            settle_network,
            external_settle_address,
            affiliate_id,
        )
        .await
    }

    async fn create_variable_shift(
        &self,
        deposit_network: &str,
        settle_network: &str,
        settle_address: &str,
        affiliate_id: &str,
    ) -> Result<ShiftResponse, String> {
        let url = format!("{}/shifts/variable", SIDESHIFT_BASE_URL);
        let body = VariableShiftRequest {
            deposit_coin: USDT_COIN.to_string(),
            deposit_network: deposit_network.to_string(),
            settle_coin: USDT_COIN.to_string(),
            settle_network: settle_network.to_string(),
            settle_address: settle_address.to_string(),
            affiliate_id: affiliate_id.to_string(),
        };

        let response = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("SideShift request failed: {}", e))?
            .check_success()
            .await
            .map_err(|e| format!("SideShift error {}: {}", e.status_code, e.text))?;

        response
            .json::<ShiftResponse>()
            .await
            .map_err(|e| format!("Failed to parse SideShift shift: {}", e))
    }

    /// Poll the status of an existing shift.
    pub async fn get_shift_status(&self, shift_id: &str) -> Result<ShiftStatus, String> {
        let url = format!("{}/shifts/{}", SIDESHIFT_BASE_URL, shift_id);

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("SideShift request failed: {}", e))?
            .check_success()
            .await
            .map_err(|e| format!("SideShift error {}: {}", e.status_code, e.text))?;

        response
            .json::<ShiftStatus>()
            .await
            .map_err(|e| format!("Failed to parse SideShift status: {}", e))
    }
}
