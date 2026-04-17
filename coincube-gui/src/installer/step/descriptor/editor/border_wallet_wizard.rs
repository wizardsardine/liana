//! Border Wallet creation wizard.
//!
//! Multi-step modal that guides the user through creating a Border Wallet
//! key derived from a visual grid pattern. Can be used in any spending path.
//!
//! Steps: Intro → RecoveryPhrase → Grid → Checksum → Confirm

use coincube_core::{
    border_wallet::{
        build_mnemonic, derive_enrollment, BorderWalletEnrollment, CellRef, GridRecoveryPhrase,
        OrderedPattern, WordGrid, PATTERN_LENGTH,
    },
    miniscript::{
        bitcoin::{bip32::DerivationPath, Network},
        descriptor::{DescriptorPublicKey, DescriptorXKey, Wildcard},
    },
};
use iced::{alignment::Horizontal, widget::scrollable, Length, Task};
use zeroize::{Zeroize, Zeroizing};

use coincube_ui::{
    color,
    component::{
        button, form,
        modal::{self},
        text::{p1_bold, p1_regular, text},
    },
    icon, theme,
    widget::{Button, Column, Container, Element, Row, TextInput},
};

use std::sync::{Arc, Mutex};

use crate::{
    hw::HardwareWallets,
    installer::{
        descriptor::{Key, KeySource},
        message::{self, Message},
        step::descriptor::editor::key::SelectedKey,
    },
    signer::Signer,
};

/// Wizard step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WizardStep {
    Intro,
    RecoveryPhrase,
    Grid,
    Checksum,
    Confirm,
}

/// Messages specific to the Border Wallet wizard.
#[derive(Clone)]
pub enum BorderWalletWizardMessage {
    Next,
    Previous,
    GeneratePhrase,
    PhraseWordEdited(usize, String),
    ToggleCellSelection(u16, u8),
    UndoLastCell,
    ClearPattern,
    ConfirmEnrollment,
}

// Manual Debug impl to redact recovery phrase words from logs.
impl std::fmt::Debug for BorderWalletWizardMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PhraseWordEdited(idx, _) => f
                .debug_tuple("PhraseWordEdited")
                .field(idx)
                .field(&"<redacted>")
                .finish(),
            Self::Next => write!(f, "Next"),
            Self::Previous => write!(f, "Previous"),
            Self::GeneratePhrase => write!(f, "GeneratePhrase"),
            Self::ToggleCellSelection(r, c) => f
                .debug_tuple("ToggleCellSelection")
                .field(r)
                .field(c)
                .finish(),
            Self::UndoLastCell => write!(f, "UndoLastCell"),
            Self::ClearPattern => write!(f, "ClearPattern"),
            Self::ConfirmEnrollment => write!(f, "ConfirmEnrollment"),
        }
    }
}

pub struct BorderWalletWizard {
    network: Network,
    coordinates: Vec<(usize, usize)>,
    step: WizardStep,

    // Recovery phrase (12 words) — zeroized on drop via custom Drop impl below.
    phrase_words: Vec<form::Value<String>>,
    phrase_valid: bool,

    // Grid + pattern
    grid: Option<WordGrid>,
    pattern: OrderedPattern,

    // Derived data — only the checksum word is retained, never the full mnemonic.
    checksum_word: Option<String>,
    enrollment: Option<BorderWalletEnrollment>,

    /// When true, allow random phrase generation. When false (default),
    /// the "Generate" button derives from the master signer via BIP-85.
    allow_random_grid_phrase: bool,
    /// Master signer for BIP-85 grid phrase derivation.
    signer: Option<Arc<Mutex<Signer>>>,

    error: Option<String>,
}

