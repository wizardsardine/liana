//! Views for the Cube Recovery Kit wizard.
//!
//! Rendered as a full-page takeover of the General Settings view when
//! `RecoveryKitState != None` (see `general.rs::general_section`). The
//! Settings card itself (shown when the wizard is inactive) lives in
//! `general.rs::recovery_kit_card`.
//!
//! Flow for a mnemonic cube in Create mode:
//!   PinEntry → PasswordEntry → Uploading → Completed
//! A passkey cube (descriptor-only) skips PinEntry.

use coincube_ui::{
    color,
    component::{button as ui_button, text::*},
    icon, theme,
    widget::{CheckBox, Container, Element, TextInput},
};
use iced::widget::{progress_bar, Column, Row, Space};
use iced::{Alignment, Length};

use crate::app::state::settings::recovery_kit::{PhoneKey, RecoveryKitState};
use crate::app::view::message::{
    Message, RecoveryKitMessage, RecoveryProtectionMode, SettingsMessage,
};
use crate::pin_input::PinInput;
use crate::services::coincube::RecoveryKitStatus;
use crate::services::recovery::{score_password, MIN_PASSWORD_LEN};

use super::general::{backup_methods_present, backup_pill};
use zeroize::Zeroizing;

fn wrap(msg: RecoveryKitMessage) -> Message {
    Message::Settings(SettingsMessage::RecoveryKit(msg))
}

/// Single "< Back" button row — mirrors backup.rs::header.
fn header<'a>() -> Element<'a, Message> {
    Row::new()
        .spacing(10)
        .align_y(Alignment::Center)
        .push(
            ui_button::secondary(None, "< Back")
                .on_press(wrap(RecoveryKitMessage::Cancel))
                .padding([8, 16])
                .width(Length::Fixed(150.0)),
        )
        .into()
}

/// PIN gate — mnemonic cubes only. Unlocks the on-disk encrypted
/// mnemonic so the seed blob can be built.
pub fn pin_entry_view<'a>(pin: &'a PinInput, error: Option<&'a str>) -> Element<'a, Message> {
    let mut col = Column::new()
        .spacing(20)
        .width(Length::Fill)
        .push(header())
        .push(Space::new().height(Length::Fixed(16.0)))
        .push(
            Row::new()
                .width(Length::Fill)
                .align_y(Alignment::Center)
                .push(Space::new().width(Length::Fill))
                .push(icon::lock_icon().size(100).color(color::ORANGE))
                .push(Space::new().width(Length::Fill)),
        )
        .push(Space::new().height(Length::Fixed(16.0)))
        .push(
            Row::new()
                .width(Length::Fill)
                .align_y(Alignment::Center)
                .push(Space::new().width(Length::Fill))
                .push(
                    Container::new(
                        text(
                            "Enter your Cube PIN to unlock your Master Seed Phrase. \
                             We'll encrypt it with your recovery password on-device \
                             before uploading.",
                        )
                        .size(18)
                        .align_x(iced::alignment::Horizontal::Center),
                    )
                    .width(Length::Fixed(600.0))
                    .align_x(iced::alignment::Horizontal::Center),
                )
                .push(Space::new().width(Length::Fill)),
        )
        .push(Space::new().height(Length::Fixed(24.0)))
        .push(
            Row::new()
                .width(Length::Fill)
                .align_y(Alignment::Center)
                .push(Space::new().width(Length::Fill))
                .push(pin.view().map(|m| wrap(RecoveryKitMessage::PinInput(m))))
                .push(Space::new().width(Length::Fill)),
        );

    col = col.push(Space::new().height(Length::Fixed(16.0)));

    if let Some(err) = error {
        col = col.push(
            Row::new()
                .width(Length::Fill)
                .align_y(Alignment::Center)
                .push(Space::new().width(Length::Fill))
                .push(text(err).size(16).color(color::RED))
                .push(Space::new().width(Length::Fill)),
        );
        col = col.push(Space::new().height(Length::Fixed(8.0)));
    }

    col = col.push(
        Row::new()
            .spacing(20)
            .width(Length::Fill)
            .align_y(Alignment::Center)
            .push(Space::new().width(Length::Fill))
            .push(
                ui_button::secondary(None, "Cancel")
                    .on_press(wrap(RecoveryKitMessage::Cancel))
                    .padding([8, 16])
                    .width(Length::Fixed(150.0)),
            )
            .push({
                let btn = ui_button::primary(None, "Unlock")
                    .padding([8, 16])
                    .width(Length::Fixed(200.0));
                if pin.is_complete() {
                    btn.on_press(wrap(RecoveryKitMessage::VerifyPin))
                } else {
                    btn
                }
            })
            .push(Space::new().width(Length::Fill)),
    );

    col.into()
}

