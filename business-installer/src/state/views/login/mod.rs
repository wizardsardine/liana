pub mod code;
pub mod email;

pub use code::CodeState;
pub use email::EmailState;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginState {
    EmailEntry,
    CodeEntry,
    Authenticated,
}

/// Login view state
#[derive(Debug, Clone)]
pub struct Login {
    pub current: LoginState,
    pub email: EmailState,
    pub code: CodeState,
}

impl Login {
    pub fn new() -> Self {
        Self {
            current: LoginState::EmailEntry,
            email: EmailState::new(),
            code: CodeState::new(),
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
