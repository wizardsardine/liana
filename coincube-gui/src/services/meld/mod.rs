pub mod api;

use crate::services::{coincube::CoincubeError, http::ResponseExt};
use reqwest::Method;

pub struct MeldClient<'client>(pub &'client super::coincube::CoincubeClient);

impl<'client> MeldClient<'client> {
    pub async fn get_crypto_purchase_limits<'a>(
        &self,
        currency_code: &'a str,
    ) -> api::MeldApiResult<Vec<api::CurrencyLimit>> {
        let url = format!("{}/api/v1/meld/limits", self.0.base_url);
        let res: Result<_, CoincubeError> = async {
            let response = self
                .0
                .client
                .request(Method::GET, &url)
                .query(&[
                    ("fiatCurrencies", currency_code),
                    ("accountFilter", "true"),
                    ("cryptoCurrencies", "BTC"),
                ])
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

    pub async fn get_crypto_sell_limits<'a>(
        &self,
        currency_code: &'a str,
    ) -> api::MeldApiResult<Vec<api::CurrencyLimit>> {
        let url = format!("{}/api/v1/meld/crypto-sell-limits", self.0.base_url);
        let res: Result<_, CoincubeError> = async {
            let response = self
                .0
                .client
                .request(Method::GET, &url)
                .query(&[
                    ("fiatCurrencies", currency_code),
                    ("accountFilter", "true"),
                    ("cryptoCurrencies", "BTC"),
                ])
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

    pub async fn find_payment_methods<'a>(
        &self,
        req: api::FindPaymentMethodsRequest,
    ) -> api::MeldApiResult<Vec<api::PaymentMethod>> {
        let url = format!("{}/api/v1/meld/payment-methods", self.0.base_url);
        let res: Result<_, CoincubeError> = async {
            let response = self
                .0
                .client
                .request(Method::GET, &url)
                .query(&req)
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

    pub async fn get_quote<'a>(
        &self,
        req: api::GetQuoteRequest<'a>,
    ) -> api::MeldApiResult<api::GetQuoteResponse> {
        let url = format!("{}/api/v1/meld/quote", self.0.base_url);
        let res: Result<_, CoincubeError> = async {
            let response = self
                .0
                .client
                .request(Method::POST, &url)
                .json(&req)
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
}