/// Password entry. Two inputs (password + confirm), live strength
/// meter, acknowledge checkbox, Submit button gated on all three.
pub fn password_entry_view<'a>(
    password: &'a Zeroizing<String>,
    confirm: &'a Zeroizing<String>,
    acknowledged: bool,
    error: Option<&'a str>,
) -> Element<'a, Message> {
    let (strength, hint) = score_password(password, &[]);
    let strength_label = strength.label();
    let strength_fraction = strength.fraction();
    // Mirror the gates that `submit_password` enforces server-side
    // (state/settings/recovery_kit.rs), in the same order. A short
    // but otherwise-varied password can clear `is_acceptable()`
    // (zxcvbn rates complexity, not length) while still failing the
    // `MIN_PASSWORD_LEN` floor — without the explicit length check
    // here, the Submit button would light up and then the handler
    // would bounce the user back with "Password must be at least
    // 12 characters", which reads as a bug.
    let can_submit = password.len() >= MIN_PASSWORD_LEN
        && password.as_str() == confirm.as_str()
        && strength.is_acceptable()
        && acknowledged;

    let mut col = Column::new()
        .spacing(20)
        .width(Length::Fill)
        .push(header())
        .push(Space::new().height(Length::Fixed(16.0)))
        .push(
            Row::new()
                .width(Length::Fill)
                .align_y(Alignment::Center)
                .push(Space::new().width(Length::Fill))
                .push(icon::key_icon().size(100).color(color::ORANGE))
                .push(Space::new().width(Length::Fill)),
        )
        .push(Space::new().height(Length::Fixed(16.0)))
        .push(
            Row::new()
                .width(Length::Fill)
                .align_y(Alignment::Center)
                .push(Space::new().width(Length::Fill))
                .push(
                    Container::new(
                        text(
                            "Choose a recovery password. This is separate from your \
                             Cube PIN — write it down somewhere safe. COINCUBE cannot \
                             recover it for you.",
                        )
                        .size(18)
                        .align_x(iced::alignment::Horizontal::Center),
                    )
                    .width(Length::Fixed(600.0))
                    .align_x(iced::alignment::Horizontal::Center),
                )
                .push(Space::new().width(Length::Fill)),
        )
        .push(Space::new().height(Length::Fixed(24.0)));

    // Password inputs — centred 600px column.
    let inputs = Column::new()
        .spacing(12)
        .width(Length::Fixed(600.0))
        .push(caption("Recovery password"))
        .push(
            TextInput::new("Choose a password", password.as_str())
                .on_input(|v| {
                    // Wrap at the message boundary — the `String`
                    // from iced's callback is the last unprotected
                    // copy in our code path; every in-flight clone
                    // from here on is in a `Zeroizing` wrapper that
                    // wipes on drop.
                    wrap(RecoveryKitMessage::PasswordChanged(Zeroizing::new(v)))
                })
                .secure(true)
                .size(16)
                .padding(12)
                .width(Length::Fill),
        )
        .push(Space::new().height(Length::Fixed(8.0)))
        .push(progress_bar(0.0..=1.0, strength_fraction).style(theme::progress_bar::primary))
        .push({
            let mut r = Row::new()
                .width(Length::Fill)
                .push(text(format!("Strength: {}", strength_label)).size(14))
                .push(Space::new().width(Length::Fill));
            if let Some(h) = hint {
                r = r.push(text(h).size(12).style(theme::text::warning));
            }
            r
        })
        .push(Space::new().height(Length::Fixed(8.0)))
        .push(caption("Confirm recovery password"))
        .push(
            TextInput::new("Re-enter password", confirm.as_str())
                .on_input(|v| wrap(RecoveryKitMessage::ConfirmChanged(Zeroizing::new(v))))
                .secure(true)
                .size(16)
                .padding(12)
                .width(Length::Fill),
        );

    col = col.push(
        Row::new()
            .width(Length::Fill)
            .align_y(Alignment::Center)
            .push(Space::new().width(Length::Fill))
            .push(inputs)
            .push(Space::new().width(Length::Fill)),
    );

    // Mismatch warning if confirm diverges.
    if !confirm.is_empty() && password.as_str() != confirm.as_str() {
        col = col.push(
            Row::new()
                .width(Length::Fill)
                .align_y(Alignment::Center)
                .push(Space::new().width(Length::Fill))
                .push(text("Passwords don't match.").size(14).color(color::RED))
                .push(Space::new().width(Length::Fill)),
        );
    }

    col = col.push(Space::new().height(Length::Fixed(16.0)));

    // Acknowledge checkbox.
    col = col.push(
        Row::new()
            .width(Length::Fill)
            .align_y(Alignment::Center)
            .push(Space::new().width(Length::Fill))
            .push(
                Container::new(
                    CheckBox::new(acknowledged)
                        .label("I've written this password down somewhere I can find it")
                        .on_toggle(|v| wrap(RecoveryKitMessage::AcknowledgeToggled(v)))
                        .style(theme::checkbox::primary),
                )
                .width(Length::Fixed(600.0)),
            )
            .push(Space::new().width(Length::Fill)),
    );

    if let Some(err) = error {
        col = col.push(
            Row::new()
                .width(Length::Fill)
                .align_y(Alignment::Center)
                .push(Space::new().width(Length::Fill))
                .push(text(err).size(16).color(color::RED))
                .push(Space::new().width(Length::Fill)),
        );
    }

    col = col.push(Space::new().height(Length::Fixed(16.0)));

    col = col.push(
        Row::new()
            .spacing(20)
            .width(Length::Fill)
            .align_y(Alignment::Center)
            .push(Space::new().width(Length::Fill))
            .push(
                ui_button::secondary(None, "Cancel")
                    .on_press(wrap(RecoveryKitMessage::Cancel))
                    .padding([8, 16])
                    .width(Length::Fixed(150.0)),
            )
            .push({
                let btn = ui_button::primary(None, "Back up Recovery Kit")
                    .padding([8, 16])
                    .width(Length::Fixed(300.0));
                if can_submit {
                    btn.on_press(wrap(RecoveryKitMessage::SubmitPassword))
                } else {
                    btn
                }
            })
            .push(Space::new().width(Length::Fill)),
    );

    col.into()
}

