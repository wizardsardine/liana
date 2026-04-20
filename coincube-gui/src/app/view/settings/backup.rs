//! Views for the master-seed backup wizard.
//!
//! Rendered as a full-page takeover of the General Settings view when
//! `BackupSeedState != None` (see `general.rs::general_section`).
//!
//! Flow: PinEntry → Intro → RecoveryPhrase → Verification → Completed.

use coincube_ui::{
    color,
    component::{button as ui_button, text::*},
    icon, theme,
    widget::*,
};
use iced::widget::{container, Column, Row, Space};
use iced::{Alignment, Length};

use crate::app::state::settings::general::BackupSeedState;
use crate::app::view::message::{BackupWalletMessage, Message, SettingsMessage};
use crate::pin_input::PinInput;

/// Shorthand: wrap a `BackupWalletMessage` in the full `Message` path.
fn wrap(msg: BackupWalletMessage) -> Message {
    Message::Settings(SettingsMessage::BackupMasterSeed(msg))
}

/// Header for backup wizard screens: a single "< Previous" button
/// that rolls back one step in the flow.
fn header<'a>(_title: &'a str) -> Element<'a, Message> {
    Row::new()
        .spacing(10)
        .align_y(Alignment::Center)
        .push(
            ui_button::transparent_border(None, "< Previous")
                .on_press(wrap(BackupWalletMessage::PreviousStep))
                .padding([8, 16])
                .width(Length::Fixed(150.0)),
        )
        .into()
}

/// PIN re-entry screen. Shown at the start of the backup flow to gate
/// access to the mnemonic. Also doubles as the decryption password.
pub fn pin_entry_view<'a>(pin: &'a PinInput, error: Option<&'a str>) -> Element<'a, Message> {
    let mut col = Column::new()
        .spacing(20)
        .width(Length::Fill)
        .push(header("Enter PIN"))
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
                            "Enter your Cube PIN to unlock and display \
                             your 12-word recovery phrase.",
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

    // PIN input widget, routed through BackupMasterSeed(PinInput(..)).
    col = col.push(
        Row::new()
            .width(Length::Fill)
            .align_y(Alignment::Center)
            .push(Space::new().width(Length::Fill))
            .push(pin.view().map(|m| wrap(BackupWalletMessage::PinInput(m))))
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
                    .on_press(wrap(BackupWalletMessage::PreviousStep))
                    .padding([8, 16])
                    .width(Length::Fixed(150.0)),
            )
            .push({
                let btn = ui_button::primary(None, "Unlock")
                    .padding([8, 16])
                    .width(Length::Fixed(200.0));
                if pin.is_complete() {
                    btn.on_press(wrap(BackupWalletMessage::VerifyPin))
                } else {
                    btn
                }
            })
            .push(Space::new().width(Length::Fill)),
    );

    col.into()
}

/// Intro screen with security warning + "I understand" checkbox.
pub fn intro_view(checked: bool) -> Element<'static, Message> {
    let primary_color = color::ORANGE;
    Column::new()
        .spacing(20)
        .width(Length::Fill)
        .push(header("Back up your wallet"))
        .push(Space::new().height(Length::Fixed(16.0)))
        .push(
            Row::new()
                .width(Length::Fill)
                .align_y(Alignment::Center)
                .push(Space::new().width(Length::Fill))
                .push(icon::file_earmark_icon().size(140).color(primary_color))
                .push(Space::new().width(Length::Fill))
        )
        .push(Space::new().height(Length::Fixed(16.0)))
        .push(
            Container::new(
                Column::new()
                    .align_x(Alignment::Center)
                    .push(
                        text("You will be shown 12 words. Write them down numbered in the same order shown and keep them in a safe place.")
                            .size(20)
                            .align_x(iced::alignment::Horizontal::Center)
                    )
                    .push(
                        text("Do not share these words with anyone.")
                            .size(20).bold().color(primary_color)
                            .align_x(iced::alignment::Horizontal::Center)
                    )
                    .push(
                        text("Without them, you will not be able to restore your wallet if you lose your computer.")
                            .size(20)
                            .align_x(iced::alignment::Horizontal::Center)
                    )
            )
            .padding(20)
            .width(Length::Fill)
            .align_x(iced::alignment::Horizontal::Center)
        )
        .push(Space::new().height(Length::Fixed(16.0)))
        .push(
            Row::new()
                .width(Length::Fill)
                .align_y(Alignment::Center)
                .push(Space::new().width(Length::Fill))
                .push(
                    CheckBox::new(checked)
                        .label("I UNDERSTAND THAT IF I LOSE THESE WORDS, MY FUNDS CANNOT BE RECOVERED")
                        .on_toggle(|_| wrap(BackupWalletMessage::ToggleBackupIntroCheck))
                        .style(theme::checkbox::primary)
                        .size(20)
                )
                .push(Space::new().width(Length::Fill))
        )
        .push(Space::new().height(Length::Fixed(16.0)))
        .push(
            Row::new()
                .spacing(20)
                .width(Length::Fill)
                .align_y(Alignment::Center)
                .push(Space::new().width(Length::Fill))
                .push(
                    ui_button::secondary(None, "Cancel")
                        .on_press(wrap(BackupWalletMessage::PreviousStep))
                        .padding([8, 16])
                        .width(Length::Fixed(150.0)),
                )
                .push({
                    let btn = ui_button::primary(None, "Show My Recovery Phrase")
                        .padding([8, 16])
                        .width(Length::Fixed(300.0));
                    if checked {
                        btn.on_press(wrap(BackupWalletMessage::NextStep))
                    } else {
                        btn
                    }
                })
                .push(Space::new().width(Length::Fill))
        )
        .into()
}

