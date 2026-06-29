use crate::{
    backend::Backend,
    state::{Msg, State},
    views::{
        entry_key_kind, intro_description, key_kind_label, layout_with_scrollable_list,
        screen_intro, INSTALLER_STEPS,
    },
};
use iced::{
    widget::{column, row, Space},
    Alignment, Length,
};
use liana_connect::ws_business::{self, KeyIdentity, KeyType, UserRole};
use liana_ui::{
    component::{
        badge, button,
        list::{self, EntrySetKeyOwner},
        pill,
        text::{self},
    },
    theme,
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
    owner: EntrySetKeyOwner,
) -> Element<'static, Msg> {
    let status = xpub_status_badge(key.xpub.is_some());
    let msg = Some(Msg::XpubSelectKey(key_id));
    let title = row![
        text::new::b5_medium(text::truncate(&key.alias, 25)),
        pill::key_kind(entry_key_kind(&key.key_type), key_kind_label(&key.key_type))
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    let body = column![
        title,
        text::new::caption(short_identity(key)).style(theme::text::tertiary)
    ]
    .spacing(3)
    .width(Length::Fill);

    list::list_entry_row(
        Some(badge::tile(entry_key_kind(&key.key_type).into()).into()),
        body,
        Some(status),
        Some(owner_accent(owner)),
        button::EntryWidth::Standard,
        msg,
    )
}

fn owner_accent(owner: EntrySetKeyOwner) -> button::ListEntryAccent {
    match owner {
        EntrySetKeyOwner::Own => |theme| theme.colors.general.accent,
        EntrySetKeyOwner::Other => |theme| {
            theme
                .colors
                .pills
                .safety_net
                .border
                .unwrap_or(theme.colors.text.secondary)
        },
    }
}

fn short_identity(key: &ws_business::Key) -> String {
    let identity = match (&key.key_type, &key.identity) {
        (
            KeyType::Cosigner | KeyType::SafetyNet,
            KeyIdentity::TokenWithProvider {
                provider: Some(provider),
                ..
            },
        ) => provider.name.clone(),
        _ => key.identity.to_string(),
    };

    if identity.is_empty() {
        "-".to_string()
    } else {
        text::short_email(&identity, 40)
    }
}

fn section_heading<'a>(label: &'a str, top_padding: f32) -> Element<'a, Msg> {
    Container::new(text::new::h3_semi(label).style(theme::text::primary))
        .width(button::STANDARD_ENTRY_WIDTH)
        .padding(iced::Padding {
            top: top_padding,
            left: 4.0,
            right: 4.0,
            bottom: 12.0,
        })
        .into()
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
    let breadcrumb = vec![org_name, wallet_name, "Set Keys".to_string()];

    let current_user_email_lower = current_user_email.to_lowercase();
    let mut owned_keys = Vec::new();
    let mut other_participant_keys = Vec::new();
    let mut external_keys = Vec::new();
    state
        .app
        .keys()
        .iter()
        .for_each(|(id, key)| match key.key_type {
            KeyType::Internal => {
                if key.identity.to_string().to_lowercase() == current_user_email_lower {
                    owned_keys.push((id, key));
                } else {
                    other_participant_keys.push((id, key));
                }
            }
            KeyType::External => {
                external_keys.push((id, key));
            }
            _ => {}
        });

    let owned_keys_set = owned_keys.iter().all(|(_, key)| key.xpub.is_some());
    let all_keys_set = owned_keys_set
        && other_participant_keys
            .iter()
            .all(|(_, key)| key.xpub.is_some())
        && external_keys.iter().all(|(_, key)| key.xpub.is_some());

    let instruction = if is_wallet_manager {
        if all_keys_set {
            "All keys are set. Once the other participants finish this step, the wallet will be ready."
        } else {
            "Select a key to complete its setup. Keys can be set up by each key manager individually, or by the wallet manager on their behalf."
        }
    } else if owned_keys_set {
        "Your assigned keys are set. Waiting for the other participants to finish this step."
    } else {
        "Select a key assigned to you to complete its setup. You can connect a hardware device or add an extended public key manually."
    };

    let header_content = screen_intro("Set Keys", Some(intro_description(instruction)), false);

    let owned_entries = if owned_keys.is_empty() {
        column![text::new::caption("No keys assigned to you.").style(theme::text::secondary)]
            .spacing(12)
            .width(button::STANDARD_ENTRY_WIDTH)
    } else {
        owned_keys
            .iter()
            .fold(column![].spacing(12), |column, (key_id, key)| {
                column.push(xpub_key_card(**key_id, key, EntrySetKeyOwner::Own))
            })
            .width(button::STANDARD_ENTRY_WIDTH)
    };

    let mut list_content = column![
        section_heading("Your keys", 4.0),
        owned_entries,
        Space::with_height(24)
    ]
    .padding([0, 20])
    .align_x(Alignment::Center)
    .spacing(0);

    if is_wallet_manager && !other_participant_keys.is_empty() {
        let entries = other_participant_keys
            .iter()
            .fold(column![].spacing(12), |column, (key_id, key)| {
                column.push(xpub_key_card(**key_id, key, EntrySetKeyOwner::Other))
            })
            .width(button::STANDARD_ENTRY_WIDTH);
        list_content = list_content
            .push(section_heading("Other participants' keys", 0.0))
            .push(entries)
            .push(Space::with_height(24));
    }

    if is_wallet_manager && !external_keys.is_empty() {
        let entries = external_keys
            .iter()
            .fold(column![].spacing(12), |column, (key_id, key)| {
                column.push(xpub_key_card(**key_id, key, EntrySetKeyOwner::Other))
            })
            .width(button::STANDARD_ENTRY_WIDTH);
        list_content = list_content
            .push(section_heading("External keys", 0.0))
            .push(entries)
            .push(Space::with_height(24));
    }

    layout_with_scrollable_list(
        (6, INSTALLER_STEPS),
        Some(current_user_email),
        is_ws_admin,
        &breadcrumb,
        Some(header_content),
        list_content,
        None,
        None,
        Some(Msg::NavigateBack),
    )
}