/// Protection-mode choice (PLAN-owner-keychain-recovery PR 2). Three options —
/// password / phone / both — plus a one-time "set up phone protection"
/// provisioning action (PR 1). Only reached when `OWNER_KEYCHAIN_RECOVERY_ENABLED`.
pub fn protection_choice_view<'a>(
    phone: &'a PhoneKey,
    error: Option<&'a str>,
    password_backed: bool,
    keychain_backed: bool,
) -> Element<'a, Message> {
    let intro = Container::new(
        text(
            "Back up with a recovery password (something you type to restore), with your \
             phone (your Keychain — restore by approving on the phone, no password to \
             remember), or both so either one restores.",
        )
        .size(18)
        .align_x(iced::alignment::Horizontal::Center),
    )
    .width(Length::Fixed(600.0))
    .align_x(iced::alignment::Horizontal::Center);

    let mut col = Column::new()
        .spacing(16)
        .width(Length::Fill)
        .push(header())
        .push(Space::new().height(Length::Fixed(8.0)))
        .push(
            Row::new()
                .width(Length::Fill)
                .push(Space::new().width(Length::Fill))
                .push(icon::key_icon().size(80).color(color::ORANGE))
                .push(Space::new().width(Length::Fill)),
        )
        .push(
            Row::new()
                .width(Length::Fill)
                .push(Space::new().width(Length::Fill))
                .push(
                    text("Choose how to protect your Recovery Kit")
                        .size(22)
                        .bold(),
                )
                .push(Space::new().width(Length::Fill)),
        )
        .push(
            Row::new()
                .width(Length::Fill)
                .push(Space::new().width(Length::Fill))
                .push(intro)
                .push(Space::new().width(Length::Fill)),
        )
        .push(Space::new().height(Length::Fixed(8.0)));

    // Show what's already backed up (Update mode). Nothing in Create mode, so
    // the row is omitted entirely.
    if password_backed || keychain_backed {
        let mut pills = Row::new()
            .spacing(6)
            .align_y(Alignment::Center)
            .push(text("Already backed up:").size(13));
        if password_backed {
            pills = pills.push(backup_pill("Password"));
        }
        if keychain_backed {
            pills = pills.push(backup_pill("Keychain"));
        }
        col = col
            .push(
                Row::new()
                    .width(Length::Fill)
                    .push(Space::new().width(Length::Fill))
                    .push(pills)
                    .push(Space::new().width(Length::Fill)),
            )
            .push(Space::new().height(Length::Fixed(8.0)));
    }

    // The phone options are only selectable once a registered `owner-self` key
    // has been detected. Registration is phone-initiated (COIN-390); the desktop
    // auto-detects on entry (see `enter_after_seed_unlock`).
    let phone_ready = matches!(phone, PhoneKey::Present);
    for (label, mode, primary, needs_phone) in [
        (
            "Use a recovery password",
            RecoveryProtectionMode::Password,
            true,
            false,
        ),
        ("Use my phone", RecoveryProtectionMode::Phone, false, true),
        ("Use both", RecoveryProtectionMode::Both, false, true),
    ] {
        let base = if primary {
            ui_button::primary(None, label)
        } else {
            ui_button::secondary(None, label)
        }
        .padding([12, 16])
        .width(Length::Fixed(380.0));
        // Password never needs the phone key; phone/both stay disabled (no
        // `on_press` → greyed out) until detection reports `Present`.
        let btn = if !needs_phone || phone_ready {
            base.on_press(wrap(RecoveryKitMessage::SelectProtectionMode(mode)))
        } else {
            base
        };
        col = col.push(
            Row::new()
                .width(Length::Fill)
                .push(Space::new().width(Length::Fill))
                .push(btn)
                .push(Space::new().width(Length::Fill)),
        );
    }

    // Detection status for the phone options, plus a "Check again" link so a
    // user who just registered the key in their Keychain can re-detect without
    // leaving the screen. `Checking` shows a spinner-ish line and no link.
    let status_line: Element<'a, Message> = match phone {
        PhoneKey::Checking => text("Checking for your phone key…").size(14).into(),
        PhoneKey::Present => text(
            "Phone key found. Choose “Use my phone” or “Use both” — you'll restore \
             by approving on your Keychain.",
        )
        .size(13)
        .into(),
        PhoneKey::Absent => text(
            "No phone key yet. Create a recovery key in your Keychain app — signed in to \
             this same Connect account — to enable phone recovery. You'll need this phone \
             (or its recovery phrase) to restore.",
        )
        .size(13)
        .into(),
        PhoneKey::Error(e) => text(e).size(14).color(color::RED).into(),
    };
    col = col.push(Space::new().height(Length::Fixed(8.0))).push(
        Row::new()
            .width(Length::Fill)
            .push(Space::new().width(Length::Fill))
            .push(Container::new(status_line).width(Length::Fixed(600.0)))
            .push(Space::new().width(Length::Fill)),
    );

    // Only offer a retry when there's a reason to — no key yet (`Absent`) or a
    // failed check (`Error`). Once a key is `Present` (or while `Checking`)
    // there's nothing to re-check.
    if matches!(phone, PhoneKey::Absent | PhoneKey::Error(_)) {
        let recheck =
            ui_button::link(None, "Check again").on_press(wrap(RecoveryKitMessage::ProvisionPhone));
        col = col.push(
            Row::new()
                .width(Length::Fill)
                .push(Space::new().width(Length::Fill))
                .push(recheck)
                .push(Space::new().width(Length::Fill)),
        );
    }

    if let Some(err) = error {
        col = col.push(
            Row::new()
                .width(Length::Fill)
                .push(Space::new().width(Length::Fill))
                .push(text(err).size(16).color(color::RED))
                .push(Space::new().width(Length::Fill)),
        );
    }

    col.into()
}

