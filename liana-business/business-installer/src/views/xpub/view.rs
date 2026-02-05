use crate::{
    backend::Backend,
    state::{Msg, State},
    views::{card_entry, format_last_edit_info, layout_with_scrollable_list},
};
use iced::{
    widget::{row, Space},
    Alignment, Length,
};
use liana_connect::ws_business::{self, UserRole};
use liana_ui::{component::text, icon, theme, widget::*};

// Card width constant (matching keys view)
const KEY_CARD_WIDTH: f32 = 600.0;

/// Create a status badge for xpub population status
fn xpub_status_badge(has_xpub: bool) -> Element<'static, Msg> {
    const BADGE_WIDTH: f32 = 100.0;

    if has_xpub {
        Container::new(
            Container::new(text::caption("✓ Set"))
                .padding([4, 12])
                .style(liana_ui::theme::pill::success)
                .width(Length::Fill)
                .center_x(Length::Fill),
        )
        .width(Length::Fixed(BADGE_WIDTH))
        .into()
    } else {
        Container::new(
            Container::new(text::caption("⚠ Not Set"))
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
    key: &ws_business::Key,
    last_edit_info: Option<String>,
) -> Element<'static, Msg> {
    // First row: |<icon>|<alias>|<identity>|<spacer>|<status_badge>
    let identity_str = key.identity.to_string();
    let header_row = Row::new()
        .spacing(10)
        .align_y(Alignment::Center)
        .push(icon::key_icon())
        .push(text::p1_medium(&key.alias).style(theme::text::primary))
        .push(text::p1_medium(identity_str).style(theme::text::primary))
        .push(Space::with_width(Length::Fill))
        .push(xpub_status_badge(key.xpub.is_some()));

    // Second row: Description
    let description = text::p2_medium(&key.description).style(theme::text::primary);

    // Third row: Key type
    let key_type_str = format!("{:?}", key.key_type);
    let key_type = text::p2_medium(key_type_str).style(theme::text::primary);

    // Fourth row: Last edit info (optional)
    let edit_info = last_edit_info.map(text::caption);

    let content = Column::new()
        .push(header_row)
        .push(description)
        .push(key_type)
        .push_maybe(edit_info)
        .spacing(5);

    card_entry(
        content.into(),
        Some(Msg::XpubSelectKey(key_id)),
        KEY_CARD_WIDTH,
    )
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
        .unwrap_or_else(|| "Organization".to_string());
    let wallet_name = state
        .app
        .selected_wallet
        .and_then(|id| state.backend.get_wallet(id))
        .map(|w| w.alias.clone())
        .unwrap_or_else(|| "Wallet".to_string());
    let breadcrumb = vec![org_name, wallet_name.clone(), "Set Keys".to_string()];

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
            "Your key is set."
        } else {
            "Your keys are set."
        };
        Row::new()
            .spacing(10)
            .align_y(Alignment::Center)
            .push(icon::clock_icon())
            .push(
                Column::new()
                    .spacing(5)
                    .push(text::p1_bold(keys_set_msg))
                    .push(text::p1_medium(
                        "Once the other participants complete their key setup, you'll be able to access the wallet.",
                    ).style(theme::text::primary)),
            )
            .into()
    } else {
        text::p1_medium(
            "Select a key to complete its setup. Keys can be set up by each key manager individually, or by the wallet manager on their behalf. You can connect a hardware device (recommended) or manually add an extended public key (xpub).",
        )
        .style(theme::text::primary)
        .into()
    };

    let header_content = Column::new()
        .spacing(10)
        .align_x(Alignment::Center)
        .padding(20)
        .push(text::h2(format!("{} - Set Keys", wallet_name)))
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
            Some(UserRole::Participant) => "No keys assigned to you",
            _ => "No keys found",
        };
        list_content =
            list_content.push(text::p1_medium(empty_message).style(theme::text::primary));
    } else {
        list_content = list_content.push(
            row![
                Space::with_width(10),
                text::h3("Your keys:").style(theme::text::primary),
                Space::with_width(Length::Fill)
            ]
            .width(KEY_CARD_WIDTH),
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
                text::h3("Other participants' keys:").style(theme::text::primary),
                Space::with_width(Length::Fill)
            ]
            .width(KEY_CARD_WIDTH),
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

    let role_badge = if is_ws_admin { Some("WS Admin") } else { None };

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
