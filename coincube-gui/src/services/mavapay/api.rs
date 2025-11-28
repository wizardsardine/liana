use crate::services::http::NotSuccessResponseInfo;
use serde::{Deserialize, Serialize};

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
            Self::Http(Some(code), msg) => write!(f, "HTTP error [{}]: {}", code, msg),
            Self::Http(None, msg) => write!(f, "HTTP error: {}", msg),
            Self::InvalidResponse(msg) => write!(f, "Invalid response: {}", msg),
            Self::ApiError(msg) => write!(f, "Api Error: {}", msg),
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
        Self::Http(None, error.to_string())
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

impl std::fmt::Display for MavapayUnitCurrency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MavapayUnitCurrency::KenyanShillingCent => write!(f, "Kenyan Cent"),
            MavapayUnitCurrency::SouthAfricanRandCent => write!(f, "South African Cent"),
            MavapayUnitCurrency::NigerianNairaKobo => write!(f, "Nigerian Kobo"),
            MavapayUnitCurrency::BitcoinSatoshi => write!(f, "Bitcoin Satoshi"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum MavapayPaymentMethod {
    Lightning,
    BankTransfer,
    Onchain,
}

impl std::fmt::Display for MavapayPaymentMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MavapayPaymentMethod::Lightning => write!(f, "Bitcoin Lightning"),
            MavapayPaymentMethod::Onchain => write!(f, "Bitcoin Mainnet Transaction"),
            MavapayPaymentMethod::BankTransfer => write!(f, "Bank Transfer"),
        }
    }
}

impl MavapayPaymentMethod {
    pub fn all() -> &'static [MavapayPaymentMethod] {
        &[
            MavapayPaymentMethod::Lightning,
            MavapayPaymentMethod::BankTransfer,
            MavapayPaymentMethod::Onchain,
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetQuoteRequest {
    pub amount: u64,
    pub source_currency: MavapayUnitCurrency,
    pub target_currency: MavapayUnitCurrency,
    pub payment_method: MavapayPaymentMethod,
    pub payment_currency: MavapayUnitCurrency,
    pub autopayout: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub customer_internal_fee: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub beneficiary: Option<Beneficiary>,
}

// TODO: This structure is always changing, with some members being deprecated or undocumented
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetQuoteResponse {
    pub id: String,
    pub exchange_rate: f64,
    pub usd_to_target_currency_rate: f64,
    pub source_currency: MavapayUnitCurrency,
    pub target_currency: MavapayUnitCurrency,
    pub transaction_fees_in_source_currency: u64,
    pub transaction_fees_in_target_currency: u64,
    pub amount_in_source_currency: u64,
    pub amount_in_target_currency: u64,
    pub payment_method: MavapayPaymentMethod,
    pub expiry: String, // TODO: use typed dates
    pub is_valid: bool,
    pub invoice: String,
    pub hash: String,
    pub total_amount_in_source_currency: u64,
    pub total_amount_in_target_currency: Option<u64>,
    pub customer_internal_fee: u64,
    pub created_at: String,
    pub updated_at: String,

    pub estimated_routing_fee: Option<u64>,
    pub order_id: Option<String>,
    pub bank_name: Option<String>,
    pub ngn_bank_account_number: Option<String>,
    pub ngn_account_name: Option<String>,
    pub ngn_bank_code: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MavapayWallet {
    pub id: String,
    pub account_id: String,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "UPPERCASE")]
pub enum TransactionStatus {
    Pending,
    Success,
    Failed,
    Paid,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "UPPERCASE")]
pub enum TransactionType {
    Withdrawal,
    Deposit,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetTransactionFilters {
    pub tx_id: Option<String>,
    pub account_name: Option<String>,
    pub status: Option<TransactionStatus>,
    #[serde(rename = "type")]
    pub _type: Option<TransactionType>,
    pub min_amount: Option<u64>,
    pub max_amount: Option<u64>,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetTransactions {
    pub page: Option<u64>,
    pub limit: Option<u64>,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub filters: Option<GetTransactionFilters>,
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

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetTransactionPagination {
    pub count: u64,
    pub next_page: bool,
    pub current_page: u64,
    pub total_pages: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(tag = "status")]
pub enum GetTransactionResponse {
    Error {
        message: String,
    },
    Success {
        #[serde(flatten)]
        pagination: GetTransactionPagination,
        data: Vec<Transaction>,
    },
}
