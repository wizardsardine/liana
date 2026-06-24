pub mod modal;

use crate::{
    backend::Backend,
    state::{Msg, State},
};
use iced::{alignment::Horizontal, widget::row, Alignment, Length};
use liana_connect::ws_business::{self, KeyIdentity, KeyType, UserRole, WalletStatus};
use liana_ui::{
    component::{
        button::{btn_add_key, btn_edit_keys, btn_mark_keys_ready},
        card, pill,
        text::{self},
    },
    icon, theme,
    widget::*,
};

use super::{key_kind_label, layout_with_scrollable_list, menu_key_entry};

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
        "—".to_string()
    } else {
        signer
    }
}

fn notice_card(
    variant: fn(Element<'static, Msg>) -> Container<'static, Msg>,
    icon: Element<'static, Msg>,
    body: &'static str,
) -> Element<'static, Msg> {
    let content = row![icon, text::new::caption(body).style(theme::text::primary),]
        .spacing(11)
        .align_y(Alignment::Start);

    variant(content.into()).width(Length::Fill).into()
}

fn notice_content(is_manager: bool, keys_ready: bool, locked: bool) -> Element<'static, Msg> {
    let mut cards = Vec::new();

    if is_manager {
        if keys_ready {
            cards.push(notice_card(
                card::success,
                icon::check_icon().style(theme::text::success).into(),
                if locked {
                    "Keys & signers marked as ready. The spending policy will be crafted from these keys."
                } else {
                    "Keys & signers marked as ready. The spending policy will be crafted from these keys. You can still edit keys if anything needs to change."
                },
            ));
        } else {
            cards.push(card::info(
                "List the keys that will be part of this wallet and assign a signer to each. The spending policy will be crafted from these keys."
            ).into());
        }
    } else if keys_ready {
        cards.push(notice_card(
            card::success,
            icon::check_icon()
                .size(16)
                .style(theme::text::success)
                .into(),
            "Marked as ready by the Wallet Manager. They've finished adding keys & signers.",
        ));
        cards.push(card::info(
            "These keys are shared with the Spending policy tab, where you arrange them into spending paths."
        ).into());
    } else {
        cards.push(notice_card(
            card::soft_warning,
            icon::tooltip_icon()
                .size(16)
                .style(theme::text::warning)
                .into(),
            "Awaiting the Wallet Manager. They haven't marked the keys & signers as ready yet.",
        ));
    }

    Column::with_children(cards).spacing(12).into()
}

fn keys_visualization(state: &State, editable: bool) -> Element<'static, Msg> {
    let key_rows: Vec<Element<'static, Msg>> = state
        .app
        .keys
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
            )
        })
        .collect();

    Container::new(Column::with_children(key_rows).spacing(12))
        .width(Length::Fill)
        .into()
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

    if is_manager && !keys_ready {
        let enough_keys = key_count >= 2;
        let hint: Option<Element<'static, Msg>> = (!enough_keys).then_some({
            Container::new(
                text::new::caption("Add at least 2 keys to continue").style(theme::text::secondary),
            )
            .width(Length::Shrink)
            .into()
        });
        let footer = row![
            hint,
            btn_mark_keys_ready(enough_keys.then_some(Msg::MarkKeysReady(true)),)
        ]
        .spacing(16)
        .align_y(Alignment::Center);

        return Some(
            Container::new(footer)
                .width(Length::Fill)
                .align_x(Horizontal::Right)
                .padding(20)
                .into(),
        );
    }

    if is_manager && keys_ready {
        let footer =
            row![btn_edit_keys(Some(Msg::MarkKeysReady(false)))].align_y(Alignment::Center);
        return Some(
            Container::new(footer)
                .width(Length::Fill)
                .align_x(Horizontal::Right)
                .padding(20)
                .into(),
        );
    }

    if !keys_ready {
        return Some(
            Container::new(
                text::new::caption("These keys are shared with the Spending policy tab")
                    .style(theme::text::secondary),
            )
            .width(Length::Fill)
            .center_x(Length::Fill)
            .padding(20)
            .into(),
        );
    }

    None
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
    let editable = !(locked || (is_manager && state.app.keys_ready));

    let header_content = notice_content(is_manager, state.app.keys_ready, locked);
    let keys_list = keys_visualization(state, editable);
    let pinned_content = editable.then_some(btn_add_key(Some(Msg::KeyAdd)).into());
    let footer_content = footer_content(
        is_manager,
        locked,
        state.app.keys_ready,
        state.app.keys.len(),
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
    let breadcrumb = vec![org_name, wallet_name, "Keys".to_string()];

    layout_with_scrollable_list(
        (0, 0),
        Some(current_user_email),
        is_ws_admin,
        &breadcrumb,
        header_content,
        keys_list,
        pinned_content,
        footer_content,
        true,
        Some(Msg::NavigateBack),
    )
}
