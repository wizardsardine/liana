use crate::state::{message::Msg, State};
use iced::{Alignment, Length};
use liana_connect::models::UserRole;
use liana_ui::{component::button, icon, widget::*};

use super::layout_with_scrollable_list;

pub mod template_visualization;

pub use template_visualization::template_visualization;

pub fn template_builder_view(state: &State) -> Element<'_, Msg> {
    let current_user_email = &state.views.login.email.form.value;

    // Determine user role from AppState
    let is_ws_manager = matches!(state.app.current_user_role, Some(UserRole::WSManager));
    let is_owner = matches!(state.app.current_user_role, Some(UserRole::Owner));

    // Template visualization as scrollable content
    let visualization = template_visualization(state);

    // Action buttons row (fixed at bottom) - role-based
    let mut buttons_row = Row::new().spacing(20).align_y(Alignment::Center);

    // WSManager: Show "Manage Keys" button, hide "Validate Template"
    if is_ws_manager {
        buttons_row = buttons_row.push(
            button::secondary(Some(icon::key_icon()), "Manage Keys").on_press(Msg::NavigateToKeys),
        );
    }

    // Owner: Show both "Manage Keys" and "Validate Template" buttons
    if is_owner {
        buttons_row = buttons_row.push(
            button::secondary(Some(icon::key_icon()), "Manage Keys").on_press(Msg::NavigateToKeys),
        );
        let is_valid = state.is_template_valid();
        let validate_button = if is_valid {
            button::primary(None, "Validate Template").on_press(Msg::TemplateValidate)
        } else {
            button::primary(None, "Validate Template")
        };
        buttons_row = buttons_row.push(validate_button);
    }

    let footer_content: Element<'_, Msg> = Container::new(buttons_row)
        .width(Length::Fill)
        .center_x(Length::Fill)
        .padding(20)
        .into();

    let role_badge = if is_ws_manager {
        Some("WS Manager")
    } else {
        None
    };

    // Empty header content - the visualization goes directly in the scrollable area
    let header_content: Element<'_, Msg> = Column::new().into();

    layout_with_scrollable_list(
        (0, 0), // No progress indicator for template builder
        Some(current_user_email),
        role_badge,
        "Template Builder",
        header_content,
        visualization,
        Some(footer_content),
        true,
        Some(Msg::NavigateBack),
    )
}
