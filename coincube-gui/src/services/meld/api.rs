use crate::services::coincube;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(untagged)]
pub enum MeldApiResult<T> {
    Error {
        code: String,
        message: String,
        errors: Vec<String>,
    },
    Success(T),
    #[serde(skip)]
    Other(coincube::CoincubeError),
}

impl<T> From<coincube::CoincubeError> for MeldApiResult<T> {
    fn from(e: coincube::CoincubeError) -> Self {
        Self::Other(e)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CurrencyLimit {
    currency_code: String,
    default_amount: f32,
    minimum_amount: f32,
    maximum_amount: f32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FindPaymentMethodsRequest {
    categories: String,
    account_filter: bool,
    countries: String,
    fiat_currencies: String,
    crypto_currencies: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaymentMethodLogo {
    dark: String,
    light: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaymentMethod {
    payment_method: String,
    name: String,
    logos: PaymentMethodLogo,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TransactionType {
    CryptoPurchase,
    CryptoSell,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetQuoteRequest<'a> {
    country_code: &'a str,
    destination_currency_code: &'a str,
    source_currency_code: &'a str,
    source_amount: f32,
    wallet_address: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Quote {
    transaction_type: TransactionType,

    wallet_address: Option<String>,
    source_amount: f32,
    destination_amount: f32,

    institution_name: Option<String>,
    exchange_rate: f32,
    total_fees: f32,

    source_currency_code: String,
    destination_currency_code: String,

    // TODO: Use enums instead
    payment_method_type: String,
    service_provider: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetQuoteResponse {
    quotes: Vec<Quote>,
    message: Option<String>,
    error: Option<String>,
}
