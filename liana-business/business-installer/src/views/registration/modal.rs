use crate::state::{
    message::Msg,
    views::registration::{RegistrationModalState, RegistrationModalStep},
    State,
};
use iced::{Alignment, Length};
use liana_ui::{
    component::{button, card, text},
    theme,
    widget::*,
};

/// Registration modal view
pub fn registration_modal_view(state: &State) -> Option<Element<'_, Msg>> {
    let modal_state = state.views.registration.modal.as_ref()?;

    let content = match &modal_state.step {
        RegistrationModalStep::Registering => registering_view(modal_state),
        RegistrationModalStep::ConfirmColdcard { .. } => confirm_coldcard_view(modal_state),
        RegistrationModalStep::Error => error_view(modal_state),
    };

    Some(card::modal(content).into())
}

fn registering_view(_modal_state: &RegistrationModalState) -> Element<'_, Msg> {
    Column::new()
        .spacing(20)
        .padding(20)
        .width(Length::Fixed(400.0))
        .align_x(Alignment::Center)
        .push(text::h3("Registering Wallet"))
        .push(
            text::p1_medium("Please confirm on your device...")
                .style(theme::text::secondary)
                .align_x(Alignment::Center),
        )
        .push(
            Row::new()
                .spacing(10)
                .push(button::secondary(None, "Cancel").on_press(Msg::RegistrationCancelModal)),
        )
        .into()
}

fn error_view(modal_state: &RegistrationModalState) -> Element<'_, Msg> {
    let error_msg = modal_state
        .error
        .as_deref()
        .unwrap_or("Unknown error occurred");

    Column::new()
        .spacing(20)
        .padding(20)
        .width(Length::Fixed(400.0))
        .align_x(Alignment::Center)
        .push(text::h3("Registration Failed"))
        .push(
            text::p1_medium(error_msg)
                .style(theme::text::warning)
                .align_x(Alignment::Center),
        )
        .push(
            Row::new()
                .spacing(10)
                .push(button::secondary(None, "Cancel").on_press(Msg::RegistrationCancelModal))
                .push(button::primary(None, "Retry").on_press(Msg::RegistrationRetry)),
        )
        .into()
}

fn confirm_coldcard_view(_modal_state: &RegistrationModalState) -> Element<'_, Msg> {
    Column::new()
        .spacing(20)
        .padding(20)
        .width(Length::Fixed(400.0))
        .align_x(Alignment::Center)
        .push(text::h3("Confirm Registration"))
        .push(
            text::p1_medium(
                "Please confirm on your Coldcard that the wallet registration completed successfully.",
            )
            .style(theme::text::secondary)
            .align_x(Alignment::Center),
        )
        .push(
            text::p1_bold("Did the registration succeed on your Coldcard?")
                .align_x(Alignment::Center),
        )
        .push(
            Row::new()
                .spacing(10)
                .push(button::secondary(None, "No").on_press(Msg::RegistrationConfirmNo))
                .push(button::primary(None, "Yes").on_press(Msg::RegistrationConfirmYes)),
        )
        .into()
}
