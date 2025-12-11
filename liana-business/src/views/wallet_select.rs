use crate::{
    backend::Backend,
    state::{Msg, State},
};
use iced::{widget::row, Alignment, Length};
use liana_ui::{component::text, widget::*};

use iced::widget::Space;
use uuid::Uuid;

use super::{layout, menu_entry};

pub fn wallet_card<'a>(alias: String, key_count: usize, id: Uuid) -> Element<'a, Msg> {
    let keys = match key_count {
        0 => "".to_string(),
        1 => "(1 key)".to_string(),
        c => format!("({c} keys)"),
    };
    let content = row![text::h3(alias), text::h4_bold(keys)]
        .spacing(10)
        .align_y(Alignment::End)
        .into();

    let message = Some(Msg::OrgWalletSelected(id));

    menu_entry(content, message)
}

pub fn create_wallet_card() -> Element<'static, Msg> {
    let content = row![text::h4_regular("+ Create wallet")]
        .spacing(10)
        .align_y(Alignment::End)
        .into();

    let message = Some(Msg::OrgCreateNewWallet);

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

    let mut wallet_list = Column::new()
        .push(title)
        .push(Space::with_height(30))
        .spacing(10)
        .align_x(Alignment::Center)
        .padding(20);

    if has_wallets {
        if let Some(org_id) = state.app.selected_org {
            if let Some(org) = state.backend.get_org(org_id) {
                for (id, wallet) in &org.wallets {
                    let key_count = wallet.template.as_ref().map(|t| t.keys.len()).unwrap_or(0);
                    let card = wallet_card(wallet.alias.clone(), key_count, *id);
                    wallet_list = wallet_list.push(card);
                }
            }
        }
    } else {
        wallet_list = wallet_list.push(create_wallet_card());
    }

    wallet_list = wallet_list.push(Space::with_height(50));

    layout(
        (4, 4),
        Some(&state.views.login.email.form.value),
        "Wallet",
        wallet_list,
        true,
        Some(Msg::NavigateBack),
    )
}
