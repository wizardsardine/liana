use crate::{
    backend::Backend,
    state::{Msg, State},
};
use iced::{
    widget::{column, row, Space},
    Alignment, Length,
};
use liana_connect::ws_business::{KeyIdentity, UserRole, Wallet, WalletStatus};
use liana_ui::{
    component::{
        list,
        text::{self, truncate},
    },
    theme,
    widget::*,
};

use uuid::Uuid;

use super::{
    menu_entry, select_list_view, SelectListView, SelectSearch, INSTALLER_STEPS,
    SEARCH_ENTRY_THRESHOLD,
};

/// Derive the user's role for a specific wallet based on wallet data and global role
/// Returns None if the user has no access to this wallet
fn derive_user_role(
    wallet: &Wallet,
    owner_email: Option<&str>,
    current_user_email: &str,
    global_role: Option<UserRole>,
) -> Option<UserRole> {
    // WizardSardineManager has access to all wallets
    if matches!(global_role, Some(UserRole::WizardSardineAdmin)) {
        return Some(UserRole::WizardSardineAdmin);
    }

    let email_lower = current_user_email.to_lowercase();
    // Check if user is wallet owner
    if let Some(owner) = owner_email {
        if owner.to_lowercase() == email_lower {
            return Some(UserRole::WalletManager);
        }
    }
    // Check if user is a participant (has keys with matching email)
    if let Some(template) = &wallet.template {
        for key in template.keys.values() {
            if let KeyIdentity::Email(key_email) = &key.identity {
                if key_email.to_lowercase() == email_lower {
                    return Some(UserRole::Participant);
                }
            }
        }
    }
    // User has no access to this wallet
    None
}

/// Check if a wallet is accessible to the current user
/// Signers cannot access Draft (Created/Drafted/Locked) wallets
fn is_wallet_accessible(
    wallet: &Wallet,
    owner_email: Option<&str>,
    current_user_email: &str,
    global_role: Option<UserRole>,
) -> bool {
    let role = match derive_user_role(wallet, owner_email, current_user_email, global_role) {
        Some(r) => r,
        None => return false, // No access
    };
    // Signers cannot access Draft or Locked wallets
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

pub fn org_card<'a>(name: String, id: Uuid, subtitle: Option<String>) -> Element<'a, Msg> {
    let name = truncate(&name, 30);
    let trailing = list::entry_chevron();

    let message = Some(Msg::OrgSelected(id));

    list::entry_organization(name, subtitle, Some(trailing), message)
}

pub fn no_org_card() -> Container<'static, Msg> {
    let content = row![
        Space::fill_width(),
        text::h5_regular("Contact Wizardsardine to create an account."),
        Space::fill_width(),
    ]
    .align_y(Alignment::Center)
    .width(Length::Fill)
    .height(Length::Fill);
    menu_entry(content, None)
}

pub fn org_select_view(state: &State) -> Element<'_, Msg> {
    let current_user_email = &state.views.login.email.form.value;
    let orgs = state.backend.get_orgs();

    // Determine if user is WSAdmin (use global role from User record)
    let is_ws_admin = matches!(
        state.app.global_user_role,
        Some(UserRole::WizardSardineAdmin)
    );

    // Scrollable list content: organization cards
    let mut list_content = column![]
        .spacing(12)
        .width(Length::Fill)
        .align_x(Alignment::Center);

    // Filter organizations by search text (case-insensitive)
    let search_filter = state.views.org_select.search_filter.to_lowercase();
    let filtered_orgs: Vec<_> = orgs
        .iter()
        .filter(|(_, org)| {
            if is_ws_admin && !search_filter.is_empty() {
                org.name.to_lowercase().contains(&search_filter)
            } else {
                true
            }
        })
        .collect();

    if filtered_orgs.is_empty() && !orgs.is_empty() {
        // Show message when search filter returns no results
        list_content = list_content.push(
            text::new::caption("No organizations found matching your search.")
                .style(theme::text::secondary),
        );
    } else if orgs.is_empty() {
        list_content = list_content.push(no_org_card());
    } else {
        // Use global role from User record for filtering
        let global_role = state.app.global_user_role;
        // Get hide_finalized setting for WS Admin wallet count filtering
        let hide_finalized = state.views.wallet_select.hide_finalized;
        for (id, org) in &filtered_orgs {
            // Count only wallets accessible to this user
            let wallet_count = org
                .wallets
                .iter()
                .filter_map(|wallet_id| state.backend.get_wallet(*wallet_id))
                .filter(|wallet| {
                    let owner_email = state
                        .backend
                        .get_user(wallet.owner)
                        .map(|u| u.email.clone());
                    is_wallet_accessible(
                        wallet,
                        owner_email.as_deref(),
                        current_user_email,
                        global_role,
                    )
                })
                // WizardSardineManager: optionally hide finalized wallets (match wallet_select filtering)
                .filter(|wallet| {
                    !(is_ws_admin
                        && hide_finalized
                        && matches!(
                            wallet.effective_status(current_user_email),
                            WalletStatus::Finalized
                        ))
                })
                .count();

            // Hide orgs with no accessible wallets
            if wallet_count == 0 {
                continue;
            }

            let wallet_label = format!(
                "{wallet_count} wallet{}",
                if wallet_count == 1 { "" } else { "s" }
            );

            let card = org_card(org.name.clone(), **id, Some(wallet_label));
            list_content = list_content.push(card);
        }
    }

    select_list_view(SelectListView {
        progress: (3, INSTALLER_STEPS),
        email: current_user_email,
        is_ws_admin,
        breadcrumb: vec!["Organizations".to_string()],
        title: "Organizations".to_string(),
        search: (is_ws_admin && orgs.len() > SEARCH_ENTRY_THRESHOLD).then_some(SelectSearch {
            placeholder: "Filter organizations...",
            value: &state.views.org_select.search_filter,
            on_change: Msg::OrgSelectUpdateSearchFilter,
        }),
        list: list_content,
        previous_message: None,
    })
}
