use std::convert::TryFrom;
use std::num::ParseFloatError;

use iced::widget::Column;
use liana::miniscript::bitcoin::Amount;
use liana_ui::component::amount::{format_f64_as_string, DisplayAmount};
use liana_ui::component::text::text;
use liana_ui::theme;
use liana_ui::widget::Container;

use crate::app::cache;
use crate::services::fiat::{Currency, PriceSource};
use crate::utils::now;

/// A non-negative fiat amount with a specific currency.
#[derive(Debug, Clone, Copy)]
pub struct FiatAmount {
    amount: f64,
    currency: Currency,
}

#[derive(Debug, Clone)]
pub enum AmountError {
    Negative,
    ParseError(String),
}

impl std::fmt::Display for AmountError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Negative => write!(f, "Amount must be non-negative"),
            Self::ParseError(e) => write!(f, "Parse error: {}", e),
        }
    }
}

impl FiatAmount {
    pub fn new(amount: f64, currency: Currency) -> Result<Self, AmountError> {
        if amount < 0.0 {
            return Err(AmountError::Negative);
        }
        Ok(Self { amount, currency })
    }

    /// Parse a fiat amount from a string in the given currency.
    pub fn from_str_in(s: &str, currency: Currency) -> Result<Self, AmountError> {
        let amount: f64 = s
            .trim()
            .parse()
            .map_err(|e: ParseFloatError| AmountError::ParseError(e.to_string()))?;
        Self::new(amount, currency)
    }

    pub fn amount(&self) -> f64 {
        self.amount
    }

    pub fn currency(&self) -> Currency {
        self.currency
    }

    /// Format a fiat amount as a string with two decimal places and no thousands separator.
    pub fn to_rounded_string(&self) -> String {
        format_f64_as_string(self.amount, "", 2, false)
    }
}

// Format a fiat amount as a string with two decimal places and a comma as the thousands separator.
impl DisplayAmount for FiatAmount {
    fn to_formatted_string(&self) -> String {
        format_f64_as_string(self.amount, ",", 2, false)
    }
}

#[derive(Debug)]
pub enum AmountConverterError {
    NonPositivePrice,
    ParseError(String),
    CurrencyMismatch {
        expected: Currency,
        actual: Currency,
    },
    ConversionError(String),
}

impl std::fmt::Display for AmountConverterError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::NonPositivePrice => write!(f, "Price per BTC must be positive"),
            Self::ParseError(e) => write!(f, "Parse error: {}", e),
            Self::CurrencyMismatch { expected, actual } => {
                write!(
                    f,
                    "Currency mismatch: expected {}, got {}",
                    expected, actual
                )
            }
            Self::ConversionError(e) => write!(f, "Conversion error: {}", e),
        }
    }
}

/// Used to convert a bitcoin `Amount` to fiat.
///
/// The price per BTC is guaranteed to be positive.
#[derive(Debug, Clone, Copy)]
pub struct FiatAmountConverter {
    price_per_btc: f64,
    /// When the price was last updated at the source (as a unix timestamp in seconds).
    updated_at: Option<u64>,
    /// The request that was used to fetch the price.
    request: cache::FiatPriceRequest,
}

impl FiatAmountConverter {
    /// Create a new `FiatAmountConverter`.
    ///
    /// Returns an error if `price_per_btc` is not positive.
    pub fn new(
        price_per_btc: f64,
        updated_at: Option<u64>,
        request: cache::FiatPriceRequest,
    ) -> Result<Self, AmountConverterError> {
        if price_per_btc <= 0.0 {
            return Err(AmountConverterError::NonPositivePrice);
        }
        Ok(Self {
            price_per_btc,
            updated_at,
            request,
        })
    }

    /// Get the price as a `FiatAmount`.
    pub fn to_fiat_amount(&self) -> FiatAmount {
        FiatAmount::new(self.price_per_btc, self.request.currency)
            .expect("price_per_btc is guaranteed to be positive")
    }

    pub fn price_per_btc(&self) -> f64 {
        self.price_per_btc
    }

    pub fn updated_at(&self) -> Option<u64> {
        self.updated_at
    }

