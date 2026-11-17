//! SideShift API client for the Spark wallet's "receive from another network"
//! bridge. Every shift it creates settles as **bitcoin** to the Spark wallet's
//! static on-chain deposit address; nothing here settles to Liquid or any other
//! network. Liquid stays only as a deposit (origin) network, alongside the
//! others in [`super::types::deposit_options`].

use reqwest::Client;

use crate::services::http::ResponseExt;

use super::types::{ShiftResponse, ShiftStatus, VariableShiftRequest, BTC_COIN, BTC_NETWORK};

const SIDESHIFT_BASE_URL: &str = "https://sideshift.ai/api/v2";

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

    /// Create a variable-rate shift that settles as **Bitcoin** into the Spark
    /// wallet's on-chain deposit address (`btc_settle_address`).
    ///
    /// This is the Spark receive bridge: any SideShift-supported asset in, BTC
    /// out. SideShift has no Lightning rail, so the settle leg is on-chain —
    /// the funds land after roughly one confirmation, not instantly, and the
    /// status UI has to say so.
    ///
    /// `refund_address` is on the **origin** chain (see
    /// [`super::types::validate_refund_address`]) and is mandatory here even
    /// though SideShift treats it as optional: a shift that fails or gets held
    /// with no refund address leaves the user's funds stranded at a third party.
    pub async fn create_btc_receive_shift(
        &self,
        deposit_coin: &str,
        deposit_network: &str,
        btc_settle_address: &str,
        refund_address: &str,
        affiliate_id: &str,
    ) -> Result<ShiftResponse, String> {
        self.post_variable_shift(btc_receive_shift_request(
            deposit_coin,
            deposit_network,
            btc_settle_address,
            refund_address,
            affiliate_id,
        ))
        .await
    }

    async fn post_variable_shift(
        &self,
        body: VariableShiftRequest,
    ) -> Result<ShiftResponse, String> {
        let url = format!("{}/shifts/variable", SIDESHIFT_BASE_URL);

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

/// The body of a Spark receive shift: `deposit_coin` on `deposit_network` in,
/// bitcoin out to `btc_settle_address`, refunds to `refund_address` on the
/// deposit network. The settle side is fixed here rather than taken as a
/// parameter, so no caller can point a shift anywhere but bitcoin.
fn btc_receive_shift_request(
    deposit_coin: &str,
    deposit_network: &str,
    btc_settle_address: &str,
    refund_address: &str,
    affiliate_id: &str,
) -> VariableShiftRequest {
    VariableShiftRequest {
        deposit_coin: deposit_coin.to_string(),
        deposit_network: deposit_network.to_string(),
        settle_coin: BTC_COIN.to_string(),
        settle_network: BTC_NETWORK.to_string(),
        settle_address: btc_settle_address.to_string(),
        affiliate_id: affiliate_id.to_string(),
        refund_address: refund_address.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::sideshift::deposit_options;

    const SPARK_DEPOSIT_ADDRESS: &str = "bc1qvrl2849aggm6qry9ea7xqp2kk39j8vaa8r3cwg";

    /// Every offered deposit, USDt on Liquid included, becomes a shift that
    /// settles as bitcoin on the Bitcoin network to the address it was given
    /// (the Spark wallet's static deposit address), with the refund address
    /// on the deposit's own chain. This is the whole wire contract with
    /// SideShift, so it is checked as the JSON that is sent.
    #[test]
    fn every_deposit_settles_as_bitcoin_to_the_spark_deposit_address() {
        for option in deposit_options() {
            let body = serde_json::to_value(btc_receive_shift_request(
                option.coin,
                option.network.network_slug(),
                SPARK_DEPOSIT_ADDRESS,
                "refund-on-origin-chain",
                "affiliate",
            ))
            .unwrap();
            assert_eq!(
                body,
                serde_json::json!({
                    "depositCoin": option.coin,
                    "depositNetwork": option.network.network_slug(),
                    "settleCoin": "btc",
                    "settleNetwork": "bitcoin",
                    "settleAddress": SPARK_DEPOSIT_ADDRESS,
                    "affiliateId": "affiliate",
                    "refundAddress": "refund-on-origin-chain",
                }),
                "{}",
                option.label
            );
        }
    }

    /// USDt on Liquid is still a deposit network: SideShift is told the
    /// deposit is on `liquid`, and the bitcoin it pays out is not.
    #[test]
    fn usdt_on_liquid_is_a_deposit_network_never_a_settle_network() {
        let body = btc_receive_shift_request(
            "usdt",
            "liquid",
            SPARK_DEPOSIT_ADDRESS,
            "lq1qq0refund",
            "affiliate",
        );
        assert_eq!(body.deposit_network, "liquid");
        assert_eq!(body.settle_network, "bitcoin");
        assert_eq!(body.settle_coin, "btc");
    }
}
