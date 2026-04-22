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

use crate::app::state::settings::recovery_kit::RecoveryKitState;
use crate::app::view::message::{Message, RecoveryKitMessage, SettingsMessage};
use crate::pin_input::PinInput;
use crate::services::recovery::{score_password, PasswordStrength};
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
    let can_submit = !password.is_empty()
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
                .on_input(|v| wrap(RecoveryKitMessage::PasswordChanged(v)))
                .secure(true)
                .size(16)
                .padding(12)
                .width(Length::Fill),
        )
        .push(Space::new().height(Length::Fixed(8.0)))
        .push(
            progress_bar(0.0..=1.0, strength_fraction)
                .style(theme::progress_bar::primary),
        )
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
                .on_input(|v| wrap(RecoveryKitMessage::ConfirmChanged(v)))
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

    let _ = PasswordStrength::VeryStrong; // silence unused import warning if strength bands shift
    col.into()
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
        .push(
            text("This takes a moment — Argon2id key derivation is intentionally slow.")
                .size(14),
        )
        .into()
}

/// "Removing" placeholder while `delete_recovery_kit` is in flight.
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
            Container::new(text(subtitle).size(16).align_x(iced::alignment::Horizontal::Center))
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
        .push(text("Couldn't complete Recovery Kit action").size(20).bold())
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
) -> Option<Element<'a, Message>> {
    match state {
        RecoveryKitState::None => None,
        RecoveryKitState::PinEntry { error, .. } => {
            Some(pin_entry_view(pin, error.as_deref()))
        }
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
        RecoveryKitState::Uploading => Some(uploading_view()),
        RecoveryKitState::Removing => Some(removing_view()),
        RecoveryKitState::Completed {
            updated_at,
            now_has_seed,
            now_has_descriptor,
        } => Some(completed_view(updated_at, *now_has_seed, *now_has_descriptor)),
        RecoveryKitState::Error { message } => Some(error_view(message)),
    }
}
