use crate::state::{
    views::path::{EditPathModalState, TimelockUnit},
    Msg, State,
};
use iced::{
    widget::{checkbox, pick_list, Space},
    Alignment, Length,
};
use liana_ui::{
    component::{button, card, form, text},
    icon, theme,
    widget::*,
};

pub fn render_modal(state: &State) -> Option<Element<'_, Msg>> {
    if let Some(modal_state) = &state.views.paths.edit_path {
        return Some(edit_path_modal(state, modal_state));
    }
    None
}

pub fn edit_path_modal<'a>(
    state: &'a State,
    modal_state: &'a EditPathModalState,
) -> Element<'a, Msg> {
    let mut content = Column::new()
        .spacing(15)
        .padding(20.0)
        .width(Length::Fixed(500.0));

    // Header
    let title = if modal_state.is_primary {
        "Edit Primary Path"
    } else if modal_state.path_index.is_some() {
        "Edit Recovery Path"
    } else {
        "Create New Path"
    };
    content = content.push(
        Row::new()
            .spacing(10)
            .align_y(Alignment::Center)
            .push(text::h3(title))
            .push(Space::with_width(Length::Fill))
            .push(
                button::transparent(Some(icon::cross_icon().size(32)), "")
                    .on_press(Msg::TemplateCancelPathModal),
            ),
    );

    // Key selection section
    content = content.push(text::p1_regular("Keys in Path:"));

    // Show all available keys with checkboxes
    let mut keys_column = Column::new().spacing(8);
    for (key_id, key) in state.app.keys.iter() {
        let is_selected = modal_state.selected_key_ids.contains(key_id);
        let name = if key.alias.is_empty() {
            format!("Key {}", key_id)
        } else {
            key.alias.clone()
        };
        // Add email in parentheses if available
        let label = if key.email.is_empty() {
            name
        } else {
            format!("{} ({})", name, key.email)
        };

        keys_column = keys_column.push(
            checkbox(label, is_selected).on_toggle(move |_| Msg::TemplateToggleKeyInPath(*key_id)),
        );
    }

    // Handle case when no keys exist
    if state.app.keys.is_empty() {
        keys_column = keys_column.push(text::p2_regular("No keys available. Add keys first."));
    }

    content = content.push(keys_column);

    // Get selected key count for threshold validation
    let selected_count = modal_state.selected_key_ids.len();
    let threshold_enabled = selected_count > 1;

    // Fixed width for labels and inputs for alignment
    const LABEL_WIDTH: f32 = 140.0;
    const INPUT_WIDTH: f32 = 110.0;

    // Threshold validation
    let mut threshold_valid = true;
    let mut threshold_warning: Option<&'static str> = None;

    // Threshold - label always visible, input only when enabled (key count > 1)
    let threshold_label_text = if threshold_enabled {
        format!("Threshold (1-{}):", selected_count)
    } else {
        "Threshold:".to_string()
    };
    let threshold_label: Element<'_, Msg> = if threshold_enabled {
        text::p1_regular(threshold_label_text).into()
    } else {
        text::p1_regular(threshold_label_text)
            .style(theme::text::secondary)
            .into()
    };

    let mut threshold_row = Row::new()
        .spacing(10)
        .align_y(Alignment::Center)
        .push(Container::new(threshold_label).width(Length::Fixed(LABEL_WIDTH)));

    // Only show input when enabled
    if threshold_enabled {
        // Validate threshold value
        if modal_state.threshold.is_empty() {
            // Empty: just disable save, no warning
            threshold_valid = false;
        } else if let Ok(n) = modal_state.threshold.parse::<usize>() {
            if n == 0 || n > selected_count {
                threshold_valid = false;
                threshold_warning = Some("Invalid threshold value");
            }
        } else {
            threshold_valid = false;
            threshold_warning = Some("Invalid threshold value");
        }

        let threshold_value = form::Value {
            value: modal_state.threshold.clone(),
            warning: None, // Don't show warning in form, show separately
            valid: threshold_valid || modal_state.threshold.is_empty(), // Don't show red border if empty
        };
        threshold_row = threshold_row.push(
            Container::new(form::Form::new(
                "n",
                &threshold_value,
                Msg::TemplateUpdateThreshold,
            ))
            .width(Length::Fixed(INPUT_WIDTH)),
        );
    }

    content = content.push(threshold_row);

    // Threshold error row (separate line) - only show if not empty
    if let Some(warning) = threshold_warning {
        content = content.push(
            Row::new()
                .push(Space::with_width(Length::Fixed(LABEL_WIDTH + 10.0)))
                .push(text::p2_regular(warning).style(theme::text::warning)),
        );
    }

    // Timelock input (only for secondary paths) - single line with dropdown
    // Also calculate timelock validation for the Save button
    let mut timelock_valid = true;
    let mut timelock_warning: Option<&'static str> = None;

    if !modal_state.is_primary {
        if let Some(value_str) = &modal_state.timelock_value {
            let is_empty = value_str.is_empty();

            // Calculate the current timelock in blocks
            let current_blocks = if let Ok(value) = value_str.parse::<u64>() {
                modal_state.timelock_unit.to_blocks(value)
            } else {
                0
            };

            // Check if timelock is empty or zero
            if is_empty {
                // Empty: just disable save, no warning
                timelock_valid = false;
            } else if current_blocks == 0 {
                timelock_valid = false;
                timelock_warning = Some("Timelock cannot be zero");
            } else {
                // Check for duplicate timelocks (excluding current path if editing)
                for (idx, (_, existing_timelock)) in state.app.secondary_paths.iter().enumerate() {
                    // Skip the path being edited
                    if modal_state.path_index == Some(idx) {
                        continue;
                    }
                    if existing_timelock.blocks == current_blocks {
                        timelock_valid = false;
                        timelock_warning = Some("Duplicate timelock");
                        break;
                    }
                }
            }

            let timelock_value = form::Value {
                value: value_str.clone(),
                warning: None, // Don't show warning in form, show separately
                valid: timelock_valid || is_empty, // Don't show red border if empty
            };
            content = content.push(
                Row::new()
                    .spacing(10)
                    .align_y(Alignment::Center)
                    .push(
                        Container::new(text::p1_regular("Timelock:"))
                            .width(Length::Fixed(LABEL_WIDTH)),
                    )
                    .push(
                        Container::new(form::Form::new(
                            "0",
                            &timelock_value,
                            Msg::TemplateUpdateTimelock,
                        ))
                        .width(Length::Fixed(INPUT_WIDTH)),
                    )
                    .push(
                        pick_list(
                            TimelockUnit::ALL.as_slice(),
                            Some(modal_state.timelock_unit),
                            Msg::TemplateUpdateTimelockUnit,
                        )
                        .width(Length::Fixed(100.0)),
                    ),
            );

            // Timelock error row (separate line)
            if let Some(warning) = timelock_warning {
                content = content.push(
                    Row::new()
                        .push(Space::with_width(Length::Fixed(LABEL_WIDTH + 10.0)))
                        .push(text::p2_regular(warning).style(theme::text::warning)),
                );
            }
        }
    }

    // Determine if Save should be enabled
    // - Must have at least one key selected
    // - Threshold must be valid (if enabled)
    // - For recovery paths: timelock must be valid (non-zero and not duplicate)
    let has_keys = !modal_state.selected_key_ids.is_empty();
    let can_save = has_keys && threshold_valid && (modal_state.is_primary || timelock_valid);

    // Buttons (aligned right)
    let save_button = if can_save {
        button::primary(None, "Save")
            .on_press(Msg::TemplateSavePath)
            .width(Length::Fixed(120.0))
    } else {
        button::secondary(None, "Save").width(Length::Fixed(120.0))
    };

    content = content.push(
        Row::new()
            .spacing(10)
            .push(Space::with_width(Length::Fill))
            .push(
                button::secondary(None, "Cancel")
                    .on_press(Msg::TemplateCancelPathModal)
                    .width(Length::Fixed(120.0)),
            )
            .push(save_button),
    );

    card::modal(content).into()
}
