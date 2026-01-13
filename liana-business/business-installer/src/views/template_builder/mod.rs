use crate::{
    backend::Backend,
    state::{message::Msg, State},
};
use iced::{Alignment, Length};
use liana_connect::models::{UserRole, WalletStatus};
use liana_ui::{component::button, icon, widget::*};

use super::layout_with_scrollable_list;

pub mod template_visualization;

pub use template_visualization::template_visualization;

pub fn template_builder_view(state: &State) -> Element<'_, Msg> {
    let current_user_email = &state.views.login.email.form.value;

    // Determine user role from AppState
    let is_ws_manager = matches!(state.app.current_user_role, Some(UserRole::WSManager));
    let is_owner = matches!(state.app.current_user_role, Some(UserRole::Owner));

    // Get current wallet status
    let wallet_status = state
        .app
        .selected_wallet
        .and_then(|id| state.backend.get_wallet(id))
        .map(|w| w.status.clone());

    let is_draft = matches!(
        wallet_status,
        Some(WalletStatus::Created) | Some(WalletStatus::Drafted)
    );
    let is_locked = matches!(wallet_status, Some(WalletStatus::Locked));

    // Template visualization as scrollable content
    let visualization = template_visualization(state);

    // Action buttons row (fixed at bottom) - role-based and status-based
    let mut buttons_row = Row::new().spacing(20).align_y(Alignment::Center);

    // "Manage Keys" button: WSManager or Owner, only on Draft status
    // Once the wallet is Locked/Validated/Finalized, keys cannot be managed
    if (is_ws_manager || is_owner) && is_draft {
        buttons_row = buttons_row.push(
            button::secondary(Some(icon::key_icon()), "Manage Keys").on_press(Msg::NavigateToKeys),
        );
    }

    // WSManager on Draft: "Lock Template" (if valid)
    let approval = "Send for approval";
    if is_ws_manager && is_draft {
        let is_valid = state.is_template_valid();
        let lock_button = if is_valid {
            button::primary(None, approval).on_press(Msg::TemplateLock)
        } else {
            button::primary(None, approval)
        };
        buttons_row = buttons_row.push(lock_button);
    }

    // WSManager on Locked: "Unlock" button
    if is_ws_manager && is_locked {
        buttons_row =
            buttons_row.push(button::secondary(None, "Unlock").on_press(Msg::TemplateUnlock));
    }

    // Owner on Locked: "Approve Template" button
    if is_owner && is_locked {
        let validate_button =
            button::primary(None, "Approve Template").on_press(Msg::TemplateValidate);
        buttons_row = buttons_row.push(validate_button);
    }

    let footer_content: Element<'_, Msg> = Container::new(buttons_row)
        .width(Length::Fill)
        .center_x(Length::Fill)
        .padding(20)
        .into();

    let role_badge = if is_ws_manager {
        Some("WS Manager")
    } else {
        None
    };

    // Build breadcrumb: org_name > wallet_name > Template
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
    let breadcrumb = vec![org_name, wallet_name, "Template".to_string()];

    // Empty header content - the visualization goes directly in the scrollable area
    let header_content: Element<'_, Msg> = Column::new().into();

    layout_with_scrollable_list(
        (0, 0), // No progress indicator for template builder
        Some(current_user_email),
        role_badge,
        &breadcrumb,
        header_content,
        visualization,
        Some(footer_content),
        true,
        Some(Msg::NavigateBack),
    )
}
