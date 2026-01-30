use crate::{
    state::{message::Msg, State},
    views::layout,
};
use iced::{
    widget::{row, Space},
    Length,
};
use liana_ui::{
    color,
    component::{button, form, text},
    icon, theme,
    widget::*,
};

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

    let btn_previous = button::secondary(Some(icon::previous_icon()), " Change Email")
        .on_press_maybe((!state.views.login.code.can_send()).then_some(Msg::NavigateBack));

    let btn_resend_token = button::secondary(None, "Resend token").on_press_maybe(
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
            text::p1_medium("An authentication token has been emailed to ")
                .style(theme::text::primary),
            text::p1_medium(&state.views.login.email.form.value).color(color::DARK_GREEN)
        ])
        .push(form)
        .push(btn_row)
        .spacing(20)
        .padding(40);

    layout(
        (2, 4),
        None,
        None, // No role badge during login
        &["Login".to_string()],
        content,
        true,
        Some(Msg::NavigateBack),
    )
}
