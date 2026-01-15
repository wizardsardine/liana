use crate::services::{coincube, http::NotSuccessResponseInfo};
use serde::{Deserialize, Serialize};

#[derive(Debug)]
pub enum MavapayApiResult<T> {
    Success(T),
    Error(String),
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for MavapayApiResult<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;

        // Check if this is a wrapped format: { "status": "ok/error", "data"?: T, "message"?: String }
        // Only treat as wrapped if status is specifically "ok", "success", or "error"
        if let Some(status) = value.get("status").and_then(|s| s.as_str()) {
            match status {
                "ok" | "success" => {
                    if let Some(data) = value.get("data") {
                        return T::deserialize(data.clone())
                            .map(MavapayApiResult::Success)
                            .map_err(serde::de::Error::custom);
                    }
                    // No "data" field with "ok"/"success" status - try parsing as T directly
                }
                "error" => {
                    let message = value
                        .get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or("Unknown error")
                        .to_string();
                    return Ok(MavapayApiResult::Error(message));
                }
                _ => {
                    // Status field exists but is not a wrapper status
                    // This is likely a direct object with a status field, parse as T
                }
            }
        }

        T::deserialize(value)
            .map(MavapayApiResult::Success)
            .map_err(serde::de::Error::custom)
    }
}

impl<T> From<coincube::CoincubeError> for MavapayApiResult<T> {
    fn from(e: coincube::CoincubeError) -> Self {
        let message = match &e {
            coincube::CoincubeError::Unsuccessful(info) => {
                serde_json::from_str::<serde_json::Value>(&info.text)
                    .ok()
                    .and_then(|v| v.get("message")?.as_str().map(String::from))
                    .unwrap_or_else(|| info.text.clone())
            }
            coincube::CoincubeError::Network(e) => format!("Network error: {e}"),
            coincube::CoincubeError::Api(msg) => msg.clone(),
            coincube::CoincubeError::Parse(e) => format!("Parse error: {e}"),
        };
        Self::Error(message)
    }
}

#[derive(Debug, Clone)]
pub enum MavapayError {
    Http(Option<u16>, String),
    InvalidResponse(String),
    ApiError(String),
    QuoteExpired,
    InsufficientFunds,
    InvalidCurrency,
    InvalidAmount,
    BankAccountValidationFailed,
    PaymentFailed,
    PaymentTimeout,
}

impl std::fmt::Display for MavapayError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Http(Some(code), msg) => write!(f, "[{}]: {}", code, msg),
            Self::Http(None, msg) => write!(f, "{}", msg),
            Self::InvalidResponse(msg) => write!(f, "Invalid response: {}", msg),
            Self::ApiError(msg) => write!(f, "Mavapay Error: {}", msg),
            Self::QuoteExpired => write!(f, "Quote has expired"),
            Self::InsufficientFunds => write!(f, "Insufficient funds"),
            Self::InvalidCurrency => write!(f, "Invalid or unsupported currency"),
            Self::InvalidAmount => write!(f, "Invalid amount"),
            Self::BankAccountValidationFailed => write!(f, "Bank account validation failed"),
            Self::PaymentFailed => write!(f, "Payment failed"),
            Self::PaymentTimeout => write!(f, "Payment timeout"),
        }
    }
}

impl From<reqwest::Error> for MavapayError {
    fn from(error: reqwest::Error) -> Self {
        let error = error.without_url();

        log::error!("[REQWEST] {:?}", error);
        Self::Http(error.status().map(|s| s.as_u16()), error.to_string())
    }
}

