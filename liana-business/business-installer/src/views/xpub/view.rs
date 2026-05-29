use crate::{
    backend::Backend,
    state::{Msg, State},
    views::{format_last_edit_info, layout_with_scrollable_list, menu_key_entry, MENU_ENTRY_WIDTH},
};
use iced::{
    widget::{row, Space},
    Alignment, Length,
};
use liana_connect::ws_business::{self, UserRole};
use liana_i18n::t;
use liana_ui::{
    component::{
        pill,
        text::{self},
    },
    icon, theme,
    widget::*,
};

/// Create a status badge for xpub population status
fn xpub_status_badge(has_xpub: bool) -> Element<'static, Msg> {
    if has_xpub {
        pill::xpub_set().into()
    } else {
        pill::xpub_not_set().into()
    }
}

/// Create a key card displaying key information with xpub status.
fn xpub_key_card(
    key_id: u8,
    key: &ws_business::Key,
    last_edit_info: Option<String>,
) -> Container<'static, Msg> {
    let icon = icon::key_icon();
    let pill = xpub_status_badge(key.xpub.is_some());
    let msg = Some(Msg::XpubSelectKey(key_id));

    menu_key_entry(key, last_edit_info, icon, pill, msg)
}

pub fn xpub_view(state: &State) -> Element<'_, Msg> {
    let current_user_email = &state.views.login.email.form.value;
    let user_role = &state.app.current_user_role;

    // Determine if user is WS Admin
    let is_ws_admin = matches!(user_role, Some(UserRole::WizardSardineAdmin));
    // or Wallet Manager
    let is_wallet_manager = matches!(user_role, Some(UserRole::WalletManager));

    // Build breadcrumb: org_name > wallet_name > Key Information
    let org_name = state
        .app
        .selected_org
        .and_then(|org_id| state.backend.get_org(org_id))
        .map(|org| org.name.clone())
        .unwrap_or_else(|| t!("business-organization"));
    let wallet_name = state
        .app
        .selected_wallet
        .and_then(|id| state.backend.get_wallet(id))
        .map(|w| w.alias.clone())
        .unwrap_or_else(|| t!("business-wallet"));
    let breadcrumb = vec![org_name, wallet_name.clone(), t!("business-set-keys")];

    // Filter keys based on role (needed before header to determine waiting state)
    let current_user_email_lower = current_user_email.to_lowercase();
    let mut owned_keys = Vec::new();
    let mut non_owned_keys = Vec::new();
    state.app.keys.iter().for_each(|(id, key)| {
        if key.identity.to_string().to_lowercase() == current_user_email_lower {
            owned_keys.push((id, key));
        } else {
            non_owned_keys.push((id, key));
        }
    });

    // Check if all user's keys are already set (for waiting state)
    let all_keys_set = owned_keys.iter().all(|(_, key)| key.xpub.is_some())
        && non_owned_keys.iter().all(|(_, key)| key.xpub.is_some());

    // Fixed header content - show waiting message if all keys are set
    let instruction: Element<'_, Msg> = if all_keys_set {
        let keys_set_msg = if owned_keys.len() == 1 {
            t!("business-your-key-set")
        } else {
            t!("business-your-keys-set")
        };
        Container::new(
            Row::new()
                .spacing(10)
                .align_y(Alignment::Center)
                .push(icon::clock_icon())
                .push(
                    Column::new()
                        .spacing(5)
                        .push(text::p1_bold(keys_set_msg))
                        .push(
                            text::p1_medium(t!("business-wait-other-key-setup"))
                                .style(theme::text::primary),
                        ),
                ),
        )
        .align_x(Alignment::Center)
        .width(Length::Fill)
        .into()
    } else {
        text::p1_medium(t!("business-xpub-instruction"))
            .style(theme::text::primary)
            .into()
    };

    let header_content = Column::new()
        .spacing(10)
        .align_x(Alignment::Center)
        .padding(20)
        .push(text::h2(t!(
            "business-wallet-set-keys",
            wallet = wallet_name
        )))
        .push(Space::with_height(10))
        .push(instruction);

    // Build scrollable key list
    let mut list_content = Column::new()
        .spacing(10)
        .padding(20)
        .align_x(Alignment::Center)
        .push(Space::with_height(20));

    if owned_keys.is_empty() {
        // Empty state: no keys match filter
        let empty_message = match user_role.as_ref() {
            Some(UserRole::Participant) => t!("business-no-keys-assigned"),
            _ => t!("business-no-keys-found"),
        };
        list_content =
            list_content.push(text::p1_medium(empty_message).style(theme::text::primary));
    } else {
        list_content = list_content.push(
            row![
                Space::with_width(10),
                text::h3(t!("business-your-keys")).style(theme::text::primary),
                Space::with_width(Length::Fill)
            ]
            .width(MENU_ENTRY_WIDTH),
        );
        // Always show key cards so users can edit/reset xpubs
        for (key_id, key) in owned_keys {
            let last_edit_info = format_last_edit_info(
                key.last_edited,
                key.last_editor,
                state,
                &current_user_email_lower,
            );

            list_content = list_content.push(xpub_key_card(*key_id, key, last_edit_info));
        }
    }

    if is_wallet_manager {
        list_content = list_content.push(Space::with_height(20)).push(
            row![
                Space::with_width(10),
                text::h3(t!("business-other-participants-keys")).style(theme::text::primary),
                Space::with_width(Length::Fill)
            ]
            .width(MENU_ENTRY_WIDTH),
        );
        // Always show key cards so users can edit/reset xpubs
        for (key_id, key) in non_owned_keys {
            let last_edit_info = format_last_edit_info(
                key.last_edited,
                key.last_editor,
                state,
                &current_user_email_lower,
            );

            list_content = list_content.push(xpub_key_card(*key_id, key, last_edit_info));
        }
    }

    list_content = list_content.push(Space::with_height(50));

    layout_with_scrollable_list(
        (0, 0), // No progress indicator
        Some(current_user_email),
        is_ws_admin,
        &breadcrumb,
        header_content,
        list_content,
        None, // No footer needed
        true,
        Some(Msg::NavigateBack),
    )
}
