pub mod modal;

use crate::{
    backend::Backend,
    state::{Msg, State},
};
use iced::{
    widget::{row, Space},
    Alignment, Length,
};
use liana_connect::ws_business::{self, UserRole};
use liana_ui::{component::text, icon, theme, widget::*};

use super::{delete_btn, format_last_edit_info, layout_with_scrollable_list, menu_entry};

/// Create a key card displaying key information.
fn key_card(
    key_id: u8,
    key: &ws_business::Key,
    last_edit_info: Option<String>,
) -> Container<'static, Msg> {
    const BADGE_WIDTH: f32 = 100.0;

    // Identity (optional - display email or other identity)
    let identity_str = key.identity.to_string();
    let identity_display = (!identity_str.is_empty())
        .then(|| text::p2_medium(identity_str).style(theme::text::accent));

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
        .align_y(Alignment::End)
        .push(icon::key_icon())
        .push(text::h3(&key.alias).style(theme::text::primary))
        .push_maybe(identity_display)
        .push(Space::with_width(Length::Fill))
        .push(badge);

    // Description (optional)
    let description = (!key.description.is_empty())
        .then(|| text::p2_medium(&key.description).style(theme::text::primary));

    // Last edit info (optional)
    let last_edit =
        last_edit_info.map(|info| text::caption(&info).style(liana_ui::theme::text::secondary));

    let content = row![Column::new()
        .spacing(5)
        .push(header_row)
        .push_maybe(description)
        .push_maybe(last_edit)]
    .width(Length::Fill)
    .height(Length::Fill);

    menu_entry(content, Some(Msg::KeyEdit(key_id)))
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

            let delete_button = delete_btn(Some(Msg::KeyDelete(*key_id)));

            Row::new()
                .spacing(15)
                .align_y(Alignment::Center)
                .push(key_card(*key_id, key, last_edit_info))
                .push(delete_button)
                .into()
        })
        .collect();

    // "Add a key" card
    let add_key_content =
        row![text::p1_medium("+ Add a key").style(liana_ui::theme::text::secondary)]
            .width(Length::Fill)
            .height(Length::Fill);

    let add_key_card = menu_entry(add_key_content, Some(Msg::KeyAdd));

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
