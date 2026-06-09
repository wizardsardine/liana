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

use crate::app::state::connect::{DuressEnrollState, DuressEnrollStep, EnrollTier};
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
// Phases 2 & 8 — enrollment wizard
// =============================================================================

/// The multi-step enrollment wizard. Tier-aware: sovereign opens with the
/// Connect-encouragement screen, Connect tiers start at the duress-PIN step.
pub fn enroll_ux(state: &DuressEnrollState) -> Element<'_, ConnectAccountMessage> {
    let body = match state.step {
        DuressEnrollStep::Encourage => encourage_step(),
        DuressEnrollStep::SovereignConfirm => sovereign_confirm_step(state),
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

    // Navigation row. `Encourage` is special (its own CTAs); every other step
    // gets Back + (Next | Complete enrollment).
    if !matches!(state.step, DuressEnrollStep::Encourage) {
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
            button::primary(None, "Next")
                .width(Length::Fixed(120.0))
                .on_press(msg(DuressMessage::EnrollNext))
        };
        col = col.push(
            Row::new()
                .spacing(12)
                .push(
                    button::secondary(None, "Back")
                        .width(Length::Fixed(120.0))
                        .on_press(msg(DuressMessage::EnrollBack)),
                )
                .push(iced::widget::Space::new().width(Length::Fill))
                .push(
                    button::transparent(None, "Cancel")
                        .on_press(msg(DuressMessage::CancelEnrollment)),
                )
                .push(primary),
        );
    }

    col.width(Length::Fill).into()
}

fn encourage_step<'a>() -> Element<'a, ConnectAccountMessage> {
    card(
        Column::new()
            .push(
                text::p1_bold("We recommend setting up Connect before enabling duress mode.")
                    .style(theme::text::primary),
            )
            .push(
                text::p2_regular("Connect makes duress mode safer and more forgiving:")
                    .color(color::GREY_3),
            )
            .push(
                text::p2_regular(
                    "• Cube Recovery Kit — restore after a wipe with no paper seed phrase.\n\
                 • All-clear passphrase — undo an accidental or coerced activation from a \
                 trusted device. Sovereign duress has no all-clear.\n\
                 • Lockout window (24h–90d) — buys you time to reach a trusted device.\n\
                 • Cross-device signaling — the Keychain signer auto-refuses during duress.\n\
                 • Trusted-device delay on new-device downloads.",
                )
                .color(color::GREY_3),
            )
            .push(iced::widget::Space::new().height(Length::Fixed(12.0)))
            .push(
                button::primary(None, "Sign up for Connect")
                    .width(Length::Fixed(240.0))
                    .on_press(msg(DuressMessage::SignUpForConnect)),
            )
            .push(
                button::transparent(None, "Continue without Connect (advanced)")
                    .on_press(msg(DuressMessage::EnrollNext)),
            ),
    )
}

fn sovereign_confirm_step(state: &DuressEnrollState) -> Element<'_, ConnectAccountMessage> {
    card(
        Column::new()
            .push(text::p1_bold("No server-side recovery").style(theme::text::warning))
            .push(
                text::p2_regular(
                    "Without a Connect account, duress mode will erase your Cubes on this device \
                 and there is NO server-side recovery path. You will need your seed-phrase \
                 backup to restore. Continue only if you have a verified offline backup.",
                )
                .color(color::GREY_3),
            )
            .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
            .push(text::p2_regular("Type: I have my seed-phrase backup").color(color::GREY_3))
            .push(
                TextInput::new("I have my seed-phrase backup", &state.sovereign_confirm)
                    .on_input(|v| msg(DuressMessage::SovereignConfirmChanged(v)))
                    .padding(15),
            ),
    )
}

fn duress_pin_step(state: &DuressEnrollState) -> Element<'_, ConnectAccountMessage> {
    card(
        Column::new()
            .push(text::p1_bold("Set your duress PIN").style(theme::text::primary))
            .push(
                text::p2_regular(
                    "Your duress PIN must be at least 2 character changes from your regular PIN. \
                 This prevents accidental activation.",
                )
                .color(color::GREY_3),
            )
            .push(text::p2_regular("Confirm your regular PIN").color(color::GREY_3))
            .push(
                TextInput::new("Regular PIN", &state.regular_pin)
                    .on_input(|v| msg(DuressMessage::RegularPinChanged(v)))
                    .secure(true)
                    .padding(15),
            )
            .push(text::p2_regular("New duress PIN").color(color::GREY_3))
            .push(
                TextInput::new("Duress PIN", &state.duress_pin)
                    .on_input(|v| msg(DuressMessage::DuressPinChanged(v)))
                    .secure(true)
                    .padding(15),
            ),
    )
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
    let mut creds = Column::new()
        .spacing(4)
        .push(text::p2_regular("• Duress PIN").color(color::GREY_3))
        .push(text::p2_regular("• All-clear passphrase").color(color::GREY_3));
    if state.tier == EnrollTier::Tier1 {
        creds = creds.push(text::p2_regular("• Duress recovery-kit password").color(color::GREY_3));
    }

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
