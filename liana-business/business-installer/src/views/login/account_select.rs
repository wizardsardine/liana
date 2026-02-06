use crate::{
    state::{Msg, State},
    views::{keys::delete_button_style, layout_with_scrollable_list, INSTALLER_STEPS},
};
use iced::{
    widget::{row, Space},
    Alignment, Length,
};
use liana_ui::{component::text, icon, theme, widget::*};

/// Create an account card that can be clicked to select the account
fn account_card(content: Element<'_, Msg>, message: Option<Msg>) -> Element<'_, Msg> {
    Container::new(
        Button::new(
            Container::new(content)
                .align_y(Alignment::Center)
                .align_x(Alignment::Center)
                .width(Length::Fill)
                .height(Length::Fill),
        )
        .on_press_maybe(message)
        .padding(15)
        .style(theme::button::container_border),
    )
    .style(theme::card::simple)
    .align_x(Alignment::Center)
    .align_y(Alignment::Center)
    .width(500)
    .height(80)
    .into()
}

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

        let card_content: Element<'_, Msg> = if processing && is_selected {
            // Show loading state for selected account
            Row::new()
                .push(Space::with_width(Length::Fill))
                .push(text::p2_medium("Connecting..."))
                .push(Space::with_width(Length::Fill))
                .align_y(iced::Alignment::Center)
                .into()
        } else {
            text::p1_medium(&account.email).into()
        };

        let card_message = if processing {
            None // Disable all cards while connecting
        } else {
            Some(Msg::AccountSelectConnect(account.email.clone()))
        };

        let account_card_element = account_card(card_content, card_message);

        // Delete button - disabled while processing
        let delete_button = Button::new(
            Container::new(icon::trash_icon())
                .width(Length::Fixed(20.0))
                .height(Length::Fixed(20.0))
                .center_x(Length::Fixed(20.0))
                .center_y(Length::Fixed(20.0)),
        )
        .padding(10)
        .on_press_maybe(if processing {
            None
        } else {
            Some(Msg::AccountSelectDelete(account.email.clone()))
        })
        .style(delete_button_style);

        // Row with account card and delete button
        let account_row = Row::new()
            .spacing(15)
            .align_y(Alignment::Center)
            .push(account_card_element)
            .push(delete_button);

        list_content = list_content.push(account_row);
    }

    // Separator
    list_content = list_content.push(Space::with_height(20));

    // "Connect with another email" button
    let new_email = liana_ui::component::button::secondary(None, "Connect with another email")
        .on_press(Msg::AccountSelectNewEmail);

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
        None,
        &["Login".to_string()],
        header_content,
        list_content,
        None, // no footer
        true,
        None,
    )
}