impl From<NotSuccessResponseInfo> for MavapayError {
    fn from(value: NotSuccessResponseInfo) -> Self {
        Self::Http(Some(value.status_code), value.text)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(tag = "status")]
pub enum MavapayResponse<T> {
    Error {
        message: String,
    },
    #[serde(alias = "ok")]
    Success {
        data: T,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MavapayCurrency {
    #[serde(rename = "KES")]
    KenyanShilling,
    #[serde(rename = "ZAR")]
    SouthAfricanRand,
    #[serde(rename = "NGN")]
    NigerianNaira,
    #[serde(rename = "BTC")]
    Bitcoin,
}

impl MavapayCurrency {
    pub fn all() -> &'static [MavapayCurrency] {
        &[
            MavapayCurrency::NigerianNaira,
            MavapayCurrency::KenyanShilling,
            MavapayCurrency::SouthAfricanRand,
            MavapayCurrency::Bitcoin,
        ]
    }
}

impl MavapayCurrency {
    pub fn from_str(string: &str) -> Option<Self> {
        match string {
            "BTC" => Some(MavapayCurrency::Bitcoin),
            "KES" => Some(MavapayCurrency::KenyanShilling),
            "ZAR" => Some(MavapayCurrency::SouthAfricanRand),
            "NGN" => Some(MavapayCurrency::NigerianNaira),
            _ => None,
        }
    }
}

impl std::fmt::Display for MavapayCurrency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MavapayCurrency::KenyanShilling => write!(f, "Kenyan Shilling (KES)"),
            MavapayCurrency::SouthAfricanRand => write!(f, "South African Rand (ZAR)"),
            MavapayCurrency::NigerianNaira => write!(f, "Nigerian Naira (NGN)"),
            MavapayCurrency::Bitcoin => write!(f, "Bitcoin (BTC)"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MavapayUnitCurrency {
    #[serde(rename = "KESCENT")]
    KenyanShillingCent,
    #[serde(rename = "ZARCENT")]
    SouthAfricanRandCent,
    #[serde(rename = "NGNKOBO")]
    NigerianNairaKobo,
    #[serde(rename = "BTCSAT")]
    BitcoinSatoshi,
}

impl MavapayUnitCurrency {
    pub const fn is_fiat(&self) -> bool {
        !matches!(self, MavapayUnitCurrency::BitcoinSatoshi)
    }
}

impl MavapayUnitCurrency {
    pub fn as_str(&self) -> &'static str {
        match self {
            MavapayUnitCurrency::KenyanShillingCent => "Kenyan Cent",
            MavapayUnitCurrency::SouthAfricanRandCent => "South African Cent",
            MavapayUnitCurrency::NigerianNairaKobo => "Nigerian Kobo",
            MavapayUnitCurrency::BitcoinSatoshi => "Satoshi",
        }
    }
}

impl From<MavapayUnitCurrency> for MavapayCurrency {
    fn from(value: MavapayUnitCurrency) -> Self {
        match value {
            MavapayUnitCurrency::KenyanShillingCent => MavapayCurrency::KenyanShilling,
            MavapayUnitCurrency::SouthAfricanRandCent => MavapayCurrency::SouthAfricanRand,
            MavapayUnitCurrency::NigerianNairaKobo => MavapayCurrency::NigerianNaira,
            MavapayUnitCurrency::BitcoinSatoshi => MavapayCurrency::Bitcoin,
        }
    }
}

impl std::fmt::Display for MavapayUnitCurrency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MavapayUnitCurrency::KenyanShillingCent => write!(f, "Kenyan Cents"),
            MavapayUnitCurrency::SouthAfricanRandCent => write!(f, "South African Cents"),
            MavapayUnitCurrency::NigerianNairaKobo => write!(f, "Nigerian Kobos"),
            MavapayUnitCurrency::BitcoinSatoshi => write!(f, "Bitcoin Satoshis"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum MavapayPaymentMethod {
    Lightning,
    BankTransfer,
    Onchain,
    USDT,
}

impl std::fmt::Display for MavapayPaymentMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MavapayPaymentMethod::Lightning => write!(f, "Bitcoin Lightning"),
            MavapayPaymentMethod::Onchain => write!(f, "Bitcoin Mainnet Transaction"),
            MavapayPaymentMethod::BankTransfer => write!(f, "Bank Transfer"),
            MavapayPaymentMethod::USDT => write!(f, "USDT Transaction"),
        }
    }
}

