use crate::state::{message::Msg, State};
use iced::{Alignment, Length};
use liana_ui::{
    component::{button, card, text},
    widget::*,
};

use iced::widget::Space;

pub mod template_visualization;

pub use template_visualization::template_visualization;

pub fn template_builder_view(state: &State) -> Element<'_, Msg> {
    // Left panel: Header, Summary, and Buttons
    let mut left_column = Column::new()
        .spacing(20)
        .padding(20.0)
        .width(Length::Fixed(300.0))
        .align_x(Alignment::Start);

    // Back button
    left_column = left_column.push(
        Row::new()
            .push(
                button::transparent(Some(liana_ui::icon::arrow_back()), "Back")
                    .on_press(Msg::NavigateBack),
            )
            .push(Space::with_width(Length::Fill)),
    );

    // Header
    left_column = left_column.push(text::h2("Liana Business template builder"));

    // Summary section
    let keys_count = state.app.keys.len();
    let secondary_paths_count = state.app.secondary_paths.len();
    let primary_keys_count = state.app.primary_path.key_ids.len();

    let summary_card = card::simple(
        Column::new()
            .spacing(10)
            .push(text::h4_regular("Summary"))
            .push(text::p1_regular(format!("Total Keys: {}", keys_count)))
            .push(text::p1_regular(format!(
                "Primary Path: {} key(s)",
                primary_keys_count
            )))
            .push(text::p1_regular(format!(
                "Secondary Paths: {}",
                secondary_paths_count
            ))),
    );

    left_column = left_column.push(summary_card);

    // Navigation buttons
    let nav_col = Column::new()
        .spacing(10)
        .push(
            button::primary(Some(liana_ui::icon::key_icon()), "Manage Keys")
                .on_press(Msg::NavigateToKeys)
                .width(Length::Fixed(200.0)),
        )
        .push(
            button::primary(Some(liana_ui::icon::recovery_icon()), "Manage Paths")
                .on_press(Msg::NavigateToPaths)
                .width(Length::Fixed(200.0)),
        );

    left_column = left_column.push(nav_col);

    // Validate template button
    let is_valid = state.is_template_valid();
    let validate_button = if is_valid {
        button::primary(None, "Validate Template")
            .on_press(Msg::TemplateValidate)
            .width(Length::Fixed(200.0))
    } else {
        // Disabled state - use secondary style and no on_press
        button::secondary(None, "Validate Template").width(Length::Fixed(200.0))
    };
    left_column = left_column.push(validate_button);

    // Right panel: Template Visualization
    let right_panel = template_visualization(state);

    // Create row layout with left and right panels
    let row = Row::new().spacing(20).push(left_column).push(
        Container::new(right_panel)
            .width(Length::Fill)
            .height(Length::Fill),
    );

    Container::new(row)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

