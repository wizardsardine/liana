use liana_ui::component::form;

use crate::app::view::message::AccountType;
use crate::services::mavapay::{MavapayClient, PriceResponse, QuoteResponse, Transaction};
use crate::services::registration::RegistrationClient;

/// Represents the runtime state of the Buy/Sell panel based on geolocation detection
#[derive(Debug, Clone)]
pub enum BuySellFlowState {
    /// Detecting user's location via IP geolocation
    DetectingLocation,
    /// For Onramper countries: shows Buy/Sell selection
    Initialization,
    /// Nigeria, Kenya and South Africa, ie Mavapay supported providers
    Mavapay(MavapayFlowState),
    /// For any all countries not supported by Mavapay, but supported by Onramper
    Onramper,
}

/// State specific to Mavapay flow
#[derive(Debug, Clone)]
pub struct MavapayFlowState {
    pub native_page: NativePage,
    pub selected_account_type: Option<AccountType>,

    // Login fields
    pub login_username: form::Value<String>,
    pub login_password: form::Value<String>,

    // Authenticated user data
    pub current_user: Option<crate::services::registration::User>,
    pub auth_token: Option<String>,

    // Registration fields
    pub first_name: form::Value<String>,
    pub last_name: form::Value<String>,
    pub email: form::Value<String>,
    pub password1: form::Value<String>,
    pub password2: form::Value<String>,
    pub terms_accepted: bool,
    pub email_verification_status: Option<bool>,

    // Mavapay-specific fields
    pub mavapay_flow_mode: MavapayFlowMode,
    pub mavapay_amount: form::Value<String>,
    pub mavapay_source_currency: form::Value<String>,
    pub mavapay_target_currency: form::Value<String>,
    pub mavapay_settlement_currency: form::Value<String>,
    pub mavapay_payment_method: MavapayPaymentMethod,
    pub mavapay_bank_account_number: form::Value<String>,
    pub mavapay_bank_account_name: form::Value<String>,
    pub mavapay_bank_code: form::Value<String>,
    pub mavapay_bank_name: form::Value<String>,
    pub mavapay_current_quote: Option<QuoteResponse>,
    pub mavapay_current_price: Option<PriceResponse>,
    pub mavapay_transactions: Vec<Transaction>,

    // API clients
    pub registration_client: RegistrationClient,
    pub mavapay_client: MavapayClient,
    pub coincube_client: crate::services::coincube::CoincubeClient,
}

/// Mavapay flow mode selection
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MavapayFlowMode {
    CreateQuote,
    OneTimePayment,
}

impl MavapayFlowMode {
    pub fn all() -> &'static [MavapayFlowMode] {
        &[
            MavapayFlowMode::CreateQuote,
            MavapayFlowMode::OneTimePayment,
        ]
    }
}

impl std::fmt::Display for MavapayFlowMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MavapayFlowMode::CreateQuote => write!(f, "Create Quote"),
            MavapayFlowMode::OneTimePayment => write!(f, "One-time Payment"),
        }
    }
}

/// Payment method for one-time payment
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MavapayPaymentMethod {
    BankTransfer,
    Lightning,
}

impl MavapayPaymentMethod {
    pub fn all() -> &'static [MavapayPaymentMethod] {
        &[
            MavapayPaymentMethod::BankTransfer,
            MavapayPaymentMethod::Lightning,
        ]
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            MavapayPaymentMethod::BankTransfer => "BANKTRANSFER",
            MavapayPaymentMethod::Lightning => "LIGHTNING",
        }
    }
}

impl std::fmt::Display for MavapayPaymentMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MavapayPaymentMethod::BankTransfer => write!(f, "Bank Transfer"),
            MavapayPaymentMethod::Lightning => write!(f, "Lightning Network"),
        }
    }
}

/// Pages in the native African flow
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativePage {
    AccountSelect,
    Login,
    Register,
    VerifyEmail,
    CoincubePay,
}

impl MavapayFlowState {
    pub fn new() -> Self {
        Self {
            native_page: NativePage::AccountSelect,
            selected_account_type: None,
            login_username: form::Value::default(),
            login_password: form::Value::default(),
            current_user: None,
            auth_token: None,
            first_name: form::Value::default(),
            last_name: form::Value::default(),
            email: form::Value::default(),
            password1: form::Value::default(),
            password2: form::Value::default(),
            terms_accepted: false,
            email_verification_status: None,
            mavapay_flow_mode: MavapayFlowMode::CreateQuote,
            mavapay_amount: form::Value {
                value: "100000".to_string(), // 100,000 sats default
                warning: None,
                valid: true,
            },
            mavapay_source_currency: form::Value {
                value: "BTCSAT".to_string(),
                warning: None,
                valid: true,
            },
            mavapay_target_currency: form::Value {
                value: "NGNKOBO".to_string(),
                warning: None,
                valid: true,
            },
            mavapay_settlement_currency: form::Value {
                value: "BTC".to_string(),
                warning: None,
                valid: true,
            },
            mavapay_payment_method: MavapayPaymentMethod::Lightning,
            mavapay_bank_account_number: form::Value::default(),
            mavapay_bank_account_name: form::Value::default(),
            mavapay_bank_code: form::Value::default(),
            mavapay_bank_name: form::Value::default(),
            mavapay_current_quote: None,
            mavapay_current_price: None,
            mavapay_transactions: Vec::new(),
            registration_client: RegistrationClient::new(
                std::env::var("COINCUBE_API_URL")
                    .unwrap_or_else(|_| "https://dev-api.coincube.io".to_string())
                    + "/api/v1",
            ),
            mavapay_client: MavapayClient::new(std::env::var("MAVAPAY_API_KEY").unwrap_or_else(
                |_| {
                    tracing::warn!("MAVAPAY_API_KEY environment variable not set");
                    String::new()
                },
            )),
            coincube_client: crate::services::coincube::CoincubeClient::new(
                std::env::var("COINCUBE_API_URL")
                    .unwrap_or_else(|_| "https://dev-api.coincube.io".to_string()),
            ),
        }
    }
}

impl Default for MavapayFlowState {
    fn default() -> Self {
        Self::new()
    }
}

impl Default for BuySellFlowState {
    fn default() -> Self {
        Self::DetectingLocation
    }
}
