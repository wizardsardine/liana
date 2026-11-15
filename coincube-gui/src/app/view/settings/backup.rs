//! Master-seed backup views, shared with Cube creation through a message mapper.
//! Settings retains its page header while the focused flow replaces the rows.
//! Flow: PinEntry → Intro → RecoveryPhrase → Verification → Completed.

use coincube_ui::locale::Text as CopyText;
use coincube_ui::{
    component::tenshu::{self, ActionKind, Tone},
    theme,
    widget::{CheckBox, Container, Element, TextInput},
};
use iced::widget::{Column, Row, Space};
use iced::{Alignment, Length};

use super::page;
use crate::app::state::settings::general::BackupSeedState;
use crate::app::view::message::{BackupWalletMessage, Message, SettingsMessage};
use crate::chain::{canonical_network_name, ChainId};
use crate::pin_input::PinInput;

fn wrap(msg: BackupWalletMessage) -> Message {
    Message::Settings(SettingsMessage::BackupMasterSeed(msg))
}

/// Match Settings' type and rhythm, keeping the task readable on wide windows.
fn flow<'a, M: 'a>(title: &'a str, detail: &'a str) -> Column<'a, M, theme::Theme, iced::Renderer> {
    Column::new()
        .width(Length::Fill)
        .max_width(640)
        .spacing(24)
        .push(Space::new().height(Length::Fixed(8.0)))
        .push(
            Column::new()
                .spacing(6)
                .push(tenshu::heading(title))
                .push(tenshu::body(detail).style(theme::tenshu::text::muted)),
        )
}

fn actions<'a, M: Clone + 'a>(
    back: &'a str,
    next: &'a str,
    enabled: bool,
    next_message: BackupWalletMessage,
    on: fn(BackupWalletMessage) -> M,
) -> Element<'a, M> {
    Column::new()
        .spacing(16)
        .push(page::rule())
        .push(
            Row::new()
                .spacing(8)
                .align_y(Alignment::Center)
                .push(page::plain_button(
                    back,
                    ActionKind::Secondary,
                    Some(on(BackupWalletMessage::PreviousStep)),
                ))
                .push(Space::new().width(Length::Fill))
                .push(page::plain_button(
                    next,
                    ActionKind::Primary,
                    enabled.then(|| on(next_message)),
                )),
        )
        .into()
}

fn error_note<'a, M: 'a>(message: &'a str) -> Element<'a, M> {
    page::note(
        Tone::Bad,
        tenshu::body(coincube_ui::locale::notice(message.to_owned()))
            .style(|t| theme::tenshu::text::tone(t, Tone::Bad)),
        None,
    )
}

/// Reuse the Launcher's compact PIN fields, including masking and digit focus.
pub fn pin_entry_view<'a>(pin: &'a PinInput, error: Option<&'a str>) -> Element<'a, Message> {
    let mut content = page::illustrated_flow(
        CopyText::UnlockYourSeedPhrase.get(),
        CopyText::EnterYourCubePinToUnlockAndDisplayYour.get(),
        coincube_ui::image::pin_unlock_lock(),
        coincube_ui::illustration::Slot::Pin,
    )
    .push(
        Column::new()
            .spacing(8)
            .push(tenshu::label(CopyText::CubePin.get()))
            .push(
                pin.face_view()
                    .map(|m| wrap(BackupWalletMessage::PinInput(m))),
            ),
    );
    if let Some(error) = error {
        content = content.push(error_note(error));
    }
    content
        .push(actions(
            CopyText::Cancel.get(),
            CopyText::Unlock.get(),
            pin.is_complete(),
            BackupWalletMessage::VerifyPin,
            wrap,
        ))
        .into()
}

/// Keep the acknowledgement explicit and preserve caller-specific notices.
pub fn intro_view<M: Clone + 'static>(
    checked: bool,
    note: Option<Element<'static, M>>,
    on: fn(BackupWalletMessage) -> M,
) -> Element<'static, M> {
    flow(
        CopyText::BackUpYourSeedPhrase.get(),
        CopyText::YouLlSee12WordsWriteThemOnPaper.get(),
    )
    .push(
        tenshu::label(CopyText::AnyoneWhoSeesTheseWordsCanTakeYourBitcoin.get())
            .style(|t| theme::tenshu::text::tone(t, Tone::Warn)),
    )
    .push(note)
    .push(
        CheckBox::new(checked)
            .label(CopyText::IUnderstandThatIfILoseTheseWordsAnd.get())
            .on_toggle(move |_| on(BackupWalletMessage::ToggleBackupIntroCheck))
            .style(theme::checkbox::primary)
            .text_size(14)
            .size(18)
            .spacing(10)
            .width(Length::Fill),
    )
    .push(actions(
        CopyText::Cancel.get(),
        CopyText::ShowSeedPhrase.get(),
        checked,
        BackupWalletMessage::NextStep,
        on,
    ))
    .into()
}

