use crate::services::{coincube::CoincubeError, http::ResponseExt};
use reqwest::Method;

use super::api::*;
use super::stream::transaction_stream;
use super::MavapayMessage;

pub struct MavapayClient<'client>(pub &'client super::super::coincube::CoincubeClient);

impl<'client> MavapayClient<'client> {
    /// Get current Bitcoin price in specified currency
    pub async fn get_price(&self, currency: MavapayCurrency) -> MavapayApiResult<GetPriceResponse> {
        let url = format!("{}/api/v1/mavapay/proxy/price", self.0.base_url);
        let res: Result<_, CoincubeError> = async {
            let response = self
                .0
                .client
                .request(Method::GET, &url)
                .query(&[("currency", currency)])
                .send()
                .await?;

            let response = response.check_success().await?;
            Ok(response.json().await?)
        }
        .await;

        match res {
            Ok(res) => res,
            Err(err) => err.into(),
        }
    }

    pub async fn ngn_customer_inquiry(
        &self,
        bank_account_number: &str,
        bank_code: &str,
    ) -> MavapayApiResult<NgnCustomerDetails> {
        let url = format!(
            "{}/api/v1/mavapay/quotes/validate-ngn-account",
            self.0.base_url
        );
        let res: Result<_, CoincubeError> = async {
            let body = serde_json::json!({
                "bankAccountNumber": bank_account_number,
                "bankCode": bank_code,
            });

            let response = self
                .0
                .client
                .request(Method::POST, &url)
                .json(&body)
                .send()
                .await?;

            let response = response.check_success().await?;
            Ok(response.json().await?)
        }
        .await;

        match res {
            Ok(res) => res,
            Err(err) => err.into(),
        }
    }

    pub async fn create_quote(
        &self,
        request: GetQuoteRequest,
    ) -> MavapayApiResult<GetQuoteResponse> {
        let url = format!("{}/api/v1/mavapay/create/quote", self.0.base_url);
        let res: Result<_, CoincubeError> = async {
            let response = self
                .0
                .client
                .request(Method::POST, &url)
                .json(&request)
                .send()
                .await?;

            let response = response.check_success().await?;
            Ok(response.json().await?)
        }
        .await;

        match res {
            Ok(res) => res,
            Err(err) => err.into(),
        }
    }

    pub async fn get_order(&self, order_id: &str) -> Result<GetOrderResponse, CoincubeError> {
        let url = format!("{}/api/v1/mavapay/orders/{}", self.0.base_url, order_id);
        let response = self.0.client.request(Method::GET, &url).send().await?;

        let response = response.check_success().await?;
        Ok(response.json().await?)
    }

    pub async fn get_transactions(&self) -> Result<GetTransactionsResponse, CoincubeError> {
        let url = format!("{}/api/v1/mavapay/transactions", self.0.base_url);
        let response = self.0.client.request(Method::GET, &url).send().await?;

        let response = response.check_success().await?;
        Ok(response.json().await?)
    }

    #[cfg(debug_assertions)]
    pub async fn simulate_pay_in(
        &self,
        request: &SimulatePayInRequest,
    ) -> MavapayApiResult<String> {
        let url = format!("{}/api/v1/mavapay/proxy/simulation/pay-in", self.0.base_url);
        let res: Result<_, CoincubeError> = async {
            let response = self
                .0
                .client
                .request(Method::POST, &url)
                .json(&request)
                .send()
                .await?;

            let response = response.check_success().await?;
            Ok(response.json().await?)
        }
        .await;

        match res {
            Ok(res) => res,
            Err(err) => err.into(),
        }
    }

    pub async fn get_banks(&self, country_code: &str) -> MavapayApiResult<MavapayBanks> {
        let url = format!("{}/api/v1/mavapay/proxy/bank/bankcodes", self.0.base_url);
        let res: Result<_, CoincubeError> = async {
            let response = self
                .0
                .client
                .request(Method::GET, &url)
                .query(&[("country", country_code)])
                .send()
                .await?;

            let response = response.check_success().await?;
            Ok(response.json().await?)
        }
        .await;

        match res {
            Ok(res) => res,
            Err(err) => err.into(),
        }
    }

    pub fn transaction_subscription(
        &self,
        order_id: String,
        user_jwt: String,
    ) -> iced::Subscription<MavapayMessage> {
        iced::Subscription::run_with((order_id, user_jwt), transaction_stream)
    }
}