/// Indeterminate "sealing to phone" state (PR 2).
pub fn phone_sealing_view() -> Element<'static, Message> {
    Column::new()
        .spacing(20)
        .width(Length::Fill)
        .align_x(Alignment::Center)
        .push(Space::new().height(Length::Fixed(80.0)))
        .push(icon::key_icon().size(100).color(color::ORANGE))
        .push(Space::new().height(Length::Fixed(16.0)))
        .push(text("Sealing to your phone…").size(20))
        .push(
            text("Encrypting your recovery material to your Keychain key and uploading.").size(14),
        )
        .into()
}

/// Indeterminate "uploading" state. Kept simple — the upload is
/// usually sub-second so a full spinner widget is overkill.
pub fn uploading_view() -> Element<'static, Message> {
    Column::new()
        .spacing(20)
        .width(Length::Fill)
        .align_x(Alignment::Center)
        .push(Space::new().height(Length::Fixed(80.0)))
        .push(icon::backup_icon().size(100).color(color::ORANGE))
        .push(Space::new().height(Length::Fixed(16.0)))
        .push(text("Encrypting and uploading…").size(20))
        .push(text("This takes a moment — Argon2id key derivation is intentionally slow.").size(14))
        .into()
}

/// The Connect-hosted backup paths a confirmed Remove will delete, given which
/// methods are present. Deterministic order (password first) so the confirm
/// screen and its regression test agree. Pure so the three method combinations
/// are unit-testable without building the Element.
pub(crate) fn confirm_remove_paths(password: bool, keychain: bool) -> Vec<&'static str> {
    let mut paths = Vec::new();
    if password {
        paths.push("Your password-encrypted Recovery Kit");
    }
    if keychain {
        paths.push("The copy sealed to your phone (Keychain)");
    }
    paths
}

