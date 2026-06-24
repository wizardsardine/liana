use crate::{
    state::{views::login::CachedAccount, Msg, State},
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
        text::new::d3("Liana Business"),
        Space::with_width(Length::Fill),
    ];

    let select_account_text = row![
        Space::with_width(Length::Fill),
        text::new::h3_semi("Select an account to continue"),
        Space::with_width(Length::Fill),
    ];

    let header_content = Column::new()
        .push(liana_business)
        .push(Space::with_height(30))
        .push(select_account_text)
        .push(Space::with_height(30));

    // Scrollable list of accounts
    let mut list_content = Column::new().spacing(15).align_x(iced::Alignment::Center);

    // One row per cached account: the account entry plus a delete button after it.
    for account in accounts {
        let is_selected = selected_email.as_ref() == Some(&account.email);
        list_content = list_content.push(account_entry(account, processing, is_selected));
    }

    // Separator
    list_content = list_content.push(Space::with_height(20));

    let new_email = btn_connect_another_email((!processing).then_some(Msg::AccountSelectNewEmail));

    // Connect button fills the row, with a spacer the size of the delete-button slot so it
    // lines up with the account entries above.
    let new_email_row = row![
        new_email,
        Space::with_width(list::ACCOUNT_DELETE_SLOT_WIDTH)
    ]
    .spacing(10)
    .align_y(Alignment::Center)
    .width(Length::Fill);

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

fn account_entry(
    account: &CachedAccount,
    processing: bool,
    is_selected: bool,
) -> Element<'static, Msg> {
    let email = if processing && is_selected {
        "Connecting...".to_string()
    } else {
        text::short_email(&account.email, 55)
    };
    list::account_entry(
        email,
        (!processing).then_some(Msg::AccountSelectConnect(account.email.clone())),
        (!processing).then_some(Msg::AccountSelectDelete(account.email.clone())),
    )
}
