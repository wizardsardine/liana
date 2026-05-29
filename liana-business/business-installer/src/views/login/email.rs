use crate::{
    state::{Msg, State},
    views::{layout, INSTALLER_STEPS},
};
use iced::{
    widget::{row, Space},
    Length,
};
use liana_i18n::t;
use liana_ui::{
    component::{
        button::{btn_primary, BtnWidth},
        form, text,
    },
    theme,
    widget::*,
};

pub fn login_email_view(state: &State) -> Element<'_, Msg> {
    let can_submit = state.views.login.email.can_send();
    let warning = state.views.login.email.form.warning.map(login_warning);
    let mut form_value = state.views.login.email.form.clone();
    form_value.warning = None;
    let form = if !state.views.login.email.processing {
        form::Form::new_trimmed(&t!("common-email"), &form_value, Msg::LoginUpdateEmail)
            .on_submit_maybe(can_submit.then_some(Msg::LoginSendToken))
    } else {
        form::Form::new_disabled(&t!("common-email"), &form_value)
    };
    let form = if let Some(warning) = warning {
        form.warning(warning)
    } else {
        form
    };
    let form = form.id("login_email").size(16).padding(10);
    let form = Container::new(form).width(Length::Fill);

    let btn = btn_primary(
        None,
        t!("installer-send-token"),
        BtnWidth::L,
        can_submit.then_some(Msg::LoginSendToken),
    );

    let liana_business = row![
        Space::with_width(Length::Fill),
        text::h2("Liana Business"),
        Space::with_width(Length::Fill),
    ];

    let content = Column::new()
        .push(liana_business)
        .push(Space::with_height(20))
        .push(text::p1_medium(t!("business-login-email-help")).style(theme::text::primary))
        .push(form)
        .push(btn)
        .spacing(20)
        .padding(40);

    layout(
        (1, INSTALLER_STEPS),
        None,
        &[t!("common-login")],
        content,
        true,
        None,
    )
}

fn login_warning(key: &str) -> String {
    match key {
        "settings-email-invalid" => t!("settings-email-invalid"),
        "business-auth-code-request-failed" => t!("business-auth-code-request-failed"),
        _ => key.to_string(),
    }
}