/// Confirmation takeover shown before any delete happens (master F5). Names the
/// exact backup paths that will be torn down (from the current
/// `backup_overview`), states the teardown is irreversible, reassures that the
/// Cube and its local backup phrase are untouched, and warns that without a
/// hosted kit the Cube can't be restored from Connect if the device is lost.
/// The destructive action uses the alert (non-primary) style; Cancel is the
/// safe default.
pub fn confirm_remove_view<'a>(password: bool, keychain: bool) -> Element<'a, Message> {
    // Which paths will be deleted. At least one is always true when this screen
    // is reachable (Remove only shows when a method is enabled).
    let mut what = Column::new().spacing(6).width(Length::Fixed(600.0));
    for path in confirm_remove_paths(password, keychain) {
        what = what.push(text(format!("• {}", path)).size(15));
    }

    let mut col = Column::new()
        .spacing(16)
        .width(Length::Fill)
        .push(header())
        .push(Space::new().height(Length::Fixed(8.0)))
        .push(
            Row::new()
                .width(Length::Fill)
                .push(Space::new().width(Length::Fill))
                .push(icon::trash_icon().size(80).color(color::RED))
                .push(Space::new().width(Length::Fill)),
        )
        .push(
            Row::new()
                .width(Length::Fill)
                .push(Space::new().width(Length::Fill))
                .push(text("Remove your Recovery Kit?").size(22).bold())
                .push(Space::new().width(Length::Fill)),
        )
        .push(Space::new().height(Length::Fixed(8.0)))
        .push(
            Row::new()
                .width(Length::Fill)
                .push(Space::new().width(Length::Fill))
                .push(
                    Container::new(text("This will delete from your Connect account:").size(16))
                        .width(Length::Fixed(600.0)),
                )
                .push(Space::new().width(Length::Fill)),
        )
        .push(
            Row::new()
                .width(Length::Fill)
                .push(Space::new().width(Length::Fill))
                .push(what)
                .push(Space::new().width(Length::Fill)),
        )
        .push(Space::new().height(Length::Fixed(8.0)))
        .push(
            Row::new()
                .width(Length::Fill)
                .push(Space::new().width(Length::Fill))
                .push(
                    Container::new(
                        text(
                            "COINCUBE can't undo this. Your Cube and the recovery phrase you \
                             wrote down stay exactly as they are — this only removes the copies \
                             hosted in Connect. Afterwards, if you lose this device you won't be \
                             able to restore this Cube from Connect; you'll need your local \
                             recovery phrase.",
                        )
                        .size(14),
                    )
                    .width(Length::Fixed(600.0)),
                )
                .push(Space::new().width(Length::Fill)),
        )
        .push(Space::new().height(Length::Fixed(16.0)));

    col = col.push(
        Row::new()
            .spacing(20)
            .width(Length::Fill)
            .align_y(Alignment::Center)
            .push(Space::new().width(Length::Fill))
            .push(
                ui_button::secondary(None, "Cancel")
                    .on_press(wrap(RecoveryKitMessage::Cancel))
                    .padding([8, 16])
                    .width(Length::Fixed(150.0)),
            )
            .push(
                ui_button::alert(None, "Remove backup")
                    .on_press(wrap(RecoveryKitMessage::ConfirmRemove))
                    .padding([8, 16])
                    .width(Length::Fixed(220.0)),
            )
            .push(Space::new().width(Length::Fill)),
    );

    col.into()
}

