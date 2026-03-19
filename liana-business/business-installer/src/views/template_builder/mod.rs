use crate::{
    backend::Backend,
    state::{message::Msg, State},
};
use iced::{Alignment, Length};
use liana_connect::ws_business::{UserRole, WalletStatus};
use liana_ui::{
    component::button::{btn_primary, btn_secondary, BtnWidth},
    icon,
    widget::*,
};

use super::layout_with_scrollable_list;

pub mod template_visualization;

pub use template_visualization::template_visualization;

pub fn template_builder_view(state: &State) -> Element<'_, Msg> {
    let current_user_email = &state.views.login.email.form.value;

    // Determine user role from AppState
    let is_ws_manager = matches!(
        state.app.current_user_role,
        Some(UserRole::WizardSardineAdmin)
    );
    let is_owner = matches!(state.app.current_user_role, Some(UserRole::WalletManager));

    // Get current wallet status
    let wallet_status = state
        .app
        .selected_wallet
        .and_then(|id| state.backend.get_wallet(id))
        .map(|w| w.status);

    let is_draft = matches!(
        wallet_status,
        Some(WalletStatus::Created) | Some(WalletStatus::Drafted)
    );
    let is_locked = matches!(wallet_status, Some(WalletStatus::Locked));

    // Template visualization as scrollable content
    let visualization = template_visualization(state);

    // Action buttons row (fixed at bottom) - role-based and status-based
    let mut buttons_row = Row::new().spacing(20).align_y(Alignment::Center);

    // "Manage Keys" button: WS Admin or Wallet Manager, only on Draft status
    // Once the wallet is Locked/Validated/Finalized, keys cannot be managed
    if is_draft {
        let icon = Some(icon::key_icon());
        if is_ws_manager {
            buttons_row = buttons_row.push(btn_secondary(
                icon,
                "Manage Keys",
                BtnWidth::XL,
                Some(Msg::NavigateToKeys),
            ));
        } else if is_owner {
            buttons_row = buttons_row.push(btn_primary(
                icon,
                "Manage Keys",
                BtnWidth::XL,
                Some(Msg::NavigateToKeys),
            ));
        }
    }

    // WS Admin on Draft: "Lock Template" (if valid)
    if is_ws_manager && is_draft {
        let is_valid = state.is_template_valid();
        let lock_button = btn_primary(
            None,
            "Send for approval",
            BtnWidth::XL,
            is_valid.then_some(Msg::TemplateLock),
        );
        buttons_row = buttons_row.push(lock_button);
    }

    // WS Admin on Locked: "Unlock" button
    if is_ws_manager && is_locked {
        buttons_row = buttons_row.push(btn_secondary(
            None,
            "Unlock",
            BtnWidth::M,
            Some(Msg::TemplateUnlock),
        ));
    }

    // Wallet Manager on Locked: "Approve Template" button
    if is_owner && is_locked {
        buttons_row = buttons_row.push(btn_primary(
            None,
            "Approve Template",
            BtnWidth::XL,
            Some(Msg::TemplateValidate),
        ));
    }

    let footer_content: Element<'_, Msg> = Container::new(buttons_row)
        .width(Length::Fill)
        .center_x(Length::Fill)
        .padding(20)
        .into();

    let role_badge = if is_ws_manager {
        Some("WS Admin")
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