/// Show the 12 mnemonic words in a 3×4 grid.
pub fn recovery_phrase_view<'a>(mnemonic: &'a [String]) -> Element<'a, Message> {
    let mut grid = Column::new().spacing(30).align_x(Alignment::Center);

    // 3 rows × 4 columns = 12 words
    for row in 0..3 {
        let mut row_widget = Row::new().spacing(40).align_y(Alignment::Center);
        for col in 0..4 {
            let index = row * 4 + col;
            let word = mnemonic.get(index).map(|s| s.as_str()).unwrap_or("???");

            let word_container = Container::new(
                text(format!("{}. {}", index + 1, word))
                    .size(16)
                    .align_x(iced::alignment::Horizontal::Center),
            )
            .padding(12)
            .width(Length::Fixed(150.0))
            .align_x(iced::alignment::Horizontal::Center)
            .style(|_theme| container::Style {
                border: iced::Border {
                    color: iced::Color::from_rgb8(0x80, 0x80, 0x80),
                    width: 1.0,
                    radius: 10.0.into(),
                },
                ..Default::default()
            });

            row_widget = row_widget.push(word_container);
        }
        grid = grid.push(row_widget);
    }

    Column::new()
        .spacing(20)
        .width(Length::Fill)
        .push(header("Your Recovery Phrase"))
        .push(
            Row::new()
                .width(Length::Fill)
                .align_y(Alignment::Center)
                .push(Space::new().width(Length::Fill))
                .push(
                    Container::new(
                        text("Write these words down in order and keep them offline. Anyone with these words can access your wallet.")
                            .size(20)
                            .align_x(iced::alignment::Horizontal::Center)
                    )
                    .width(Length::Fixed(700.0))
                    .align_x(iced::alignment::Horizontal::Center)
                )
                .push(Space::new().width(Length::Fill))
        )
        .push(Space::new().height(Length::Fixed(24.0)))
        .push(
            Row::new()
                .width(Length::Fill)
                .align_y(Alignment::Center)
                .push(Space::new().width(Length::Fill))
                .push(grid)
                .push(Space::new().width(Length::Fill))
        )
        .push(Space::new().height(Length::Fixed(24.0)))
        .push(
            Row::new()
                .spacing(20)
                .width(Length::Fill)
                .align_y(Alignment::Center)
                .push(Space::new().width(Length::Fill))
                .push(
                    ui_button::secondary(None, "Back")
                        .on_press(wrap(BackupWalletMessage::PreviousStep))
                        .padding([8, 16])
                        .width(Length::Fixed(150.0)),
                )
                .push(
                    ui_button::primary(None, "I've Written It Down")
                        .on_press(wrap(BackupWalletMessage::NextStep))
                        .padding([8, 16])
                        .width(Length::Fixed(300.0)),
                )
                .push(Space::new().width(Length::Fill))
        )
        .into()
}

