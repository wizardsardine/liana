//! Duress recovery flow (Phase 6) and enrollment wizard (Phases 2 & 8) views.
//!
//! Both render `Element<ConnectAccountMessage>` and emit
//! `ConnectAccountMessage::Duress(DuressMessage::…)`. The recovery view is shown
//! as a [`ConnectFlowStep`](crate::app::state::connect::ConnectFlowStep); the
//! wizard is shown in place of the duress dashboard panel while
//! `ConnectAccountPanel::duress_enroll` is `Some`.

use coincube_ui::{
    color,
    component::{button, text},
    theme,
    widget::*,
};
use iced::Length;

use crate::app::state::connect::{DuressDisableState, DuressEnrollState, DuressEnrollStep};
use crate::app::view::{ConnectAccountMessage, DuressMessage};
use crate::services::duress::enroll::{DuressDelay, MIN_ALL_CLEAR_LEN};

fn msg(m: DuressMessage) -> ConnectAccountMessage {
    ConnectAccountMessage::Duress(m)
}

fn card(content: Column<'_, ConnectAccountMessage>) -> Element<'_, ConnectAccountMessage> {
    Container::new(content.padding(20).spacing(8))
        .style(theme::card::simple)
        .width(Length::Fill)
        .into()
}

// =============================================================================
// Phase 6 — post-lockout recovery
// =============================================================================

/// The recovery screen shown first after sign-in when the account is in duress.
pub fn recovery_ux<'a>(
    unlock_at: Option<&chrono::DateTime<chrono::Utc>>,
    passphrase: &'a str,
    submitting: bool,
    cleared: bool,
) -> Element<'a, ConnectAccountMessage> {
    let mut col = Column::new()
        .spacing(16)
        .max_width(560)
        .push(text::h4_bold("Duress Mode").style(theme::text::primary));

    if cleared {
        // Phase 6 Task 6.2 hand-off into the CRK download / restore flow.
        col = col
            .push(text::p1_regular(
                "Duress cleared. Download your Cube Recovery Kit to restore your Cubes.",
            ))
            .push(
                button::primary(None, "Continue")
                    .width(Length::Fixed(220.0))
                    .on_press(msg(DuressMessage::FinishRecovery)),
            );
        return col.width(Length::Fill).into();
    }

    let now = chrono::Utc::now();
    let locked = unlock_at.map(|u| now < *u).unwrap_or(false);

    if locked {
        let when = unlock_at
            .map(|u| {
                u.with_timezone(&chrono::Local)
                    .format("%b %d, %Y %H:%M %Z")
                    .to_string()
            })
            .unwrap_or_else(|| "later".to_string());
        col = col.push(card(
            Column::new()
                .push(text::p1_bold("Account locked").style(theme::text::primary))
                .push(
                    text::p2_regular(format!("Account is locked until {when}. Come back then."))
                        .color(color::GREY_3),
                ),
        ));
        return col.width(Length::Fill).into();
    }

    // Window elapsed — collect the all-clear passphrase.
    let submit: Element<ConnectAccountMessage> = if submitting {
        button::primary(None, "Clearing…")
            .width(Length::Fixed(220.0))
            .into()
    } else {
        button::primary(None, "Clear Duress")
            .width(Length::Fixed(220.0))
            .on_press(msg(DuressMessage::SubmitClear))
            .into()
    };

    col = col.push(card(
        Column::new()
            .push(text::p1_bold("Enter your all-clear passphrase").style(theme::text::primary))
            .push(
                TextInput::new("All-clear passphrase", passphrase)
                    .on_input(|v| msg(DuressMessage::RecoveryPassphraseChanged(v)))
                    .on_submit(msg(DuressMessage::SubmitClear))
                    .secure(true)
                    .padding(15),
            )
            .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
            .push(submit)
            .push(
                button::transparent(None, "Forgot all-clear passphrase?")
                    .on_press(msg(DuressMessage::ForgotAllClear)),
            ),
    ));

    col.width(Length::Fill).into()
}

// =============================================================================
// Issue 2 — disable (step-up re-auth)
// =============================================================================