impl MavapayPaymentMethod {
    pub fn all() -> &'static [MavapayPaymentMethod] {
        &[
            MavapayPaymentMethod::Lightning,
            MavapayPaymentMethod::BankTransfer,
            MavapayPaymentMethod::Onchain,
            MavapayPaymentMethod::USDT,
        ]
    }
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all_fields = "camelCase")]
#[serde(untagged)]
pub enum Beneficiary {
    Lightning {
        ln_invoice: String,
    },
    Onchain {
        on_chain_address: String,
    },
    NGN {
        bank_account_number: String,
        bank_account_name: String,
        bank_code: String,
        bank_name: String,
    },
    ZAR {
        name: String,
        bank_name: String,
        bank_account_number: String,
    },
    KES(KenyanBeneficiary),
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "lowercase")]
#[serde(rename_all_fields = "camelCase")]
#[serde(tag = "identifierType", content = "identifiers")]
pub enum KenyanBeneficiary {
    PayToPhone {
        account_name: String,
        phone_number: String,
    },
    PayToBill {
        account_name: String,
        account_number: String,
        paybill_number: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all_fields = "camelCase")]
#[serde(untagged)]
pub enum MavapayBanks {
    Nigerian(Vec<NigerianBank>),
    SouthAfrican(Vec<String>),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct NigerianBank {
    pub bank_name: String,
    pub nip_bank_code: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetPriceResponse {
    pub currency: MavapayCurrency,
    pub btc_price_in_unit_currency: f64,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum OnchainTransferSpeed {
    Slow,
    Medium,
    Fast,
}

impl std::fmt::Display for OnchainTransferSpeed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OnchainTransferSpeed::Slow => "Slow",
            OnchainTransferSpeed::Medium => "Medium",
            OnchainTransferSpeed::Fast => "Fast",
        }
        .fmt(f)
    }
}