impl BorderWalletWizard {
    pub fn new(
        network: Network,
        coordinates: Vec<(usize, usize)>,
        signer: Arc<Mutex<Signer>>,
        allow_random_grid_phrase: bool,
    ) -> Self {
        Self {
            network,
            coordinates,
            step: WizardStep::Intro,
            phrase_words: vec![form::Value::default(); 12],
            phrase_valid: false,
            grid: None,
            pattern: OrderedPattern::new(),
            checksum_word: None,
            enrollment: None,
            allow_random_grid_phrase,
            signer: Some(signer),
            error: None,
        }
    }

    fn on_next(&mut self) -> Task<Message> {
        self.error = None;
        match self.step {
            WizardStep::Intro => {
                self.step = WizardStep::RecoveryPhrase;
            }
            WizardStep::RecoveryPhrase => {
                let phrase_str = Zeroizing::new(
                    self.phrase_words
                        .iter()
                        .map(|w| w.value.trim().to_lowercase())
                        .collect::<Vec<_>>()
                        .join(" "),
                );

                match GridRecoveryPhrase::from_phrase(&phrase_str) {
                    Ok(rp) => {
                        self.grid = Some(rp.generate_grid());
                        self.pattern = OrderedPattern::new();
                        self.step = WizardStep::Grid;
                    }
                    Err(_) => {
                        self.error = Some(
                            "Invalid recovery phrase. Please enter a valid 12-word BIP39 mnemonic."
                                .to_string(),
                        );
                    }
                }
            }
            WizardStep::Grid => {
                if !self.pattern.is_complete() {
                    self.error = Some(format!(
                        "Please select exactly {} cells. Currently selected: {}",
                        PATTERN_LENGTH,
                        self.pattern.len()
                    ));
                    return Task::none();
                }
                if let Some(grid) = &self.grid {
                    match build_mnemonic(grid, &self.pattern) {
                        Ok((mnemonic, checksum_word)) => {
                            // Keep only the checksum word; the full mnemonic is
                            // used solely for derive_enrollment and then dropped.
                            self.checksum_word = Some(checksum_word.to_string());

                            let secp =
                                coincube_core::miniscript::bitcoin::secp256k1::Secp256k1::new();
                            match derive_enrollment(&mnemonic, self.network, &secp) {
                                Ok(enrollment) => {
                                    self.enrollment = Some(enrollment);
                                    self.step = WizardStep::Checksum;
                                }
                                Err(e) => {
                                    self.error = Some(format!("Key derivation failed: {:?}", e));
                                }
                            }
                            // `mnemonic` is dropped here — no full mnemonic retained.
                        }
                        Err(e) => {
                            self.error = Some(format!("Mnemonic construction failed: {:?}", e));
                        }
                    }
                }
            }
            WizardStep::Checksum => {
                self.step = WizardStep::Confirm;
            }
            WizardStep::Confirm => {
                return self.on_confirm();
            }
        }
        Task::none()
    }

    fn on_previous(&mut self) -> Task<Message> {
        self.error = None;
        match self.step {
            WizardStep::Intro => {}
            WizardStep::RecoveryPhrase => {
                self.step = WizardStep::Intro;
            }
            WizardStep::Grid => {
                self.step = WizardStep::RecoveryPhrase;
            }
            WizardStep::Checksum => {
                self.step = WizardStep::Grid;
            }
            WizardStep::Confirm => {
                self.step = WizardStep::Checksum;
            }
        }
        Task::none()
    }

    fn on_generate_phrase(&mut self) -> Task<Message> {
        // Default: derive from master signer via BIP-85.
        // Fallback to random if allow_random_grid_phrase is true or signer is unavailable.
        let result = if !self.allow_random_grid_phrase {
            if let Some(signer) = &self.signer {
                match signer.lock() {
                    Ok(signer) => signer
                        .derive_grid_recovery_phrase()
                        .map_err(|e| format!("{:?}", e)),
                    Err(e) => {
                        self.error = Some(format!(
                            "Failed to generate recovery phrase: signer lock poisoned: {}",
                            e
                        ));
                        return Task::none();
                    }
                }
            } else {
                GridRecoveryPhrase::generate().map_err(|e| format!("{:?}", e))
            }
        } else {
            GridRecoveryPhrase::generate().map_err(|e| format!("{:?}", e))
        };

        match result {
            Ok(rp) => {
                let words: Vec<&str> = rp.as_str().split_whitespace().collect();
                for (i, word) in words.iter().enumerate() {
                    if i < 12 {
                        self.phrase_words[i] = form::Value {
                            value: word.to_string(),
                            warning: None,
                            valid: true,
                        };
                    }
                }
                self.phrase_valid = true;
                self.error = None;
            }
            Err(e) => {
                self.error = Some(format!("Failed to generate recovery phrase: {}", e));
            }
        }
        Task::none()
    }

