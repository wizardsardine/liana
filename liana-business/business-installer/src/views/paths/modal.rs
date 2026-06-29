use crate::{
    state::{
        views::paths::{EditPathModalState, TimelockUnit},
        Msg, State,
    },
    views::format_last_edit_info,
};
use iced::{
    widget::{checkbox, column, row, Space},
    Alignment,
};
use liana_ui::{
    component::{
        button::{btn_cancel, btn_save},
        form,
        modal::{modal_view, ModalWidth},
        pick_list,
        text::{self, short_email, truncate},
    },
    theme,
    widget::*,
};

fn compact_label<'a>(label: impl Into<String>) -> Element<'a, Msg> {
    text::new::b5_medium(label.into())
        .style(theme::text::primary)
        .into()
}

pub fn path_modal_view(state: &State) -> Option<Element<'_, Msg>> {
    if let Some(modal_state) = &state.views.paths.edit_path_modal {
        return Some(edit_path_modal_view(state, modal_state));
    }
    None
}

pub fn edit_path_modal_view<'a>(
    state: &'a State,
    modal_state: &'a EditPathModalState,
) -> Element<'a, Msg> {
    // Fixed width for labels and inputs for alignment
    const LABEL_WIDTH: f32 = 140.0;
    const INPUT_WIDTH: f32 = 110.0;

    // Header
    let title = if modal_state.is_primary {
        "Edit Primary Path"
    } else if modal_state.path_index.is_some() {
        "Edit Recovery Path"
    } else {
        "Create New Path"
    };

    // Get last edit info for the path being edited
    let current_user_email_lower = state.views.login.email.form.value.to_lowercase();
    let last_edit_info: Option<Element<'_, Msg>> = if modal_state.is_primary {
        format_last_edit_info(
            state.app.primary_path().last_edited,
            state.app.primary_path().last_editor,
            state,
            &current_user_email_lower,
        )
        .map(|info| {
            text::new::caption(info)
                .style(theme::text::secondary)
                .into()
        })
    } else if let Some(idx) = modal_state.path_index {
        state
            .app
            .secondary_paths()
            .get(idx)
            .and_then(|secondary| {
                format_last_edit_info(
                    secondary.path.last_edited,
                    secondary.path.last_editor,
                    state,
                    &current_user_email_lower,
                )
            })
            .map(|info| {
                text::new::caption(info)
                    .style(theme::text::secondary)
                    .into()
            })
    } else {
        None
    };

    // Key selection section
    let keys_label = compact_label("Keys in Path:");

    let keys_column = if state.app.keys().is_empty() {
        column![
            text::new::caption("No keys available. Add keys first.").style(theme::text::secondary)
        ]
        .spacing(8)
    } else {
        let mut col = column![].spacing(8);
        for (key_id, key) in state.app.keys().iter() {
            let is_selected = modal_state.selected_key_ids.contains(key_id);
            let mut name = if key.alias.is_empty() {
                format!("Key {key_id}")
            } else {
                key.alias.clone()
            };
            let mut identity_str = key.identity.to_string();
            let label = if identity_str.is_empty() {
                truncate(&name, 40)
            } else {
                let mut name_len = name.chars().count();
                let id_len = identity_str.chars().count();
                if (name_len + id_len) > 50 {
                    if name_len > 20 {
                        name_len = 20;
                        name = truncate(&name, name_len);
                    }
                    if (name_len + id_len) > 50 {
                        identity_str = short_email(&identity_str, 30);
                    }
                }
                format!("{name} ({identity_str})")
            };
            col = col.push(
                checkbox(is_selected)
                    .label(label)
                    .on_toggle(move |_| Msg::TemplateToggleKeyInPath(*key_id)),
            );
        }
        col
    };

    // Threshold validation
    let selected_count = modal_state.selected_key_ids.len();
    let threshold_enabled = selected_count > 1;

    let (threshold_valid, threshold_warning) = if !threshold_enabled {
        (true, None)
    } else if modal_state.threshold.is_empty() {
        (false, None)
    } else if let Ok(n) = modal_state.threshold.parse::<usize>() {
        if n == 0 || n > selected_count {
            (false, Some("Invalid threshold value"))
        } else {
            (true, None)
        }
    } else {
        (false, Some("Invalid threshold value"))
    };

    // Threshold row (only shown when 2+ keys are selected)
    let threshold_row: Option<Element<'_, Msg>> = threshold_enabled.then_some({
        let threshold_label_text = format!("Threshold (1-{selected_count}):");
        let threshold_label = compact_label(threshold_label_text);
        let threshold_value = form::Value {
            value: modal_state.threshold.clone(),
            warning: None,
            valid: threshold_valid || modal_state.threshold.is_empty(),
        };
        let input = row![
            Container::new(threshold_label).width(LABEL_WIDTH),
            Container::new(
                form::Form::new("n", &threshold_value, Msg::TemplateUpdateThreshold).compact(),
            )
            .width(INPUT_WIDTH)
        ]
        .spacing(10)
        .align_y(Alignment::Center);

        let warning = threshold_warning.map(|warning| {
            row![
                Space::with_width(LABEL_WIDTH + 10.0),
                text::new::small_caption(warning).style(theme::text::warning)
            ]
        });

        column![input, warning].spacing(4).into()
    });

    // Timelock validation and row (only for non-primary paths)
    let (timelock_valid, timelock_section) = if modal_state.is_primary {
        (true, None)
    } else if let Some(value_str) = &modal_state.timelock_value {
        let is_empty = value_str.is_empty();

        // Parse the input value and compute capped blocks for duplicate detection
        let parsed_value = value_str.parse::<u64>().ok();
        let current_blocks = parsed_value
            .map(|v| modal_state.timelock_unit.to_blocks_capped(v))
            .unwrap_or(0);

        let (valid, warning) = if is_empty {
            (false, None)
        } else if current_blocks == 0 {
            (false, Some("Timelock cannot be zero".to_string()))
        } else if parsed_value.is_some_and(|v| v > modal_state.timelock_unit.max_value()) {
            (
                false,
                Some(format!(
                    "Max {} {}",
                    modal_state.timelock_unit.max_value(),
                    modal_state.timelock_unit
                )),
            )
        } else {
            // Check for duplicate timelocks
            let duplicate =
                state
                    .app
                    .secondary_paths()
                    .iter()
                    .enumerate()
                    .any(|(idx, secondary)| {
                        modal_state.path_index != Some(idx)
                            && secondary.timelock.blocks == current_blocks
                    });
            if duplicate {
                (false, Some("Duplicate timelock".to_string()))
            } else {
                (true, None)
            }
        };

        let timelock_value = form::Value {
            value: value_str.clone(),
            warning: None,
            valid: valid || is_empty,
        };

        let input = row![
            Container::new(compact_label("Timelock:")).width(LABEL_WIDTH),
            Container::new(
                form::Form::new("0", &timelock_value, Msg::TemplateUpdateTimelock).compact(),
            )
            .width(INPUT_WIDTH),
            pick_list::pick_list(
                TimelockUnit::ALL.as_slice(),
                Some(modal_state.timelock_unit),
                Msg::TemplateUpdateTimelockUnit,
            )
            .width(100.0)
        ]
        .spacing(10)
        .align_y(Alignment::Center);

        let label = warning
            .map(|w| text::new::small_caption(w).style(theme::text::warning))
            .or_else(|| {
                let hint = format!(
                    "Max: {} {}",
                    modal_state.timelock_unit.max_value(),
                    modal_state.timelock_unit
                );
                Some(text::new::small_caption(hint).style(theme::text::secondary))
            });

        let label = label.map(|l| row![Space::with_width(LABEL_WIDTH + 10.0), l]);

        let section = column![input, label].spacing(4);

        (valid, Some(section))
    } else {
        (true, None)
    };

    // Footer buttons
    let has_keys = !modal_state.selected_key_ids.is_empty();
    let can_save = has_keys && threshold_valid && (modal_state.is_primary || timelock_valid);

    let save_button = btn_save(can_save.then_some(Msg::TemplateSavePath));

    let footer = row![
        Space::fill_width(),
        btn_cancel(Some(Msg::TemplateCancelPathModal)),
        save_button
    ]
    .spacing(10);

    let body = column![
        last_edit_info,
        keys_label,
        keys_column,
        threshold_row,
        timelock_section,
        footer
    ]
    .spacing(15);

    modal_view(
        Some(title.to_string()),
        None,
        Some(Msg::TemplateCancelPathModal),
        ModalWidth::M,
        body,
    )
}
