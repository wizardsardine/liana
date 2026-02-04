use crate::{
    state::{Msg, State},
    views::{layout, INSTALLER_STEPS},
};
use iced::{
    widget::{row, Space},
    Length,
};
use liana_ui::{
    component::{button, form, text},
    theme,
    widget::*,
};

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

    let btn = button::primary(None, "Send token")
        .on_press_maybe((state.views.login.email.can_send()).then_some(Msg::LoginSendToken));

    let liana_business = row![
        Space::with_width(Length::Fill),
        text::h2("Liana Business"),
        Space::with_width(Length::Fill),
    ];

    let content = Column::new()
        .push(liana_business)
        .push(Space::with_height(20))
        .push(
            text::p1_medium("Enter the email associated with your account")
                .style(theme::text::primary),
        )
        .push(form)
        .push(btn)
        .spacing(20)
        .padding(40);

    layout(
        (1, INSTALLER_STEPS),
        None,
        None,
        &["Login".to_string()],
        content,
        true,
        None,
    )
}