/// "Removing" placeholder while the delete set is in flight.
pub fn removing_view() -> Element<'static, Message> {
    Column::new()
        .spacing(20)
        .width(Length::Fill)
        .align_x(Alignment::Center)
        .push(Space::new().height(Length::Fixed(80.0)))
        .push(icon::trash_icon().size(100).color(color::RED))
        .push(Space::new().height(Length::Fixed(16.0)))
        .push(text("Removing Recovery Kit from Connect…").size(20))
        .into()
}

/// Success screen. Shown after a successful upload; user dismisses
/// via the button which fires `DismissCompleted` and reloads status.
pub fn completed_view(
    updated_at: &str,
    has_seed: bool,
    has_descriptor: bool,
) -> Element<'static, Message> {
    let subtitle = match (has_seed, has_descriptor) {
        (true, true) => "Both your Master Seed Phrase and Wallet Descriptor are backed up.",
        (true, false) => {
            "Your Master Seed Phrase is backed up. Add your Wallet Descriptor once you \
             have a Vault."
        }
        (false, true) => "Your Wallet Descriptor is backed up.",
        (false, false) => "Nothing is currently backed up.",
    };
    Column::new()
        .spacing(20)
        .width(Length::Fill)
        .align_x(Alignment::Center)
        .push(Space::new().height(Length::Fixed(40.0)))
        .push(icon::check_icon().size(100).color(color::GREEN))
        .push(Space::new().height(Length::Fixed(16.0)))
        .push(text("Recovery Kit backed up").size(24).bold())
        .push(
            Container::new(
                text(subtitle)
                    .size(16)
                    .align_x(iced::alignment::Horizontal::Center),
            )
            .width(Length::Fixed(600.0)),
        )
        .push(Space::new().height(Length::Fixed(8.0)))
        .push(text(format!("Last updated: {}", updated_at)).size(14))
        .push(Space::new().height(Length::Fixed(24.0)))
        .push(
            ui_button::primary(None, "Back to Settings")
                .on_press(wrap(RecoveryKitMessage::DismissCompleted))
                .padding([8, 16])
                .width(Length::Fixed(300.0)),
        )
        .into()
}

