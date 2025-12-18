pub mod modal;

use crate::state::{Msg, State};
use iced::{
    widget::{
        button::{Status, Style},
        Space,
    },
    Alignment, Background, Border, Length,
};
use liana_connect::models::UserRole;
use liana_ui::{
    color,
    component::text,
    icon,
    theme::Theme,
    widget::*,
};

use super::layout_with_scrollable_list;

// Card width constant (matching path cards)
const KEY_CARD_WIDTH: f32 = 600.0;

/// Custom button style for key cards: dark grey border when not hovered, green when hovered
fn key_card_button(_theme: &Theme, status: Status) -> Style {
    bordered_button_style(status, 25.0)
}

/// Custom button style for delete button: dark grey border when not hovered, green when hovered
fn delete_button_style(_theme: &Theme, status: Status) -> Style {
    bordered_button_style(status, 50.0) // Fully round
}

/// Shared bordered button style with configurable border radius
fn bordered_button_style(status: Status, radius: f32) -> Style {
    let grey_border = color::GREY_7;
    let green_border = color::GREEN;

    match status {
        Status::Active => Style {
            background: Some(Background::Color(color::TRANSPARENT)),
            text_color: color::GREY_2,
            border: Border {
                radius: radius.into(),
                width: 1.0,
                color: grey_border,
            },
            ..Default::default()
        },
        Status::Hovered => Style {
            background: Some(Background::Color(color::TRANSPARENT)),
            text_color: color::GREEN,
            border: Border {
                radius: radius.into(),
                width: 1.0,
                color: green_border,
            },
            ..Default::default()
        },
        Status::Pressed => Style {
            background: Some(Background::Color(color::TRANSPARENT)),
            text_color: color::GREEN,
            border: Border {
                radius: radius.into(),
                width: 1.0,
                color: green_border,
            },
            ..Default::default()
        },
        Status::Disabled => Style {
            background: Some(Background::Color(color::TRANSPARENT)),
            text_color: color::GREY_2,
            border: Border {
                radius: radius.into(),
                width: 1.0,
                color: grey_border,
            },
            ..Default::default()
        },
    }
}

/// Create a key card displaying key information.
fn key_card(key_id: u8, key: &liana_connect::Key) -> Element<'static, Msg> {
    let mut content = Column::new().spacing(5);

    // First row: |<icon>|<Key_name>|<email>|<spacer>|<key_type_badge>
    let mut header_row = Row::new()
        .spacing(10)
        .align_y(Alignment::Center)
        .push(icon::key_icon())
        .push(text::p1_regular(&key.alias));

    // Email (if present)
    if !key.email.is_empty() {
        header_row = header_row.push(text::p2_regular(&key.email));
    }

    // Spacer to push badge to the right
    header_row = header_row.push(Space::with_width(Length::Fill));

    // Key type badge with fixed width
    const BADGE_WIDTH: f32 = 100.0; // Fixed width for all key type variants
    header_row = header_row.push(
        Container::new(
            Container::new(text::caption(key.key_type.as_str()))
                .padding([4, 12])
                .style(liana_ui::theme::pill::simple)
                .width(Length::Fill)
                .center_x(Length::Fill),
        )
        .width(Length::Fixed(BADGE_WIDTH)),
    );

    content = content.push(header_row);

    // Description (if present)
    if !key.description.is_empty() {
        content = content.push(text::p2_regular(&key.description));
    }

    // Wrap card content - use Fill width so Button controls the final width
    let card_content = Container::new(content).padding(15).width(Length::Fill);

    // Make card clickable
    Button::new(card_content)
        .width(Length::Fixed(KEY_CARD_WIDTH))
        .on_press(Msg::KeyEdit(key_id))
        .style(key_card_button)
        .into()
}

pub fn keys_view(state: &State) -> Element<'_, Msg> {
    let current_user_email = &state.views.login.email.form.value;

    // Determine user role from AppState
    let is_ws_manager = matches!(state.app.current_user_role, Some(UserRole::WSManager));

    // Keys visualization as scrollable content
    let keys_list = keys_visualization(state);

    // Empty header content - the keys list goes directly in the scrollable area
    let header_content: Element<'_, Msg> = Column::new().into();

    let role_badge = if is_ws_manager {
        Some("WS Manager")
    } else {
        None
    };

    layout_with_scrollable_list(
        (0, 0), // No progress indicator
        Some(current_user_email),
        role_badge,
        "Manage Keys",
        header_content,
        keys_list,
        None, // No footer needed
        true,
        Some(Msg::NavigateBack),
    )
}

fn keys_visualization(state: &State) -> Element<'static, Msg> {
    let keys = &state.app.keys;

    let mut column = Column::new()
        .spacing(10)
        .padding(20.0)
        .push(Space::with_height(50));

    // List all keys as clickable cards with delete buttons
    for (key_id, key) in keys.iter() {
        let mut key_row = Row::new()
            .spacing(15)
            .align_y(Alignment::Center)
            .push(key_card(*key_id, key));

        // Delete button on the right
        key_row = key_row.push(
            Button::new(
                Container::new(icon::trash_icon())
                    .width(Length::Fixed(20.0))
                    .height(Length::Fixed(20.0))
                    .center_x(Length::Fixed(20.0))
                    .center_y(Length::Fixed(20.0)),
            )
            .padding(10)
            .on_press(Msg::KeyDelete(*key_id))
            .style(delete_button_style),
        );

        column = column.push(key_row);
    }

    // "Add a key" card at the bottom
    let add_key_content = Container::new(
        text::p1_regular("+ Add a key").style(liana_ui::theme::text::secondary),
    )
    .padding(15)
    .width(Length::Fill);

    let add_key_card = Button::new(add_key_content)
        .width(Length::Fixed(KEY_CARD_WIDTH))
        .on_press(Msg::KeyAdd)
        .style(key_card_button);

    column = column.push(add_key_card);

    Container::new(column)
        .center_x(Length::Shrink)
        .center_y(Length::Shrink)
        .into()
}