/// The "Disable Duress Mode" step-up dialog. Takes over the duress panel (like
/// the enrollment wizard) while `ConnectAccountPanel::duress_disable` is `Some`.
/// Which factor is asked for depends on the device — see
/// [`crate::app::DuressStepUpMethod`], probed once when the dialog opens:
///
/// * **PIN** — re-enter the regular unlock PIN of any PIN-protected Cube here,
///   never the duress PIN. Any such Cube anchors it because duress arms and
///   disarms across all of them at once, so there is no canonical one to name.
/// * **Passkey** — a device whose Cubes are all passkey Cubes has no PIN to
///   re-enter, so a fresh assertion stands in. Same class of proof: this
///   machine plus the user's biometric, neither of which a stolen Connect
///   session carries.
/// * **Unavailable** — neither, so the disable is refused here.
///
/// The dialog therefore has four shapes, including the one where the probe has
/// not landed yet. It opens neutral rather than defaulting to the PIN field,
/// which would flicker into a passkey button on the device that needs it most.
pub fn disable_ux(state: &DuressDisableState) -> Element<'_, ConnectAccountMessage> {
    use crate::app::DuressStepUpMethod;

    // (card body, confirm button) per factor. The confirm button is absent where
    // there is nothing to confirm with — a dead button on a device that cannot
    // proceed just invites repeated pressing.
    let (body, confirm): (
        Column<ConnectAccountMessage>,
        Option<Element<ConnectAccountMessage>>,
    ) = match &state.method {
        // The probe is still running. Say so plainly rather than showing a
        // PIN field that might be replaced by a passkey button a moment
        // later.
        None => (
            Column::new()
                .push(text::p1_bold("Checking this device…").style(theme::text::primary))
                .push(
                    text::p2_regular("Looking for a Cube here that can confirm it's you.")
                        .color(color::GREY_3),
                ),
            None,
        ),
        Some(DuressStepUpMethod::Pin) => (
            Column::new()
                .push(text::p1_bold("Confirm with a Cube unlock PIN").style(theme::text::primary))
                .push(
                    text::p2_regular(
                        "Turning off duress disarms it on all your devices. Enter the \
                             regular unlock PIN of any PIN-protected Cube on this device — \
                             not your duress PIN.",
                    )
                    .color(color::GREY_3),
                )
                .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
                .push(
                    TextInput::new("Unlock PIN for a PIN-protected Cube", &state.pin)
                        .on_input(|v| msg(DuressMessage::DisablePinChanged(v)))
                        .on_submit(msg(DuressMessage::DisableSubmit))
                        .secure(true)
                        .padding(15),
                ),
            Some(if state.submitting {
                button::primary(None, "Disabling…")
                    .width(Length::Fixed(220.0))
                    .into()
            } else {
                button::primary(None, "Disable Duress Mode")
                    .width(Length::Fixed(220.0))
                    .on_press_maybe(
                        (!state.pin.is_empty()).then(|| msg(DuressMessage::DisableSubmit)),
                    )
                    .into()
            }),
        ),
        // Deliberately not named "Touch ID": this same ceremony runs on Macs
        // without it, where macOS confirms with the login password or a
        // nearby iPhone instead — the reasoning `passkey_unlock` records for
        // its own copy.
        Some(DuressStepUpMethod::Passkey(cube)) => (
            Column::new()
                .push(text::p1_bold("Confirm with your passkey").style(theme::text::primary))
                .push(
                    text::p2_regular(format!(
                        "Turning off duress disarms it on all your devices. No Cube here \
                             uses an unlock PIN, so confirm with the passkey for '{}' instead.",
                        cube.name
                    ))
                    .color(color::GREY_3),
                ),
            Some(if state.submitting {
                button::primary(None, "Waiting for passkey…")
                    .width(Length::Fixed(220.0))
                    .into()
            } else {
                button::primary(None, "Confirm with passkey")
                    .width(Length::Fixed(220.0))
                    .on_press(msg(DuressMessage::DisablePasskeySubmit))
                    .into()
            }),
        ),
        Some(DuressStepUpMethod::Unavailable) => (
            Column::new()
                .push(text::p1_bold("Can't confirm on this device").style(theme::text::primary))
                .push(text::p2_regular(crate::app::DURESS_STEP_UP_NO_PIN_MSG).color(color::GREY_3)),
            None,
        ),
    };

    let mut col = Column::new()
        .spacing(16)
        .max_width(560)
        .push(text::h4_bold("Disable Duress Mode").style(theme::text::primary))
        .push(card(body));

    if let Some(err) = &state.error {
        col = col.push(text::p2_regular(err.clone()).color(color::RED));
    }

    col = col.push(
        Row::new()
            .spacing(12)
            .push(
                // Disabled mid-flight: cancelling between the server disable and
                // the local disarm would orphan the in-flight result.
                button::secondary(None, "Cancel")
                    .width(Length::Fixed(120.0))
                    .on_press_maybe((!state.submitting).then(|| msg(DuressMessage::DisableCancel))),
            )
            .push(iced::widget::Space::new().width(Length::Fill))
            .push_maybe(confirm),
    );

    col.width(Length::Fill).into()
}

