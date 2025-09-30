use liana_ui::component::form;

use crate::app::buysell::meld::MeldClient;
use crate::app::view::message::AccountType;
use crate::services::mavapay::{
    MavapayClient, PaymentStatusResponse, PriceResponse, QuoteResponse, Transaction,
};
use crate::services::registration::RegistrationClient;

/// Represents the runtime state of the Buy/Sell panel based on geolocation detection
#[derive(Debug, Clone)]
pub enum BuySellFlowState {
    /// Before geolocation detection completes
    DetectingRegion,

    /// African users: Mavapay native login/registration flow
    Africa(AfricaFlowState),

    /// International users: Provider selection + embedded webview
    International(InternationalFlowState),

    /// Geolocation detection failed - show provider selection as fallback
    DetectionFailed,
}

/// State specific to African (Mavapay) flow
#[derive(Debug, Clone)]
pub struct AfricaFlowState {
    pub native_page: NativePage,
    pub selected_account_type: Option<AccountType>,

    // Login fields
    pub login_username: form::Value<String>,
    pub login_password: form::Value<String>,

    // Registration fields
    pub first_name: form::Value<String>,
    pub last_name: form::Value<String>,
    pub email: form::Value<String>,
    pub password1: form::Value<String>,
    pub password2: form::Value<String>,
    pub terms_accepted: bool,
    pub email_verification_status: Option<bool>,

    // Mavapay-specific fields
    pub mavapay_amount: form::Value<String>,
    pub mavapay_source_currency: form::Value<String>,
    pub mavapay_target_currency: form::Value<String>,
    pub mavapay_bank_account_number: form::Value<String>,
    pub mavapay_bank_account_name: form::Value<String>,
    pub mavapay_bank_code: form::Value<String>,
    pub mavapay_bank_name: form::Value<String>,
    pub mavapay_current_quote: Option<QuoteResponse>,
    pub mavapay_current_price: Option<PriceResponse>,
    pub mavapay_transactions: Vec<Transaction>,
    pub mavapay_payment_status: Option<PaymentStatusResponse>,
    pub mavapay_polling_active: bool,

    // API clients
    pub registration_client: RegistrationClient,
    pub mavapay_client: MavapayClient,
}

/// State specific to International (Meld/Onramper) flow
#[derive(Debug, Clone)]
pub struct InternationalFlowState {
    pub meld_client: MeldClient,
    pub selected_provider: Option<InternationalProvider>,
}

/// International payment providers
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InternationalProvider {
    Meld,
    Onramper,
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

impl AfricaFlowState {
    pub fn new() -> Self {
        Self {
            native_page: NativePage::AccountSelect,
            selected_account_type: None,
            login_username: form::Value::default(),
            login_password: form::Value::default(),
            first_name: form::Value::default(),
            last_name: form::Value::default(),
            email: form::Value::default(),
            password1: form::Value::default(),
            password2: form::Value::default(),
            terms_accepted: false,
            email_verification_status: None,
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
            mavapay_bank_account_number: form::Value::default(),
            mavapay_bank_account_name: form::Value::default(),
            mavapay_bank_code: form::Value::default(),
            mavapay_bank_name: form::Value::default(),
            mavapay_current_quote: None,
            mavapay_current_price: None,
            mavapay_transactions: Vec::new(),
            mavapay_payment_status: None,
            mavapay_polling_active: false,
            registration_client: RegistrationClient::new(
                "https://dev-api.coincube.io/api/v1".to_string(),
            ),
            mavapay_client: MavapayClient::new(std::env::var("MAVAPAY_API_KEY").unwrap_or_else(
                |_| {
                    tracing::warn!("MAVAPAY_API_KEY environment variable not set");
                    String::new()
                },
            )),
        }
    }
}

impl Default for AfricaFlowState {
    fn default() -> Self {
        Self::new()
    }
}

impl InternationalFlowState {
    pub fn new() -> Self {
        Self {
            meld_client: MeldClient::new(),
            selected_provider: None,
        }
    }
}

impl Default for InternationalFlowState {
    fn default() -> Self {
        Self::new()
    }
}

impl Default for BuySellFlowState {
    fn default() -> Self {
        Self::DetectingRegion
    }
}
