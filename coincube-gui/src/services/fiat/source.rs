use std::str::FromStr;

use super::api::{GetPriceResult, ListCurrenciesResult, PriceApiError};
use super::currency::Currency;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum PriceSource {
    #[default]
    CoinGecko,
    MempoolSpace,
}

/// All variants of `PriceSource`.
pub const ALL_PRICE_SOURCES: [PriceSource; 2] = [PriceSource::MempoolSpace, PriceSource::CoinGecko];

impl std::fmt::Display for PriceSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CoinGecko => write!(f, "coingecko"),
            Self::MempoolSpace => write!(f, "mempool.space"),
        }
    }
}

impl FromStr for PriceSource {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "coingecko" => Ok(Self::CoinGecko),
            "mempool.space" => Ok(Self::MempoolSpace),
            _ => Err("Invalid price source".to_string()),
        }
    }
}

impl PriceSource {
    /// Required attribution for the price source, if any.
    pub fn attribution(&self) -> Option<String> {
        match self {
            // See https://www.coingecko.com/en/api_terms
            Self::CoinGecko => Some("Powered by CoinGecko"),
            Self::MempoolSpace => None,
        }
        .map(|s| s.to_string())
    }

    /// Returns the URL to fetch the price for a given currency.
    pub fn get_price_url(&self, _currency: Currency) -> String {
        match self {
            Self::CoinGecko => "https://api.coingecko.com/api/v3/exchange_rates".to_string(),
            Self::MempoolSpace => "https://mempool.space/api/v1/prices".to_string(),
        }
    }

    /// Returns the URL to fetch the list of supported currencies.
    pub fn list_currencies_url(&self) -> String {
        match self {
            Self::CoinGecko => "https://api.coingecko.com/api/v3/exchange_rates".to_string(),
            Self::MempoolSpace => "https://mempool.space/api/v1/prices".to_string(),
        }
    }

    /// Parses the price data in the API response from the `get_price_url` endpoint.
    pub fn parse_price_data(
        &self,
        currency: Currency,
        data: &serde_json::Value,
    ) -> Result<GetPriceResult, PriceApiError> {
        let (value, updated_at) = match self {
            Self::CoinGecko => {
                let value = data
                    .get("rates")
                    .and_then(|rates| rates.get(currency.to_string().to_lowercase()))
                    .and_then(|curr| curr.get("value"))
                    .and_then(|num| num.as_f64())
                    .ok_or(PriceApiError::CannotParseData("price".to_string()))?;
                (value, None)
            }
            Self::MempoolSpace => {
                let value = data
                    .get(currency.to_string())
                    .and_then(|curr| curr.as_u64())
                    .map(|v| v as f64)
                    .ok_or(PriceApiError::CannotParseData("price".to_string()))?;
                let updated_at = data.get("time").and_then(|t| t.as_u64());
                (value, updated_at)
            }
        };
        Ok(GetPriceResult { value, updated_at })
    }

    /// Parses the currencies data in the API response from the `list_currencies_url` endpoint.
    pub fn parse_currencies_data(
        &self,
        data: &serde_json::Value,
    ) -> Result<ListCurrenciesResult, PriceApiError> {
        let currencies: Vec<_> = match self {
            Self::CoinGecko => data
                .get("rates")
                .and_then(|rates| rates.as_object())
                .ok_or(PriceApiError::CannotParseData(
                    "data is not object".to_string(),
                ))?
                .keys()
                .filter_map(|v| v.parse::<Currency>().ok())
                .collect(),
            Self::MempoolSpace => data
                .as_object()
                .ok_or(PriceApiError::CannotParseData(
                    "data is not object".to_string(),
                ))?
                .keys()
                .filter_map(|s| s.parse::<Currency>().ok())
                .collect(),
        };
        Ok(ListCurrenciesResult { currencies })
    }
}