    fn on_phrase_word_edited(&mut self, index: usize, word: String) -> Task<Message> {
        if index < 12 {
            self.phrase_words[index].value = word;
            self.phrase_words[index].valid = true;
            self.phrase_words[index].warning = None;
        }
        self.phrase_valid = self.phrase_words.iter().all(|w| !w.value.trim().is_empty());
        Task::none()
    }

    fn on_toggle_cell(&mut self, row: u16, col: u8) -> Task<Message> {
        let cell = CellRef::new(row, col);
        if let Some(pos) = self.pattern.cells().iter().position(|c| c == &cell) {
            self.pattern.remove_at(pos);
            self.error = None;
        } else {
            match self.pattern.add(cell) {
                Ok(()) => self.error = None,
                Err(e) => self.error = Some(format!("{:?}", e)),
            }
        }
        Task::none()
    }

    fn on_confirm(&mut self) -> Task<Message> {
        if let Some(enrollment) = &self.enrollment {
            let fingerprint = enrollment.fingerprint;
            let derivation_path = enrollment.derivation_path.clone();
            let xpub = enrollment.xpub;

            let desc_xpub = DescriptorPublicKey::XPub(DescriptorXKey {
                origin: Some((fingerprint, derivation_path)),
                xkey: xpub,
                derivation_path: DerivationPath::master(),
                wildcard: Wildcard::Unhardened,
            });

            let key = Key {
                source: KeySource::BorderWallet,
                name: "Border Wallet".to_string(),
                fingerprint,
                key: desc_xpub,
                account: None,
            };

            return Task::done(Message::DefineDescriptor(
                message::DefineDescriptor::KeysEdited(
                    self.coordinates.clone(),
                    SelectedKey::New(Box::new(key)),
                ),
            ));
        }
        Task::none()
    }

    // --- View methods ---

    fn wizard_msg(&self, msg: BorderWalletWizardMessage) -> Message {
        Message::BorderWalletWizard(msg)
    }

    fn view_intro(&self) -> Element<Message> {
        let header = modal::header(
            Some("Border Wallet".to_string()),
            None::<fn() -> Message>,
            Some(|| Message::Close),
        );

        let description = Column::new()
            .spacing(8)
            .push(p1_bold("What is a Border Wallet?"))
            .push(p1_regular(
                "A Border Wallet key is derived from a visual pattern \
                 you select on a word grid. The key is never stored \u{2014} it exists only \
                 while you are using it.",
            ))
            .push(p1_regular("To create one, you will:"))
            .push(p1_regular(
                "1. Generate or enter a 12-word recovery phrase (the Entropy Grid seed)",
            ))
            .push(p1_regular(
                "2. Select 11 cells from the word grid (your Pattern)",
            ))
            .push(p1_regular(
                "3. Memorize the checksum word (the final derived word)",
            ))
            .push(p1_regular("4. Review and confirm the derived key"))
            .push(p1_regular(
                "Important: To reconstruct this key you need all three components: \
                 the recovery phrase, the exact pattern, and the checksum word. \
                 Write them down and store them securely.",
            ));

        let next_btn = button::primary(None, "Get Started")
            .on_press(self.wizard_msg(BorderWalletWizardMessage::Next))
            .width(Length::Fill);

        let col = Column::new()
            .spacing(15)
            .push(header)
            .push(description)
            .push(next_btn)
            .width(500);

        Container::new(col)
            .padding(20)
            .style(theme::card::modal)
            .into()
    }

