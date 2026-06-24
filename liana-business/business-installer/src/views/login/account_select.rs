use crate::{
    state::{views::login::CachedAccount, Msg, State},
    views::{intro_prompt, layout_with_scrollable_list, screen_intro, INSTALLER_STEPS},
};
use iced::{
    widget::{column, row, Space},
    Alignment, Length,
};
use liana_ui::{
    component::{
        button::{btn_connect_another_email, EntryWidth, ENTRY_DELETE_GAP, ENTRY_DELETE_SLOT},
        list, text,
    },
    widget::*,
};

pub fn account_select_view(state: &State) -> Element<'_, Msg> {
    let accounts = &state.views.login.account_select.accounts;
    let processing = state.views.login.account_select.processing;
    let selected_email = &state.views.login.account_select.selected_email;

    let header_content = screen_intro(
        "Liana Business",
        Some(intro_prompt("Select an account to continue", None)),
    );

    // Scrollable list of accounts
    let mut list_content = column![].spacing(15).align_x(iced::Alignment::Center);

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
        Container::new(new_email).width(EntryWidth::Deletable),
        Space::with_width(ENTRY_DELETE_SLOT),
    ]
    .spacing(ENTRY_DELETE_GAP)
    .align_y(Alignment::Center)
    .width(Length::Shrink);

    list_content = list_content.push(new_email_row);

    layout_with_scrollable_list(
        (1, INSTALLER_STEPS),
        None,
        false,
        &["Login".to_string()],
        Some(header_content),
        list_content,
        None,
        None,
        true,
        None,
    )
}

fn account_entry(
    account: &CachedAccount,
    processing: bool,
    is_selected: bool,
) -> Element<'static, Msg> {
    list::account_entry(
        text::short_email(&account.email, 55),
        processing && is_selected,
        (!processing).then_some(Msg::AccountSelectConnect(account.email.clone())),
        (!processing).then_some(Msg::AccountSelectDelete(account.email.clone())),
    )
}