    pub fn source(&self) -> PriceSource {
        self.request.source
    }

    pub fn currency(&self) -> Currency {
        self.request.currency
    }

    pub fn requested_at(&self) -> std::time::Instant {
        self.request.instant
    }

    /// Convert a bitcoin `Amount` to a `FiatAmount`.
    pub fn convert(&self, btc_amount: Amount) -> FiatAmount {
        // Note that price_per_btc is guaranteed to be positive by FiatAmountConverter::new()
        // and a BTC `Amount` converted to f64 must be non-negative.
        let fiat_amt = btc_amount.to_btc() * self.price_per_btc();
        FiatAmount::new(fiat_amt, self.currency()).expect("fiat amount is non-negative")
    }

    /// Convert a `FiatAmount` to a bitcoin `Amount`.
    pub fn convert_to_btc(&self, fiat_amount: &FiatAmount) -> Result<Amount, AmountConverterError> {
        if fiat_amount.currency() != self.currency() {
            return Err(AmountConverterError::CurrencyMismatch {
                expected: self.currency(),
                actual: fiat_amount.currency(),
            });
        }
        // Note that price_per_btc is guaranteed to be positive by FiatAmountConverter::new().
        let btc_amt = fiat_amount.amount() / self.price_per_btc();
        // Round to 8 decimal places so that we can convert to a BTC `Amount`.
        let rounded = (btc_amt * 100_000_000.0).round() / 100_000_000.0;
        Amount::from_btc(rounded).map_err(|e| AmountConverterError::ConversionError(e.to_string()))
    }

    /// Create a container with summary information about the fiat price.
    pub fn to_container_summary<'a, M: 'a>(&self) -> Container<'a, M> {
        Container::new(
            Column::new()
                .push(text(format!(
                    "Exchange rate: 1 BTC ~ {} {}",
                    self.to_fiat_amount().to_formatted_string(),
                    self.currency()
                )))
                .push(text(format!("Source: {}", self.source())))
                .push(text(format!(
                    "Last updated at source: {}",
                    self.updated_at()
                        .map(|t| format!("{} seconds ago", now().as_secs().saturating_sub(t)))
                        .unwrap_or("N/A".to_string())
                )))
                .push(text(format!(
                    "Last requested: {} seconds ago",
                    self.requested_at().elapsed().as_secs()
                ))),
        )
        .style(theme::card::simple)
        .padding(10)
    }
}

impl TryFrom<&cache::FiatPrice> for FiatAmountConverter {
    type Error = AmountConverterError;

    fn try_from(fiat_price: &cache::FiatPrice) -> Result<Self, Self::Error> {
        let cache::FiatPrice { res, request, .. } = fiat_price;
        res.as_ref()
            .map_err(|e| AmountConverterError::ParseError(e.to_string()))
            .and_then(|price| Self::new(price.value, price.updated_at, *request))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_fiat_amount_() {
        // Try with negative amounts.
        for amt in &[-1000.0, -10.5, -0.1] {
            let result = FiatAmount::new(*amt, Currency::USD);
            assert!(result.is_err());
            assert!(matches!(result.unwrap_err(), AmountError::Negative));
        }

        // Check non-negaitve amounts work.
        for amt in &[-0.0, 0.0, 0.1, 27.12] {
            let result = FiatAmount::new(*amt, Currency::USD);
            assert!(result.is_ok());
        }
    }

    #[test]
    fn test_new_fiat_amount_converter() {
        let request = cache::FiatPriceRequest {
            source: PriceSource::CoinGecko,
            currency: Currency::USD,
            instant: std::time::Instant::now(),
        };
        // Try with non-positive prices.
        for price in &[-1000.0, -10.5, -0.0, 0.0] {
            let result = FiatAmountConverter::new(*price, None, request);
            assert!(result.is_err());
            assert!(matches!(
                result.unwrap_err(),
                AmountConverterError::NonPositivePrice
            ));
        }

        // Check a positive price works.
        assert!(FiatAmountConverter::new(27.12, None, request).is_ok());
    }
}