    fn view_recovery_phrase(&self) -> Element<Message> {
        let header = modal::header(
            Some("Recovery Phrase".to_string()),
            None::<fn() -> Message>,
            Some(|| Message::Close),
        );

        let back_btn = button::transparent(Some(icon::previous_icon()), "Back")
            .on_press(self.wizard_msg(BorderWalletWizardMessage::Previous));

        let generate_label = if self.allow_random_grid_phrase {
            "Generate Random Phrase"
        } else {
            "Derive from Master Key"
        };
        let generate_btn = button::secondary(None, generate_label)
            .on_press(self.wizard_msg(BorderWalletWizardMessage::GeneratePhrase))
            .width(Length::Fill);

        let mut word_rows = Column::new().spacing(6);
        for chunk_start in (0..12).step_by(3) {
            let mut row_widget = Row::new().spacing(8);
            for i in chunk_start..std::cmp::min(chunk_start + 3, 12) {
                let label = format!("{}.", i + 1);
                let idx = i;
                let input = TextInput::new(&format!("Word {}", i + 1), &self.phrase_words[i].value)
                    .on_input(move |s| {
                        Message::BorderWalletWizard(BorderWalletWizardMessage::PhraseWordEdited(
                            idx, s,
                        ))
                    })
                    .width(Length::Fill);
                row_widget = row_widget.push(
                    Row::new()
                        .push(text(label).width(25))
                        .push(input)
                        .spacing(4)
                        .width(Length::Fill),
                );
            }
            word_rows = word_rows.push(row_widget);
        }

        let error_text: Option<Element<Message>> = self
            .error
            .as_ref()
            .map(|e| p1_regular(e.as_str()).color(color::RED).into());

        let next_btn = if self.phrase_valid {
            button::primary(None, "Next")
                .on_press(self.wizard_msg(BorderWalletWizardMessage::Next))
                .width(Length::Fill)
        } else {
            button::primary(None, "Next").width(Length::Fill)
        };

        let mut col = Column::new()
            .spacing(12)
            .push(header)
            .push(p1_regular(
                "Enter or generate the 12-word recovery phrase that will seed the word grid.",
            ))
            .push(generate_btn)
            .push(word_rows);

        if let Some(err) = error_text {
            col = col.push(err);
        }

        col = col.push(Row::new().spacing(10).push(back_btn).push(next_btn));

        Container::new(col.width(550))
            .padding(20)
            .style(theme::card::modal)
            .into()
    }

