use crate::{
    backend::Backend,
    state::{message::Msg, State},
};
use iced::{
    alignment::Horizontal,
    widget::{column, row, Space},
    Alignment, Length,
};
use liana_connect::ws_business::{UserRole, WalletStatus};
use liana_ui::{
    component::{
        button::{
            self, btn_add_recovery_path, btn_approve, btn_send_for_approval, btn_unlock, EntryWidth,
        },
        card, text, tooltip,
    },
    spacing::VSpacing,
    theme,
    widget::*,
};

use super::{layout_with_scrollable_list, wallet_edit::wallet_edit_tab_header, INSTALLER_STEPS};

pub mod entry_path_list;
pub mod modal;

pub use entry_path_list::path_list;

fn banner_card(
    variant: fn(Element<'static, Msg>) -> Container<'static, Msg>,
    icon_style: fn(&theme::Theme) -> iced::widget::text::Style,
    body: &'static str,
) -> Element<'static, Msg> {
    let content = row![
        tooltip::tooltip_with_style(body, icon_style),
        text::new::caption(body).style(icon_style),
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    variant(content.into()).width(Length::Fill).into()
}

fn header_content(
    is_ws_admin: bool,
    is_manager: bool,
    is_locked: bool,
) -> Option<Element<'static, Msg>> {
    if is_locked {
        return Some(if is_manager {
            banner_card(
                card::soft_warning,
                theme::text::warning,
                "Template is locked and pending approval. You must approve it to continue.",
            )
        } else {
            card::info("Template is locked and pending approval. Unlock to make further changes.")
                .into()
        });
    }

    if is_manager && !is_ws_admin {
        return Some(card::info("Read-only. Only a WS Admin can edit this template.").into());
    }

    None
}

fn footer_content(
    is_ws_admin: bool,
    is_manager: bool,
    is_locked: bool,
    can_send_for_approval: bool,
) -> Option<Element<'static, Msg>> {
    let footer: Element<'static, Msg> = if is_ws_admin && !is_locked {
        btn_send_for_approval(can_send_for_approval.then_some(Msg::TemplateLock)).into()
    } else if is_ws_admin && is_locked {
        btn_unlock(Some(Msg::TemplateUnlock)).into()
    } else if is_manager && is_locked {
        let help = button::btn_template_help(Some(Msg::TemplateHelpShowModal));
        column![btn_approve(Some(Msg::TemplateValidate)), help]
            .spacing(VSpacing::M)
            .align_x(Horizontal::Center)
            .into()
    } else {
        return None;
    };

    Some(
        Container::new(footer)
            .center_x(Length::Fill)
            .padding(20)
            .into(),
    )
}

pub fn template_builder_view(state: &State) -> Element<'_, Msg> {
    let current_user_email = &state.views.login.email.form.value;

    let is_ws_admin = matches!(
        state.app.current_user_role,
        Some(UserRole::WizardSardineAdmin)
    );
    let is_manager = matches!(state.app.current_user_role, Some(UserRole::WalletManager));

    let wallet_status = state
        .app
        .selected_wallet
        .and_then(|id| state.backend.get_wallet(id))
        .map(|w| w.status);

    let is_locked = matches!(wallet_status, Some(WalletStatus::Locked));
    let editable = is_ws_admin && !is_locked;

    let list_content = path_list(state, editable);
    let header_content = column![
        wallet_edit_tab_header(state),
        header_content(is_ws_admin, is_manager, is_locked)
    ]
    .align_x(Alignment::Center)
    .width(Length::Fill)
    .spacing(VSpacing::M)
    .into();
    let add_recovery_path = editable.then_some(
        column![
            btn_add_recovery_path(Some(Msg::TemplateNewPathModal)).width(
                if state.app.secondary_paths().is_empty() {
                    EntryWidth::Standard
                } else {
                    EntryWidth::Deletable
                }
            ),
            Space::with_height(VSpacing::M)
        ]
        .into(),
    );
    let footer_content = footer_content(
        is_ws_admin,
        is_manager,
        is_locked,
        state.is_template_valid(),
    );

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
    let breadcrumb = vec![org_name, wallet_name];

    layout_with_scrollable_list(
        (5, INSTALLER_STEPS),
        Some(current_user_email),
        is_ws_admin,
        &breadcrumb,
        Some(header_content),
        list_content,
        add_recovery_path,
        footer_content,
        Some(Msg::NavigateBack),
    )
}
