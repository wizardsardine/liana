use crate::{
    backend::Backend,
    state::{Msg, State},
};
use iced::{widget::row, Alignment, Length};
use liana_connect::models::{UserRole, Wallet, WalletStatus};
use liana_ui::{component::text, widget::*};

use iced::widget::Space;
use uuid::Uuid;

use super::{layout, menu_entry};

/// Derive the user's role for a specific wallet
fn derive_user_role(wallet: &Wallet, current_user_email: &str) -> UserRole {
    let email_lower = current_user_email.to_lowercase();
    // Check if user is wallet owner
    if wallet.owner.email.to_lowercase() == email_lower {
        return UserRole::Owner;
    }
    // Check if user is a participant (has keys with matching email)
    if let Some(template) = &wallet.template {
        for key in template.keys.values() {
            if key.email.to_lowercase() == email_lower {
                return UserRole::Participant;
            }
        }
    }
    // Default to WSManager (platform admin)
    UserRole::WSManager
}

/// Check if a wallet is accessible to the current user
/// Participants cannot access Draft (Created/Drafted) wallets
fn is_wallet_accessible(wallet: &Wallet, current_user_email: &str) -> bool {
    let role = derive_user_role(wallet, current_user_email);
    // Participants cannot access Draft wallets
    if matches!(role, UserRole::Participant)
        && matches!(
            wallet.status,
            WalletStatus::Created | WalletStatus::Drafted
        )
    {
        return false;
    }
    true
}

pub fn org_card<'a>(name: String, count: usize, id: Uuid) -> Element<'a, Msg> {
    let wallets = match count {
        0 => "".to_string(),
        1 => "(1 wallet)".to_string(),
        c => format!("({c} wallets)"),
    };
    let content = row![text::h3(name), text::h4_bold(wallets)]
        .spacing(10)
        .align_y(Alignment::End)
        .into();

    let message = Some(Msg::OrgSelected(id));

    menu_entry(content, message)
}

pub fn no_org_card() -> Element<'static, Msg> {
    let content = text::h3("Contact WizardSardine to create an account.").into();
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

    let mut org_list = Column::new()
        .push(title)
        .push(Space::with_height(30))
        .spacing(10)
        .align_x(Alignment::Center)
        .padding(20);
    let orgs = state.backend.get_orgs();
    if orgs.is_empty() {
        org_list = org_list.push(no_org_card());
    } else {
        for (id, org) in &orgs {
            // Count only wallets accessible to this user
            let wallet_count = org
                .wallets
                .iter()
                .filter_map(|wallet_id| state.backend.get_wallet(*wallet_id))
                .filter(|wallet| is_wallet_accessible(wallet, current_user_email))
                .count();
            let card = org_card(org.name.clone(), wallet_count, *id);
            org_list = org_list.push(card);
        }
    }
    org_list = org_list.push(Space::with_height(50));

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

    let role_badge = if is_ws_manager {
        Some("WS Manager")
    } else {
        None
    };

    layout(
        (3, 4),
        Some(current_user_email),
        role_badge,
        "Organization",
        org_list,
        true,
        None,
    )
}