    fn view_grid(&self) -> Element<Message> {
        let header = modal::header(
            Some("Select Pattern".to_string()),
            None::<fn() -> Message>,
            Some(|| Message::Close),
        );

        let back_btn = button::transparent(Some(icon::previous_icon()), "Back")
            .on_press(self.wizard_msg(BorderWalletWizardMessage::Previous));

        let status = p1_bold(format!(
            "Selected: {} / {} cells",
            self.pattern.len(),
            PATTERN_LENGTH
        ));

        let undo_btn = if !self.pattern.is_empty() {
            button::secondary(None, "Undo")
                .on_press(self.wizard_msg(BorderWalletWizardMessage::UndoLastCell))
        } else {
            button::secondary(None, "Undo")
        };

        let clear_btn = if !self.pattern.is_empty() {
            button::secondary(None, "Clear")
                .on_press(self.wizard_msg(BorderWalletWizardMessage::ClearPattern))
        } else {
            button::secondary(None, "Clear")
        };

        let grid_content: Element<Message> = if let Some(grid) = &self.grid {
            let mut grid_col = Column::new().spacing(1);

            for row_idx in 0..WordGrid::ROWS {
                let mut row_widget = Row::new().spacing(1);
                for col_idx in 0..WordGrid::COLS {
                    let word = grid.word_at(row_idx, col_idx).unwrap_or("???");
                    let prefix = &word[..std::cmp::min(4, word.len())];
                    let cell = CellRef::new(row_idx as u16, col_idx as u8);
                    let is_selected = self.pattern.cells().contains(&cell);

                    let order_num = self
                        .pattern
                        .cells()
                        .iter()
                        .position(|c| c == &cell)
                        .map(|p| p + 1);

                    let label = if let Some(n) = order_num {
                        format!("{}", n)
                    } else {
                        prefix.to_string()
                    };

                    let cell_style = if is_selected {
                        theme::button::primary
                    } else {
                        theme::button::secondary
                    };

                    let r = row_idx as u16;
                    let c = col_idx as u8;
                    let cell_btn = Button::new(text(label).size(11).align_x(Horizontal::Center))
                        .style(cell_style)
                        .on_press(
                            self.wizard_msg(BorderWalletWizardMessage::ToggleCellSelection(r, c)),
                        )
                        .width(50)
                        .height(28);

                    row_widget = row_widget.push(cell_btn);
                }
                grid_col = grid_col.push(row_widget);
            }
            scrollable(grid_col).height(400).into()
        } else {
            p1_regular("No grid available").into()
        };

        let error_text: Option<Element<Message>> = self
            .error
            .as_ref()
            .map(|e| p1_regular(e.as_str()).color(color::RED).into());

        let next_btn = if self.pattern.is_complete() {
            button::primary(None, "Next")
                .on_press(self.wizard_msg(BorderWalletWizardMessage::Next))
                .width(Length::Fill)
        } else {
            button::primary(None, "Next").width(Length::Fill)
        };

        let mut col = Column::new()
            .spacing(10)
            .push(header)
            .push(p1_regular(
                "Select 11 cells from the grid in order. This pattern is your key \u{2014} remember it exactly.",
            ))
            .push(
                Row::new()
                    .spacing(10)
                    .push(status)
                    .push(undo_btn)
                    .push(clear_btn),
            )
            .push(grid_content);

        if let Some(err) = error_text {
            col = col.push(err);
        }

        col = col.push(Row::new().spacing(10).push(back_btn).push(next_btn));

        Container::new(col.width(850))
            .padding(20)
            .style(theme::card::modal)
            .into()
    }

    fn view_checksum(&self) -> Element<Message> {
        let header = modal::header(
            Some("Checksum & Key".to_string()),
            None::<fn() -> Message>,
            Some(|| Message::Close),
        );

        let back_btn = button::transparent(Some(icon::previous_icon()), "Back")
            .on_press(self.wizard_msg(BorderWalletWizardMessage::Previous));

        let checksum_word = self.checksum_word.as_deref().unwrap_or("???");

        let enrollment_display = if let Some(enrollment) = &self.enrollment {
            Column::new()
                .spacing(4)
                .push(p1_bold("Derived Key Info"))
                .push(p1_regular(format!(
                    "Fingerprint: {}",
                    enrollment.fingerprint
                )))
                .push(p1_regular(format!(
                    "Derivation Path: {}",
                    enrollment.derivation_path
                )))
                .push(p1_regular(format!("Network: {:?}", enrollment.network)))
        } else {
            Column::new().push(p1_regular("No enrollment data"))
        };

        let next_btn = button::primary(None, "Next")
            .on_press(self.wizard_msg(BorderWalletWizardMessage::Next))
            .width(Length::Fill);

        let col = Column::new()
            .spacing(12)
            .push(header)
            .push(p1_bold(format!("Checksum word: \"{}\"", checksum_word)))
            .push(p1_regular(
                "This is the only word you need to memorize. The checksum word is \
                 the 12th word of the derived mnemonic, calculated automatically from \
                 your pattern. Together with the recovery phrase and your pattern, it \
                 forms the three components needed to reconstruct this key.",
            ))
            .push(enrollment_display)
            .push(Row::new().spacing(10).push(back_btn).push(next_btn))
            .width(500);

        Container::new(col)
            .padding(20)
            .style(theme::card::modal)
            .into()
    }

