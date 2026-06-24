pub mod modal;

use crate::{
    backend::Backend,
    state::{Msg, State},
};
use iced::{
    widget::{row, Space},
    Alignment, Length,
};
use liana_connect::ws_business::{self, KeyType, UserRole};
use liana_ui::{
    component::{
        pill,
        text::{self},
    },
    theme,
    widget::*,
};

use super::{
    delete_btn, format_last_edit_info, layout_with_scrollable_list, menu_entry, menu_key_entry,
};

pub fn pill<'a, T: 'a>(key_type: &KeyType) -> Container<'a, T> {
    match key_type {
        KeyType::Internal => pill::key_internal(),
        KeyType::External => pill::key_external(),
        KeyType::Cosigner => pill::key_cosigner(),
        KeyType::SafetyNet => pill::key_safety_net(),
    }
}

/// Create a key card displaying key information.
fn key_card(
    key_id: u8,
    key: &ws_business::Key,
    last_edit_info: Option<String>,
) -> Element<'static, Msg> {
    let msg = Some(Msg::KeyEdit(key_id));
    let pill = pill(&key.key_type).into();

    menu_key_entry(key, last_edit_info, pill, msg)
}

pub fn keys_view(state: &State) -> Element<'_, Msg> {
    let current_user_email = &state.views.login.email.form.value;

    // Determine user role from AppState
    let is_ws_admin = matches!(
        state.app.current_user_role,
        Some(UserRole::WizardSardineAdmin)
    );

    // Keys visualization as scrollable content
    let keys_list = keys_visualization(state);

    // Empty header content - the keys list goes directly in the scrollable area
    let header_content: Element<'_, Msg> = Column::new().into();

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
        is_ws_admin,
        &breadcrumb,
        header_content,
        keys_list,
        Some(add_key_card()),
        None,
        true,
        Some(Msg::NavigateBack),
    )
}

fn add_key_card() -> Element<'static, Msg> {
    let add_key_content =
        row![text::new::caption("+ Add a key").style(liana_ui::theme::text::secondary)]
            .width(Length::Fill)
            .height(Length::Fill);

    menu_entry(add_key_content, Some(Msg::KeyAdd)).into()
}

fn keys_visualization(state: &State) -> Element<'static, Msg> {
    let current_user_email_lower = state.views.login.email.form.value.to_lowercase();

    let instruction = text::new::caption(
                            "Add the keys that will be part of this wallet and link each one to its owner's email address.",
                        ).style(theme::text::secondary);
    let instruction: Element<'_, Msg> =
        Container::new(Row::new().align_y(Alignment::Center).push(instruction))
            .align_x(Alignment::Center)
            .into();

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

    // Build the column with all elements
    let mut column = Column::new()
        .push(Space::with_height(50))
        .push(instruction)
        .push(Space::with_height(20))
        .spacing(10)
        .padding(20.0);

    for row in key_rows {
        column = column.push(row);
    }

    Container::new(column)
        .center_x(Length::Shrink)
        .center_y(Length::Shrink)
        .into()
}
