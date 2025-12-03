use reqwest::{Method, RequestBuilder};

use super::api::*;
use crate::services::http::ResponseExt;

/// Mavapay API client
#[derive(Debug, Clone)]
pub struct MavapayClient {
    http: reqwest::Client,
    base_url: &'static str,
}

impl MavapayClient {
    /// Create a new Mavapay client
    pub fn new() -> Self {
        let api_key = match (option_env!("MAVAPAY_API_KEY"), cfg!(debug_assertions)) {
            // staging api key
            (None, true) => "6361fa8e19e150db46d0dc614b9874fd199c95d80a9",
            (None, false) => {
                panic!("Unable to initialize Mavapay Client, API key not set at compile time for release builds")
            }
            (Some(k), _) => k,
        };

        let mut headers = reqwest::header::HeaderMap::new();
        let base_url = if cfg!(debug_assertions) {
            "https://staging.api.mavapay.co/api/v1"
        } else {
            "https://api.mavapay.co/api/v1"
        };

        headers.append(
            "key",
            reqwest::header::HeaderValue::from_str(api_key).unwrap(),
        );

        Self {
            http: reqwest::Client::builder()
                .default_headers(headers)
                .build()
                .unwrap(),
            base_url,
        }
    }

    /// Create an authenticated request
    fn request(&self, method: Method, endpoint: &str) -> RequestBuilder {
        let url = format!("{}{}", self.base_url, endpoint);
        self.http.request(method, &url)
    }

    /// Get current Bitcoin price in specified currency
    pub async fn get_price(
        &self,
        currency: MavapayCurrency,
    ) -> Result<GetPriceResponse, MavapayError> {
        let response = self
            .request(Method::GET, "/price")
            .query(&[("currency", currency)])
            .send()
            .await?
            .check_success()
            .await?;

        match response.json::<MavapayResponse<GetPriceResponse>>().await? {
            MavapayResponse::Error { message } => Err(MavapayError::ApiError(message)),
            MavapayResponse::Success { data } => Ok(data),
        }
    }

    /// Create a quote for currency conversion
    pub async fn create_quote(
        &self,
        request: GetQuoteRequest,
    ) -> Result<GetQuoteResponse, MavapayError> {
        let response = self
            .request(Method::POST, "/quote")
            .json(&request)
            .send()
            .await?;

        let response = response.check_success().await?;
        let response: MavapayResponse<GetQuoteResponse> = response.json().await?;

        // check for errors
        let quote = match response {
            MavapayResponse::Error { message } => return Err(MavapayError::ApiError(message)),
            MavapayResponse::Success { data } => data,
        };

        Ok(quote)
    }

    pub async fn get_banks(&self, country_code: &str) -> Result<MavapayBanks, MavapayError> {
        let response = self
            .request(Method::GET, "/bank/bankcode")
            .query(&[("country", country_code)])
            .send()
            .await?;
        let response = response.check_success().await?;

        match response.json().await? {
            MavapayResponse::Success { data } => Ok(data),
            MavapayResponse::Error { message } => Err(MavapayError::ApiError(message)),
        }
    }

    pub async fn bank_customer_inquiry(
        &self,
        account_number: &str,
        bank_code: &str,
    ) -> Result<BankCustomerInquiry, MavapayError> {
        let response = self
            .request(Method::GET, "/bank/name-enquiry")
            .query(&[("accountNumber", account_number), ("bankCode", bank_code)])
            .send()
            .await?
            .check_success()
            .await?;

        match response.json().await? {
            MavapayResponse::Success { data } => Ok(data),
            MavapayResponse::Error { message } => Err(MavapayError::ApiError(message)),
        }
    }

    pub async fn get_wallet(
        &self,
        id: &str,
        currency: Option<&str>,
    ) -> Result<Vec<MavapayWallet>, MavapayError> {
        let request = self.request(Method::GET, "/wallet");
        let request = match currency {
            Some(c) => request.query([("currency", c), ("walletId", id)].as_slice()),
            None => request.query([("walletId", id)].as_slice()),
        };

        let response = request.send().await?.check_success().await?;
        match response.json().await? {
            MavapayResponse::Success { data } => Ok(data),
            MavapayResponse::Error { message } => Err(MavapayError::ApiError(message)),
        }
    }

    /// Get transaction history
    pub async fn get_transactions(
        &self,
        options: GetTransactions,
    ) -> Result<(GetTransactionPagination, Vec<Transaction>), MavapayError> {
        let request = self.request(Method::GET, "/transactions").query(&options);
        let response = request.send().await?.check_success().await?;

        // `GET /transactions` has a custom response structure
        match response.json().await? {
            GetTransactionResponse::Error { message } => Err(MavapayError::ApiError(message)),
            GetTransactionResponse::Success { pagination, data } => Ok((pagination, data)),
        }
    }

    pub async fn get_transaction(&self, transaction_id: &str) -> Result<Transaction, MavapayError> {
        let response = self
            .request(Method::GET, &format!("/transactions/{}", transaction_id))
            .send()
            .await?
            .check_success()
            .await?;

        match response.json().await? {
            MavapayResponse::Error { message } => Err(MavapayError::ApiError(message)),
            MavapayResponse::Success { data } => Ok(data),
        }
    }
}
