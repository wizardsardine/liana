use crate::{
    state::{message::Msg, State},
    views::{intro_prompt, layout, screen_intro, INSTALLER_STEPS},
};
use iced::{
    widget::{column, row},
    Length,
};
use liana_ui::{
    component::{
        button::{btn_change_email, btn_resend_token},
        form,
    },
    spacing::{HSpacing, VSpacing},
    widget::*,
};

use super::LOGIN_WIDTH;

pub fn login_code_view(state: &State) -> Element<'_, Msg> {
    let form = if !state.views.login.code.processing {
        form::Form::new_trimmed("Token", &state.views.login.code.form, Msg::LoginUpdateCode)
    } else {
        form::Form::new_disabled("Token", &state.views.login.code.form)
    }
    .id("login_code")
    .size(16)
    .padding(10);
    let form = Container::new(form).width(Length::Fill);

    let btn_previous =
        btn_change_email((!state.views.login.code.can_send()).then_some(Msg::NavigateBack));

    let resend_msg = state
        .views
        .login
        .code
        .can_resend_token
        .then_some(Msg::LoginResendToken);
    let btn_resend_token = btn_resend_token(resend_msg);

    let btn_row = row![btn_previous, btn_resend_token].spacing(HSpacing::M);

    let content = Container::new(
        column![
            screen_intro(
                "Liana Business",
                Some(intro_prompt(
                    "An authentication token has been emailed to ",
                    Some(&state.views.login.email.form.value),
                )),
                true,
            ),
            form,
            btn_row,
        ]
        .spacing(VSpacing::L)
        .width(LOGIN_WIDTH),
    )
    .padding(40)
    .center_x(Length::Fill);

    layout(
        (2, INSTALLER_STEPS),
        state.network,
        None,
        &["Login".to_string()],
        content,
        Some(Msg::NavigateBack),
    )
}
