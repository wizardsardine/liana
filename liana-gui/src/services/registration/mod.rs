pub mod client;
pub use client::*;

use liana_ui::component::form;

pub struct RegistrationState {
    pub client: RegistrationClient,

    pub login_username: form::Value<String>,
    pub login_password: form::Value<String>,

    // Default build: account type selection state
    pub selected_account_type: Option<crate::app::view::AccountType>,

    // Native flow current page
    pub native_page: crate::app::view::buysell::NativePage,

    // Registration fields (native flow)
    pub first_name: form::Value<String>,
    pub last_name: form::Value<String>,
    pub email: form::Value<String>,
    pub password1: form::Value<String>,
    pub password2: form::Value<String>,
    pub terms_accepted: bool,
    pub email_verification_status: Option<bool>, // None = checking, Some(true) = verified, Some(false) = pending
}

impl Default for RegistrationState {
    fn default() -> Self {
        Self {
            client: RegistrationClient::new("https://dev-api.coincube.io/api/v1".to_string()),
            selected_account_type: None,
            native_page: crate::app::view::buysell::NativePage::AccountSelect,

            login_username: form::Value {
                value: String::new(),
                warning: None,
                valid: false,
            },
            login_password: form::Value {
                value: String::new(),
                warning: None,
                valid: false,
            },

            // Native registration defaults
            first_name: form::Value::default(),
            last_name: form::Value::default(),
            email: form::Value::default(),
            password1: form::Value::default(),
            password2: form::Value::default(),
            terms_accepted: false,
            email_verification_status: None,
        }
    }
}