/// Helper: a labelled word input field with a bottom border.
fn word_input_field<'a, F, S>(
    word_num: usize,
    word_value: &'a str,
    no_border_style: S,
    on_input: F,
) -> Element<'a, Message>
where
    F: Fn(String) -> Message + 'a,
    S: Fn(
            &coincube_ui::theme::Theme,
            iced::widget::text_input::Status,
        ) -> iced::widget::text_input::Style
        + 'a,
{
    Column::new()
        .push(
            Row::new()
                .spacing(12)
                .align_y(Alignment::Center)
                .push(text(format!("{}.", word_num)).size(18))
                .push(
                    TextInput::new("", word_value)
                        .on_input(on_input)
                        .padding(8)
                        .width(Length::Fixed(300.0))
                        .style(no_border_style),
                ),
        )
        .push(
            Container::new(Space::new().height(Length::Fixed(0.0)))
                .width(Length::Fixed(340.0))
                .height(Length::Fixed(1.0))
                .style(|_theme| container::Style {
                    background: Some(iced::Background::Color(iced::Color::from_rgb8(
                        0x80, 0x80, 0x80,
                    ))),
                    ..Default::default()
                }),
        )
        .into()
}

/// Verification screen — ask the user for 3 random words from the mnemonic.
pub fn verification_view<'a>(
    word_indices: &'a [usize; 3],
    word_inputs: &'a [String; 3],
    error: Option<&'a str>,
    saving: bool,
) -> Element<'a, Message> {
    let all_filled = word_inputs.iter().all(|w| !w.is_empty());

    let mut content = Column::new().spacing(20).width(Length::Fill);

    content = content.push(header("Verify Your Recovery Phrase"));

    content = content.push(
        Row::new()
            .width(Length::Fill)
            .align_y(Alignment::Center)
            .push(Space::new().width(Length::Fill))
            .push(
                text("To make sure you saved your recovery phrase correctly, please enter the correct words.")
                    .size(20)
                    .align_x(iced::alignment::Horizontal::Center)
                    .width(Length::Fixed(700.0))
            )
            .push(Space::new().width(Length::Fill))
    );

    content = content.push(Space::new().height(Length::Fixed(24.0)));

    if let Some(err) = error {
        content = content.push(
            Row::new()
                .width(Length::Fill)
                .align_y(Alignment::Center)
                .push(Space::new().width(Length::Fill))
                .push(
                    Container::new(text(err).size(16).color(color::RED))
                        .padding(12)
                        .style(|_theme| container::Style {
                            background: Some(iced::Background::Color(iced::Color::from_rgb8(
                                0x4c, 0x01, 0x01,
                            ))),
                            border: iced::Border {
                                radius: 8.0.into(),
                                ..Default::default()
                            },
                            ..Default::default()
                        }),
                )
                .push(Space::new().width(Length::Fill)),
        );
        content = content.push(Space::new().height(Length::Fixed(16.0)));
    }

    // Custom text input style with no border
    let no_border_style = |theme: &coincube_ui::theme::Theme,
                           status: iced::widget::text_input::Status| {
        let default_style = theme::text_input::primary(theme, status);
        iced::widget::text_input::Style {
            border: iced::Border {
                width: 0.0,
                ..default_style.border
            },
            ..default_style
        }
    };

    let mut input_fields = Column::new().spacing(40).align_x(Alignment::Center);

    for (i, &word_idx) in word_indices.iter().enumerate() {
        input_fields = input_fields.push(word_input_field(
            word_idx,
            &word_inputs[i],
            no_border_style,
            move |input| {
                wrap(BackupWalletMessage::WordInput {
                    index: word_idx as u8,
                    input,
                })
            },
        ));
    }

    content = content.push(
        Row::new()
            .width(Length::Fill)
            .align_y(Alignment::Center)
            .push(Space::new().width(Length::Fill))
            .push(input_fields)
            .push(Space::new().width(Length::Fill)),
    );

    content = content.push(Space::new().height(Length::Fixed(24.0)));

    content = content.push(
        Row::new()
            .spacing(20)
            .width(Length::Fill)
            .align_y(Alignment::Center)
            .push(Space::new().width(Length::Fill))
            .push(
                ui_button::secondary(None, "Back")
                    .on_press(wrap(BackupWalletMessage::PreviousStep))
                    .padding([8, 16])
                    .width(Length::Fixed(150.0)),
            )
            .push({
                let label = if saving { "Saving…" } else { "Verify" };
                let btn = ui_button::primary(None, label)
                    .padding([8, 16])
                    .width(Length::Fixed(300.0));
                if all_filled && !saving {
                    btn.on_press(wrap(BackupWalletMessage::VerifyPhrase))
                } else {
                    btn
                }
            })
            .push(Space::new().width(Length::Fill)),
    );

    content.into()
}

