use reqwest::{Method, RequestBuilder};

use super::api::*;
use crate::services::http::ResponseExt;

/// Mavapay API client
#[derive(Debug, Clone)]
pub struct MavapayClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
}

impl MavapayClient {
    /// Create a new Mavapay client
    pub fn new(api_key: String) -> Self {
        let base_url = if cfg!(debug_assertions) {
            "https://staging.api.mavapay.co/api/v1".to_string()
        } else {
            "https://api.mavapay.co/api/v1".to_string()
        };

        if api_key.is_empty() {
            tracing::warn!("Mavapay API key is empty - API calls will fail");
        } else {
            tracing::info!(
                "Mavapay client initialized with API key: {}...",
                &api_key[..api_key.len().min(8)]
            );
        }

        Self {
            http: reqwest::Client::new(),
            base_url,
            api_key,
        }
    }

    /// Create a new client with custom base URL (for testing)
    pub fn with_base_url(api_key: String, base_url: String) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url,
            api_key,
        }
    }

    /// Create an authenticated request
    fn request(&self, method: Method, endpoint: &str) -> RequestBuilder {
        let url = format!("{}/{}", self.base_url, endpoint.trim_start_matches('/'));

        tracing::debug!(
            "Mavapay API request: {} {} with API key: {}...",
            method,
            url,
            &self.api_key[..self.api_key.len().min(8)]
        );

        self.http
            .request(method, &url)
            .header("x-api-key", &self.api_key)
            .header("Content-Type", "application/json")
    }

    /// Get current Bitcoin price in specified currency
    pub async fn get_price(&self, currency: &str) -> Result<PriceResponse, MavapayError> {
        let response = self
            .request(Method::GET, "price")
            .query(&[("currency", currency)])
            .send()
            .await?
            .check_success()
            .await?;

        let price_data: serde_json::Value = response.json().await?;

        tracing::debug!(
            "Price API response: {}",
            serde_json::to_string_pretty(&price_data).unwrap_or_default()
        );

        // Parse price from the actual API response structure
        // Priority order: root level fields first (actual API format), then fallback to nested structures
        let price = if let Some(btc_price) = price_data
            .get("btcPriceInUnitCurrency")
            .and_then(|p| p.as_str())
        {
            // Primary: Handle the actual API response format at root level - btcPriceInUnitCurrency as string
            tracing::debug!(
                "Found root-level btcPriceInUnitCurrency as string: {}",
                btc_price
            );
            btc_price.parse::<f64>().map_err(|_| {
                MavapayError::InvalidResponse(format!(
                    "Cannot parse btcPriceInUnitCurrency '{}' as float",
                    btc_price
                ))
            })?
        } else if let Some(btc_price) = price_data
            .get("btcPriceInUnitCurrency")
            .and_then(|p| p.as_f64())
        {
            // Primary: Handle the actual API response format at root level - btcPriceInUnitCurrency as number
            tracing::debug!(
                "Found root-level btcPriceInUnitCurrency as number: {}",
                btc_price
            );
            btc_price
        } else if let Some(unit_price) = price_data
            .get("unitPricePerSat")
            .and_then(|p| p.get("amount"))
            .and_then(|a| a.as_str())
        {
            // Alternative: use unit price per sat at root level
            tracing::debug!(
                "Found root-level unitPricePerSat amount as string: {}",
                unit_price
            );
            unit_price.parse::<f64>().map_err(|_| {
                MavapayError::InvalidResponse(format!(
                    "Cannot parse unitPricePerSat amount '{}' as float",
                    unit_price
                ))
            })?
        } else if let Some(p) = price_data.get("price").and_then(|p| p.as_f64()) {
            // Fallback: direct price field as number
            tracing::debug!("Found root-level price as number: {}", p);
            p
        } else if let Some(p) = price_data.get("rate").and_then(|p| p.as_f64()) {
            // Fallback: rate field as number
            tracing::debug!("Found root-level rate as number: {}", p);
            p
        } else if let Some(data) = price_data.get("data") {
            // Fallback: Handle nested data structure for other API formats
            tracing::debug!(
                "Found nested data structure: {}",
                serde_json::to_string_pretty(&data).unwrap_or_default()
            );

            if let Some(btc_price) = data.get("btcPriceInUnitCurrency").and_then(|p| p.as_str()) {
                tracing::debug!(
                    "Found nested btcPriceInUnitCurrency as string: {}",
                    btc_price
                );
                btc_price.parse::<f64>().map_err(|_| {
                    MavapayError::InvalidResponse(format!(
                        "Cannot parse nested btcPriceInUnitCurrency '{}' as float",
                        btc_price
                    ))
                })?
            } else if let Some(btc_price) =
                data.get("btcPriceInUnitCurrency").and_then(|p| p.as_f64())
            {
                tracing::debug!(
                    "Found nested btcPriceInUnitCurrency as number: {}",
                    btc_price
                );
                btc_price
            } else if let Some(unit_price) = data
                .get("unitPricePerSat")
                .and_then(|p| p.get("amount"))
                .and_then(|a| a.as_str())
            {
                tracing::debug!(
                    "Found nested unitPricePerSat amount as string: {}",
                    unit_price
                );
                unit_price.parse::<f64>().map_err(|_| {
                    MavapayError::InvalidResponse(format!(
                        "Cannot parse nested unitPricePerSat amount '{}' as float",
                        unit_price
                    ))
                })?
            } else if let Some(p) = data.get("price").and_then(|p| p.as_f64()) {
                tracing::debug!("Found nested price as number: {}", p);
                p
            } else if let Some(p) = data.get("rate").and_then(|p| p.as_f64()) {
                tracing::debug!("Found nested rate as number: {}", p);
                p
            } else if let Some(p) = data.as_f64() {
                tracing::debug!("Found nested data as number: {}", p);
                p
            } else {
                tracing::error!(
                    "No price found in nested data. Available fields: {:?}",
                    data.as_object()
                        .map(|o| o.keys().collect::<Vec<_>>())
                        .unwrap_or_default()
                );
                return Err(MavapayError::InvalidResponse(format!(
                    "No price found in nested data object: {}",
                    data
                )));
            }
        } else {
            return Err(MavapayError::InvalidResponse(format!(
                "No price found in response. Available fields: {:?}",
                price_data
                    .as_object()
                    .map(|o| o.keys().collect::<Vec<_>>())
                    .unwrap_or_default()
            )));
        };

        Ok(PriceResponse {
            price,
            currency: currency.to_string(),
            timestamp: None,
        })
    }

    /// Get exchange rate between two currencies
    pub async fn get_exchange_rate(&self, pair: &str) -> Result<f64, MavapayError> {
        let response = self
            .request(Method::GET, "price/ticker")
            .query(&[("pair", pair)])
            .send()
            .await?
            .check_success()
            .await?;

        let rate_data: serde_json::Value = response.json().await?;

        let rate = rate_data
            .get("rate")
            .and_then(|r| r.as_f64())
            .ok_or_else(|| MavapayError::InvalidResponse("Missing rate field".to_string()))?;

        Ok(rate)
    }

    /// Create a quote for currency conversion
    pub async fn create_quote(&self, request: QuoteRequest) -> Result<QuoteResponse, MavapayError> {
        let response = self
            .request(Method::POST, "quote")
            .json(&request)
            .send()
            .await?
            .check_success()
            .await?;

        let api_response: ApiResponse<QuoteResponse> = response.json().await.map_err(|e| {
            MavapayError::InvalidResponse(format!("Failed to parse quote response: {}", e))
        })?;

        if api_response.status != "ok" {
            return Err(MavapayError::InvalidResponse(
                "API returned non-ok status".to_string(),
            ));
        }

        Ok(api_response.data)
    }

    /// Create a hosted checkout payment link
    pub async fn create_payment_link(
        &self,
        request: PaymentLinkRequest,
    ) -> Result<String, MavapayError> {
        #[cfg(debug_assertions)]
        tracing::debug!(
            "[PAYMENT_LINK] Request: {}",
            serde_json::to_string_pretty(&request).unwrap_or_default()
        );

        let response = self
            .request(Method::POST, "paymentlink")
            .json(&request)
            .send()
            .await?
            .check_success()
            .await?;

        // Try to parse standard { status, data } wrapper
        let try_wrapped: Result<ApiResponse<PaymentLinkResponse>, _> = response.json().await;
        if let Ok(api_response) = try_wrapped {
            if api_response.status != "ok" {
                return Err(MavapayError::InvalidResponse(
                    "API returned non-ok status".to_string(),
                ));
            }
            return Ok(api_response.data.url);
        }

        // Fallback: parse as plain { url: "..." }
        let fallback: PaymentLinkResponse = self
            .request(Method::POST, "paymentlink")
            .json(&request)
            .send()
            .await?
            .check_success()
            .await?
            .json()
            .await
            .map_err(|e| {
                MavapayError::InvalidResponse(format!("Failed to parse payment link: {}", e))
            })?;

        Ok(fallback.url)
    }

    /// Get list of supported banks for a country
    pub async fn get_banks(&self, country_code: &str) -> Result<Vec<BankInfo>, MavapayError> {
        let response = self
            .request(Method::GET, "bank/bankcode")
            .query(&[("country", country_code)])
            .send()
            .await?
            .check_success()
            .await?;

        let banks_data: serde_json::Value = response.json().await?;

        // Parse banks array from response
        let banks = banks_data
            .get("data")
            .and_then(|d| d.as_array())
            .ok_or_else(|| MavapayError::InvalidResponse("Missing banks data".to_string()))?;

        let mut bank_list = Vec::new();
        for bank in banks {
            if let (Some(code), Some(name)) = (
                bank.get("code").and_then(|c| c.as_str()),
                bank.get("name").and_then(|n| n.as_str()),
            ) {
                bank_list.push(BankInfo {
                    code: code.to_string(),
                    name: name.to_string(),
                    country: country_code.to_string(),
                });
            }
        }

        Ok(bank_list)
    }

    /// Validate bank account details
    pub async fn validate_bank_account(
        &self,
        account_number: &str,
        bank_code: &str,
    ) -> Result<String, MavapayError> {
        let response = self
            .request(Method::GET, "bank/name-enquiry")
            .query(&[("accountNumber", account_number), ("bankCode", bank_code)])
            .send()
            .await?
            .check_success()
            .await?;

        let account_data: serde_json::Value = response.json().await?;

        let account_name = account_data
            .get("data")
            .and_then(|d| d.get("accountName"))
            .and_then(|n| n.as_str())
            .ok_or(MavapayError::BankAccountValidationFailed)?;

        Ok(account_name.to_string())
    }

    /// Get wallet balance
    pub async fn get_wallet_balance(
        &self,
        currency: Option<&str>,
    ) -> Result<Vec<WalletBalance>, MavapayError> {
        let mut request = self.request(Method::GET, "wallet");

        if let Some(curr) = currency {
            request = request.query(&[("currency", curr)]);
        }

        let response = request.send().await?.check_success().await?;

        let wallet_data: serde_json::Value = response.json().await?;

        // Parse wallet balances
        let balances = wallet_data
            .get("data")
            .and_then(|d| d.as_array())
            .ok_or_else(|| MavapayError::InvalidResponse("Missing wallet data".to_string()))?;

        let mut balance_list = Vec::new();
        for balance in balances {
            if let (Some(currency), Some(balance_val), Some(available)) = (
                balance.get("currency").and_then(|c| c.as_str()),
                balance.get("balance").and_then(|b| b.as_u64()),
                balance.get("availableBalance").and_then(|a| a.as_u64()),
            ) {
                balance_list.push(WalletBalance {
                    currency: currency.to_string(),
                    balance: balance_val,
                    available_balance: available,
                });
            }
        }

        Ok(balance_list)
    }

    /// Get transaction history
    pub async fn get_transactions(
        &self,
        page: Option<u32>,
        limit: Option<u32>,
        status: Option<TransactionStatus>,
    ) -> Result<Vec<Transaction>, MavapayError> {
        let mut request = self.request(Method::GET, "transactions");

        let mut query_params = Vec::new();
        if let Some(p) = page {
            query_params.push(("page", p.to_string()));
        }
        if let Some(l) = limit {
            query_params.push(("limit", l.to_string()));
        }
        if let Some(s) = status {
            let status_str = match s {
                TransactionStatus::Pending => "pending",
                TransactionStatus::Success => "success",
                TransactionStatus::Failed => "failed",
                TransactionStatus::Paid => "paid",
            };
            query_params.push(("status", status_str.to_string()));
        }

        if !query_params.is_empty() {
            let query_refs: Vec<(&str, &str)> =
                query_params.iter().map(|(k, v)| (*k, v.as_str())).collect();
            request = request.query(&query_refs);
        }

        let response = request.send().await?.check_success().await?;

        let tx_data: serde_json::Value = response.json().await?;

        // Parse transactions
        let transactions = tx_data
            .get("data")
            .and_then(|d| d.get("transactions"))
            .and_then(|t| t.as_array())
            .ok_or_else(|| {
                MavapayError::InvalidResponse("Missing transactions data".to_string())
            })?;

        let mut tx_list = Vec::new();
        for tx in transactions {
            if let (
                Some(id),
                Some(amount),
                Some(currency),
                Some(status),
                Some(created),
                Some(updated),
            ) = (
                tx.get("id").and_then(|i| i.as_str()),
                tx.get("amount").and_then(|a| a.as_u64()),
                tx.get("currency").and_then(|c| c.as_str()),
                tx.get("status").and_then(|s| s.as_str()),
                tx.get("createdAt").and_then(|c| c.as_str()),
                tx.get("updatedAt").and_then(|u| u.as_str()),
            ) {
                let status_enum = match status {
                    "PENDING" => TransactionStatus::Pending,
                    "SUCCESS" => TransactionStatus::Success,
                    "FAILED" => TransactionStatus::Failed,
                    "PAID" => TransactionStatus::Paid,
                    _ => continue, // Skip unknown status
                };

                tx_list.push(Transaction {
                    id: id.to_string(),
                    amount,
                    currency: currency.to_string(),
                    status: status_enum,
                    created_at: created.to_string(),
                    updated_at: updated.to_string(),
                    hash: tx
                        .get("hash")
                        .and_then(|h| h.as_str())
                        .map(|s| s.to_string()),
                });
            }
        }

        Ok(tx_list)
    }

    /// Confirm a quote and initiate payment processing
    pub async fn confirm_quote(
        &self,
        quote_id: String,
    ) -> Result<PaymentStatusResponse, MavapayError> {
        let request = PaymentConfirmationRequest { quote_id };

        let response = self
            .request(Method::POST, "quote/confirm")
            .json(&request)
            .send()
            .await?
            .check_success()
            .await?;

        let api_response: ApiResponse<PaymentStatusResponse> =
            response.json().await.map_err(|e| {
                MavapayError::InvalidResponse(format!(
                    "Failed to parse confirmation response: {}",
                    e
                ))
            })?;

        if api_response.status != "ok" {
            return Err(MavapayError::InvalidResponse(
                "API returned non-ok status".to_string(),
            ));
        }

        Ok(api_response.data)
    }

    /// Get payment status by quote ID
    pub async fn get_payment_status(
        &self,
        quote_id: &str,
    ) -> Result<PaymentStatusResponse, MavapayError> {
        let response = self
            .request(Method::GET, "payment/status")
            .query(&[("quoteId", quote_id)])
            .send()
            .await?
            .check_success()
            .await?;

        let api_response: ApiResponse<PaymentStatusResponse> =
            response.json().await.map_err(|e| {
                MavapayError::InvalidResponse(format!("Failed to parse status response: {}", e))
            })?;

        if api_response.status != "ok" {
            return Err(MavapayError::InvalidResponse(
                "API returned non-ok status".to_string(),
            ));
        }

        Ok(api_response.data)
    }

    /// Get single transaction by ID
    pub async fn get_transaction(&self, transaction_id: &str) -> Result<Transaction, MavapayError> {
        let response = self
            .request(Method::GET, &format!("transactions/{}", transaction_id))
            .send()
            .await?
            .check_success()
            .await?;

        let api_response: ApiResponse<Transaction> = response.json().await.map_err(|e| {
            MavapayError::InvalidResponse(format!("Failed to parse transaction response: {}", e))
        })?;

        if api_response.status != "ok" {
            return Err(MavapayError::InvalidResponse(
                "API returned non-ok status".to_string(),
            ));
        }

        Ok(api_response.data)
    }

    /// Register webhook URL for real-time notifications
    pub async fn register_webhook(&self, url: &str, secret: &str) -> Result<(), MavapayError> {
        let request = serde_json::json!({
            "url": url,
            "secret": secret
        });

        let response = self
            .request(Method::POST, "webhook/register")
            .json(&request)
            .send()
            .await?
            .check_success()
            .await?;

        let api_response: ApiResponse<serde_json::Value> = response.json().await.map_err(|e| {
            MavapayError::InvalidResponse(format!("Failed to parse webhook response: {}", e))
        })?;

        if api_response.status != "ok" {
            return Err(MavapayError::InvalidResponse(
                "Failed to register webhook".to_string(),
            ));
        }

        Ok(())
    }

    /// Poll transaction status with automatic retries
    pub async fn poll_transaction_status(
        &self,
        quote_id: &str,
        max_attempts: u32,
        interval_seconds: u64,
    ) -> Result<PaymentStatusResponse, MavapayError> {
        use tokio::time::{sleep, Duration};

        for attempt in 1..=max_attempts {
            tracing::debug!(
                "Polling payment status, attempt {}/{}",
                attempt,
                max_attempts
            );

            match self.get_payment_status(quote_id).await {
                Ok(status) => match status.status.as_str() {
                    "PAID" | "SUCCESS" => {
                        tracing::info!("Payment completed with status: {}", status.status);
                        return Ok(status);
                    }
                    "FAILED" => {
                        tracing::error!("Payment failed for quote: {}", quote_id);
                        return Err(MavapayError::PaymentFailed);
                    }
                    "PENDING" => {
                        tracing::debug!("Payment still pending, will retry...");
                        if attempt < max_attempts {
                            sleep(Duration::from_secs(interval_seconds)).await;
                        }
                    }
                    _ => {
                        tracing::warn!("Unknown payment status: {}", status.status);
                        if attempt < max_attempts {
                            sleep(Duration::from_secs(interval_seconds)).await;
                        }
                    }
                },
                Err(e) => {
                    tracing::error!("Error polling payment status: {}", e);
                    if attempt < max_attempts {
                        sleep(Duration::from_secs(interval_seconds)).await;
                    } else {
                        return Err(e);
                    }
                }
            }
        }

        Err(MavapayError::PaymentTimeout)
    }
}