pub fn error_view(message: &str) -> Element<'static, Message> {
    Column::new()
        .spacing(20)
        .width(Length::Fill)
        .align_x(Alignment::Center)
        .push(Space::new().height(Length::Fixed(40.0)))
        .push(icon::warning_icon().size(80).color(color::RED))
        .push(Space::new().height(Length::Fixed(16.0)))
        .push(
            text("Couldn't complete Recovery Kit action")
                .size(20)
                .bold(),
        )
        .push(
            Container::new(
                text(message.to_string())
                    .size(16)
                    .align_x(iced::alignment::Horizontal::Center),
            )
            .width(Length::Fixed(600.0)),
        )
        .push(Space::new().height(Length::Fixed(24.0)))
        .push(
            ui_button::primary(None, "Back to Settings")
                .on_press(wrap(RecoveryKitMessage::Cancel))
                .padding([8, 16])
                .width(Length::Fixed(300.0)),
        )
        .into()
}

/// Returns `Some(wizard)` when the flow is active and should take
/// over the settings page, `None` when the card should render inline.
pub fn dispatch<'a>(
    state: &'a RecoveryKitState,
    pin: &'a PinInput,
    status: Option<&'a RecoveryKitStatus>,
) -> Option<Element<'a, Message>> {
    match state {
        RecoveryKitState::None => None,
        RecoveryKitState::PinEntry { error, .. } => Some(pin_entry_view(pin, error.as_deref())),
        RecoveryKitState::ProtectionChoice { phone, error, .. } => {
            // Surface what's already backed up so the user knows what an Update
            // will re-encrypt / add to (empty in Create mode → no pills).
            let (password_backed, keychain_backed) = backup_methods_present(status);
            Some(protection_choice_view(
                phone,
                error.as_deref(),
                password_backed,
                keychain_backed,
            ))
        }
        RecoveryKitState::PhoneSealing { .. } => Some(phone_sealing_view()),
        RecoveryKitState::PasswordEntry {
            password,
            confirm,
            acknowledged,
            error,
            ..
        } => Some(password_entry_view(
            password,
            confirm,
            *acknowledged,
            error.as_deref(),
        )),
        RecoveryKitState::Uploading { .. } => Some(uploading_view()),
        RecoveryKitState::ConfirmRemove => {
            // Name exactly which backup paths the confirm will delete, from the
            // current per-method presence (master F5).
            let (password, keychain) = backup_methods_present(status);
            Some(confirm_remove_view(password, keychain))
        }
        RecoveryKitState::Removing => Some(removing_view()),
        RecoveryKitState::Completed {
            updated_at,
            now_has_seed,
            now_has_descriptor,
        } => Some(completed_view(
            updated_at,
            *now_has_seed,
            *now_has_descriptor,
        )),
        RecoveryKitState::Error { message } => Some(error_view(message)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The confirm screen must name exactly the backup paths that will be
    // deleted, in a deterministic order, for each method combination (plan
    // §PR2). Testing the pure path list is the Element-free equivalent of the
    // three-combination confirm-view snapshot.

    #[test]
    fn confirm_remove_paths_password_only() {
        assert_eq!(
            confirm_remove_paths(true, false),
            vec!["Your password-encrypted Recovery Kit"]
        );
    }

    #[test]
    fn confirm_remove_paths_keychain_only() {
        assert_eq!(
            confirm_remove_paths(false, true),
            vec!["The copy sealed to your phone (Keychain)"]
        );
    }

    #[test]
    fn confirm_remove_paths_both_lists_password_first() {
        assert_eq!(
            confirm_remove_paths(true, true),
            vec![
                "Your password-encrypted Recovery Kit",
                "The copy sealed to your phone (Keychain)",
            ]
        );
    }
}
