use crate::{
    state::{
        views::path::{EditPathModalState, TimelockUnit},
        Msg, State,
    },
    views::format_last_edit_info,
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
    let header = Row::new()
        .spacing(10)
        .align_y(Alignment::Center)
        .push(text::h3(title))
        .push(Space::with_width(Length::Fill))
        .push(
            button::transparent(Some(icon::cross_icon().size(32)), "")
                .on_press(Msg::TemplateCancelPathModal),
        );

    // Get last edit info for the path being edited
    let current_user_email_lower = state.views.login.email.form.value.to_lowercase();
    let last_edit_info: Option<Element<'_, Msg>> = if modal_state.is_primary {
        format_last_edit_info(
            state.app.primary_path.last_edited,
            state.app.primary_path.last_editor,
            state,
            &current_user_email_lower,
        )
        .map(|info| text::caption(info).style(theme::text::secondary).into())
    } else if let Some(idx) = modal_state.path_index {
        state
            .app
            .secondary_paths
            .get(idx)
            .and_then(|secondary| {
                format_last_edit_info(
                    secondary.path.last_edited,
                    secondary.path.last_editor,
                    state,
                    &current_user_email_lower,
                )
            })
            .map(|info| text::caption(info).style(theme::text::secondary).into())
    } else {
        None
    };

    // Key selection section
    let keys_label = text::p1_medium("Keys in Path:").style(theme::text::primary);

    let keys_column = if state.app.keys.is_empty() {
        Column::new()
            .spacing(8)
            .push(text::p2_medium("No keys available. Add keys first.").style(theme::text::primary))
    } else {
        let mut col = Column::new().spacing(8);
        for (key_id, key) in state.app.keys.iter() {
            let is_selected = modal_state.selected_key_ids.contains(key_id);
            let name = if key.alias.is_empty() {
                format!("Key {}", key_id)
            } else {
                key.alias.clone()
            };
            let identity_str = key.identity.to_string();
            let label = if identity_str.is_empty() {
                name
            } else {
                format!("{} ({})", name, identity_str)
            };
            col = col.push(
                checkbox(label, is_selected)
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
        let threshold_label_text = format!("Threshold (1-{}):", selected_count);
        let threshold_label: Element<'_, Msg> = text::p1_medium(threshold_label_text)
            .style(theme::text::primary)
            .into();
        let threshold_value = form::Value {
            value: modal_state.threshold.clone(),
            warning: None,
            valid: threshold_valid || modal_state.threshold.is_empty(),
        };
        Row::new()
            .spacing(10)
            .align_y(Alignment::Center)
            .push(Container::new(threshold_label).width(Length::Fixed(LABEL_WIDTH)))
            .push(
                Container::new(form::Form::new(
                    "n",
                    &threshold_value,
                    Msg::TemplateUpdateThreshold,
                ))
                .width(Length::Fixed(INPUT_WIDTH)),
            )
            .into()
    });

    // Threshold warning (optional)
    let threshold_warning_row = threshold_warning.map(|warning| {
        Row::new()
            .push(Space::with_width(Length::Fixed(LABEL_WIDTH + 10.0)))
            .push(text::p2_medium(warning).style(theme::text::warning))
    });

    // Timelock validation and row (only for non-primary paths)
    let (timelock_valid, timelock_section) = if modal_state.is_primary {
        (true, None)
    } else if let Some(value_str) = &modal_state.timelock_value {
        let is_empty = value_str.is_empty();

        let current_blocks = if let Ok(value) = value_str.parse::<u64>() {
            modal_state.timelock_unit.to_blocks(value)
        } else {
            0
        };

        let (valid, warning) = if is_empty {
            (false, None)
        } else if current_blocks == 0 {
            (false, Some("Timelock cannot be zero"))
        } else {
            // Check for duplicate timelocks
            let duplicate = state
                .app
                .secondary_paths
                .iter()
                .enumerate()
                .any(|(idx, secondary)| {
                    modal_state.path_index != Some(idx)
                        && secondary.timelock.blocks == current_blocks
                });
            if duplicate {
                (false, Some("Duplicate timelock"))
            } else {
                (true, None)
            }
        };

        let timelock_value = form::Value {
            value: value_str.clone(),
            warning: None,
            valid: valid || is_empty,
        };

        let timelock_row = Row::new()
            .spacing(10)
            .align_y(Alignment::Center)
            .push(
                Container::new(text::p1_medium("Timelock:").style(theme::text::primary))
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
            );

        let timelock_warning_row = warning.map(|w| {
            Row::new()
                .push(Space::with_width(Length::Fixed(LABEL_WIDTH + 10.0)))
                .push(text::p2_medium(w).style(theme::text::warning))
        });

        let section = Column::new()
            .spacing(15)
            .push(timelock_row)
            .push_maybe(timelock_warning_row);

        (valid, Some(section))
    } else {
        (true, None)
    };

    // Footer buttons
    let has_keys = !modal_state.selected_key_ids.is_empty();
    let can_save = has_keys && threshold_valid && (modal_state.is_primary || timelock_valid);

    let save_button = if can_save {
        button::primary(None, "Save")
            .on_press(Msg::TemplateSavePath)
            .width(Length::Fixed(120.0))
    } else {
        button::secondary(None, "Save").width(Length::Fixed(120.0))
    };

    let footer = Row::new()
        .spacing(10)
        .push(Space::with_width(Length::Fill))
        .push(
            button::secondary(None, "Cancel")
                .on_press(Msg::TemplateCancelPathModal)
                .width(Length::Fixed(120.0)),
        )
        .push(save_button);

    let content = Column::new()
        .push(header)
        .push_maybe(last_edit_info)
        .push(keys_label)
        .push(keys_column)
        .push_maybe(threshold_row)
        .push_maybe(threshold_warning_row)
        .push_maybe(timelock_section)
        .push(footer)
        .spacing(15)
        .padding(20.0)
        .width(Length::Fixed(500.0));

    card::modal(content).into()
}
