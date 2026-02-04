pub mod modal;

pub use modal::{EditPathModalState, TimelockUnit};

/// Paths view state
#[derive(Debug, Clone, Default)]
pub struct PathsViewState {
    pub edit_path_modal: Option<EditPathModalState>,
}

impl PathsViewState {
    pub fn on_template_update_threshold(&mut self, value: String) {
        if let Some(modal) = &mut self.edit_path_modal {
            // Only allow numeric characters
            let filtered: String = value.chars().filter(|c| c.is_ascii_digit()).collect();
            modal.threshold = filtered;
        }
    }

    pub fn on_template_update_timelock(&mut self, value: String) {
        if let Some(modal) = &mut self.edit_path_modal {
            // Only allow numeric characters
            let filtered: String = value.chars().filter(|c| c.is_ascii_digit()).collect();
            modal.timelock_value = Some(filtered);
        }
    }

    pub fn on_template_update_timelock_unit(&mut self, unit: TimelockUnit) {
        if let Some(modal) = &mut self.edit_path_modal {
            modal.timelock_unit = unit;
        }
    }

    pub fn on_template_toggle_key_in_path(&mut self, key_id: u8) {
        if let Some(modal) = &mut self.edit_path_modal {
            let old_count = modal.selected_key_ids.len();
            let was_enabled = old_count > 1;
            let was_at_max = modal.threshold.parse::<usize>().ok() == Some(old_count);

            let is_adding =
                if let Some(pos) = modal.selected_key_ids.iter().position(|&id| id == key_id) {
                    // Key is in path - remove it
                    modal.selected_key_ids.remove(pos);
                    false
                } else {
                    // Key is not in path - add it
                    modal.selected_key_ids.push(key_id);
                    true
                };

            let new_count = modal.selected_key_ids.len();
            let is_enabled = new_count > 1;

            // Update threshold based on enabled state
            if !is_enabled {
                // Disabled: clear threshold value
                modal.threshold = String::new();
            } else if !was_enabled && is_enabled {
                // Just became enabled: set to max (all keys required)
                modal.threshold = new_count.to_string();
            } else if is_adding && was_at_max {
                // Adding a key while threshold was at max: keep it at max
                modal.threshold = new_count.to_string();
            }
        }
    }

    pub fn on_template_cancel_path_modal(&mut self) {
        self.edit_path_modal = None;
    }
}
