use std::str::FromStr;

use super::api::{GetPriceResult, ListCurrenciesResult, PriceApiError};
use super::currency::Currency;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum PriceSource {
    #[default]
    Coincube,
    CoinGecko,
    MempoolSpace,
}

/// All variants of `PriceSource`.
pub const ALL_PRICE_SOURCES: [PriceSource; 3] = [
    PriceSource::Coincube,
    PriceSource::MempoolSpace,
    PriceSource::CoinGecko,
];

impl std::fmt::Display for PriceSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Coincube => write!(f, "COINCUBE"),
            Self::CoinGecko => write!(f, "coingecko"),
            Self::MempoolSpace => write!(f, "mempool.space"),
        }
    }
}

impl FromStr for PriceSource {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "coincube" => Ok(Self::Coincube),
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
            Self::Coincube => Some("Powered by Coincube"),
            // See https://www.coingecko.com/en/api_terms
            Self::CoinGecko => Some("Powered by CoinGecko"),
            Self::MempoolSpace => None,
        }
        .map(|s| s.to_string())
    }

    /// Returns the URL to fetch the price for a given currency.
    pub fn get_price_url(&self, currency: Currency) -> String {
        match self {
            Self::Coincube => format!(
                "https://api.coincube.io/api/v1/exchange-rates/price/{}",
                currency
            ),
            Self::CoinGecko => "https://api.coingecko.com/api/v3/exchange_rates".to_string(),
            Self::MempoolSpace => "https://mempool.space/api/v1/prices".to_string(),
        }
    }

    /// Returns the URL to fetch the list of supported currencies.
    pub fn list_currencies_url(&self) -> String {
        match self {
            Self::Coincube => {
                "https://api.coincube.io/api/v1/exchange-rates/currencies".to_string()
            }
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
            // {"attribution":"Powered by CoinGecko","currency":"USD","source":"coingecko","value":89395.27}
            Self::Coincube => {
                let value = data
                    .get("value")
                    .and_then(|v| v.as_f64())
                    .ok_or(PriceApiError::CannotParseData("price".to_string()))?;
                (value, None)
            }
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
            // {"currencies":["AED","ILS","NGN","XAG","HNL","SVC","HUF","NOK","RON","BTC","USD","CHF","IDR","KWD","SATS","ZMW","MMK","VND","LKR","MXN","TRY","KES","DOP","BNB","XLM","ARS","CLP","VEF","GEL","NZD","RUB","PKR","ETH","AUD","BMD","CNY","GBP","MYR","PHP","EOS","CZK","INR","COP","CRC","AMD","DOT","XDR","LBP","BRL","DKK","HKD","JPY","SGD","THB","UAH","BDT","KRW","SAR","BITS","LTC","LINK","YFI","EUR","PLN","SEK","GTQ","BHD","CAD","BAM","TWD","ZAR","XAU","SOL","PEN","BCH","XRP"]}
            Self::Coincube => data
                .get("currencies")
                .and_then(|currencies| currencies.as_array())
                .ok_or(PriceApiError::CannotParseData(
                    "data is not array".to_string(),
                ))?
                .iter()
                .filter_map(|s| s.as_str().and_then(|s| s.parse::<Currency>().ok()))
                .collect(),
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
