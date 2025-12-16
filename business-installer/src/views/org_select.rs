use crate::{
    backend::Backend,
    state::{Msg, State},
};
use iced::{widget::row, Alignment, Length};
use liana_ui::{component::text, widget::*};

use iced::widget::Space;
use uuid::Uuid;

use super::{layout, menu_entry};

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
        for (id, org) in orgs {
            let wallet_count = org.wallets.len();
            let card = org_card(org.name.clone(), wallet_count, id);
            org_list = org_list.push(card);
        }
    }
    org_list = org_list.push(Space::with_height(50));

    layout(
        (3, 4),
        Some(&state.views.login.email.form.value),
        "Organization",
        org_list,
        true,
        None,
    )
}
