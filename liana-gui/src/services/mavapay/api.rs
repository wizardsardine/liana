use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::services::http::NotSuccessResponseInfo;

/// Mavapay API error types
#[derive(Debug, Clone)]
pub enum MavapayError {
    Http(Option<u16>, String),
    InvalidResponse(String),
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
            Self::Http(code, msg) => write!(f, "HTTP error [{:?}]: {}", code, msg),
            Self::InvalidResponse(msg) => write!(f, "Invalid response: {}", msg),
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

/// Supported currencies
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Currency {
    #[serde(rename = "BTCSAT")]
    BitcoinSatoshi,
    #[serde(rename = "NGNKOBO")]
    NigerianNairaKobo,
    #[serde(rename = "ZARCENT")]
    SouthAfricanRandCent,
    #[serde(rename = "KESCENT")]
    KenyanShillingCent,
}

impl Currency {
    pub fn as_str(&self) -> &'static str {
        match self {
            Currency::BitcoinSatoshi => "BTCSAT",
            Currency::NigerianNairaKobo => "NGNKOBO",
            Currency::SouthAfricanRandCent => "ZARCENT",
            Currency::KenyanShillingCent => "KESCENT",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Currency::BitcoinSatoshi => "Bitcoin (Satoshi)",
            Currency::NigerianNairaKobo => "Nigerian Naira (Kobo)",
            Currency::SouthAfricanRandCent => "South African Rand (Cent)",
            Currency::KenyanShillingCent => "Kenyan Shilling (Cent)",
        }
    }

    pub fn symbol(&self) -> &'static str {
        match self {
            Currency::BitcoinSatoshi => "sats",
            Currency::NigerianNairaKobo => "NGN",
            Currency::SouthAfricanRandCent => "ZAR",
            Currency::KenyanShillingCent => "KES",
        }
    }
}

/// Payment methods
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PaymentMethod {
    #[serde(rename = "LIGHTNING")]
    Lightning,
    #[serde(rename = "BANKTRANSFER")]
    BankTransfer,
    #[serde(rename = "NGNBANKTRANSFER")]
    NgnBankTransfer,
}

impl PaymentMethod {
    pub fn as_str(&self) -> &'static str {
        match self {
            PaymentMethod::Lightning => "LIGHTNING",
            PaymentMethod::BankTransfer => "BANKTRANSFER",
            PaymentMethod::NgnBankTransfer => "NGNBANKTRANSFER",
        }
    }
}

/// Bank account details for fiat payouts
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BankAccount {
    #[serde(rename = "bankAccountNumber")]
    pub account_number: String,
    #[serde(rename = "bankAccountName")]
    pub account_name: String,
    #[serde(rename = "bankCode")]
    pub bank_code: String,
    #[serde(rename = "bankName")]
    pub bank_name: String,
}

/// M-Pesa payment details for Kenyan Shilling
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MpesaPayment {
    #[serde(rename = "identifierType")]
    pub identifier_type: String, // "paytophone", "paytotill", "paytobill"
    pub identifiers: HashMap<String, String>,
}

/// Beneficiary for payments
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Beneficiary {
    Bank(BankAccount),
    Mpesa(MpesaPayment),
}

/// Quote request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuoteRequest {
    pub amount: String,
    #[serde(rename = "sourceCurrency")]
    pub source_currency: Currency,
    #[serde(rename = "targetCurrency")]
    pub target_currency: Currency,
    #[serde(rename = "paymentMethod")]
    pub payment_method: PaymentMethod,
    #[serde(rename = "paymentCurrency")]
    pub payment_currency: Currency,
    pub autopayout: bool,
    #[serde(rename = "customerInternalFee")]
    pub customer_internal_fee: String,
    pub beneficiary: Beneficiary,
}

/// Payment link request (hosted checkout)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentLinkRequest {
    pub amount: String,
    #[serde(rename = "sourceCurrency")]
    pub source_currency: Currency,
    #[serde(rename = "targetCurrency")]
    pub target_currency: Currency,
    #[serde(rename = "paymentMethod")]
    pub payment_method: PaymentMethod,
    #[serde(rename = "paymentCurrency")]
    pub payment_currency: Currency,
    pub beneficiary: Beneficiary,
}