/// The line that names the Cube's network beside its words. A BIP39 phrase is
/// identical on every chain, so a paper backup that omits the network cannot
/// say which chain to restore it to.
pub fn network_note(chain: ChainId) -> String {
    CopyText::WriteTheNetworkWithYourWords.get().replacen(
        "{network}",
        canonical_network_name(chain),
        1,
    )
}

/// Twelve numbered words, with flexible columns and theme-aware separators.
pub fn recovery_phrase_view<'a, M: Clone + 'a>(
    mnemonic: &'a [String],
    chain: ChainId,
    on: fn(BackupWalletMessage) -> M,
) -> Element<'a, M> {
    let mut grid = Column::new()
        .spacing(0)
        .width(Length::Fill)
        .push(page::rule());
    for row in 0..3 {
        let mut words = Row::new().spacing(16).width(Length::Fill);
        for col in 0..4 {
            let index = row * 4 + col;
            let word = mnemonic.get(index).map(|s| s.as_str()).unwrap_or("???");
            words = words.push(
                Container::new(
                    Row::new()
                        .spacing(8)
                        .align_y(Alignment::Center)
                        .push(Container::new(tenshu::caption(format!("{}", index + 1))).width(22))
                        .push(tenshu::label(word)),
                )
                .width(Length::FillPortion(1))
                .padding([14, 0]),
            );
        }
        grid = grid.push(words).push(page::rule());
    }
    flow(
        CopyText::WriteDownYourSeedPhrase.get(),
        CopyText::WriteEachWordNextToItsNumberThenCheck.get(),
    )
    .push(tenshu::label(network_note(chain)).style(|t| theme::tenshu::text::tone(t, Tone::Warn)))
    .push(grid)
    .push(actions(
        CopyText::Back.get(),
        CopyText::IVeWrittenItDown.get(),
        true,
        BackupWalletMessage::NextStep,
        on,
    ))
    .into()
}

/// Three labelled, bordered fields retain the app's visible input focus style.
pub fn verification_view<'a, M: Clone + 'a>(
    word_indices: &'a [usize; 3],
    word_inputs: &'a [String; 3],
    error: Option<&'a str>,
    saving: bool,
    on: fn(BackupWalletMessage) -> M,
) -> Element<'a, M> {
    let mut content = flow(
        CopyText::ConfirmYourSeedPhrase.get(),
        CopyText::TypeTheWordForEachNumberFromYourPaper.get(),
    );
    let mut fields = Column::new().spacing(16).width(Length::Fill).max_width(360);
    for (i, &word_idx) in word_indices.iter().enumerate() {
        fields = fields.push(
            Column::new()
                .spacing(6)
                .push(tenshu::label(CopyText::RecoveryWordNumber.get().replacen(
                    "{number}",
                    &word_idx.to_string(),
                    1,
                )))
                .push(
                    TextInput::new("", &word_inputs[i])
                        .on_input(move |input| {
                            on(BackupWalletMessage::WordInput {
                                index: word_idx as u8,
                                input,
                            })
                        })
                        .size(14)
                        .padding([10, 12])
                        .width(Length::Fill)
                        .style(|t, status| theme::tenshu::control::field(t, status, 8.0, false)),
                ),
        );
    }
    content = content.push(fields);
    if let Some(error) = error {
        content = content.push(error_note(error));
    }
    content
        .push(actions(
            CopyText::Back.get(),
            if saving {
                CopyText::Saving.get()
            } else {
                CopyText::Verify.get()
            },
            word_inputs.iter().all(|w| !w.trim().is_empty()) && !saving,
            BackupWalletMessage::VerifyPhrase,
            on,
        ))
        .into()
}

pub fn completed_view<M: Clone + 'static>(on: fn(BackupWalletMessage) -> M) -> Element<'static, M> {
    flow(
        CopyText::SeedPhraseConfirmed.get(),
        CopyText::TheThreeWordCheckWasSavedForThisCube.get(),
    )
    .push(page::status(page::Status::Ok, CopyText::Confirmed.get()))
    .push(page::rule())
    .push(page::plain_button(
        CopyText::BackToSettings.get(),
        ActionKind::Primary,
        Some(on(BackupWalletMessage::Complete)),
    ))
    .into()
}

pub fn passkey_reauth_view() -> Element<'static, Message> {
    flow(
        CopyText::UnlockYourSeedPhrase.get(),
        CopyText::ConfirmWithTouchIdToShowThisCubeS.get(),
    )
    .push(page::status(
        page::Status::Busy,
        CopyText::WaitingForTouchId.get(),
    ))
    .push(page::rule())
    .push(page::button(
        "tenshu.backup.passkey.cancel",
        CopyText::Cancel.get(),
        Some(wrap(BackupWalletMessage::PreviousStep)),
    ))
    .into()
}

