use crate::{
    backend::Backend,
    state::{Msg, State},
};
use iced::{
    widget::{checkbox, row, Space},
    Alignment, Length,
};
use liana_connect::ws_business::{KeyIdentity, UserRole, Wallet, WalletStatus};
use liana_ui::{
    component::{form, text},
    theme,
    widget::*,
};

use uuid::Uuid;

use super::{format_last_edit_info, layout_with_scrollable_list, menu_entry};

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
    // Check if user is a signer (has keys with matching email)
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

/// Fixed width for status badges to ensure alignment
const STATUS_BADGE_WIDTH: f32 = 80.0;

/// Render a colored status badge for wallet status
fn status_badge(status: &WalletStatus) -> Element<'static, Msg> {
    match status {
        WalletStatus::Created | WalletStatus::Drafted => Container::new(text::caption("Draft"))
            .padding([4, 12])
            .width(STATUS_BADGE_WIDTH)
            .center_x(STATUS_BADGE_WIDTH)
            .style(theme::pill::simple)
            .into(),
        WalletStatus::Locked => Container::new(text::caption("Locked"))
            .padding([4, 12])
            .width(STATUS_BADGE_WIDTH)
            .center_x(STATUS_BADGE_WIDTH)
            .style(theme::pill::simple)
            .into(),
        WalletStatus::Validated => Container::new(text::caption("Set keys"))
            .padding([4, 12])
            .width(STATUS_BADGE_WIDTH)
            .center_x(STATUS_BADGE_WIDTH)
            .style(theme::pill::warning)
            .into(),
        WalletStatus::Finalized => Container::new(text::caption("Active"))
            .padding([4, 12])
            .width(STATUS_BADGE_WIDTH)
            .center_x(STATUS_BADGE_WIDTH)
            .style(theme::pill::success)
            .into(),
    }
}

/// Get a display label for the user role
fn role_label(role: &UserRole) -> &'static str {
    match role {
        UserRole::WizardSardineAdmin => "Manager",
        UserRole::WalletManager => "Owner",
        UserRole::Participant => "Participant",
    }
}

/// Get sort priority for wallet status (lower = shown first)
/// Order: Draft (0) -> Locked (1) -> Validated (2) -> Finalized (3)
fn status_sort_priority(status: &WalletStatus) -> u8 {
    match status {
        WalletStatus::Created | WalletStatus::Drafted => 0,
        WalletStatus::Locked => 1,
        WalletStatus::Validated => 2,
        WalletStatus::Finalized => 3,
    }
}

pub fn wallet_card<'a>(
    alias: String,
    key_count: usize,
    status: &WalletStatus,
    role: &UserRole,
    id: Uuid,
    last_edit_info: Option<String>,
) -> Element<'a, Msg> {
    let keys = match key_count {
        0 => "".to_string(),
        1 => "(1 key)".to_string(),
        c => format!("({c} keys)"),
    };

    // Left side: wallet name, key count, and edit info
    let mut left_col = Column::new()
        .push(text::h3(alias))
        .push(text::p1_regular(keys))
        .spacing(4);

    if let Some(info) = last_edit_info {
        left_col = left_col.push(text::caption(info).style(liana_ui::theme::text::secondary));
    }

    // Right side: status badge and role label
    // Don't show "Manager" role - it's already in the header for WSManager users
    let mut right_col = Column::new()
        .push(status_badge(status))
        .spacing(4)
        .width(STATUS_BADGE_WIDTH)
        .align_x(Alignment::Center);

    // Only show role for Owner and Participant (not WSManager)
    if !matches!(role, UserRole::WizardSardineAdmin) {
        right_col = right_col.push(text::p2_regular(role_label(role)));
    }

    let content = Row::new()
        .push(left_col)
        .push(Space::with_width(Length::Fill))
        .push(right_col)
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .into();

    let message = Some(Msg::OrgWalletSelected(id));

    menu_entry(content, message)
}

