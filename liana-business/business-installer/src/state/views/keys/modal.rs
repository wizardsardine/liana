use liana_connect::{keys::api::Provider, ws_business};
use std::fmt::{self, Display};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignerOption {
    pub name: String,
    pub email: String,
    pub already_used: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignerComboboxOption {
    Member(SignerOption),
    FreeEmail(String),
}

impl SignerComboboxOption {
    pub fn email(&self) -> &str {
        match self {
            Self::Member(option) => &option.email,
            Self::FreeEmail(email) => email,
        }
    }
}

impl Display for SignerComboboxOption {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Member(option) => write!(f, "{} <{}>", option.name, option.email),
            Self::FreeEmail(email) => f.write_str(email),
        }
    }
}

/// State for Edit Key modal
#[derive(Debug, Clone)]
pub struct EditKeyModalState {
    pub key_id: u8,
    pub alias: String,
    pub description: String,
    pub key_type: ws_business::KeyType,
    pub is_new: bool,
    // Identity fields - only one is active based on key_type
    pub email: String,
    // The email the modal opened with; while the input still equals it the list is unfiltered.
    original_email: String,
    pub token: String,
    pub provider: Option<Provider>,
    // Token validation state
    pub token_warning: Option<&'static str>,
    pub signer_options: Vec<SignerOption>,
}

impl EditKeyModalState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        key_id: u8,
        alias: String,
        description: String,
        key_type: ws_business::KeyType,
        is_new: bool,
        email: String,
        token: String,
        provider: Option<Provider>,
        signer_options: Vec<SignerOption>,
    ) -> Self {
        Self {
            key_id,
            alias,
            description,
            key_type,
            is_new,
            original_email: email.clone(),
            email,
            token,
            provider,
            token_warning: None,
            signer_options,
        }
    }

    pub fn set_email(&mut self, email: String) {
        self.email = email;
    }

    pub fn refresh_signer_options(&mut self, signer_options: Vec<SignerOption>) {
        self.signer_options = signer_options;
    }

    pub fn filtered_signer_options(&self) -> Vec<&SignerOption> {
        let query = self.email.trim().to_lowercase();

        // Until the input is changed away from the email the modal opened with, show everything.
        if query.is_empty() || query == self.original_email.trim().to_lowercase() {
            return self.signer_options.iter().collect();
        }

        self.signer_options
            .iter()
            .filter(|option| {
                option.name.to_lowercase().contains(&query)
                    || option.email.to_lowercase().contains(&query)
            })
            .collect()
    }

    pub fn fallback_signer(&self) -> Option<String> {
        let email = self.email.trim();
        if email.is_empty() || !is_valid_email(email) {
            return None;
        }

        (!self
            .signer_options
            .iter()
            .any(|option| option.email.eq_ignore_ascii_case(email)))
        .then_some(email.to_string())
    }
}

fn is_valid_email(email: &str) -> bool {
    email_address::EmailAddress::parse_with_options(
        email,
        email_address::Options::default().with_required_tld(),
    )
    .is_ok()
}
