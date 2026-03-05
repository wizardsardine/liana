pub mod modal;

use liana_connect::ws_business::{self, Key, KeyIdentity};
pub use modal::EditKeyModalState;
use std::{collections::BTreeMap, str::FromStr};

/// Keys view state
#[derive(Debug, Clone, Default)]
pub struct KeysViewState {
    pub edit_key_modal: Option<EditKeyModalState>,
}

impl KeysViewState {
    pub fn on_key_update_alias(&mut self, value: String) {
        if let Some(modal) = &mut self.edit_key_modal {
            modal.alias = value;
        }
    }

    pub fn on_key_update_descr(&mut self, value: String) {
        if let Some(modal) = &mut self.edit_key_modal {
            modal.description = value;
        }
    }

    pub fn on_key_update_email(&mut self, value: String) {
        if let Some(modal) = &mut self.edit_key_modal {
            modal.email = value;
        }
    }

    pub fn on_key_update_token(&mut self, value: String, keys: &BTreeMap<u8, Key>) {
        if let Some(modal) = &mut self.edit_key_modal {
            modal.token = value.clone();

            if value.trim().is_empty() {
                modal.token_warning = None;
                return;
            }

            // Local format validation
            if liana_connect::keys::token::Token::from_str(&value).is_err() {
                modal.token_warning = Some("Invalid token!");
                return;
            }

            // Check for duplicate tokens in existing keys
            let editing_key_id = modal.key_id;
            let is_duplicate = keys.iter().any(|(&id, k)| {
                id != editing_key_id && matches!(&k.identity, KeyIdentity::Token(t) if t == &value)
            });
            if is_duplicate {
                modal.token_warning = Some("Duplicate token");
                return;
            }

            modal.token_warning = None;
        }
    }

    pub fn on_key_update_type(&mut self, key_type: ws_business::KeyType) {
        if let Some(modal) = &mut self.edit_key_modal {
            modal.key_type = key_type;
            modal.token_warning = None;
        }
    }

    pub fn on_key_cancel_modal(&mut self) {
        self.edit_key_modal = None;
    }

    /// Whether the current key type uses token identity (Cosigner/SafetyNet)
    pub fn uses_token_identity(&self) -> bool {
        self.edit_key_modal
            .as_ref()
            .map(|modal| {
                matches!(
                    modal.key_type,
                    ws_business::KeyType::Cosigner | ws_business::KeyType::SafetyNet
                )
            })
            .unwrap_or(false)
    }

    pub fn is_alias_valid(&self) -> bool {
        if let Some(modal) = &self.edit_key_modal {
            !modal.alias.trim().is_empty()
        } else {
            false
        }
    }

    pub fn is_email_valid(&self) -> bool {
        if let Some(modal) = &self.edit_key_modal {
            email_address::EmailAddress::parse_with_options(
                &modal.email,
                email_address::Options::default().with_required_tld(),
            )
            .is_ok()
        } else {
            false
        }
    }

    pub fn is_token_format_valid(&self) -> bool {
        if let Some(modal) = &self.edit_key_modal {
            use std::str::FromStr;
            liana_connect::keys::token::Token::from_str(&modal.token).is_ok()
        } else {
            false
        }
    }

    /// Check if the key can be saved based on current key type
    pub fn can_save(&self) -> bool {
        if !self.is_alias_valid() {
            return false;
        }
        if self.uses_token_identity() {
            self.is_token_format_valid()
                && self
                    .edit_key_modal
                    .as_ref()
                    .is_some_and(|m| m.token_warning.is_none())
        } else {
            self.is_email_valid()
        }
    }
}
