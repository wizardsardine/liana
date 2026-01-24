use super::api::{GetPriceResult, ListCurrenciesResult, PriceApi, PriceApiError};
use super::currency::Currency;
use super::source::PriceSource;

use async_trait::async_trait;

use crate::services::http::ResponseExt;

pub struct PriceClient<C> {
    inner: C,
    pub source: PriceSource,
}

impl<C> PriceClient<C> {
    pub fn new(inner: C, source: PriceSource) -> Self {
        Self { inner, source }
    }
}

impl<C: Default> PriceClient<C> {
    pub fn default_from_source(source: PriceSource) -> Self {
        Self::new(C::default(), source)
    }
}

#[async_trait]
impl PriceApi for PriceClient<reqwest::Client> {
    async fn get_price(&self, currency: Currency) -> Result<GetPriceResult, PriceApiError> {
        let url = self.source.get_price_url(currency);
        tracing::debug!("Fetching price from {} for {}: {}", self.source, currency, url);
        let data = get_data(&self.inner, &url).await?;
        tracing::debug!("Received data from {}: {:?}", self.source, data);
        let result = self.source.parse_price_data(currency, &data);
        tracing::debug!("Parsed price result from {}: {:?}", self.source, result);
        result
    }

    async fn list_currencies(&self) -> Result<ListCurrenciesResult, PriceApiError> {
        let url = self.source.list_currencies_url();
        let data = get_data(&self.inner, &url).await?;
        self.source.parse_currencies_data(&data)
    }
}

// Sends a GET request to the specified URL and returns the parsed JSON response.
// If the request fails or the response is not successful, it returns an error.
async fn get_data(client: &reqwest::Client, url: &str) -> Result<serde_json::Value, PriceApiError> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| PriceApiError::RequestFailed(e.to_string()))?
        .check_success()
        .await
        .map_err(PriceApiError::NotSuccessResponse)?;
    let data: serde_json::Value = response
        .json()
        .await
        .map_err(|e| PriceApiError::CannotParseResponse(e.to_string()))?;
    Ok(data)
}
