use crate::{
    state::{Msg, State},
    views::{layout_with_scrollable_list, menu_entry},
};
use iced::{
    widget::{row, Space},
    Length,
};
use liana_ui::{component::text, widget::*};

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

    // Card for each cached account
    for account in accounts {
        let is_selected = selected_email
            .as_ref()
            .map(|e| e == &account.email)
            .unwrap_or(false);

        let card_content: Element<'_, Msg> = if processing && is_selected {
            // Show loading state for selected account
            Row::new()
                .push(Space::with_width(Length::Fill))
                .push(text::p2_regular("Connecting..."))
                .push(Space::with_width(Length::Fill))
                .align_y(iced::Alignment::Center)
                .into()
        } else {
            text::p1_regular(&account.email).into()
        };

        let message = if processing {
            None // Disable all cards while connecting
        } else {
            Some(Msg::AccountSelectConnect(account.email.clone()))
        };

        list_content = list_content.push(menu_entry(card_content, message));
    }

    // Separator
    list_content = list_content.push(Space::with_height(10));

    // "Connect with another email" card
    let new_email_content: Element<'_, Msg> = text::p1_regular("Connect with another email").into();
    let new_email_message = if processing {
        None
    } else {
        Some(Msg::AccountSelectNewEmail)
    };
    list_content = list_content.push(menu_entry(new_email_content, new_email_message));

    layout_with_scrollable_list(
        (1, 4),
        Some(""),
        None,
        &["Login".to_string()],
        header_content,
        list_content,
        None, // no footer
        true,
        None,
    )
}
