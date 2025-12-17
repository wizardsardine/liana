use crate::{
    client::BACKEND_URL,
    state::{message::Msg, State},
    views::layout,
};
use iced::widget::row;
use iced::{widget::Space, Length};
use liana_ui::{
    color,
    component::{button, form, text},
    icon,
    widget::*,
};

pub fn login_code_view(state: &State) -> Element<'_, Msg> {
    let form = if !state.views.login.code.processing {
        form::Form::new_trimmed("Token", &state.views.login.code.form, Msg::LoginUpdateCode)
    } else {
        form::Form::new_disabled("Token", &state.views.login.code.form)
    }
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

    // Debug mode hint
    let debug_hint = if BACKEND_URL == "debug" {
        Some(text::p2_regular("Debug mode - Use code: 123456").color(color::GREY_2))
    } else {
        None
    };

    let content = Column::new()
        .push(liana_business)
        .push(Space::with_height(20))
        .push(row![
            text::p1_regular("An authentication token has been emailed to "),
            text::p1_regular(&state.views.login.email.form.value).color(color::GREEN)
        ])
        .push(form)
        .push_maybe(debug_hint)
        .push(btn_row)
        .spacing(20)
        .padding(40);

    layout(
        (2, 4),
        None,
        None, // No role badge during login
        "Login",
        content,
        true,
        Some(Msg::NavigateBack),
    )
}