    fn view_confirm(&self) -> Element<Message> {
        let header = modal::header(
            Some("Confirm".to_string()),
            None::<fn() -> Message>,
            Some(|| Message::Close),
        );

        let back_btn = button::transparent(Some(icon::previous_icon()), "Back")
            .on_press(self.wizard_msg(BorderWalletWizardMessage::Previous));

        let fingerprint = self
            .enrollment
            .as_ref()
            .map(|e| format!("{}", e.fingerprint))
            .unwrap_or_default();

        let checksum_word = self.checksum_word.as_deref().unwrap_or("???");

        let warning = Column::new()
            .spacing(8)
            .push(p1_bold("Important \u{2014} Read carefully"))
            .push(p1_regular(
                "After confirming, the mnemonic and private keys will be permanently \
                 erased from memory. Only the public key (xpub) and fingerprint will \
                 be stored in the wallet descriptor.",
            ))
            .push(p1_regular(
                "To reconstruct this key in the future, you need all three components:",
            ))
            .push(p1_regular(
                "1. The recovery phrase (12 words that seed the grid)",
            ))
            .push(p1_regular("2. Your exact pattern (the 11 cells in order)"))
            .push(p1_bold(format!(
                "3. The checksum word: \"{}\"",
                checksum_word
            )))
            .push(p1_regular(
                "Write down and securely store all three. Without any one of them, \
                 the key cannot be recovered.",
            ))
            .push(p1_bold(format!("Key fingerprint: {}", fingerprint)));

        let confirm_btn = button::primary(None, "Confirm & Enroll Key")
            .on_press(self.wizard_msg(BorderWalletWizardMessage::ConfirmEnrollment))
            .width(Length::Fill);

        let col = Column::new()
            .spacing(15)
            .push(header)
            .push(warning)
            .push(Row::new().spacing(10).push(back_btn).push(confirm_btn))
            .width(500);

        Container::new(col)
            .padding(20)
            .style(theme::card::modal)
            .into()
    }
}

/// Zeroize recovery phrase words when the wizard is dropped so they don't
/// linger in memory after the user navigates away or completes enrollment.
impl Drop for BorderWalletWizard {
    fn drop(&mut self) {
        for word in &mut self.phrase_words {
            word.value.zeroize();
        }
    }
}

impl super::DescriptorEditModal for BorderWalletWizard {
    fn processing(&self) -> bool {
        false
    }

    fn update(&mut self, _hws: &mut HardwareWallets, message: Message) -> Task<Message> {
        match message {
            Message::BorderWalletWizard(msg) => match msg {
                BorderWalletWizardMessage::Next => self.on_next(),
                BorderWalletWizardMessage::Previous => self.on_previous(),
                BorderWalletWizardMessage::GeneratePhrase => self.on_generate_phrase(),
                BorderWalletWizardMessage::PhraseWordEdited(idx, word) => {
                    self.on_phrase_word_edited(idx, word)
                }
                BorderWalletWizardMessage::ToggleCellSelection(row, col) => {
                    self.on_toggle_cell(row, col)
                }
                BorderWalletWizardMessage::UndoLastCell => {
                    self.pattern.undo_last();
                    self.error = None;
                    Task::none()
                }
                BorderWalletWizardMessage::ClearPattern => {
                    self.pattern.clear();
                    self.error = None;
                    Task::none()
                }
                BorderWalletWizardMessage::ConfirmEnrollment => self.on_confirm(),
            },
            _ => Task::none(),
        }
    }

    fn view<'a>(&'a self, _hws: &'a HardwareWallets) -> Element<'a, Message> {
        match self.step {
            WizardStep::Intro => self.view_intro(),
            WizardStep::RecoveryPhrase => self.view_recovery_phrase(),
            WizardStep::Grid => self.view_grid(),
            WizardStep::Checksum => self.view_checksum(),
            WizardStep::Confirm => self.view_confirm(),
        }
    }
}
