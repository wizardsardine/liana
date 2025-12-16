pub mod modal;

pub use modal::EditPathModalState;

/// Paths view state
#[derive(Debug, Clone, Default)]
pub struct PathsViewState {
    pub edit_path: Option<EditPathModalState>,
}

impl PathsViewState {
    pub fn on_template_update_threshold(&mut self, value: String) {
        if let Some(modal) = &mut self.edit_path {
            modal.threshold = value;
        }
    }

    pub fn on_template_update_timelock(&mut self, value: String) {
        if let Some(modal) = &mut self.edit_path {
            modal.timelock = Some(value);
        }
    }

    pub fn on_template_cancel_path_modal(&mut self) {
        self.edit_path = None;
    }
}
