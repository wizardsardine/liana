pub mod modal;

pub use modal::EditKeyModalState;

/// Keys view state
#[derive(Debug, Clone, Default)]
pub struct KeysViewState {
    pub edit_key: Option<EditKeyModalState>,
}

impl KeysViewState {
    pub fn on_key_update_alias(&mut self, value: String) {
        if let Some(modal) = &mut self.edit_key {
            modal.alias = value;
        }
    }

    pub fn on_key_update_descr(&mut self, value: String) {
        if let Some(modal) = &mut self.edit_key {
            modal.description = value;
        }
    }

    pub fn on_key_update_email(&mut self, value: String) {
        if let Some(modal) = &mut self.edit_key {
            modal.email = value;
        }
    }

    pub fn on_key_update_type(&mut self, key_type: crate::models::KeyType) {
        if let Some(modal) = &mut self.edit_key {
            modal.key_type = key_type;
        }
    }

    pub fn on_key_cancel_modal(&mut self) {
        self.edit_key = None;
    }
}

