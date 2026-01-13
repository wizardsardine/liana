pub mod modal;

use liana_connect::ws_business;
pub use modal::EditKeyModalState;

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

    pub fn on_key_update_type(&mut self, key_type: ws_business::KeyType) {
        if let Some(modal) = &mut self.edit_key_modal {
            modal.key_type = key_type;
        }
    }

    pub fn on_key_cancel_modal(&mut self) {
        self.edit_key_modal = None;
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

    /// Check if both alias and email are valid
    pub fn can_save(&self) -> bool {
        self.is_alias_valid() && self.is_email_valid()
    }
}
