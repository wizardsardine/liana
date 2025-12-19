pub mod code;
pub mod email;

pub use code::CodeState;
pub use email::EmailState;
pub use liana_gui::services::connect::client::auth::AccessTokenResponse;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginState {
    AccountSelect,
    EmailEntry,
    CodeEntry,
    Authenticated,
}

/// A cached account with email and tokens
#[derive(Debug, Clone)]
pub struct CachedAccount {
    pub email: String,
    pub tokens: AccessTokenResponse,
}

/// State for the account selection view
#[derive(Debug, Clone, Default)]
pub struct AccountSelectState {
    pub accounts: Vec<CachedAccount>,
    pub processing: bool,
    pub selected_email: Option<String>,
}

/// Login view state
#[derive(Debug, Clone)]
pub struct Login {
    pub current: LoginState,
    pub email: EmailState,
    pub code: CodeState,
    pub account_select: AccountSelectState,
}

impl Login {
    pub fn new() -> Self {
        Self {
            current: LoginState::EmailEntry,
            email: EmailState::new(),
            code: CodeState::new(),
            account_select: AccountSelectState::default(),
        }
    }

    /// Create a new Login state with cached accounts for account selection
    pub fn with_cached_accounts(accounts: Vec<CachedAccount>) -> Self {
        Self {
            current: if accounts.is_empty() {
                LoginState::EmailEntry
            } else {
                LoginState::AccountSelect
            },
            email: EmailState::new(),
            code: CodeState::new(),
            account_select: AccountSelectState {
                accounts,
                processing: false,
                selected_email: None,
            },
        }
    }

    pub fn on_update_email(&mut self, email: String) {
        self.email.form.valid = email_address::EmailAddress::parse_with_options(
            &email,
            email_address::Options::default().with_required_tld(),
        )
        .is_ok();
        self.email.form.warning = (!self.email.form.valid).then_some("Invalid email!");
        self.email.form.value = email;
    }
    pub fn on_update_code(&mut self, code: String) {
        let all_numerical = code.chars().all(|c| c.is_ascii_digit());
        let code_len = code.len();

        let is_invalid = code_len > 6 || !all_numerical;

        let warning = is_invalid.then_some("Code must contains only 6 numbers");

        self.code.form = liana_ui::component::form::Value {
            value: code,
            warning,
            valid: !is_invalid,
        };
    }
}

impl Default for Login {
    fn default() -> Self {
        Self::new()
    }
}