// =============================================================================
// Phases 2 & 8 — enrollment wizard
// =============================================================================

/// The multi-step enrollment wizard: duress PIN → all-clear passphrase → duress
/// recovery-kit password → unlock delay → confirm. Single path — duress is a
/// paid feature behind the hard, server-verified Recovery-Kit gate.
pub fn enroll_ux(state: &DuressEnrollState) -> Element<'_, ConnectAccountMessage> {
    let body = match state.step {
        DuressEnrollStep::SetDuressPin => duress_pin_step(state),
        DuressEnrollStep::SetAllClear => all_clear_step(state),
        DuressEnrollStep::SetCrkPassword => crk_password_step(state),
        DuressEnrollStep::PickDelay => delay_step(state),
        DuressEnrollStep::Confirm => confirm_step(state),
    };

    let mut col = Column::new()
        .spacing(16)
        .max_width(560)
        .push(text::h4_bold("Set up Duress Mode").style(theme::text::primary))
        .push(body);

    if let Some(err) = &state.error {
        col = col.push(text::p2_regular(err.clone()).color(color::RED));
    }

    // Navigation row: Back + (Next | Complete enrollment).
    let is_last = matches!(state.step, DuressEnrollStep::Confirm);
    let primary = if is_last {
        if state.submitting {
            button::primary(None, "Enrolling…").width(Length::Fixed(200.0))
        } else {
            button::primary(None, "Complete enrollment")
                .width(Length::Fixed(200.0))
                .on_press(msg(DuressMessage::SubmitEnrollment))
        }
    } else {
        // Each step advances freely; its own validation runs on press.
        button::primary(None, "Next")
            .width(Length::Fixed(120.0))
            .on_press(msg(DuressMessage::EnrollNext))
    };
    col = col.push(
        Row::new()
            .spacing(12)
            .push(
                // Inert on the first step: there is no previous step to return
                // to (matches the Cancel affordance's disabled pattern).
                button::secondary(None, "Back")
                    .width(Length::Fixed(120.0))
                    .on_press_maybe(
                        (!matches!(state.step, DuressEnrollStep::SetDuressPin))
                            .then(|| msg(DuressMessage::EnrollBack)),
                    ),
            )
            .push(iced::widget::Space::new().width(Length::Fill))
            .push(
                // Disabled while submitting: cancelling mid-enroll would
                // zeroize the duress secrets before the server result lands.
                button::transparent(None, "Cancel").on_press_maybe(
                    (!state.submitting).then(|| msg(DuressMessage::CancelEnrollment)),
                ),
            )
            .push(primary),
    );

    col.width(Length::Fill).into()
}