/// Completed screen — backup is recorded, show a confirmation.
pub fn completed_view() -> Element<'static, Message> {
    let primary_color = color::ORANGE;

    Column::new()
        .spacing(20)
        .width(Length::Fill)
        .push(header("Backup Complete"))
        .push(Space::new().height(Length::Fixed(20.0)))
        .push(
            Row::new()
                .width(Length::Fill)
                .align_y(Alignment::Center)
                .push(Space::new().width(Length::Fill))
                .push(icon::check_circle_icon().size(140).color(primary_color))
                .push(Space::new().width(Length::Fill))
        )
        .push(Space::new().height(Length::Fixed(16.0)))
        .push(
            Row::new()
                .width(Length::Fill)
                .align_y(Alignment::Center)
                .push(Space::new().width(Length::Fill))
                .push(
                    text("Your recovery phrase has been securely backed up. Keep it safe. It's the only way to restore your wallet.")
                        .size(20)
                        .align_x(iced::alignment::Horizontal::Center)
                        .width(Length::Fixed(700.0))
                )
                .push(Space::new().width(Length::Fill))
        )
        .push(Space::new().height(Length::Fixed(24.0)))
        .push(
            Row::new()
                .width(Length::Fill)
                .align_y(Alignment::Center)
                .push(Space::new().width(Length::Fill))
                .push(
                    ui_button::primary(None, "Back to Settings")
                        .on_press(wrap(BackupWalletMessage::Complete))
                        .padding([8, 16])
                        .width(Length::Fixed(300.0))
                )
                .push(Space::new().width(Length::Fill))
        )
        .into()
}

/// Shown for passkey-derived Cubes. The mnemonic can be re-derived from
/// the WebAuthn PRF output, but passkey re-authentication isn't wired up
/// yet. Tell the user what's going on and how to proceed.
pub fn passkey_pending_view() -> Element<'static, Message> {
    Column::new()
        .spacing(20)
        .width(Length::Fill)
        .push(header("Backup via Passkey"))
        .push(Space::new().height(Length::Fixed(20.0)))
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
                            "This Cube uses a passkey-derived master key. \
                             To display your 12-word recovery phrase we need to \
                             re-authenticate with your passkey — this feature is \
                             coming soon. In the meantime, make sure you keep \
                             access to the device or security key that holds \
                             your passkey.",
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
                .push(
                    ui_button::primary(None, "Back to Settings")
                        .on_press(wrap(BackupWalletMessage::PreviousStep))
                        .padding([8, 16])
                        .width(Length::Fixed(300.0)),
                )
                .push(Space::new().width(Length::Fill)),
        )
        .into()
}

/// Fallback — shouldn't be visible in the normal flow but useful for
/// debugging state transitions.
pub fn dispatch<'a>(
    state: &'a BackupSeedState,
    pin: &'a PinInput,
    mnemonic: Option<&'a [String]>,
) -> Option<Element<'a, Message>> {
    match state {
        BackupSeedState::None => None,
        BackupSeedState::PinEntry { error } => Some(pin_entry_view(pin, error.as_deref())),
        BackupSeedState::Intro(checked) => Some(intro_view(*checked)),
        BackupSeedState::RecoveryPhrase => {
            // Without a loaded mnemonic we can't show anything useful —
            // bail back to the normal settings page. This shouldn't happen
            // under normal flow because the mnemonic is loaded in VerifyPin.
            let mnemonic = mnemonic?;
            Some(recovery_phrase_view(mnemonic))
        }
        BackupSeedState::Verification {
            word_indices,
            word_inputs,
            error,
            saving,
        } => Some(verification_view(
            word_indices,
            word_inputs,
            error.as_deref(),
            *saving,
        )),
        BackupSeedState::Completed => Some(completed_view()),
        BackupSeedState::PasskeyPending => Some(passkey_pending_view()),
    }
}
