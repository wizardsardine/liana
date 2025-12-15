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
        let (api_key, base_url) = match option_env!("MAVAPAY_API_KEY") {
            // staging api key
            None => {
                log::info!("[MAVAPAY] Using staging environment");
                (
                    "6361fa8e19e150db46d0dc614b9874fd199c95d80a9",
                    "https://staging.api.mavapay.co/api",
                )
            }
            // use production endpoint if api key is set
            Some(k) => {
                log::info!("[MAVAPAY] Using production environment");
                (k, "https://api.mavapay.co/api")
            }
        };

        let mut headers = reqwest::header::HeaderMap::new();
        headers.append(
            "X-API-KEY",
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
            .request(Method::GET, "/v1/price")
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

    pub async fn create_quote(
        &self,
        request: GetQuoteRequest,
    ) -> Result<GetQuoteResponse, MavapayError> {
        let response = self
            .request(Method::POST, "/v1/quote")
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

    pub async fn get_order(&self, order_id: &str) -> Result<GetOrderResponse, MavapayError> {
        let response = self
            .request(Method::GET, "/v1/order")
            .query(&[("id", order_id)])
            .send()
            .await?
            .check_success()
            .await?;

        match response.json().await? {
            MavapayResponse::Error { message } => Err(MavapayError::ApiError(message)),
            MavapayResponse::Success { data } => Ok(data),
        }
    }

    #[cfg(debug_assertions)]
    pub async fn simulate_pay_in(
        &self,
        request: &SimulatePayInRequest,
    ) -> Result<String, MavapayError> {
        let response = self
            .request(Method::POST, "/v1/simulation/pay-in")
            .json(&request)
            .send()
            .await?;
        let response = response.check_success().await?;

        match response.json().await? {
            MavapayResponse::Error { message } => Err(MavapayError::ApiError(message)),
            MavapayResponse::Success { data } => Ok(data),
        }
    }

    pub async fn get_banks(&self, country_code: &str) -> Result<MavapayBanks, MavapayError> {
        let response = self
            .request(Method::GET, "/v1/bank/bankcode")
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
            .request(Method::GET, "/v1/bank/name-enquiry")
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

    /// Get transaction history
    pub async fn get_transactions(
        &self,
        options: GetTransactions,
    ) -> Result<(GetTransactionPagination, Vec<Transaction>), MavapayError> {
        let request = self
            .request(Method::GET, "/v1/transactions")
            .query(&options);
        let response = request.send().await?.check_success().await?;

        // `GET /transactions` has a custom response structure
        match response.json().await? {
            GetTransactionResponse::Error { message } => Err(MavapayError::ApiError(message)),
            GetTransactionResponse::Success { pagination, data } => Ok((pagination, data)),
        }
    }

    pub async fn get_transaction(
        &self,
        options: GetTransaction<'_>,
    ) -> Result<Transaction, MavapayError> {
        let response = self
            .request(Method::GET, "/v1/transaction")
            .query(&options)
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