pub fn wallet_select_view(state: &State) -> Element<'_, Msg> {
    // Determine if there are wallets and get wallet count
    let has_wallets = if let Some(org_id) = state.app.selected_org {
        if let Some(org) = state.backend.get_org(org_id) {
            !org.wallets.is_empty()
        } else {
            false
        }
    } else {
        false
    };

    // Set title based on whether wallets exist
    let title_text = if has_wallets {
        "Select wallet"
    } else {
        "Create a wallet"
    };
    let title = text::h2(title_text);
    let title = row![
        Space::with_width(Length::Fill),
        title,
        Space::with_width(Length::Fill),
    ];

    // Get current user email for role derivation
    let current_user_email = &state.views.login.email.form.value;
    let hide_finalized = state.views.wallet_select.hide_finalized;

    // Determine if user is WizardSardineManager for ALL wallets in this org
    // (not owner and not signer of any wallet)
    let is_ws_manager = if let Some(org_id) = state.app.selected_org {
        if let Some(org) = state.backend.get_org(org_id) {
            let email_lower = current_user_email.to_lowercase();
            let mut is_owner_or_signer = false;

            'outer: for wallet_id in &org.wallets {
                if let Some(wallet) = state.backend.get_wallet(*wallet_id) {
                    // Check if owner
                    if let Some(owner) = state.backend.get_user(wallet.owner) {
                        if owner.email.to_lowercase() == email_lower {
                            is_owner_or_signer = true;
                            break 'outer;
                        }
                    }
                    // Check if signer (has matching key)
                    if let Some(template) = &wallet.template {
                        for key in template.keys.values() {
                            if let KeyIdentity::Email(key_email) = &key.identity {
                                if key_email.to_lowercase() == email_lower {
                                    is_owner_or_signer = true;
                                    break 'outer;
                                }
                            }
                        }
                    }
                }
            }
            !is_owner_or_signer
        } else {
            false
        }
    } else {
        false
    };

    // Fixed header content: title, filter checkbox, and search bar
    let mut header_content = Column::new()
        .push(title)
        .push(Space::with_height(30))
        .spacing(10)
        .align_x(Alignment::Center)
        .padding(20);

    // Add filter checkbox for WSManager users (centered)
    if is_ws_manager && has_wallets {
        let filter_checkbox = Row::new()
            .push(Space::with_width(Length::Fill))
            .push(
                checkbox("Hide finalized wallets", hide_finalized)
                    .on_toggle(Msg::WalletSelectToggleHideFinalized),
            )
            .push(Space::with_width(Length::Fill))
            .width(Length::Fill);
        header_content = header_content.push(filter_checkbox);
        header_content = header_content.push(Space::with_height(10));
    }

    // Add search bar for all users when there are wallets
    if has_wallets {
        let search_value = form::Value {
            value: state.views.wallet_select.search_filter.clone(),
            warning: None,
            valid: true,
        };
        let search_form = form::Form::new_trimmed(
            "Search wallets...",
            &search_value,
            Msg::WalletSelectUpdateSearchFilter,
        )
        .size(16)
        .padding(10);
        let search_container = Container::new(search_form)
            .width(Length::Fixed(500.0))
            .align_x(Alignment::Center);
        header_content = header_content.push(search_container);
        header_content = header_content.push(Space::with_height(10));
    }

    // Scrollable list content: wallet cards
    let mut list_content = Column::new()
        .spacing(10)
        .align_x(Alignment::Center)
        .padding([0, 20]);

    // Filter wallets by search text (case-insensitive)
    let search_filter = state.views.wallet_select.search_filter.to_lowercase();

    // Get the user's global role (from User record, not per-wallet)
    let global_role = state.app.global_user_role;

    if has_wallets {
        if let Some(org_id) = state.app.selected_org {
            if let Some(org) = state.backend.get_org(org_id) {
                // Collect wallets with their derived roles, filtering out inaccessible ones
                let mut wallets_to_display: Vec<_> = org
                    .wallets
                    .iter()
                    .filter_map(|wallet_id| {
                        let wallet = state.backend.get_wallet(*wallet_id)?;
                        let owner_email = state
                            .backend
                            .get_user(wallet.owner)
                            .map(|u| u.email.clone());
                        // Get role for this wallet (None = no access)
                        let role = derive_user_role(
                            &wallet,
                            owner_email.as_deref(),
                            current_user_email,
                            global_role,
                        )?;

                        // Signers should NOT see Draft or Locked wallets
                        let is_draft_or_locked = matches!(
                            wallet.status,
                            WalletStatus::Created | WalletStatus::Drafted | WalletStatus::Locked
                        );
                        if is_draft_or_locked && role == UserRole::Participant {
                            return None; // Skip this wallet for signers
                        }

                        // WizardSardineManager: optionally hide finalized wallets
                        if is_ws_manager
                            && hide_finalized
                            && matches!(wallet.status, WalletStatus::Finalized)
                        {
                            return None;
                        }

                        // Filter by search text (case-insensitive)
                        if !search_filter.is_empty()
                            && !wallet.alias.to_lowercase().contains(&search_filter)
                        {
                            return None;
                        }

                        Some((*wallet_id, wallet, role))
                    })
                    .collect();

                // Sort by status: Draft first, Finalized last
                wallets_to_display
                    .sort_by_key(|(_, wallet, _)| status_sort_priority(&wallet.status));

                // Show message when search filter returns no results
                if wallets_to_display.is_empty() && !search_filter.is_empty() {
                    list_content = list_content
                        .push(text::p1_regular("No wallets found matching your search."));
                } else {
                    let current_user_email_lower = current_user_email.to_lowercase();
                    // Render sorted wallets
                    for (id, wallet, role) in wallets_to_display {
                        let key_count = wallet.template.as_ref().map(|t| t.keys.len()).unwrap_or(0);

                        let last_edit_info = format_last_edit_info(
                            wallet.last_edited,
                            wallet.last_editor,
                            state,
                            &current_user_email_lower,
                        );

                        let card = wallet_card(
                            wallet.alias.clone(),
                            key_count,
                            &wallet.status,
                            &role,
                            id,
                            last_edit_info,
                        );
                        list_content = list_content.push(card);
                    }
                }
            }
        }
    } else {
        // FIXME: doe we display something if no wallet?
    }

    list_content = list_content.push(Space::with_height(50));

    let role_badge = if is_ws_manager {
        Some("WS Manager")
    } else {
        None
    };

    // Build breadcrumb: org_name > Wallets
    let org_name = state
        .app
        .selected_org
        .and_then(|org_id| state.backend.get_org(org_id))
        .map(|org| org.name.clone())
        .unwrap_or_else(|| "Organization".to_string());
    let breadcrumb = vec![org_name, "Wallets".to_string()];

    layout_with_scrollable_list(
        (4, 4),
        Some(&state.views.login.email.form.value),
        role_badge,
        &breadcrumb,
        header_content,
        list_content,
        None, // footer_content
        true,
        Some(Msg::NavigateBack),
    )
}