pub fn backup_error_view(message: &str) -> Element<'_, Message> {
    flow(CopyText::CouldnTUnlockYourSeedPhrase.get(), "")
        .push(error_note(message))
        .push(page::rule())
        .push(page::primary(
            "tenshu.backup.error.back",
            CopyText::BackToSettings.get(),
            Some(wrap(BackupWalletMessage::PreviousStep)),
        ))
        .into()
}

pub fn dispatch<'a>(
    state: &'a BackupSeedState,
    pin: &'a PinInput,
    mnemonic: Option<&'a [String]>,
    chain: ChainId,
) -> Option<Element<'a, Message>> {
    match state {
        BackupSeedState::None => None,
        BackupSeedState::PinEntry { error } => Some(pin_entry_view(pin, error.as_deref())),
        BackupSeedState::Intro(checked) => Some(intro_view(*checked, None, wrap)),
        BackupSeedState::RecoveryPhrase => Some(recovery_phrase_view(mnemonic?, chain, wrap)),
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
            wrap,
        )),
        BackupSeedState::Completed => Some(completed_view(wrap)),
        BackupSeedState::PasskeyReauth => Some(passkey_reauth_view()),
        BackupSeedState::Error(message) => Some(backup_error_view(message)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::view::shared::landing::capture;

    /// Render every state using synthetic placeholders, including the minimum
    /// desktop window and both palettes. The shared probe rejects text overflow.
    #[test]
    fn seed_flow_fits_desktop_sizes_and_themes() {
        let pin = PinInput::new();
        // Deliberately not a mnemonic: no wallet material belongs in captures.
        let words: Vec<String> = (1..=12).map(|i| format!("sample-{i:02}")).collect();
        let states = [
            ("pin", BackupSeedState::PinEntry { error: None }),
            (
                "pin-error",
                BackupSeedState::PinEntry {
                    error: Some("Incorrect PIN. Please try again.".into()),
                },
            ),
            ("intro", BackupSeedState::Intro(false)),
            ("intro-checked", BackupSeedState::Intro(true)),
            ("words", BackupSeedState::RecoveryPhrase),
            (
                "verification",
                BackupSeedState::Verification {
                    word_indices: [2, 6, 11],
                    word_inputs: Default::default(),
                    error: None,
                    saving: false,
                },
            ),
            (
                "verification-error",
                BackupSeedState::Verification {
                    word_indices: [2, 6, 11],
                    word_inputs: Default::default(),
                    error: Some(
                        "Those words don't match. Check your written copy and try again.".into(),
                    ),
                    saving: false,
                },
            ),
            (
                "saving",
                BackupSeedState::Verification {
                    word_indices: [2, 6, 11],
                    word_inputs: ["sample-02".into(), "sample-06".into(), "sample-11".into()],
                    error: None,
                    saving: true,
                },
            ),
            ("complete", BackupSeedState::Completed),
            ("passkey", BackupSeedState::PasskeyReauth),
            (
                "error",
                BackupSeedState::Error(
                    "Authentication was cancelled. You can try again from Backup & protection."
                        .into(),
                ),
            ),
        ];
        for (appearance, palette) in [
            ("dark", theme::Theme::dark()),
            ("light", theme::Theme::light()),
        ] {
            for size in [(1024, 768), (1440, 900)] {
                for (name, state) in &states {
                    let inspection = capture::inspect(
                        &format!("seed-flow-{name}"),
                        // One of the longest canonical names, so the network line is
                        // measured at its widest.
                        dispatch(state, &pin, Some(&words), ChainId::BitcoinBlake2bTestnet4)
                            .unwrap(),
                        size,
                        appearance,
                        palette,
                    );
                    assert!(
                        !inspection.text.is_empty(),
                        "{} must show its instructions",
                        name
                    );
                    assert!(
                        inspection
                            .text
                            .iter()
                            .all(|(_, bounds)| bounds.y + bounds.height <= (size.1 - 100) as f32),
                        "{} must fit above the fold at {:?}",
                        name,
                        size
                    );
                    if name == &"words" {
                        for word in &words {
                            assert!(
                                inspection.contains(word),
                                "every numbered word must be visible"
                            );
                        }
                        assert!(
                            inspection.contains("Bitcoin BLAKE2b · Testnet4"),
                            "the words screen must name the Cube's network"
                        );
                    }
                }
            }
        }
    }

    /// The seed words are the same on every chain, so the paper copy must
    /// carry the network. Each chain gets its stable technical name.
    #[test]
    fn network_note_names_each_chain() {
        for chain in ChainId::ALL {
            let note = network_note(chain);
            assert!(note.contains(canonical_network_name(chain)), "{}", note);
            assert!(!note.contains("{network}"), "{}", note);
        }
        assert_ne!(
            network_note(ChainId::Bitcoin),
            network_note(ChainId::BitcoinBlake2b)
        );
    }
}
