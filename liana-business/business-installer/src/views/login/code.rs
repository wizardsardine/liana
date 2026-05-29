use crate::{
    state::{message::Msg, State},
    views::{layout, INSTALLER_STEPS},
};
use iced::{
    widget::{row, Space},
    Length,
};
use liana_i18n::t;
use liana_ui::{
    component::{
        button::{btn_secondary, BtnWidth},
        form, text,
    },
    icon, theme,
    widget::*,
};

pub fn login_code_view(state: &State) -> Element<'_, Msg> {
    let warning = state.views.login.code.form.warning.map(login_warning);
    let mut form_value = state.views.login.code.form.clone();
    form_value.warning = None;
    let form = if !state.views.login.code.processing {
        form::Form::new_trimmed(&t!("common-token"), &form_value, Msg::LoginUpdateCode)
    } else {
        form::Form::new_disabled(&t!("common-token"), &form_value)
    };
    let form = if let Some(warning) = warning {
        form.warning(warning)
    } else {
        form
    };
    let form = form.id("login_code").size(16).padding(10);
    let form = Container::new(form).width(Length::Fill);

    let btn_previous = btn_secondary(
        Some(icon::previous_icon()),
        t!("installer-change-email"),
        BtnWidth::XL,
        (!state.views.login.code.can_send()).then_some(Msg::NavigateBack),
    );

    let btn_resend_token = btn_secondary(
        None,
        t!("lianalite-resend-token"),
        BtnWidth::XL,
        state
            .views
            .login
            .code
            .can_resend_token
            .then_some(Msg::LoginResendToken),
    );

    let btn_row = row![btn_previous, btn_resend_token].spacing(10);

    let liana_business = row![
        Space::with_width(Length::Fill),
        text::h2("Liana Business"),
        Space::with_width(Length::Fill),
    ];

    let content = Column::new()
        .push(liana_business)
        .push(Space::with_height(20))
        .push(row![
            text::p1_medium(t!("installer-auth-token-emailed-to")).style(theme::text::primary),
            text::p1_medium(&state.views.login.email.form.value).style(theme::text::accent)
        ])
        .push(form)
        .push(btn_row)
        .spacing(20)
        .padding(40);

    layout(
        (2, INSTALLER_STEPS),
        None,
        &[t!("common-login")],
        content,
        true,
        Some(Msg::NavigateBack),
    )
}

fn login_warning(key: &str) -> String {
    match key {
        "business-login-failed" => t!("business-login-failed"),
        "business-code-six-digits" => t!("business-code-six-digits"),
        _ => key.to_string(),
    }
}
