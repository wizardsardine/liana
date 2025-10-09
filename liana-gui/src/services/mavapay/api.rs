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

    /// Get settlement currency format (NGN, BTC, ZAR, KES) for payment links
    pub fn to_settlement_currency(&self) -> &'static str {
        match self {
            Currency::BitcoinSatoshi => "BTC",
            Currency::NigerianNairaKobo => "NGN",
            Currency::SouthAfricanRandCent => "ZAR",
            Currency::KenyanShillingCent => "KES",
        }
    }

    /// Get all available currencies
    pub fn all() -> &'static [Currency] {
        &[
            Currency::BitcoinSatoshi,
            Currency::NigerianNairaKobo,
            Currency::SouthAfricanRandCent,
            Currency::KenyanShillingCent,
        ]
    }

    /// Parse currency from string
    pub fn from_str(s: &str) -> Option<Currency> {
        match s {
            "BTCSAT" => Some(Currency::BitcoinSatoshi),
            "NGNKOBO" => Some(Currency::NigerianNairaKobo),
            "ZARCENT" => Some(Currency::SouthAfricanRandCent),
            "KESCENT" => Some(Currency::KenyanShillingCent),
            _ => None,
        }
    }
}

impl std::fmt::Display for Currency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display_name())
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
    #[serde(rename = "customerInternalFee", skip_serializing_if = "Option::is_none")]
    pub customer_internal_fee: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub beneficiary: Option<Beneficiary>,
}

/// Payment link type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PaymentLinkType {
    #[serde(rename = "One_Time")]
    OneTime,
    #[serde(rename = "Recurring")]
    Recurring,
}

/// Payment link request (hosted checkout)
/// Based on actual API spec from Postman collection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentLinkRequest {
    pub name: String,
    pub description: String,
    #[serde(rename = "type")]
    pub link_type: PaymentLinkType,
    #[serde(rename = "addFeeToTotalCost")]
    pub add_fee_to_total_cost: bool,
    #[serde(rename = "settlementCurrency")]
    pub settlement_currency: String, // e.g., "BTC", "NGN"
    #[serde(rename = "paymentMethods")]
    pub payment_methods: Vec<String>, // e.g., ["LIGHTNING", "BANKTRANSFER"]
    pub amount: u64,
    #[serde(rename = "callbackUrl", skip_serializing_if = "Option::is_none")]
    pub callback_url: Option<String>,
}

/// Payment link response
#[derive(Debug, Clone, Deserialize)]
pub struct PaymentLinkResponse {
    #[serde(rename = "paymentLink")]
    pub payment_link: String,
    #[serde(rename = "paymentRef")]
    pub payment_ref: String,
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
    pub source_currency: String, // Changed to String to handle any currency code
    #[serde(rename = "targetCurrency")]
    pub target_currency: String, // Changed to String to handle any currency code
    #[serde(rename = "transactionFeesInSourceCurrency")]
    pub transaction_fees_in_source_currency: u64, // API returns integer
    #[serde(rename = "transactionFeesInTargetCurrency")]
    pub transaction_fees_in_target_currency: u64, // API returns integer
    #[serde(rename = "amountInSourceCurrency")]
    pub amount_in_source_currency: u64,
    #[serde(rename = "amountInTargetCurrency")]
    pub amount_in_target_currency: u64, // API returns integer
    #[serde(rename = "paymentMethod")]
    pub payment_method: String, // Changed to String to handle any payment method
    pub expiry: String,
    #[serde(rename = "isValid")]
    pub is_valid: bool,
    #[serde(default)]
    pub invoice: String, // Lightning invoice for BTC payments - empty string if not present
    #[serde(default)]
    pub hash: String, // Payment hash
    #[serde(rename = "totalAmountInSourceCurrency")]
    pub total_amount_in_source_currency: u64,
    #[serde(rename = "totalAmountInTargetCurrency", default)]
    pub total_amount_in_target_currency: Option<u64>,
    #[serde(rename = "paymentCurrency", default)]
    pub payment_currency: Option<String>,
    #[serde(rename = "customerInternalFee")]
    pub customer_internal_fee: u64, // API returns this as number
    #[serde(rename = "estimatedRoutingFee", default)]
    pub estimated_routing_fee: u64,
    #[serde(rename = "orderId", default)]
    pub order_id: String,
    // NGN specific fields for buy-BTC flow (when API returns bank details to pay Mavapay)
    #[serde(rename = "bankName", default)]
    pub bank_name: String,
    #[serde(rename = "ngnBankAccountNumber", default)]
    pub ngn_bank_account_number: String,
    #[serde(rename = "ngnAccountName", default)]
    pub ngn_account_name: String,
    #[serde(rename = "ngnBankCode", default)]
    pub ngn_bank_code: String, // Also present in the response
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
