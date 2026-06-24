pub mod modal;

use liana_connect::ws_business::{self, Key, KeyIdentity};
pub use modal::{EditKeyModalState, SignerOption};
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
        let value = value.trim().to_string();
        if let Some(modal) = &mut self.edit_key_modal {
            modal.set_email(value);
        }
    }

    pub fn on_key_update_token(&mut self, value: String, keys: &BTreeMap<u8, Key>) {
        let value = value.trim().to_string();
        if let Some(modal) = &mut self.edit_key_modal {
            modal.token = value.clone();
            modal.provider = None;

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
                id != editing_key_id
                    && matches!(&k.identity, KeyIdentity::TokenWithProvider{ token: t, .. } if t == &value)
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

    pub fn refresh_signer_options(&mut self, signer_options: Vec<SignerOption>) {
        if let Some(modal) = &mut self.edit_key_modal {
            modal.refresh_signer_options(signer_options);
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use liana_connect::{keys::api::Provider, ws_business::KeyType};

    fn modal_state() -> EditKeyModalState {
        EditKeyModalState::new(
            1,
            "Treasury".into(),
            String::new(),
            KeyType::Internal,
            true,
            "alice@example.com".into(),
            String::new(),
            None,
            vec![SignerOption {
                name: "Alice".into(),
                email: "alice@example.com".into(),
                already_used: false,
            }],
        )
    }

    #[test]
    fn can_save_requires_valid_email_for_signer_keys() {
        let mut state = KeysViewState {
            edit_key_modal: Some(modal_state()),
        };
        assert!(state.can_save());

        state.on_key_update_email("invalid".into());
        assert!(!state.can_save());

        state.on_key_update_alias(String::new());
        state.on_key_update_email("alice@example.com".into());
        assert!(!state.can_save());
    }

    #[test]
    fn refresh_signer_options_keeps_typed_email() {
        let mut state = KeysViewState {
            edit_key_modal: Some(EditKeyModalState::new(
                1,
                "Treasury".into(),
                String::new(),
                KeyType::Internal,
                true,
                "new-signer@example.com".into(),
                String::new(),
                None,
                Vec::new(),
            )),
        };

        state.refresh_signer_options(vec![SignerOption {
            name: "Alice".into(),
            email: "alice@example.com".into(),
            already_used: false,
        }]);

        let modal = state.edit_key_modal.as_ref().expect("modal");
        assert_eq!(modal.email, "new-signer@example.com");
        assert_eq!(
            modal.fallback_signer().as_deref(),
            Some("new-signer@example.com")
        );
    }

    #[test]
    fn token_update_clears_cached_provider() {
        let mut state = KeysViewState {
            edit_key_modal: Some(EditKeyModalState::new(
                1,
                "Cosigner".into(),
                String::new(),
                KeyType::Cosigner,
                false,
                String::new(),
                "42-absent-cake-eagle".into(),
                Some(Provider {
                    uuid: "provider".into(),
                    name: "Provider".into(),
                }),
                Vec::new(),
            )),
        };

        state.on_key_update_token("43-absent-cake-eagle".into(), &BTreeMap::new());

        assert!(state
            .edit_key_modal
            .as_ref()
            .expect("modal")
            .provider
            .is_none());
    }

    #[test]
    fn clone_keeps_modal_runtime_state() {
        let mut modal = modal_state();
        modal.token_warning = Some("Duplicate token");

        let state = KeysViewState {
            edit_key_modal: Some(modal),
        };
        let cloned = state.clone();

        let modal = cloned.edit_key_modal.as_ref().expect("modal");
        assert_eq!(modal.token_warning, Some("Duplicate token"));
    }
}
