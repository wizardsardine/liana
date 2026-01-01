use crate::{
    backend::Backend,
    state::{Msg, State},
    views::format_last_edit_info,
};
use iced::{
    widget::{
        button::{Status, Style},
        Space,
    },
    Alignment, Background, Border, Length,
};
use liana_connect::models::UserRole;
use liana_ui::{color, component::text, icon, theme::Theme, widget::*};

use crate::views::layout_with_scrollable_list;

// Card width constant (matching keys view)
const KEY_CARD_WIDTH: f32 = 600.0;

/// Custom button style for key cards: dark grey border when not hovered, green when hovered
fn key_card_button(_theme: &Theme, status: Status) -> Style {
    let grey_border = color::GREY_7;
    let green_border = color::GREEN;

    match status {
        Status::Active => Style {
            background: Some(Background::Color(color::TRANSPARENT)),
            text_color: color::GREY_2,
            border: Border {
                radius: 25.0.into(),
                width: 1.0,
                color: grey_border,
            },
            ..Default::default()
        },
        Status::Hovered => Style {
            background: Some(Background::Color(color::TRANSPARENT)),
            text_color: color::GREEN,
            border: Border {
                radius: 25.0.into(),
                width: 1.0,
                color: green_border,
            },
            ..Default::default()
        },
        Status::Pressed => Style {
            background: Some(Background::Color(color::TRANSPARENT)),
            text_color: color::GREEN,
            border: Border {
                radius: 25.0.into(),
                width: 1.0,
                color: green_border,
            },
            ..Default::default()
        },
        Status::Disabled => Style {
            background: Some(Background::Color(color::TRANSPARENT)),
            text_color: color::GREY_2,
            border: Border {
                radius: 25.0.into(),
                width: 1.0,
                color: grey_border,
            },
            ..Default::default()
        },
    }
}

/// Create a status badge for xpub population status
fn xpub_status_badge(has_xpub: bool) -> Element<'static, Msg> {
    const BADGE_WIDTH: f32 = 100.0;

    if has_xpub {
        Container::new(
            Container::new(text::caption("✓ Populated"))
                .padding([4, 12])
                .style(liana_ui::theme::pill::success)
                .width(Length::Fill)
                .center_x(Length::Fill),
        )
        .width(Length::Fixed(BADGE_WIDTH))
        .into()
    } else {
        Container::new(
            Container::new(text::caption("⚠ Missing"))
                .padding([4, 12])
                .style(liana_ui::theme::pill::warning)
                .width(Length::Fill)
                .center_x(Length::Fill),
        )
        .width(Length::Fixed(BADGE_WIDTH))
        .into()
    }
}

/// Create a key card displaying key information with xpub status.
fn xpub_key_card(
    key_id: u8,
    key: &liana_connect::Key,
    last_edit_info: Option<String>,
) -> Element<'static, Msg> {
    let mut content = Column::new().spacing(5);

    // First row: |<icon>|<alias>|<spacer>|<status_badge>
    let mut header_row = Row::new()
        .spacing(10)
        .align_y(Alignment::Center)
        .push(icon::key_icon())
        .push(text::p1_regular(&key.alias));

    // Spacer to push badge to the right
    header_row = header_row.push(Space::with_width(Length::Fill));

    // Status badge
    header_row = header_row.push(xpub_status_badge(key.xpub.is_some()));

    content = content.push(header_row);

    // Second row: Email
    if !key.email.is_empty() {
        content = content.push(text::p2_regular(&key.email));
    }

    // Third row: Key type
    content = content.push(text::p2_regular(key.key_type.as_str()));

    // Fourth row: Last edit info (optional)
    if let Some(info) = last_edit_info {
        content = content.push(text::caption(info).style(liana_ui::theme::text::secondary));
    }

    // Wrap card content - use Fill width so Button controls the final width
    let card_content = Container::new(content).padding(15).width(Length::Fill);

    // Make card clickable
    Button::new(card_content)
        .width(Length::Fixed(KEY_CARD_WIDTH))
        .on_press(Msg::XpubSelectKey(key_id))
        .style(key_card_button)
        .into()
}

pub fn xpub_view(state: &State) -> Element<'_, Msg> {
    let current_user_email = &state.views.login.email.form.value;
    let user_role = &state.app.current_user_role;

    // Determine if user is WS Manager
    let is_ws_manager = matches!(user_role, Some(UserRole::WSManager));

    // Build breadcrumb: org_name > wallet_name > Key Information
    let org_name = state
        .app
        .selected_org
        .and_then(|org_id| state.backend.get_org(org_id))
        .map(|org| org.name.clone())
        .unwrap_or_else(|| "Organization".to_string());
    let wallet_name = state
        .app
        .selected_wallet
        .and_then(|id| state.backend.get_wallet(id))
        .map(|w| w.alias.clone())
        .unwrap_or_else(|| "Wallet".to_string());
    let breadcrumb = vec![org_name, wallet_name.clone(), "Key Information".to_string()];

    // Fixed header content
    let header_content = Column::new()
        .spacing(10)
        .align_x(Alignment::Center)
        .padding(20)
        .push(text::h2(format!("{} - Add Key Information", wallet_name)))
        .push(Space::with_height(10))
        .push(text::p1_regular(
            "Select a key to provide extended public key (xpub) information.",
        ));

    // Filter keys based on role
    let current_user_email_lower = current_user_email.to_lowercase();
    let filtered_keys: Vec<(u8, &liana_connect::Key)> = state
        .app
        .keys
        .iter()
        .filter(|(_id, key)| {
            // For participants: only show keys matching their email
            // For WSManager/Owner: show all keys
            match user_role.as_ref() {
                Some(UserRole::Participant) => key.email.to_lowercase() == current_user_email_lower,
                Some(UserRole::WSManager) | Some(UserRole::Owner) | None => true,
            }
        })
        .map(|(id, key)| (*id, key))
        .collect();

    // Build scrollable key list
    let mut list_content = Column::new()
        .spacing(10)
        .padding(20)
        .align_x(Alignment::Center)
        .push(Space::with_height(20));

    if filtered_keys.is_empty() {
        // Empty state: no keys match filter
        let empty_message = match user_role.as_ref() {
            Some(UserRole::Participant) => "No keys assigned to you",
            _ => "No keys found",
        };
        list_content = list_content.push(text::p1_regular(empty_message));
    } else {
        // Always show key cards so users can edit/reset xpubs
        for (key_id, key) in filtered_keys {
            let last_edit_info = format_last_edit_info(
                key.last_edited,
                key.last_editor,
                state,
                &current_user_email_lower,
            );

            list_content = list_content.push(xpub_key_card(key_id, key, last_edit_info));
        }
    }

    list_content = list_content.push(Space::with_height(50));

    let role_badge = if is_ws_manager {
        Some("WS Manager")
    } else {
        None
    };

    layout_with_scrollable_list(
        (0, 0), // No progress indicator
        Some(current_user_email),
        role_badge,
        &breadcrumb,
        header_content,
        list_content,
        None, // No footer needed
        true,
        Some(Msg::NavigateBack),
    )
}