impl OnchainTransferSpeed {
    pub fn all() -> &'static [OnchainTransferSpeed] {
        &[
            OnchainTransferSpeed::Slow,
            OnchainTransferSpeed::Medium,
            OnchainTransferSpeed::Fast,
        ]
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetQuoteRequest {
    pub amount: u64,
    pub source_currency: MavapayUnitCurrency,
    pub target_currency: MavapayUnitCurrency,
    pub payment_method: MavapayPaymentMethod,
    pub payment_currency: MavapayUnitCurrency,
    pub speed: OnchainTransferSpeed,
    pub autopayout: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub customer_internal_fee: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub beneficiary: Option<Beneficiary>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetQuoteResponse {
    pub id: String,
    pub order_id: Option<String>,
    pub exchange_rate: f64,
    pub usd_to_target_currency_rate: f64,
    pub source_currency: MavapayUnitCurrency,
    pub target_currency: MavapayUnitCurrency,
    pub transaction_fees_in_source_currency: u64,
    pub transaction_fees_in_target_currency: u64,
    pub amount_in_source_currency: u64,
    pub amount_in_target_currency: u64,
    pub payment_method: MavapayPaymentMethod,
    pub expiry: String,
    pub is_valid: bool,
    pub invoice: String,
    pub hash: String,
    pub total_amount_in_source_currency: u64,
    pub total_amount_in_target_currency: Option<u64>,
    pub customer_internal_fee: u64,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,

    // undocumented fields
    pub estimated_routing_fee: Option<u64>,
    pub bank_name: Option<String>,
    pub ngn_bank_account_number: Option<String>,
    pub ngn_account_name: Option<String>,
    pub ngn_bank_code: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MavapayOrder {
    pub order_id: String,
    pub quote_id: String,
    pub currency: MavapayCurrency,
    pub balance: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BankCustomerInquiry {
    pub account_name: String,
    pub account_number: String,
    pub kyc_level: String,
    pub name_inquiry_reference: String,
    pub channel_code: String,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OrderQuote {
    pub id: String,
    #[serde(default)]
    pub account_id: Option<String>,
    pub exchange_rate: f64,
    pub usd_to_target_currency_rate: f64,
    #[serde(default)]
    pub usd_amount: Option<u64>,
    pub transaction_fees_in_source_currency: u64,
    pub transaction_fees_in_target_currency: u64,
    pub transaction_fees_in_usd_cent: u64,
    pub payment_btc_detail: String,
    pub payment_method: MavapayPaymentMethod,
    pub total_amount: u64,
    pub equivalent_amount: u64,
    pub expiry: String,
    pub source_currency: MavapayUnitCurrency,
    pub target_currency: MavapayUnitCurrency,
    pub payment_currency: MavapayUnitCurrency,
    #[serde(default)]
    pub btc_usd_rate: Option<f64>,
    #[serde(default)]
    pub customer_internal_fee_in_usd_cent: Option<u64>,
    pub customer_internal_fee: u64,
    pub created_at: String,
    pub updated_at: String,
}

/// Nested order data wrapper from the API response
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OrderDataWrapper {
    pub status: String,
    pub data: OrderDataInner,
}

/// Inner order data containing quotes
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OrderDataInner {
    pub quotes: Vec<OrderQuote>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GetOrderResponse {
    #[serde(rename = "id")]
    pub internal_id: u64,
    pub order_id: String,
    #[serde(default)]
    pub quote_id: Option<String>,
    #[serde(default)]
    pub user_id: Option<u64>,
    pub amount: u64,
    pub status: TransactionStatus,
    pub currency: MavapayCurrency,
    pub payment_method: MavapayPaymentMethod,
    #[serde(default)]
    pub is_valid: Option<bool>,
    #[serde(default)]
    pub payment_btc_detail: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub order_data: Option<OrderDataWrapper>,
}

impl GetOrderResponse {
    /// Get all quotes from order data
    pub fn quotes(&self) -> &[OrderQuote] {
        self.order_data
            .as_ref()
            .map(|od| od.data.quotes.as_slice())
            .unwrap_or(&[])
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "UPPERCASE")]
pub enum TransactionStatus {
    Pending,
    Success,
    Expired,
    Failed,
    Paid,
}

impl std::fmt::Display for TransactionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransactionStatus::Pending => write!(f, "PENDING"),
            TransactionStatus::Success => write!(f, "SUCCESS"),
            TransactionStatus::Expired => write!(f, "EXPIRED"),
            TransactionStatus::Failed => write!(f, "FAILED"),
            TransactionStatus::Paid => write!(f, "PAID"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "UPPERCASE")]
pub enum TransactionType {
    Withdrawal,
    Deposit,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transaction {
    pub id: String,
    pub hash: Option<String>,
    pub amount: u64,
    pub currency: MavapayCurrency,
    #[serde(rename = "type")]
    pub _type: TransactionType,
    pub status: TransactionStatus,
    pub created_at: String,
    pub updated_at: String,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Default, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetTransaction<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash: Option<&'a str>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OrderTransaction {
    pub id: u64,
    pub order_id: String,
    pub user_id: u64,
    pub event_type: String,
    pub transaction_id: String,
    pub wallet_id: String,
    pub amount: u64,
    pub fees: u64,
    pub currency: MavapayCurrency,
    #[serde(rename = "transactionType")]
    pub transaction_type: TransactionType,
    pub status: TransactionStatus,
    pub payment_method: MavapayPaymentMethod,
    pub external_ref: String,
    pub ln_invoice: String,
    pub on_chain_address: String,
    pub raw_payload: serde_json::Value,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GetTransactionsResponse {
    pub count: u64,
    pub transactions: Vec<OrderTransaction>,
}

#[cfg(debug_assertions)]
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SimulatePayInRequest {
    pub order_id: String,
    pub amount: u64,
    pub currency: MavapayCurrency,
}
