use std::convert::TryFrom;

use liana::miniscript::bitcoin::Amount;
use liana_ui::component::amount::{format_f64_as_string, DisplayAmount};

use crate::app::cache;
use crate::services::fiat::Currency;

/// A fiat amount with a specific currency.
pub struct FiatAmount {
    pub amount: f64,
    pub currency: Currency,
}

impl FiatAmount {
    pub fn new(amount: f64, currency: Currency) -> Self {
        FiatAmount { amount, currency }
    }
}

// Format a fiat amount as a string with two decimal places and a comma as the thousands separator.
impl DisplayAmount for FiatAmount {
    fn to_formatted_string(&self) -> String {
        format_f64_as_string(self.amount, ",", 2, false)
    }
}

/// Used to convert a bitcoin `Amount` to fiat.
pub struct FiatAmountConverter {
    pub price_per_btc: f64,
    pub currency: Currency,
}

impl FiatAmountConverter {
    pub fn convert(&self, btc_amount: Amount) -> FiatAmount {
        let fiat_amt = btc_amount.to_btc() * self.price_per_btc;
        FiatAmount::new(fiat_amt, self.currency)
    }
}

impl TryFrom<cache::FiatPrice> for FiatAmountConverter {
    type Error = String;

    fn try_from(fiat_price: cache::FiatPrice) -> Result<Self, Self::Error> {
        let cache::FiatPrice { res, request, .. } = fiat_price;
        res.map(|price| FiatAmountConverter {
            price_per_btc: price.value,
            currency: request.currency,
        })
        .map_err(|e| e.to_string())
    }
}
