use crate::{
    state::{views::login::CachedAccount, Msg, State},
    views::{intro_prompt, layout_with_scrollable_list, screen_intro, INSTALLER_STEPS},
};
use iced::widget::column;
use liana_ui::{
    component::{
        button::{btn_connect_another_email, EntryWidth},
        list, text,
    },
    spacing::VSpacing,
    widget::*,
};

pub fn account_select_view(state: &State) -> Element<'_, Msg> {
    let accounts = &state.views.login.account_select.accounts;
    let processing = state.views.login.account_select.processing;
    let selected_email = &state.views.login.account_select.selected_email;

    let header_content = screen_intro(
        "Liana Business",
        Some(intro_prompt("Select an account to continue", None)),
        false,
    );

    // Scrollable list of accounts
    let mut list_content = column![]
        .spacing(VSpacing::M)
        .align_x(iced::Alignment::Center);

    // One row per cached account: the account entry plus a delete button after it.
    for account in accounts {
        let is_selected = selected_email.as_ref() == Some(&account.email);
        list_content = list_content.push(account_entry(account, processing, is_selected));
    }

    let new_email = btn_connect_another_email((!processing).then_some(Msg::AccountSelectNewEmail))
        .width(if processing || accounts.is_empty() {
            EntryWidth::Standard
        } else {
            EntryWidth::Deletable
        });

    layout_with_scrollable_list(
        (1, INSTALLER_STEPS),
        None,
        false,
        &["Login".to_string()],
        Some(header_content),
        list_content,
        Some(new_email.into()),
        None,
        None,
    )
}

fn account_entry(
    account: &CachedAccount,
    processing: bool,
    is_selected: bool,
) -> Element<'static, Msg> {
    list::account_entry(
        text::short_email(&account.email, 40),
        processing && is_selected,
        (!processing).then_some(Msg::AccountSelectConnect(account.email.clone())),
        (!processing).then_some(Msg::AccountSelectDelete(account.email.clone())),
    )
}
