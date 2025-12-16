use crate::state::{views::path::EditPathModalState, Msg, State};
use iced::{widget::Space, Alignment, Length};
use liana_ui::{
    component::{button, card, form, text},
    icon,
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
    } else {
        "Edit Recovery Path"
    };
    content = content.push(
        Row::new()
            .spacing(10)
            .align_y(Alignment::Center)
            .push(text::h3(title))
            .push(Space::with_width(Length::Fill))
            .push(button::transparent(Some(icon::cross_icon()), "").on_press(Msg::TemplateCancelPathModal)),
    );

    // Get current max (m) for threshold validation
    let max_m = if modal_state.is_primary {
        state.app.primary_path.key_ids.len()
    } else if let Some(path_index) = modal_state.path_index {
        if let Some((path, _timelock)) = state.app.secondary_paths.get(path_index) {
            path.key_ids.len()
        } else {
            0
        }
    } else {
        0
    };

    content = content.push(text::p1_regular(format!(
        "Current keys in path: {} (threshold n must be between 1 and {})",
        max_m, max_m
    )));

    // Threshold N input (always shown)
    let threshold_value = form::Value {
        value: modal_state.threshold.clone(),
        warning: None,
        valid: true,
    };
    content = content.push(
        Column::new()
            .spacing(5)
            .push(text::p1_regular("Threshold (n)"))
            .push(form::Form::new(
                "Enter threshold",
                &threshold_value,
                Msg::TemplateUpdateThreshold,
            )),
    );

    // Timelock input (only for secondary paths)
    if !modal_state.is_primary {
        if let Some(blocks_str) = &modal_state.timelock {
            let blocks_value = form::Value {
                value: blocks_str.clone(),
                warning: None,
                valid: true,
            };
            content = content.push(
                Column::new()
                    .spacing(5)
                    .push(text::p1_regular("Timelock (blocks)"))
                    .push(form::Form::new("0", &blocks_value, Msg::TemplateUpdateTimelock)),
            );
        }
    }

    // Buttons
    content = content.push(
        Row::new()
            .spacing(10)
            .push(
                button::secondary(None, "Cancel")
                    .on_press(Msg::TemplateCancelPathModal)
                    .width(Length::Fixed(120.0)),
            )
            .push(
                button::primary(None, "Save")
                    .on_press(Msg::TemplateSavePath)
                    .width(Length::Fixed(120.0)),
            ),
    );

    card::modal(content).into()
}
