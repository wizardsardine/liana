use crate::state::{
    message::Msg,
    views::registration::{RegistrationModalState, RegistrationModalStep},
    State,
};
use iced::Alignment;
use liana_ui::{
    component::{
        button::{btn_cancel, btn_no, btn_retry, btn_yes},
        modal::{modal_view, none_fn, ModalWidth},
        text,
    },
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

    Some(content)
}

fn registering_view(_modal_state: &RegistrationModalState) -> Element<'_, Msg> {
    let body = Column::new()
        .spacing(15)
        .align_x(Alignment::Center)
        .push(
            text::p1_medium("Please confirm on your device...")
                .style(theme::text::secondary)
                .align_x(Alignment::Center),
        )
        .push(
            Row::new()
                .spacing(10)
                .push(btn_cancel(Some(Msg::RegistrationCancelModal))),
        )
        .align_x(Alignment::Center);

    modal_view(
        Some("Registering Wallet".to_string()),
        none_fn(),
        none_fn(),
        ModalWidth::S,
        body,
    )
}

fn error_view(modal_state: &RegistrationModalState) -> Element<'_, Msg> {
    let error_msg = modal_state
        .error
        .as_deref()
        .unwrap_or("Unknown error occurred");

    let body = Column::new()
        .spacing(15)
        .align_x(Alignment::Center)
        .push(
            text::p1_medium(error_msg)
                .style(theme::text::warning)
                .align_x(Alignment::Center),
        )
        .push(
            Row::new()
                .spacing(10)
                .push(btn_cancel(Some(Msg::RegistrationCancelModal)))
                .push(btn_retry(Some(Msg::RegistrationRetry))),
        );

    modal_view(
        Some("Registration Failed".to_string()),
        none_fn(),
        none_fn(),
        ModalWidth::S,
        body,
    )
}

fn confirm_coldcard_view(_modal_state: &RegistrationModalState) -> Element<'_, Msg> {
    let body = Column::new()
        .spacing(15)
        .align_x(Alignment::Center)
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
                .push(btn_no(Some(Msg::RegistrationConfirmNo)))
                .push(btn_yes(Some(Msg::RegistrationConfirmYes))),
        );

    modal_view(
        Some("Confirm Registration".to_string()),
        none_fn(),
        none_fn(),
        ModalWidth::S,
        body,
    )
}
