use crate::{
    state::{Msg, State},
    views::{intro_prompt, layout, screen_intro, INSTALLER_STEPS},
};
use iced::{widget::column, Length};
use liana_ui::{
    component::{button::btn_send_token, form},
    widget::*,
};

use super::LOGIN_WIDTH;

pub fn login_email_view(state: &State) -> Element<'_, Msg> {
    let can_submit = state.views.login.email.can_send();
    let form = if !state.views.login.email.processing {
        form::Form::new_trimmed(
            "Email",
            &state.views.login.email.form,
            Msg::LoginUpdateEmail,
        )
        .on_submit_maybe(can_submit.then_some(Msg::LoginSendToken))
    } else {
        form::Form::new_disabled("Email", &state.views.login.email.form)
    }
    .id("login_email")
    .size(16)
    .padding(10);
    let form = Container::new(form).width(Length::Fill);

    let btn = btn_send_token(can_submit.then_some(Msg::LoginSendToken));

    let content = Container::new(
        column![
            screen_intro(
                "Liana Business",
                Some(intro_prompt(
                    "Enter the email associated with your account",
                    None,
                )),
            ),
            form,
            btn,
        ]
        .spacing(20)
        .width(LOGIN_WIDTH),
    )
    .padding(40)
    .center_x(Length::Fill);

    let previous =
        (!state.views.login.account_select.accounts.is_empty()).then_some(Msg::NavigateBack);
    layout(
        (1, INSTALLER_STEPS),
        None,
        &["Login".to_string()],
        content,
        true,
        previous,
    )
}