fn duress_pin_step(state: &DuressEnrollState) -> Element<'_, ConnectAccountMessage> {
    let pin_valid =
        state.duress_pin.len() == 4 && state.duress_pin.chars().all(|c| c.is_ascii_digit());
    let confirm_valid = state.duress_pin == state.duress_pin_confirm;

    let hint_color = if state.duress_pin.is_empty() {
        color::GREY_3
    } else if pin_valid {
        color::GREEN
    } else {
        color::RED
    };

    let mut col = Column::new()
        .push(text::p1_bold("Set your duress PIN").style(theme::text::primary))
        .push(
            text::p2_regular(
                "Choose a PIN you don't use to unlock any of your Cubes. Entering it at any \
                 Cube's unlock screen triggers a duress wipe, so it can't be one of your real \
                 unlock PINs.",
            )
            .color(color::GREY_3),
        )
        .push(text::p2_regular("Duress PIN (4 digits)").color(color::GREY_3))
        .push(
            TextInput::new("Duress PIN", &state.duress_pin)
                .on_input(|v| msg(DuressMessage::DuressPinChanged(v)))
                .secure(true)
                .padding(15),
        )
        .push(text::caption("Must be exactly 4 digits.").color(hint_color))
        .push(text::p2_regular("Confirm duress PIN").color(color::GREY_3))
        .push(
            TextInput::new("Confirm duress PIN", &state.duress_pin_confirm)
                .on_input(|v| msg(DuressMessage::DuressPinConfirmChanged(v)))
                .secure(true)
                .padding(15),
        );

    if !state.duress_pin_confirm.is_empty() && !confirm_valid {
        col = col.push(text::caption("PINs do not match.").color(color::RED));
    }

    card(col)
}

fn all_clear_step(state: &DuressEnrollState) -> Element<'_, ConnectAccountMessage> {
    card(
        Column::new()
            .push(text::p1_bold("Set your all-clear passphrase").style(theme::text::primary))
            .push(
                text::p2_regular(format!(
                "A memorable phrase, at least {MIN_ALL_CLEAR_LEN} characters. You'll need this \
                 to recover your account from a trusted device — choose something you can \
                 remember even after months of disuse.",
            ))
                .color(color::GREY_3),
            )
            .push(
                TextInput::new("All-clear passphrase", &state.all_clear)
                    .on_input(|v| msg(DuressMessage::AllClearChanged(v)))
                    .secure(true)
                    .padding(15),
            ),
    )
}

fn crk_password_step(state: &DuressEnrollState) -> Element<'_, ConnectAccountMessage> {
    card(
        Column::new()
            .push(
                text::p1_bold("Set your duress recovery-kit password").style(theme::text::primary),
            )
            .push(
                text::p2_regular(
                    "This password applies to all your Cubes. If you ever enter it on a recovery \
                 screen, the entire account enters duress.",
                )
                .color(color::GREY_3),
            )
            .push(
                TextInput::new("Duress recovery-kit password", &state.crk_password)
                    .on_input(|v| msg(DuressMessage::CrkPasswordChanged(v)))
                    .secure(true)
                    .padding(15),
            ),
    )
}

fn delay_step(state: &DuressEnrollState) -> Element<'_, ConnectAccountMessage> {
    let mut chips = Row::new().spacing(8);
    for d in DuressDelay::ALL {
        let selected = d == state.delay;
        let chip = if selected {
            button::primary(None, d.label()).width(Length::Fixed(90.0))
        } else {
            button::secondary(None, d.label())
                .width(Length::Fixed(90.0))
                .on_press(msg(DuressMessage::DelaySelected(d)))
        };
        chips = chips.push(chip);
    }
    card(
        Column::new()
            .push(text::p1_bold("Pick an unlock delay").style(theme::text::primary))
            .push(
                text::p2_regular(
                    "Connect refuses recovery-kit downloads during this window, giving you time \
                 to reach a trusted device.",
                )
                .color(color::GREY_3),
            )
            .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
            .push(chips),
    )
}

fn confirm_step(state: &DuressEnrollState) -> Element<'_, ConnectAccountMessage> {
    // The single enrollment path always collects all three credentials: a
    // duress PIN, an all-clear passphrase, and the account-level duress
    // recovery-kit password.
    let creds = Column::new()
        .spacing(4)
        .push(text::p2_regular("• Duress PIN").color(color::GREY_3))
        .push(text::p2_regular("• All-clear passphrase").color(color::GREY_3))
        .push(text::p2_regular("• Duress recovery-kit password").color(color::GREY_3));

    card(
        Column::new()
            .push(text::p1_bold("Memorize your credentials").style(theme::text::primary))
            .push(
                text::p2_regular(
                    "Make sure you have memorized the following. They are never shown again.",
                )
                .color(color::GREY_3),
            )
            .push(creds)
            .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
            .push(
                CheckBox::new(state.memorized)
                    .label("I have memorized all credentials")
                    .on_toggle(|v| msg(DuressMessage::MemorizedToggled(v)))
                    .style(theme::checkbox::primary),
            ),
    )
}
