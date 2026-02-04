pub mod modal;

use crate::{
    backend::Backend,
    state::{Msg, State},
};
use iced::{
    widget::{
        button::{Status, Style},
        Space,
    },
    Alignment, Background, Border, Length,
};
use liana_connect::ws_business::{self, UserRole};
use liana_ui::{color, component::text, icon, theme, theme::Theme, widget::*};

use super::{card_entry, format_last_edit_info, layout_with_scrollable_list};

// Card width constant (matching path cards)
const KEY_CARD_WIDTH: f32 = 600.0;

/// Custom button style for delete button: circular with grey background and shadow
pub fn delete_button_style(_theme: &Theme, status: Status) -> Style {
    use iced::{Color, Shadow, Vector};

    let background = color::LIGHT_BG_SECONDARY;
    let border_active = color::BUSINESS_BLUE_DARK;
    let shadow = Shadow {
        color: Color::from_rgba(0.0, 0.0, 0.0, 0.15),
        offset: Vector::new(0.0, 2.0),
        blur_radius: 8.0,
    };

    match status {
        Status::Active => Style {
            background: Some(Background::Color(background)),
            text_color: color::DARK_TEXT_SECONDARY,
            border: Border {
                radius: 50.0.into(),
                width: 1.0,
                color: color::TRANSPARENT,
            },
            shadow,
        },
        Status::Hovered | Status::Pressed => Style {
            background: Some(Background::Color(background)),
            text_color: color::BUSINESS_BLUE_DARK,
            border: Border {
                radius: 50.0.into(),
                width: 1.0,
                color: border_active,
            },
            shadow,
        },
        Status::Disabled => Style {
            background: Some(Background::Color(background)),
            text_color: color::DARK_TEXT_TERTIARY,
            border: Border {
                radius: 50.0.into(),
                width: 1.0,
                color: color::TRANSPARENT,
            },
            shadow,
        },
    }
}

/// Create a key card displaying key information.
fn key_card(
    key_id: u8,
    key: &ws_business::Key,
    last_edit_info: Option<String>,
) -> Element<'static, Msg> {
    const BADGE_WIDTH: f32 = 100.0;

    // Identity (optional - display email or other identity)
    let identity_str = key.identity.to_string();
    let identity_display = (!identity_str.is_empty())
        .then(|| text::p2_medium(identity_str).style(theme::text::primary));

    // Key type badge
    let key_type_str = format!("{:?}", key.key_type);
    let badge = Container::new(
        Container::new(text::caption(key_type_str))
            .padding([4, 12])
            .style(liana_ui::theme::pill::simple)
            .width(Length::Fill)
            .center_x(Length::Fill),
    )
    .width(Length::Fixed(BADGE_WIDTH));

    // Header row: |<icon>|<Key_name>|<identity>|<spacer>|<key_type_badge>
    let header_row = Row::new()
        .spacing(10)
        .align_y(Alignment::Center)
        .push(icon::key_icon())
        .push(text::p1_medium(&key.alias).style(theme::text::primary))
        .push_maybe(identity_display)
        .push(Space::with_width(Length::Fill))
        .push(badge);

    // Description (optional)
    let description = (!key.description.is_empty())
        .then(|| text::p2_medium(&key.description).style(theme::text::primary));

    // Last edit info (optional)
    let last_edit =
        last_edit_info.map(|info| text::caption(&info).style(liana_ui::theme::text::secondary));

    let content = Column::new()
        .spacing(5)
        .push(header_row)
        .push_maybe(description)
        .push_maybe(last_edit);

    card_entry(content.into(), Some(Msg::KeyEdit(key_id)), KEY_CARD_WIDTH)
}

pub fn keys_view(state: &State) -> Element<'_, Msg> {
    let current_user_email = &state.views.login.email.form.value;

    // Determine user role from AppState
    let is_ws_manager = matches!(
        state.app.current_user_role,
        Some(UserRole::WizardSardineAdmin)
    );

    // Keys visualization as scrollable content
    let keys_list = keys_visualization(state);

    // Empty header content - the keys list goes directly in the scrollable area
    let header_content: Element<'_, Msg> = Column::new().into();

    let role_badge = if is_ws_manager {
        Some("WS Admin")
    } else {
        None
    };

    // Build breadcrumb: org_name > wallet_name > Keys
    let org_name = state
        .app
        .selected_org
        .and_then(|org_id| state.backend.get_org(org_id))
        .map(|org| org.name.clone())
        .unwrap_or_else(|| "Organization".to_string());
    let wallet_name = state
        .app
        .selected_wallet
        .and_then(|wallet_id| state.backend.get_wallet(wallet_id))
        .map(|wallet| wallet.alias.clone())
        .unwrap_or_else(|| "Wallet".to_string());
    let breadcrumb = vec![org_name, wallet_name, "Keys".to_string()];

    layout_with_scrollable_list(
        (0, 0), // No progress indicator
        Some(current_user_email),
        role_badge,
        &breadcrumb,
        header_content,
        keys_list,
        None, // No footer needed
        true,
        Some(Msg::NavigateBack),
    )
}

fn keys_visualization(state: &State) -> Element<'static, Msg> {
    let current_user_email_lower = state.views.login.email.form.value.to_lowercase();

    // Build key rows with delete buttons
    let key_rows: Vec<Element<'static, Msg>> = state
        .app
        .keys
        .iter()
        .map(|(key_id, key)| {
            let last_edit_info = format_last_edit_info(
                key.last_edited,
                key.last_editor,
                state,
                &current_user_email_lower,
            );

            let delete_button = Button::new(
                Container::new(icon::trash_icon())
                    .width(Length::Fixed(20.0))
                    .height(Length::Fixed(20.0))
                    .center_x(Length::Fixed(20.0))
                    .center_y(Length::Fixed(20.0)),
            )
            .padding(10)
            .on_press(Msg::KeyDelete(*key_id))
            .style(delete_button_style);

            Row::new()
                .spacing(15)
                .align_y(Alignment::Center)
                .push(key_card(*key_id, key, last_edit_info))
                .push(delete_button)
                .into()
        })
        .collect();

    // "Add a key" card
    let add_key_content = text::p1_medium("+ Add a key").style(liana_ui::theme::text::secondary);

    let add_key_card = card_entry(add_key_content.into(), Some(Msg::KeyAdd), KEY_CARD_WIDTH);

    // Build the column with all elements
    let mut column = Column::new()
        .spacing(10)
        .padding(20.0)
        .push(Space::with_height(50));

    for row in key_rows {
        column = column.push(row);
    }
    column = column.push(add_key_card);

    Container::new(column)
        .center_x(Length::Shrink)
        .center_y(Length::Shrink)
        .into()
}
