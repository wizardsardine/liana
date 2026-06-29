pub mod modal;

use crate::{
    backend::Backend,
    state::{Msg, State},
};
use iced::{
    widget::{column, row, Space},
    Alignment, Length, Padding,
};
use liana_connect::ws_business::{self, KeyIdentity, KeyType, UserRole, WalletStatus};
use liana_ui::{
    component::{
        button::{self, btn_add_key, btn_edit_keys, btn_mark_keys_ready, EntryWidth},
        card, pill,
        text::{self},
    },
    icon, theme,
    widget::*,
};

use super::{
    key_kind_label, layout_with_scrollable_list, menu_key_entry,
    wallet_edit::wallet_edit_tab_header, INSTALLER_STEPS,
};

fn key_signer(key: &ws_business::Key) -> String {
    let signer = match &key.key_type {
        KeyType::Cosigner | KeyType::SafetyNet => match &key.identity {
            KeyIdentity::TokenWithProvider {
                provider: Some(provider),
                ..
            } => provider.name.clone(),
            _ => "Professional service".to_string(),
        },
        _ => key.identity.to_string(),
    };

    if signer.is_empty() {
        "-".to_string()
    } else {
        signer
    }
}

const NOTICE_ICON_SIZE: u32 = 16;

fn notice_card(
    variant: fn(Element<'static, Msg>) -> Container<'static, Msg>,
    icon: Text<'static>,
    body: &'static str,
) -> Element<'static, Msg> {
    let content = row![
        icon.size(NOTICE_ICON_SIZE),
        text::new::caption(body).style(theme::text::primary),
    ]
    .spacing(11)
    .align_y(Alignment::Center);

    variant(content.into()).width(Length::Fill).into()
}

fn notice_content(is_manager: bool, keys_ready: bool, locked: bool) -> Element<'static, Msg> {
    let cards = if is_manager {
        if keys_ready {
            column![notice_card(
                card::success,
                icon::check_icon().style(theme::text::success),
                if locked {
                    "Keys & signers marked as ready. The spending policy will be crafted from these keys."
                } else {
                    "Keys & signers marked as ready. The spending policy will be crafted from these keys. You can still edit keys if anything needs to change."
                },
            )]
        } else {
            column![card::info(
                "List the keys that will be part of this wallet and assign a signer to each. The spending policy will be crafted from these keys."
            )]
        }
    } else if keys_ready {
        column![
            notice_card(
                card::success,
                icon::check_icon().style(theme::text::success),
                "Marked as ready by the Wallet Manager. They've finished adding keys & signers.",
            ),
            card::info(
                "These keys are shared with the Spending policy tab, where you arrange them into spending paths."
            ),
        ]
    } else {
        column![notice_card(
            card::soft_warning,
            icon::tooltip_icon().style(theme::text::warning),
            "Awaiting the Wallet Manager. They haven't marked the keys & signers as ready yet.",
        )]
    };

    cards.spacing(12).width(Length::Fill).into()
}

fn keys_list(state: &State, editable: bool) -> Element<'static, Msg> {
    let keys_column = state
        .app
        .keys()
        .iter()
        .map(|(key_id, key)| {
            let signer = key_signer(key);
            let trailing: Element<'static, Msg> = if editable {
                icon::pencil_icon()
                    .size(16)
                    .style(theme::text::tertiary)
                    .into()
            } else {
                pill::signer_assigned().into()
            };

            menu_key_entry(
                key,
                signer,
                pill::key_kind(
                    super::entry_key_kind(&key.key_type),
                    key_kind_label(&key.key_type),
                )
                .into(),
                trailing,
                editable.then_some(Msg::KeyEdit(*key_id)),
                editable.then_some(Msg::KeyDelete(*key_id)),
            )
        })
        .fold(column![], |col, entry| col.push(entry))
        .spacing(12);

    Container::new(keys_column).width(Length::Fill).into()
}

fn footer_content(
    is_manager: bool,
    locked: bool,
    keys_ready: bool,
    key_count: usize,
) -> Option<Element<'static, Msg>> {
    if is_manager && locked {
        return None;
    }

    let content = if is_manager && !keys_ready {
        let enough_keys = key_count >= 2;
        let hint: Option<Element<'static, Msg>> = (!enough_keys).then_some({
            Container::new(
                text::new::caption("Add at least 2 keys to continue").style(theme::text::secondary),
            )
            .width(Length::Shrink)
            .into()
        });
        let mark_ready = btn_mark_keys_ready(enough_keys.then_some(Msg::MarkKeysReady(true)));
        let footer = if let Some(hint) = hint {
            row![hint, mark_ready]
        } else {
            row![mark_ready]
        }
        .spacing(16)
        .align_y(Alignment::Center);
        footer
    } else if is_manager && keys_ready {
        row![btn_edit_keys(Some(Msg::MarkKeysReady(false)))].align_y(Alignment::Center)
    } else if !keys_ready {
        row![
            text::new::caption("These keys are shared with the Spending policy tab")
                .style(theme::text::secondary)
        ]
    } else {
        return None;
    };

    Some(
        Container::new(content)
            .center_x(Length::Fill)
            .padding(Padding {
                top: 0.0,
                right: 20.0,
                bottom: 20.0,
                left: 20.0,
            })
            .into(),
    )
}

pub fn keys_view(state: &State) -> Element<'_, Msg> {
    let current_user_email = &state.views.login.email.form.value;
    let is_ws_admin = matches!(
        state.app.current_user_role,
        Some(UserRole::WizardSardineAdmin)
    );
    let is_manager = matches!(state.app.current_user_role, Some(UserRole::WalletManager));
    let wallet_status = state
        .app
        .selected_wallet
        .and_then(|wallet_id| state.backend.get_wallet(wallet_id))
        .map(|wallet| wallet.status);
    let locked = matches!(wallet_status, Some(WalletStatus::Locked));
    let editable = !(locked || (is_manager && state.app.keys_ready()));

    let header_content = column![
        wallet_edit_tab_header(state),
        notice_content(is_manager, state.app.keys_ready(), locked),
    ]
    .width(button::STANDARD_ENTRY_WIDTH)
    .align_x(Alignment::Center)
    .into();
    let keys_list = keys_list(state, editable);
    let add_key = editable.then_some(
        column![
            btn_add_key(Some(Msg::KeyAdd)).width(if state.app.keys().is_empty() {
                EntryWidth::Standard
            } else {
                EntryWidth::Deletable
            }),
            Space::with_height(12)
        ]
        .into(),
    );
    let footer_content = footer_content(
        is_manager,
        locked,
        state.app.keys_ready(),
        state.app.keys().len(),
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
        keys_list,
        add_key,
        footer_content,
        Some(Msg::NavigateBack),
    )
}
