use crate::state::{
    message::Msg,
    views::registration::{RegistrationModalState, RegistrationModalStep},
    State,
};
use iced::{
    widget::{column, row},
    Alignment, Length,
};
use liana_ui::{
    component::{
        button::{btn_cancel, btn_no, btn_retry, btn_yes},
        modal::{modal_view, ModalWidth},
        text,
    },
    icon,
    spacing::{HSpacing, VSpacing},
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
    let body = column![
        icon::usb_icon().size(100),
        text::new::caption("Please confirm on your device...")
            .style(theme::text::secondary)
            .align_x(Alignment::Center),
        row![btn_cancel(Some(Msg::RegistrationCancelModal))].spacing(HSpacing::M),
    ]
    .spacing(VSpacing::M)
    .width(Length::Fill)
    .align_x(Alignment::Center);

    modal_view(
        Some("Registering Wallet".to_string()),
        None,
        None,
        ModalWidth::S,
        body,
    )
}

fn error_view(modal_state: &RegistrationModalState) -> Element<'_, Msg> {
    let error_msg = modal_state
        .error
        .as_deref()
        .unwrap_or("Unknown error occurred");

    let body = column![
        icon::warning_icon().size(80),
        text::new::caption(error_msg)
            .style(theme::text::warning)
            .align_x(Alignment::Center),
        row![
            btn_cancel(Some(Msg::RegistrationCancelModal)),
            btn_retry(Some(Msg::RegistrationRetry)),
        ]
        .spacing(HSpacing::M),
    ]
    .spacing(VSpacing::M)
    .align_x(Alignment::Center);

    modal_view(
        Some("Registration Failed".to_string()),
        None,
        None,
        ModalWidth::S,
        body,
    )
}

fn confirm_coldcard_view(_modal_state: &RegistrationModalState) -> Element<'_, Msg> {
    let body = column![
        text::new::caption(
            "Please confirm on your Coldcard that the wallet registration completed successfully.",
        )
        .style(theme::text::secondary)
        .align_x(Alignment::Center),
        text::new::b5_bold("Did the registration succeed on your Coldcard?")
            .align_x(Alignment::Center),
        row![
            btn_no(Some(Msg::RegistrationConfirmNo)),
            btn_yes(Some(Msg::RegistrationConfirmYes)),
        ]
        .spacing(HSpacing::M),
    ]
    .spacing(VSpacing::M)
    .align_x(Alignment::Center);

    modal_view(
        Some("Confirm Registration".to_string()),
        None,
        None,
        ModalWidth::S,
        body,
    )
}
