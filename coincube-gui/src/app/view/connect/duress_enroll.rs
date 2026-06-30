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

use crate::app::state::connect::{
    DuressDisableState, DuressEnrollState, DuressEnrollStep, EnrollTier, BACKUP_ACK_PHRASE,
};
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
/// The user re-enters their regular Cube unlock PIN — not the duress PIN — to
/// authorize turning duress off on every device.
pub fn disable_ux(state: &DuressDisableState) -> Element<'_, ConnectAccountMessage> {
    let confirm: Element<ConnectAccountMessage> = if state.submitting {
        button::primary(None, "Disabling…")
            .width(Length::Fixed(220.0))
            .into()
    } else {
        button::primary(None, "Disable Duress Mode")
            .width(Length::Fixed(220.0))
            .on_press_maybe((!state.pin.is_empty()).then(|| msg(DuressMessage::DisableSubmit)))
            .into()
    };

    let mut col = Column::new()
        .spacing(16)
        .max_width(560)
        .push(text::h4_bold("Disable Duress Mode").style(theme::text::primary))
        .push(card(
            Column::new()
                .push(text::p1_bold("Confirm with your Cube PIN").style(theme::text::primary))
                .push(
                    text::p2_regular(
                        "Turning off duress disarms it on all your devices. Re-enter your \
                         regular Cube unlock PIN to confirm — not your duress PIN.",
                    )
                    .color(color::GREY_3),
                )
                .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
                .push(
                    TextInput::new("Cube unlock PIN", &state.pin)
                        .on_input(|v| msg(DuressMessage::DisablePinChanged(v)))
                        .on_submit(msg(DuressMessage::DisableSubmit))
                        .secure(true)
                        .padding(15),
                ),
        ));

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
            .push(confirm),
    );

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
        DuressEnrollStep::BackupAck => backup_ack_step(state),
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
            // On the backup-acknowledgement gate, "Next" stays disabled until
            // the phrase is an exact, case-sensitive match — no paraphrase, no
            // checkbox shortcut. Every other step advances freely (its own
            // validation runs on press).
            let ready = !matches!(state.step, DuressEnrollStep::BackupAck)
                || state.backup_ack_satisfied();
            button::primary(None, "Next")
                .width(Length::Fixed(120.0))
                .on_press_maybe(ready.then(|| msg(DuressMessage::EnrollNext)))
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
                    // Disabled while submitting: cancelling mid-enroll would
                    // zeroize the duress secrets before the server result lands.
                    button::transparent(None, "Cancel").on_press_maybe(
                        (!state.submitting).then(|| msg(DuressMessage::CancelEnrollment)),
                    ),
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

fn backup_ack_step(state: &DuressEnrollState) -> Element<'_, ConnectAccountMessage> {
    // Name BOTH destroyed artifacts explicitly — the Master Seed Phrase(s) AND
    // the Vault Wallet Descriptor(s) — and the funds consequence. A duress wipe
    // without a Cube Recovery Kit is only reversible from the user's own
    // external backup of both.
    let mut col = Column::new()
        .push(text::p1_bold("No recovery without your own backup").style(theme::text::warning))
        .push(
            text::p2_regular(
                "Activating duress permanently destroys every Cube on this device. Without a \
                 Cube Recovery Kit, the only way back is your OWN external backup of BOTH your \
                 Master Seed Phrase(s) AND your Vault Wallet Descriptor(s). If you don't have \
                 both, the wipe is irreversible and any funds held in those Cubes are gone — \
                 there is no server-side recovery path.",
            )
            .color(color::GREY_3),
        )
        .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
        .push(text::p2_regular("Type the following exactly to continue:").color(color::GREY_3))
        .push(text::p2_bold(BACKUP_ACK_PHRASE).style(theme::text::primary))
        .push(
            TextInput::new("Type the confirmation phrase exactly", &state.backup_ack)
                .on_input(|v| msg(DuressMessage::BackupAckChanged(v)))
                .padding(15),
        );

    // Connect tiers: keep the recommended path one tap away. Cancelling returns
    // to the Duress panel, where the per-Cube recovery-kit checklist lives.
    // Sovereign has no recovery kit, so there's no off-ramp to offer.
    if state.tier != EnrollTier::Sovereign {
        col = col
            .push(iced::widget::Space::new().height(Length::Fixed(8.0)))
            .push(
                button::secondary(None, "Set up a Recovery Kit first")
                    .on_press(msg(DuressMessage::CancelEnrollment)),
            );
    }

    card(col)
}

fn duress_pin_step(state: &DuressEnrollState) -> Element<'_, ConnectAccountMessage> {
    let pin_valid = state.duress_pin.len() == 4
        && state.duress_pin.chars().all(|c| c.is_ascii_digit());
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
    // Every tier sets a duress PIN; only Connect tiers collect an all-clear
    // passphrase, and only Tier 1 a recovery-kit password. Sovereign never
    // creates an all-clear, so don't tell it to memorize one.
    let mut creds = Column::new()
        .spacing(4)
        .push(text::p2_regular("• Duress PIN").color(color::GREY_3));
    if state.tier != EnrollTier::Sovereign {
        creds = creds.push(text::p2_regular("• All-clear passphrase").color(color::GREY_3));
    }
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
