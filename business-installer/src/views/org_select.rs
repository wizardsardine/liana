use crate::{
    backend::Backend,
    state::{Msg, State},
};
use iced::{
    widget::{row, Space},
    Alignment, Length,
};
use liana_connect::models::{UserRole, Wallet, WalletStatus};
use liana_ui::{
    component::{form, text},
    widget::*,
};

use uuid::Uuid;

use super::{format_last_edit_info, layout_with_scrollable_list, menu_entry};

/// Derive the user's role for a specific wallet based on wallet data and global role
/// Returns None if the user has no access to this wallet
fn derive_user_role(
    wallet: &Wallet,
    current_user_email: &str,
    global_role: Option<UserRole>,
) -> Option<UserRole> {
    // WSManager has access to all wallets
    if matches!(global_role, Some(UserRole::WSManager)) {
        return Some(UserRole::WSManager);
    }

    let email_lower = current_user_email.to_lowercase();
    // Check if user is wallet owner
    if wallet.owner.email.to_lowercase() == email_lower {
        return Some(UserRole::Owner);
    }
    // Check if user is a participant (has keys with matching email)
    if let Some(template) = &wallet.template {
        for key in template.keys.values() {
            if key.email.to_lowercase() == email_lower {
                return Some(UserRole::Participant);
            }
        }
    }
    // User has no access to this wallet
    None
}

/// Check if a wallet is accessible to the current user
/// Participants cannot access Draft (Created/Drafted/Locked) wallets
fn is_wallet_accessible(
    wallet: &Wallet,
    current_user_email: &str,
    global_role: Option<UserRole>,
) -> bool {
    let role = match derive_user_role(wallet, current_user_email, global_role) {
        Some(r) => r,
        None => return false, // No access
    };
    // Participants cannot access Draft or Locked wallets
    if matches!(role, UserRole::Participant)
        && matches!(
            wallet.status,
            WalletStatus::Created | WalletStatus::Drafted | WalletStatus::Locked
        )
    {
        return false;
    }
    true
}

pub fn org_card<'a>(
    name: String,
    count: usize,
    id: Uuid,
    last_edit_info: Option<String>,
) -> Element<'a, Msg> {
    let wallets = match count {
        0 => "".to_string(),
        1 => "(1 wallet)".to_string(),
        c => format!("({c} wallets)"),
    };

    let header = row![text::h3(name), text::h4_bold(wallets)]
        .spacing(10)
        .align_y(Alignment::End);

    let content: Element<'_, Msg> = if let Some(info) = last_edit_info {
        Column::new()
            .spacing(5)
            .push(header)
            .push(text::caption(info).style(liana_ui::theme::text::secondary))
            .into()
    } else {
        header.into()
    };

    let message = Some(Msg::OrgSelected(id));

    menu_entry(content, message)
}

pub fn no_org_card() -> Element<'static, Msg> {
    let content = text::h5_regular("Contact WizardSardine to create an account.").into();
    menu_entry(content, None)
}

pub fn org_select_view(state: &State) -> Element<'_, Msg> {
    let current_user_email = &state.views.login.email.form.value;

    let title = text::h2("Select Organization");
    let title = row![
        Space::with_width(Length::Fill),
        title,
        Space::with_width(Length::Fill),
    ];

    // Fixed header content: title and search bar
    let mut header_content = Column::new()
        .push(title)
        .push(Space::with_height(30))
        .spacing(10)
        .align_x(Alignment::Center)
        .padding(20);

    let orgs = state.backend.get_orgs();

    // Determine if user is WSManager (not owner/participant of any wallet in any org)
    let is_ws_manager = {
        let email_lower = current_user_email.to_lowercase();
        let mut is_owner_or_participant = false;

        for org_id in orgs.keys() {
            if let Some(org_data) = state.backend.get_org(*org_id) {
                for wallet in org_data.wallets.values() {
                    // Check if owner
                    if wallet.owner.email.to_lowercase() == email_lower {
                        is_owner_or_participant = true;
                        break;
                    }
                    // Check if participant (has matching key)
                    if let Some(template) = &wallet.template {
                        for key in template.keys.values() {
                            if key.email.to_lowercase() == email_lower {
                                is_owner_or_participant = true;
                                break;
                            }
                        }
                    }
                    if is_owner_or_participant {
                        break;
                    }
                }
            }
            if is_owner_or_participant {
                break;
            }
        }
        !is_owner_or_participant && !orgs.is_empty()
    };

    // Add search bar for WS Manager users
    if is_ws_manager && !orgs.is_empty() {
        let search_value = form::Value {
            value: state.views.org_select.search_filter.clone(),
            warning: None,
            valid: true,
        };
        let search_form = form::Form::new_trimmed(
            "Search organizations...",
            &search_value,
            Msg::OrgSelectUpdateSearchFilter,
        )
        .size(16)
        .padding(10);
        let search_container = Container::new(search_form)
            .width(Length::Fixed(500.0))
            .align_x(Alignment::Center);
        header_content = header_content.push(search_container);
        header_content = header_content.push(Space::with_height(10));
    }

    // Scrollable list content: organization cards
    let mut list_content = Column::new()
        .spacing(10)
        .align_x(Alignment::Center)
        .padding([0, 20]);

    // Filter organizations by search text (case-insensitive)
    let search_filter = state.views.org_select.search_filter.to_lowercase();
    let filtered_orgs: Vec<_> = orgs
        .iter()
        .filter(|(_, org)| {
            if is_ws_manager && !search_filter.is_empty() {
                org.name.to_lowercase().contains(&search_filter)
            } else {
                true
            }
        })
        .collect();

    if filtered_orgs.is_empty() && !orgs.is_empty() {
        // Show message when search filter returns no results
        list_content = list_content.push(text::p1_regular(
            "No organizations found matching your search.",
        ));
    } else if orgs.is_empty() {
        list_content = list_content.push(no_org_card());
    } else {
        let current_user_email_lower = current_user_email.to_lowercase();
        // Use global role from User record for filtering
        let global_role = state.app.global_user_role.clone();
        for (id, org) in &filtered_orgs {
            // Count only wallets accessible to this user
            let wallet_count = org
                .wallets
                .iter()
                .filter_map(|wallet_id| state.backend.get_wallet(*wallet_id))
                .filter(|wallet| {
                    is_wallet_accessible(wallet, current_user_email, global_role.clone())
                })
                .count();

            let last_edit_info = format_last_edit_info(
                org.last_edited,
                org.last_editor,
                state,
                &current_user_email_lower,
            );

            let card = org_card(org.name.clone(), wallet_count, **id, last_edit_info);
            list_content = list_content.push(card);
        }
    }
    list_content = list_content.push(Space::with_height(50));

    let role_badge = if is_ws_manager {
        Some("WS Manager")
    } else {
        None
    };

    layout_with_scrollable_list(
        (3, 4),
        Some(current_user_email),
        role_badge,
        &["Organization".to_string()],
        header_content,
        list_content,
        None, // footer_content
        true,
        None,
    )
}