/// Payment link response
#[derive(Debug, Clone, Deserialize)]
pub struct PaymentLinkResponse {
    #[serde(rename = "url")]
    pub url: String,
}

/// Quote response
#[derive(Debug, Clone, Deserialize)]
pub struct QuoteResponse {
    pub id: String,
    #[serde(rename = "exchangeRate")]
    pub exchange_rate: f64,
    #[serde(rename = "usdToTargetCurrencyRate")]
    pub usd_to_target_currency_rate: f64,
    #[serde(rename = "sourceCurrency")]
    pub source_currency: Currency,
    #[serde(rename = "targetCurrency")]
    pub target_currency: Currency,
    #[serde(rename = "transactionFeesInSourceCurrency")]
    pub transaction_fees_in_source_currency: u64,
    #[serde(rename = "transactionFeesInTargetCurrency")]
    pub transaction_fees_in_target_currency: u64,
    #[serde(rename = "amountInSourceCurrency")]
    pub amount_in_source_currency: u64,
    #[serde(rename = "amountInTargetCurrency")]
    pub amount_in_target_currency: u64,
    #[serde(rename = "paymentMethod")]
    pub payment_method: PaymentMethod,
    pub expiry: String,
    #[serde(rename = "isValid")]
    pub is_valid: bool,
    pub invoice: Option<String>, // Lightning invoice for BTC payments
    pub hash: Option<String>,
    #[serde(rename = "totalAmountInSourceCurrency")]
    pub total_amount_in_source_currency: u64,
    #[serde(rename = "customerInternalFee")]
    pub customer_internal_fee: String,
    // NGN specific fields
    #[serde(rename = "bankName")]
    pub bank_name: Option<String>,
    #[serde(rename = "ngnBankAccountNumber")]
    pub ngn_bank_account_number: Option<String>,
    #[serde(rename = "ngnAccountName")]
    pub ngn_account_name: Option<String>,
}

/// API response wrapper
#[derive(Debug, Clone, Deserialize)]
pub struct ApiResponse<T> {
    pub status: String,
    pub data: T,
}

/// Price response
#[derive(Debug, Clone, Deserialize)]
pub struct PriceResponse {
    pub price: f64,
    pub currency: String,
    pub timestamp: Option<u64>,
}

/// Bank information
#[derive(Debug, Clone, Deserialize)]
pub struct BankInfo {
    pub code: String,
    pub name: String,
    pub country: String,
}

/// Transaction status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum TransactionStatus {
    #[serde(rename = "PENDING")]
    Pending,
    #[serde(rename = "SUCCESS")]
    Success,
    #[serde(rename = "FAILED")]
    Failed,
    #[serde(rename = "PAID")]
    Paid,
}

/// Payment confirmation request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentConfirmationRequest {
    #[serde(rename = "quoteId")]
    pub quote_id: String,
}

/// Payment status response
#[derive(Debug, Clone, Deserialize)]
pub struct PaymentStatusResponse {
    pub id: String,
    #[serde(rename = "quoteId")]
    pub quote_id: String,
    pub amount: u64,
    pub currency: String,
    #[serde(rename = "paymentMethod")]
    pub payment_method: String,
    pub status: String,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
    pub transactions: Vec<Transaction>,
}

/// Webhook event types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WebhookEventType {
    #[serde(rename = "payment.received")]
    PaymentReceived,
    #[serde(rename = "payment.sent")]
    PaymentSent,
}

/// Webhook payload
#[derive(Debug, Clone, Deserialize)]
pub struct WebhookPayload {
    pub event: WebhookEventType,
    pub data: serde_json::Value,
    pub timestamp: String,
}

/// Transaction information
#[derive(Debug, Clone, Deserialize)]
pub struct Transaction {
    pub id: String,
    pub amount: u64,
    pub currency: String,
    pub status: TransactionStatus,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
    pub hash: Option<String>,
}

/// Wallet balance
#[derive(Debug, Clone, Deserialize)]
pub struct WalletBalance {
    pub currency: String,
    pub balance: u64,
    #[serde(rename = "availableBalance")]
    pub available_balance: u64,
}
