use crate::{
    state::{Msg, State},
    views::{layout_with_scrollable_list, INSTALLER_STEPS},
};
use iced::{
    widget::{row, Space},
    Alignment, Length,
};
use liana_ui::{
    component::{button::btn_connect_another_email, list, text},
    widget::*,
};

pub fn account_select_view(state: &State) -> Element<'_, Msg> {
    let accounts = &state.views.login.account_select.accounts;
    let processing = state.views.login.account_select.processing;
    let selected_email = &state.views.login.account_select.selected_email;

    // Header content
    let liana_business = row![
        Space::with_width(Length::Fill),
        text::h2("Liana Business"),
        Space::with_width(Length::Fill),
    ];

    let select_account_text = row![
        Space::with_width(Length::Fill),
        text::h3("Select an account to continue"),
        Space::with_width(Length::Fill),
    ];

    let header_content = Column::new()
        .push(liana_business)
        .push(Space::with_height(30))
        .push(select_account_text)
        .push(Space::with_height(30));

    // Scrollable list of accounts
    let mut list_content = Column::new().spacing(15).align_x(iced::Alignment::Center);

    // Card for each cached account with delete button
    for account in accounts {
        let is_selected = selected_email
            .as_ref()
            .map(|e| e == &account.email)
            .unwrap_or(false);

        let title = if processing && is_selected {
            "Connecting...".to_string()
        } else {
            liana_ui::component::text::short_email(&account.email, 55)
        };

        let card_message = if processing {
            None
        } else {
            Some(Msg::AccountSelectConnect(account.email.clone()))
        };

        let delete_msg = if processing {
            None
        } else {
            Some(Msg::AccountSelectDelete(account.email.clone()))
        };

        list_content = list_content.push(list::account_entry(title, card_message, delete_msg));
    }

    // Separator
    list_content = list_content.push(Space::with_height(20));

    let new_email = btn_connect_another_email((!processing).then_some(Msg::AccountSelectNewEmail));

    // Wrap in a container to maintain alignment with account rows
    let new_email_row = Row::new()
        .spacing(15)
        .align_y(Alignment::Center)
        .push(new_email)
        .push(Space::with_width(Length::Fixed(50.0))); // Match delete button width

    list_content = list_content.push(new_email_row);

    layout_with_scrollable_list(
        (1, INSTALLER_STEPS),
        None,
        false,
        &["Login".to_string()],
        header_content,
        list_content,
        None, // no footer
        true,
        None,
    )
}
