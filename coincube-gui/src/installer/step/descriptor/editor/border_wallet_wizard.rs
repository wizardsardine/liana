//! Border Wallet creation wizard.
//!
//! Multi-step modal that guides the user through creating a Border Wallet
//! key derived from a visual grid pattern. Can be used in any spending path.
//!
//! Steps: Intro → RecoveryPhrase → Grid → Checksum → Confirm

use crate::app::settings::GridSeedSource;

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
use iced::{
    advanced::widget::Id,
    alignment::{Horizontal, Vertical},
    widget::{scrollable, Space},
    Length, Task,
};
use rand::{rngs::OsRng, seq::SliceRandom};
use zeroize::{Zeroize, Zeroizing};

use coincube_ui::locale::copy as localized_copy;
use coincube_ui::localized_format;
use coincube_ui::{
    component::{
        button, form,
        text::{p1_bold, p1_regular, text},
    },
    icon, theme,
    widget::{action::Action, Column, Container, Element, Row, TextInput},
};

use std::sync::{Arc, Mutex};

use crate::{
    home::restore_frame,
    hw::HardwareWallets,
    installer::{
        descriptor::{Key, KeySource},
        message::{self, Message},
        step::descriptor::editor::key::SelectedKey,
    },
    signer::Signer,
};

/// The global Tab handler reveals focus here after the outer app scroll area.
pub(crate) const BORDER_WALLET_GRID_SCROLL_ID: &str = "tenshu.border-wallet.grid-scroll";

/// The numbered stages on the sheet's rail.
const STAGES: [&str; 4] = ["Phrase", "Pattern", "Checksum", "Confirm"];
/// The sheet header's height, as the Create Cube sheet's.
const HEADER_HEIGHT: f32 = 64.0;
/// Footer actions match the header's Close.
const FOOTER_ACTION_HEIGHT: f32 = 36.0;
/// Width of the text steps' sheet.
const SHEET_WIDTH: f32 = 560.0;
/// A grid cell's width: four letters at 11 px with room for the focus ring.
const CELL_WIDTH: f32 = 46.0;
/// The pattern sheet fits the row numbers, sixteen cells, the embedded
/// scrollbar and the body padding, and stays inside the 1024 px minimum
/// window less the modal margin.
const GRID_SHEET_WIDTH: f32 = 920.0;
/// The pattern sheet grows with the window up to this height; the grid
/// scrolls within it.
const GRID_SHEET_MAX_HEIGHT: f32 = 860.0;

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
            // The ordered pattern is the key. Coordinates must not enter logs.
            Self::ToggleCellSelection(_, _) => write!(f, "ToggleCellSelection(<redacted>)"),
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
    // Opaque, stable focus IDs. Their allocation order is shuffled so numeric
    // ID deltas do not encode grid coordinates if FocusAction is ever logged.
    grid_focus_ids: Vec<Id>,

    // Derived data — only the checksum word is retained, never the full mnemonic.
    checksum_word: Option<String>,
    enrollment: Option<BorderWalletEnrollment>,

    /// When true, allow random phrase generation. When false (default),
    /// the "Generate" button derives from the master signer via BIP-85.
    allow_random_grid_phrase: bool,
    /// Master signer for BIP-85 grid phrase derivation.
    signer: Option<Arc<Mutex<Signer>>>,
    /// Where [`Self::phrase_words`] currently comes from.
    ///
    /// Set by "Generate" — naming the signer it derived from — and reset to
    /// [`GridSeedSource::Independent`] by anything that makes the words
    /// something else: a random generate, or a keystroke in any of the twelve
    /// fields. It rides out on [`KeySource::BorderWallet`] so the signing flow
    /// can re-derive the phrase from that exact seed, or know to ask.
    phrase_source: GridSeedSource,

    error: Option<String>,
}

impl BorderWalletWizard {
    fn grid_focus_ids() -> Vec<Id> {
        let mut ids: Vec<_> = (0..WordGrid::TOTAL_CELLS).map(|_| Id::unique()).collect();
        ids.shuffle(&mut OsRng);
        ids
    }

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
            grid_focus_ids: Self::grid_focus_ids(),
            checksum_word: None,
            enrollment: None,
            allow_random_grid_phrase,
            signer: Some(signer),
            phrase_source: GridSeedSource::Independent,
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
                    self.error = Some(localized_format!(
                        "Please select exactly {} cells. Currently selected: {}",
                        "Selecciona exactamente {} celdas. Seleccionadas: {}",
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
                                    self.error = Some(localized_format!(
                                        "Key derivation failed: {:?}",
                                        "No se pudo derivar la clave: {:?}",
                                        e
                                    ));
                                }
                            }
                            // `mnemonic` is dropped here — no full mnemonic retained.
                        }
                        Err(e) => {
                            self.error = Some(localized_format!(
                                "Mnemonic construction failed: {:?}",
                                "No se pudo construir la frase: {:?}",
                                e
                            ));
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
        // Tracks which branch below produced the phrase, and which seed, so the
        // recorded source describes what is actually in the fields rather than
        // what the Cube was configured to prefer.
        let mut source = GridSeedSource::Independent;
        let result = if !self.allow_random_grid_phrase {
            if let Some(signer) = &self.signer {
                match signer.lock() {
                    Ok(signer) => {
                        source = GridSeedSource::MasterDerived {
                            fingerprint: signer.fingerprint(),
                        };
                        signer
                            .derive_grid_recovery_phrase()
                            .map_err(|e| format!("{:?}", e))
                    }
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
                self.phrase_source = source;
                self.error = None;
            }
            Err(e) => {
                self.phrase_source = GridSeedSource::Independent;
                self.error = Some(localized_format!(
                    "Failed to generate recovery phrase: {}",
                    "No se pudo generar la frase de recuperación: {}",
                    e
                ));
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
        // One keystroke is enough: the phrase is no longer the one that signer
        // derives, so it can no longer be re-derived at signing time.
        self.phrase_source = GridSeedSource::Independent;
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
                Err(e) => self.error = Some(crate::user_error::border_wallet_cell_message(&e)),
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
                source: KeySource::BorderWallet {
                    grid_seed_source: self.phrase_source,
                },
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

    fn primary_control<'a>(
        id: &'static str,
        label: &'static str,
        message: Option<BorderWalletWizardMessage>,
    ) -> Action<'a, Message> {
        button::primary_action(id, None, localized_copy(label), Message::FocusAction)
            .width(Length::Shrink)
            .height(Length::Fixed(FOOTER_ACTION_HEIGHT))
            .padding([0, 16])
            .on_press_maybe(message.map(Message::BorderWalletWizard))
    }

    fn secondary_control<'a>(
        id: &'static str,
        label: &'static str,
        message: Option<BorderWalletWizardMessage>,
    ) -> Action<'a, Message> {
        button::secondary_action(id, None, localized_copy(label), Message::FocusAction)
            .on_press_maybe(message.map(Message::BorderWalletWizard))
    }

    /// Back in the sheet's footer, drawn as the sheet's Close is.
    fn back_control<'a>(id: &'static str, message: Message) -> Action<'a, Message> {
        Action::new(
            id,
            Container::new(
                Row::new()
                    .spacing(6)
                    .align_y(Vertical::Center)
                    .push(
                        icon::previous_icon()
                            .size(14)
                            .style(theme::tenshu::text::ink),
                    )
                    .push(coincube_ui::component::tenshu::text::label(localized_copy(
                        "Back",
                    ))),
            )
            .center_y(Length::Fill),
            Message::FocusAction,
        )
        .height(Length::Fixed(FOOTER_ACTION_HEIGHT))
        .padding([0, 12])
        .style(|theme, status| {
            theme::tenshu::button::action(
                theme,
                status,
                theme::tenshu::button::ActionKind::Secondary,
                theme::tenshu::button::ActionShape::Compact,
            )
        })
        .on_press(message)
    }

    fn grid_cell_style(
        theme: &theme::Theme,
        status: iced::widget::button::Status,
        selected: bool,
    ) -> iced::widget::button::Style {
        let shell = theme.colors.shell;
        iced::widget::button::Style {
            background: Some(iced::Background::Color(if selected {
                shell.accent
            } else {
                shell.panel
            })),
            text_color: if selected {
                theme::tenshu::ON_ACCENT
            } else {
                shell.ink
            },
            border: iced::Border {
                color: if selected {
                    shell.accent
                } else if matches!(status, iced::widget::button::Status::Hovered) {
                    shell.muted
                } else {
                    shell.line
                },
                width: 1.0,
                radius: 4.0.into(),
            },
            ..Default::default()
        }
    }

    /// The four numbered stages, as the Create Cube sheet's step strip. The
    /// intro explains the flow and has none.
    fn rail(&self) -> Option<Element<Message>> {
        let current = match self.step {
            WizardStep::Intro => return None,
            WizardStep::RecoveryPhrase => 0,
            WizardStep::Grid => 1,
            WizardStep::Checksum => 2,
            WizardStep::Confirm => 3,
        };
        Some(
            Container::new(restore_frame::stepper(&STAGES.map(localized_copy), current))
                .padding([14, 24])
                .width(Length::Fill)
                .into(),
        )
    }

    /// A step's sheet: header with the title and Close, the stage rail, the
    /// body, and a footer pinned under it with Back on the left and the
    /// step's action on the right. The footer stays on screen however tall
    /// the body is: the pattern step's grid scrolls inside the body.
    fn sheet<'a>(
        &'a self,
        title: &'static str,
        width: f32,
        body: Element<'a, Message>,
        back: Action<'a, Message>,
        primary: Action<'a, Message>,
    ) -> Element<'a, Message> {
        let fills = self.step == WizardStep::Grid;
        let header = Container::new(
            Row::new()
                .align_y(Vertical::Center)
                .push(
                    iced::widget::text(localized_copy(title))
                        .size(20)
                        .font(coincube_ui::font::SEMIBOLD),
                )
                .push(Space::new().width(Length::Fill))
                .push(restore_frame::close_button(
                    "border-wallet.close",
                    Some(Message::Close),
                    Message::FocusAction,
                )),
        )
        .height(Length::Fixed(HEADER_HEIGHT))
        .center_y(Length::Fixed(HEADER_HEIGHT))
        .padding([0, 24])
        .width(Length::Fill);
        let body = Container::new(body)
            .padding([20, 24])
            .width(Length::Fill)
            .height(if fills { Length::Fill } else { Length::Shrink });
        let footer = Container::new(
            Row::new()
                .align_y(Vertical::Center)
                .push(back)
                .push(Space::new().width(Length::Fill))
                .push(primary),
        )
        .padding([14, 24])
        .width(Length::Fill);
        let mut column = Column::new()
            .width(Length::Fill)
            .push(header)
            .push(restore_frame::full_rule());
        if let Some(rail) = self.rail() {
            column = column.push(rail).push(restore_frame::full_rule());
        }
        let column = column
            .push(body)
            .push(restore_frame::full_rule())
            .push(footer);
        let sheet = Container::new(column)
            .width(Length::Fixed(width))
            .style(restore_frame::sheet_style);
        if fills {
            sheet
                .height(Length::Fill)
                .max_height(GRID_SHEET_MAX_HEIGHT)
                .into()
        } else {
            sheet.into()
        }
    }

    fn error_line(&self) -> Option<Element<Message>> {
        self.error.as_ref().map(|e| {
            p1_regular(coincube_ui::locale::notice(e.clone()))
                .style(theme::text::error)
                .into()
        })
    }

    fn view_intro(&self) -> Element<Message> {
        let body = Column::new()
            .spacing(8)
            .push(p1_bold(localized_copy("What is a Border Wallet?")))
            .push(p1_regular(localized_copy(
                "A Border Wallet key is derived from a visual pattern \
                 you select on a word grid. The key is never stored \u{2014} it exists only \
                 while you are using it.",
            )))
            .push(p1_regular(localized_copy("To create one, you will:")))
            .push(p1_regular(localized_copy(
                "1. Generate or enter a 12-word recovery phrase (the Entropy Grid seed)",
            )))
            .push(p1_regular(localized_copy(
                "2. Select 11 cells from the word grid (your Pattern)",
            )))
            .push(p1_regular(localized_copy(
                "3. Review the automatically calculated checksum word",
            )))
            .push(p1_regular(localized_copy(
                "4. Review and confirm the derived key",
            )))
            .push(p1_regular(localized_copy(
                "To reconstruct this key, you need your recovery phrase and exact pattern. \
                 Keep your recovery phrase safe and make sure you can reproduce your \
                 pattern in the same order. The checksum word is calculated automatically.",
            )));

        // Intro is the first step of the wizard, so "Back" returns the user
        // to the Select-key-source picker for this slot (via
        // `DefineDescriptor::ReopenKeyModal`) rather than dropping them all
        // the way back to the descriptor editor.
        self.sheet(
            "Border Wallet",
            SHEET_WIDTH,
            body.into(),
            Self::back_control(
                "border-wallet.intro.back",
                Message::DefineDescriptor(message::DefineDescriptor::ReopenKeyModal(
                    self.coordinates.clone(),
                )),
            ),
            Self::primary_control(
                "border-wallet.intro.start",
                "Get Started",
                Some(BorderWalletWizardMessage::Next),
            ),
        )
    }

    fn view_recovery_phrase(&self) -> Element<Message> {
        let generate_label: &'static str = if self.allow_random_grid_phrase {
            "Generate Random Phrase"
        } else {
            "Derive from Master Key"
        };
        let generate_btn = Self::secondary_control(
            "border-wallet.phrase.generate",
            generate_label,
            Some(BorderWalletWizardMessage::GeneratePhrase),
        )
        .width(Length::Fill);

        let mut word_rows = Column::new().spacing(6);
        for chunk_start in (0..12).step_by(3) {
            let mut row_widget = Row::new().spacing(8);
            for i in chunk_start..std::cmp::min(chunk_start + 3, 12) {
                let label = format!("{}.", i + 1);
                let idx = i;
                let input = TextInput::new(
                    &localized_format!("Word {}", "Palabra {}", i + 1),
                    &self.phrase_words[i].value,
                )
                .on_input(move |s| {
                    Message::BorderWalletWizard(BorderWalletWizardMessage::PhraseWordEdited(idx, s))
                })
                .style(|theme: &theme::Theme, status| {
                    let mut style = theme::text_input::primary(theme, status);
                    if matches!(status, iced::widget::text_input::Status::Focused { .. }) {
                        style.border.color = theme.colors.shell.focus;
                        style.border.width = 2.0;
                    }
                    style
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

        let body = Column::new()
            .spacing(12)
            .push(p1_regular(localized_copy(
                "Enter or generate the 12-word recovery phrase that will seed the word grid.",
            )))
            .push(generate_btn)
            .push(word_rows)
            .push(self.error_line());

        self.sheet(
            "Recovery Phrase",
            SHEET_WIDTH,
            body.into(),
            Self::back_control(
                "border-wallet.phrase.back",
                self.wizard_msg(BorderWalletWizardMessage::Previous),
            ),
            Self::primary_control(
                "border-wallet.phrase.next",
                "Continue",
                self.phrase_valid.then_some(BorderWalletWizardMessage::Next),
            ),
        )
    }

    fn view_grid(&self) -> Element<Message> {
        let status = p1_bold(localized_format!(
            "Selected: {} / {} cells",
            "Seleccionadas: {} / {} celdas",
            self.pattern.len(),
            PATTERN_LENGTH
        ));

        let undo_btn = Self::secondary_control(
            "border-wallet.pattern.undo",
            "Undo",
            (!self.pattern.is_empty()).then_some(BorderWalletWizardMessage::UndoLastCell),
        )
        .width(Length::Shrink);

        let clear_btn = Self::secondary_control(
            "border-wallet.pattern.clear",
            "Clear",
            (!self.pattern.is_empty()).then_some(BorderWalletWizardMessage::ClearPattern),
        )
        .width(Length::Shrink);

        let grid_content: Element<Message> = if let Some(grid) = &self.grid {
            // Reserve the focus ring outset at both scroll endpoints.
            let mut grid_col = Column::new().spacing(4).push(Space::new().height(4));

            let mut column_numbers = Row::new().spacing(4).push(Space::new().width(24));
            for col_idx in 0..WordGrid::COLS {
                column_numbers = column_numbers.push(
                    text(format!("{}", col_idx + 1))
                        .size(10)
                        .width(CELL_WIDTH)
                        .align_x(Horizontal::Center)
                        .style(theme::text::secondary),
                );
            }
            for row_idx in 0..WordGrid::ROWS {
                let mut row_widget = Row::new().spacing(4).align_y(Vertical::Center).push(
                    text(format!("{}", row_idx + 1))
                        .size(10)
                        .width(24)
                        .align_x(Horizontal::Right)
                        .style(theme::text::secondary),
                );
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

                    let r = row_idx as u16;
                    let c = col_idx as u8;
                    let cell_btn = Action::new(
                        self.grid_focus_ids[row_idx * WordGrid::COLS + col_idx].clone(),
                        text(label).size(11).align_x(Horizontal::Center),
                        Message::FocusAction,
                    )
                    .style(move |theme, status| Self::grid_cell_style(theme, status, is_selected))
                    .focus_radius(4.0)
                    .on_press(self.wizard_msg(BorderWalletWizardMessage::ToggleCellSelection(r, c)))
                    .width(CELL_WIDTH)
                    .height(28);

                    row_widget = row_widget.push(cell_btn);
                }
                grid_col = grid_col.push(row_widget);
            }
            grid_col = grid_col.push(Space::new().height(4));
            Column::new()
                .spacing(4)
                .height(Length::Fill)
                .push(column_numbers)
                .push(
                    scrollable(grid_col)
                        .id(BORDER_WALLET_GRID_SCROLL_ID)
                        // Embed the scrollbar beside the last column instead
                        // of floating over its text and focus ring.
                        .spacing(4)
                        .height(Length::Fill),
                )
                .into()
        } else {
            p1_regular(localized_copy("No grid available")).into()
        };

        let body = Column::new()
            .spacing(12)
            .height(Length::Fill)
            .push(p1_regular(localized_copy("Select 11 cells from the grid in order. This pattern is your key \u{2014} remember it exactly.")))
            .push(
                Row::new()
                    .spacing(10)
                    .align_y(Vertical::Center)
                    .push(status)
                    .push(Space::new().width(Length::Fill))
                    .push(
                        text(localized_format!(
                            "{} rows × {} columns · first four letters shown",
                            "{} filas × {} columnas · se muestran las cuatro primeras letras",
                            WordGrid::ROWS,
                            WordGrid::COLS
                        ))
                        .size(12)
                        .style(theme::text::secondary),
                    )
                    .push(undo_btn)
                    .push(clear_btn),
            )
            .push(grid_content)
            .push(self.error_line());

        self.sheet(
            "Select Pattern",
            GRID_SHEET_WIDTH,
            body.into(),
            Self::back_control(
                "border-wallet.pattern.back",
                self.wizard_msg(BorderWalletWizardMessage::Previous),
            ),
            Self::primary_control(
                "border-wallet.pattern.next",
                "Continue",
                pattern_can_continue(&self.pattern).then_some(BorderWalletWizardMessage::Next),
            ),
        )
    }

    fn view_checksum(&self) -> Element<Message> {
        let checksum_word = self.checksum_word.as_deref().unwrap_or("???");

        let enrollment_display = if let Some(enrollment) = &self.enrollment {
            Column::new()
                .spacing(4)
                .push(p1_bold(localized_copy("Derived Key Info")))
                .push(p1_regular(localized_format!(
                    "Fingerprint: {}",
                    "Huella digital: {}",
                    enrollment.fingerprint
                )))
                .push(p1_regular(localized_format!(
                    "Derivation Path: {}",
                    "Ruta de derivación: {}",
                    enrollment.derivation_path
                )))
                .push(p1_regular(localized_format!(
                    "Network: {:?}",
                    "Red: {:?}",
                    enrollment.network
                )))
        } else {
            Column::new().push(p1_regular(localized_copy("No enrollment data")))
        };

        let body = Column::new()
            .spacing(12)
            .push(p1_bold(localized_format!(
                "Checksum word: \"{}\"",
                "Palabra de control: \"{}\"",
                checksum_word
            )))
            .push(p1_regular(localized_copy(
                "This word is generated automatically. You can keep it as an optional \
                 reference to help check your pattern.",
            )))
            .push(enrollment_display);

        self.sheet(
            "Checksum & Key",
            SHEET_WIDTH,
            body.into(),
            Self::back_control(
                "border-wallet.checksum.back",
                self.wizard_msg(BorderWalletWizardMessage::Previous),
            ),
            Self::primary_control(
                "border-wallet.checksum.next",
                "Continue",
                Some(BorderWalletWizardMessage::Next),
            ),
        )
    }

    fn view_confirm(&self) -> Element<Message> {
        let fingerprint = self
            .enrollment
            .as_ref()
            .map(|e| format!("{}", e.fingerprint))
            .unwrap_or_default();

        let body = Column::new()
            .spacing(8)
            .push(p1_bold(localized_copy("Important \u{2014} Read carefully")))
            .push(p1_regular(localized_copy(
                "After confirming, the mnemonic and private keys will be permanently \
                 erased from memory. Only the public key (xpub) and fingerprint will \
                 be stored in the wallet descriptor.",
            )))
            .push(p1_regular(localized_copy(
                "To reconstruct this key in the future, you need:",
            )))
            .push(p1_regular(localized_copy(
                "1. The recovery phrase (12 words that seed the grid)",
            )))
            .push(p1_regular(localized_copy(
                "2. Your exact pattern (the 11 cells in order)",
            )))
            .push(p1_regular(localized_copy(
                "Keep your recovery phrase safe and make sure you can reproduce your \
                 pattern in the same order. Both are required to recover your key. \
                 The checksum word is recalculated automatically; you don't need to memorize it.",
            )))
            .push(p1_bold(localized_format!(
                "Key fingerprint: {}",
                "Huella digital de la clave: {}",
                fingerprint
            )));

        self.sheet(
            "Confirm",
            SHEET_WIDTH,
            body.into(),
            Self::back_control(
                "border-wallet.confirm.back",
                self.wizard_msg(BorderWalletWizardMessage::Previous),
            ),
            Self::primary_control(
                "border-wallet.confirm.enroll",
                "Confirm & Enroll Key",
                Some(BorderWalletWizardMessage::ConfirmEnrollment),
            ),
        )
    }
}

/// The pattern step's Continue is enabled exactly when the full ordered
/// pattern is chosen; [`BorderWalletWizard::on_next`] checks it again.
fn pattern_can_continue(pattern: &OrderedPattern) -> bool {
    pattern.is_complete()
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

#[cfg(test)]
mod step_tests {
    use super::*;

    /// A public BIP39 test vector: a synthetic grid fixture, never a user's
    /// recovery phrase.
    fn wizard_on_grid() -> BorderWalletWizard {
        let grid = GridRecoveryPhrase::from_phrase(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
        )
        .unwrap()
        .generate_grid();
        BorderWalletWizard {
            network: Network::Regtest,
            coordinates: Vec::new(),
            step: WizardStep::Grid,
            phrase_words: vec![form::Value::default(); 12],
            phrase_valid: false,
            grid: Some(grid),
            pattern: OrderedPattern::new(),
            grid_focus_ids: BorderWalletWizard::grid_focus_ids(),
            checksum_word: None,
            enrollment: None,
            allow_random_grid_phrase: true,
            signer: None,
            phrase_source: GridSeedSource::Independent,
            error: None,
        }
    }

    /// The wizard's handling of `message`, as its `update` dispatches it.
    fn send(wizard: &mut BorderWalletWizard, message: BorderWalletWizardMessage) {
        let _ = match message {
            BorderWalletWizardMessage::Next => wizard.on_next(),
            BorderWalletWizardMessage::Previous => wizard.on_previous(),
            BorderWalletWizardMessage::ToggleCellSelection(row, col) => {
                wizard.on_toggle_cell(row, col)
            }
            _ => unreachable!("not used by these tests"),
        };
    }

    #[test]
    fn full_pattern_continues_to_checksum_and_back() {
        let mut wizard = wizard_on_grid();
        for col in 0..(PATTERN_LENGTH as u8 - 1) {
            send(
                &mut wizard,
                BorderWalletWizardMessage::ToggleCellSelection(3, col),
            );
        }
        assert!(!pattern_can_continue(&wizard.pattern), "10 of 11 cells");
        send(&mut wizard, BorderWalletWizardMessage::Next);
        assert_eq!(wizard.step, WizardStep::Grid, "an incomplete pattern stays");
        assert!(wizard.error.is_some());

        send(
            &mut wizard,
            BorderWalletWizardMessage::ToggleCellSelection(4, 0),
        );
        assert!(pattern_can_continue(&wizard.pattern), "11 of 11 cells");
        send(&mut wizard, BorderWalletWizardMessage::Next);
        assert_eq!(wizard.step, WizardStep::Checksum);
        assert!(wizard.error.is_none());
        assert!(wizard.checksum_word.is_some() && wizard.enrollment.is_some());

        send(&mut wizard, BorderWalletWizardMessage::Next);
        assert_eq!(wizard.step, WizardStep::Confirm);
        send(&mut wizard, BorderWalletWizardMessage::Previous);
        assert_eq!(wizard.step, WizardStep::Checksum);
        send(&mut wizard, BorderWalletWizardMessage::Previous);
        assert_eq!(wizard.step, WizardStep::Grid);
        assert_eq!(
            wizard.pattern.len(),
            PATTERN_LENGTH,
            "Back keeps the pattern"
        );
    }

    /// The reported bug: inside the Create Vault frame the modal was laid out
    /// against the step's small bounds, which clipped the sheet below the
    /// grid, so the pattern step's Continue was never on screen. Over a base
    /// the size of that step, a window-covering modal keeps every column and
    /// Continue inside the minimum window once the pattern is complete. The
    /// editor's own test checks that it mounts its modals this way.
    #[test]
    fn pattern_continue_is_on_screen_over_a_small_base() {
        use iced::advanced::widget::{operation, operation::focusable, Operation};
        use iced::{Rectangle, Size};
        use iced_runtime::user_interface::{Cache, UserInterface};

        struct Find {
            last_column: Id,
            next: Option<Rectangle>,
            cell: Option<Rectangle>,
        }
        impl Operation for Find {
            fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
                operate(self);
            }
            fn focusable(
                &mut self,
                id: Option<&Id>,
                bounds: Rectangle,
                _: &mut dyn focusable::Focusable,
            ) {
                if id == Some(&Id::new("border-wallet.pattern.next")) {
                    self.next = Some(bounds);
                } else if id == Some(&self.last_column) {
                    self.cell = Some(bounds);
                }
            }
        }

        for bytes in coincube_ui::font::load() {
            iced::advanced::graphics::text::font_system()
                .write()
                .unwrap()
                .load_font(bytes);
        }
        let mut renderer = iced::futures::executor::block_on(
            <iced::Renderer as iced::advanced::renderer::Headless>::new(
                coincube_ui::font::REGULAR,
                iced::Pixels(14.0),
                Some("tiny-skia"),
            ),
        )
        .unwrap();
        let window = Size::new(1024.0, 768.0);

        let mut wizard = wizard_on_grid();
        for col in 0..PATTERN_LENGTH as u8 {
            send(
                &mut wizard,
                BorderWalletWizardMessage::ToggleCellSelection(0, col),
            );
        }
        // The Policy step's height in the screenshot that reported the bug.
        let base = Container::new(text("Set keys"))
            .width(Length::Fixed(668.0))
            .height(Length::Fixed(488.0));
        let element: Element<'_, Message> =
            coincube_ui::widget::modal::Modal::new(base, wizard.view_grid())
                .cover_window()
                .into();
        let last_column = wizard.grid_focus_ids[WordGrid::COLS - 1].clone();
        let mut ui = UserInterface::build(element, window, Cache::new(), &mut renderer);
        let mut find = Find {
            last_column,
            next: None,
            cell: None,
        };
        ui.operate(&renderer, &mut operation::black_box(&mut find));
        let next = find.next.expect("the pattern step's Continue is enabled");
        let cell = find.cell.expect("column 16 is laid out");
        // A sheet narrower than the grid squeezes its last columns to
        // nothing; Continue is right-aligned in the footer, so a 16th column
        // past its right edge is cut off by the sheet.
        assert_eq!(cell.width, CELL_WIDTH, "column 16 is squeezed");
        assert!(
            cell.x + cell.width <= next.x + next.width,
            "column 16 at {:?} overflows the sheet (Continue at {:?})",
            cell,
            next
        );
        assert!(
            next.x >= 0.0
                && next.y >= 0.0
                && next.x + next.width <= window.width
                && next.y + next.height <= window.height,
            "Continue at {:?} is outside the {:?} window",
            next,
            window
        );
    }
}

#[cfg(test)]
mod presentation_tests {
    use super::*;
    use coincube_ui::widget::action;
    use iced::advanced::{
        clipboard::Null,
        layout,
        renderer::Headless,
        widget::{
            operation, operation::focusable, operation::Focusable, operation::Outcome, Operation,
            Tree,
        },
        Layout, Shell,
    };
    use iced::futures::StreamExt;
    use iced::{keyboard, mouse, Event, Rectangle, Size, Vector};

    /// The grid fills the sheet, so its viewport can end on a fractional
    /// pixel while scroll offsets snap to whole ones: a revealed cell's ring
    /// may sit up to half a pixel short of the 4 px margin at the bottom.
    const SNAP: f32 = 0.5;

    #[derive(Default)]
    struct FocusTargets(Vec<Id>);

    impl Operation for FocusTargets {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
            operate(self);
        }

        fn focusable(&mut self, id: Option<&Id>, _bounds: Rectangle, _state: &mut dyn Focusable) {
            if let Some(id) = id {
                self.0.push(id.clone());
            }
        }
    }

    struct GridViewport {
        focused_id: Id,
        viewport: Option<Rectangle>,
        offset: f32,
        focused: Option<Rectangle>,
    }

    impl GridViewport {
        fn new(focused_id: Id) -> Self {
            Self {
                focused_id,
                viewport: None,
                offset: 0.0,
                focused: None,
            }
        }

        fn screen_bounds(&self) -> Rectangle {
            let mut bounds = self.focused.expect("focused grid cell must be found");
            bounds.y -= self.offset;
            bounds
        }
    }

    impl Operation for GridViewport {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
            operate(self);
        }

        fn scrollable(
            &mut self,
            id: Option<&Id>,
            bounds: Rectangle,
            _content_bounds: Rectangle,
            translation: Vector,
            _state: &mut dyn operation::Scrollable,
        ) {
            if id == Some(&Id::new(BORDER_WALLET_GRID_SCROLL_ID)) {
                self.viewport = Some(bounds);
                self.offset = translation.y;
            }
        }

        fn focusable(&mut self, id: Option<&Id>, bounds: Rectangle, state: &mut dyn Focusable) {
            if id == Some(&self.focused_id) && state.is_focused() {
                self.focused = Some(bounds);
            }
        }
    }

    fn operate<T: 'static>(
        element: &mut Element<'_, Message>,
        tree: &mut Tree,
        node: &layout::Node,
        renderer: &iced::Renderer,
        operation: impl Operation<T> + 'static,
    ) -> Option<T> {
        let mut operation: Box<dyn Operation<T>> = Box::new(operation);
        loop {
            element.as_widget_mut().operate(
                tree,
                Layout::new(node),
                renderer,
                &mut operation::black_box(operation.as_mut()),
            );
            match operation.finish() {
                Outcome::None => return None,
                Outcome::Some(result) => return Some(result),
                Outcome::Chain(next) => operation = next,
            }
        }
    }

    async fn apply_focus_task(
        task: Task<Message>,
        element: &mut Element<'_, Message>,
        tree: &mut Tree,
        node: &mut layout::Node,
        renderer: &iced::Renderer,
        size: Size,
    ) {
        let mut stream = iced_runtime::task::into_stream(task).expect("focus task has operations");
        while let Some(action) = stream.next().await {
            let iced_runtime::Action::Widget(mut operation) = action else {
                panic!("focus task must only operate on widgets");
            };
            loop {
                element.as_widget_mut().operate(
                    tree,
                    Layout::new(node),
                    renderer,
                    operation.as_mut(),
                );
                match operation.finish() {
                    Outcome::None | Outcome::Some(()) => break,
                    Outcome::Chain(next) => operation = next,
                }
            }
            *node = element.as_widget_mut().layout(
                tree,
                renderer,
                &layout::Limits::new(Size::ZERO, size),
            );
        }
    }

    fn capture(
        path: &std::path::Path,
        element: &mut Element<'_, Message>,
        tree: &mut Tree,
        node: &layout::Node,
        renderer: &mut iced::Renderer,
        theme: &theme::Theme,
        size: Size,
    ) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // Buttons settle their status on a redraw; until then they draw as
        // disabled.
        element.as_widget_mut().update(
            tree,
            &Event::Window(iced::window::Event::RedrawRequested(
                iced::time::Instant::now(),
            )),
            Layout::new(node),
            mouse::Cursor::Unavailable,
            renderer,
            &mut Null,
            &mut Shell::new(&mut Vec::new()),
            &Rectangle::with_size(size),
        );
        iced::advanced::Renderer::reset(renderer, Rectangle::with_size(size));
        let style = iced::advanced::renderer::Style {
            text_color: theme.colors.shell.ink,
        };
        element.as_widget().draw(
            tree,
            renderer,
            theme,
            &style,
            Layout::new(node),
            iced::mouse::Cursor::Unavailable,
            &Rectangle::with_size(size),
        );
        let pixels = renderer.screenshot(Size::new(1024, 768), 1.0, theme.colors.shell.background);
        image::save_buffer(path, &pixels, 1024, 768, image::ColorType::Rgba8).unwrap();
    }

    fn capture_completed_grid_steps(
        wizard: &mut BorderWalletWizard,
        renderer: &mut iced::Renderer,
        theme: &theme::Theme,
        size: Size,
        name: &str,
    ) {
        if let Ok(directory) = std::env::var("BORDER_WALLET_CAPTURE_DIR") {
            wizard.error = None;
            for col in 0..PATTERN_LENGTH as u8 {
                let _ = wizard.on_toggle_cell(3, col);
            }
            for step in ["complete", "checksum", "confirm"] {
                if step != "complete" {
                    let _ = wizard.on_next();
                }
                let view = match wizard.step {
                    WizardStep::Grid => wizard.view_grid(),
                    WizardStep::Checksum => wizard.view_checksum(),
                    _ => wizard.view_confirm(),
                };
                let mut element: Element<'_, Message> = Container::new(view)
                    .padding(coincube_ui::widget::modal::WINDOW_MARGIN)
                    .center_x(Length::Fill)
                    .center_y(Length::Fill)
                    .into();
                let mut tree = Tree::new(element.as_widget());
                let node = element.as_widget_mut().layout(
                    &mut tree,
                    renderer,
                    &layout::Limits::new(Size::ZERO, size),
                );
                capture(
                    &std::path::Path::new(&directory).join(format!(
                        "border-wallet-synthetic-{}-{}-1024x768.png",
                        step, name
                    )),
                    &mut element,
                    &mut tree,
                    &node,
                    renderer,
                    theme,
                    size,
                );
            }
        }
    }

    #[tokio::test]
    async fn tab_reveals_below_viewport_grid_cell_at_1024_in_both_themes() {
        assert_eq!(
            format!(
                "{:?}",
                BorderWalletWizardMessage::ToggleCellSelection(127, 15)
            ),
            "ToggleCellSelection(<redacted>)"
        );
        for (name, theme) in [
            ("dark", theme::Theme::dark()),
            ("light", theme::Theme::light()),
        ] {
            for bytes in coincube_ui::font::load() {
                iced::advanced::graphics::text::font_system()
                    .write()
                    .unwrap()
                    .load_font(bytes);
            }
            // A public BIP39 test vector is a synthetic grid fixture, never a
            // user's recovery phrase. Neither phrase nor pattern is logged.
            let grid = GridRecoveryPhrase::from_phrase(
                "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
            )
            .unwrap()
            .generate_grid();
            let mut wizard = BorderWalletWizard {
                network: Network::Regtest,
                coordinates: Vec::new(),
                step: WizardStep::Grid,
                phrase_words: vec![form::Value::default(); 12],
                phrase_valid: false,
                grid: Some(grid),
                pattern: OrderedPattern::new(),
                grid_focus_ids: BorderWalletWizard::grid_focus_ids(),
                checksum_word: None,
                enrollment: None,
                allow_random_grid_phrase: true,
                signer: None,
                phrase_source: GridSeedSource::Independent,
                error: None,
            };
            let mut element: Element<'_, Message> = Container::new(wizard.view_grid())
                // The modal lays the sheet out in the window less its margin.
                .padding(coincube_ui::widget::modal::WINDOW_MARGIN)
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .into();
            let mut renderer = <iced::Renderer as Headless>::new(
                coincube_ui::font::REGULAR,
                iced::Pixels(14.0),
                Some("tiny-skia"),
            )
            .await
            .unwrap();
            let size = Size::new(1024.0, 768.0);
            let mut tree = Tree::new(element.as_widget());
            let mut node = element.as_widget_mut().layout(
                &mut tree,
                &renderer,
                &layout::Limits::new(Size::ZERO, size),
            );
            let modal = node.children()[0].bounds();
            assert!(modal.x >= 0.0 && modal.x + modal.width <= size.width);
            assert!(modal.y >= 0.0 && modal.y + modal.height <= size.height);
            let mut targets = FocusTargets::default();
            element
                .as_widget_mut()
                .operate(&mut tree, Layout::new(&node), &renderer, &mut targets);
            assert!(targets.0.contains(&Id::new("border-wallet.pattern.back")));
            for disabled in [
                "border-wallet.pattern.undo",
                "border-wallet.pattern.clear",
                "border-wallet.pattern.next",
            ] {
                assert!(!targets.0.contains(&Id::new(disabled)));
            }
            let first_cell = wizard.grid_focus_ids[0].clone();
            operate(
                &mut element,
                &mut tree,
                &node,
                &renderer,
                focusable::focus::<()>(first_cell.clone()),
            );
            apply_focus_task(
                action::reveal_focused(BORDER_WALLET_GRID_SCROLL_ID),
                &mut element,
                &mut tree,
                &mut node,
                &renderer,
                size,
            )
            .await;
            let mut top = GridViewport::new(first_cell);
            element
                .as_widget_mut()
                .operate(&mut tree, Layout::new(&node), &renderer, &mut top);
            let first_bounds = top.screen_bounds();
            let top_viewport = top.viewport.unwrap();
            assert!(first_bounds.y >= top_viewport.y + 4.0);
            // The grid fills the sheet, so its height follows the window:
            // Tab from the last fully visible row's last cell must scroll.
            let row_pitch = first_bounds.height + 4.0;
            let visible_rows = ((top_viewport.height - 8.0) / row_pitch).floor() as usize;
            assert!(visible_rows >= 8, "the grid shows {} rows", visible_rows);
            let previous = wizard.grid_focus_ids[(visible_rows - 1) * WordGrid::COLS + 15].clone();
            let target = wizard.grid_focus_ids[visible_rows * WordGrid::COLS].clone();
            if let Ok(directory) = std::env::var("BORDER_WALLET_CAPTURE_DIR") {
                capture(
                    &std::path::Path::new(&directory).join(format!(
                        "border-wallet-synthetic-top-left-{name}-1024x768.png"
                    )),
                    &mut element,
                    &mut tree,
                    &node,
                    &mut renderer,
                    &theme,
                    size,
                );
            }
            operate(
                &mut element,
                &mut tree,
                &node,
                &renderer,
                focusable::focus::<()>(previous.clone()),
            );
            apply_focus_task(
                action::reveal_focused(BORDER_WALLET_GRID_SCROLL_ID),
                &mut element,
                &mut tree,
                &mut node,
                &renderer,
                size,
            )
            .await;
            let mut before = GridViewport::new(previous);
            element
                .as_widget_mut()
                .operate(&mut tree, Layout::new(&node), &renderer, &mut before);
            let before_offset = before.offset;
            let before_bounds = before.screen_bounds();
            let viewport = before.viewport.unwrap();
            assert!(before_bounds.y >= viewport.y + 4.0);
            assert!(
                before_bounds.y + before_bounds.height <= viewport.y + viewport.height - 4.0 + SNAP
            );

            // This is the global Tab path: keep the outer shell reveal and
            // then reveal the focused cell in the nested Border Wallet grid.
            apply_focus_task(
                action::focus_next_visible(crate::app::view::nav::shell::MIDDLE_SCROLL_ID)
                    .chain(action::reveal_focused(BORDER_WALLET_GRID_SCROLL_ID)),
                &mut element,
                &mut tree,
                &mut node,
                &renderer,
                size,
            )
            .await;
            let mut after = GridViewport::new(target);
            element
                .as_widget_mut()
                .operate(&mut tree, Layout::new(&node), &renderer, &mut after);
            assert!(
                after.offset > before_offset,
                "Tab must scroll to the next row"
            );
            let focused = after.screen_bounds();
            let viewport = after.viewport.unwrap();
            assert!(focused.y >= viewport.y + 4.0);
            assert!(focused.y + focused.height <= viewport.y + viewport.height - 4.0 + SNAP);

            let key = keyboard::Key::Named(keyboard::key::Named::Enter);
            let press = Event::Keyboard(keyboard::Event::KeyPressed {
                key: key.clone(),
                modified_key: key,
                physical_key: keyboard::key::Physical::Unidentified(
                    keyboard::key::NativeCode::Unidentified,
                ),
                location: keyboard::Location::Standard,
                modifiers: keyboard::Modifiers::empty(),
                text: None,
                repeat: false,
            });
            let mut messages = Vec::new();
            element.as_widget_mut().update(
                &mut tree,
                &press,
                Layout::new(&node),
                mouse::Cursor::Unavailable,
                &renderer,
                &mut Null,
                &mut Shell::new(&mut messages),
                &Rectangle::with_size(size),
            );
            assert_eq!(
                messages
                    .iter()
                    .filter(|message| matches!(
                        message,
                        Message::BorderWalletWizard(
                            BorderWalletWizardMessage::ToggleCellSelection(row, 0)
                        ) if *row as usize == visible_rows
                    ))
                    .count(),
                1,
                "Enter must activate the visible focused cell once"
            );

            // Tab into the final row through the same reveal chain used by
            // the GUI, then inspect the last cell's scrollbar clearance.
            let penultimate_row_end =
                wizard.grid_focus_ids[WordGrid::TOTAL_CELLS - WordGrid::COLS - 1].clone();
            let final_row_start =
                wizard.grid_focus_ids[WordGrid::TOTAL_CELLS - WordGrid::COLS].clone();
            operate(
                &mut element,
                &mut tree,
                &node,
                &renderer,
                focusable::focus::<()>(penultimate_row_end.clone()),
            );
            apply_focus_task(
                action::reveal_focused(BORDER_WALLET_GRID_SCROLL_ID),
                &mut element,
                &mut tree,
                &mut node,
                &renderer,
                size,
            )
            .await;
            let mut penultimate = GridViewport::new(penultimate_row_end);
            element.as_widget_mut().operate(
                &mut tree,
                Layout::new(&node),
                &renderer,
                &mut penultimate,
            );
            let mut final_row = GridViewport::new(final_row_start);
            apply_focus_task(
                action::focus_next_visible(crate::app::view::nav::shell::MIDDLE_SCROLL_ID)
                    .chain(action::reveal_focused(BORDER_WALLET_GRID_SCROLL_ID)),
                &mut element,
                &mut tree,
                &mut node,
                &renderer,
                size,
            )
            .await;
            element.as_widget_mut().operate(
                &mut tree,
                Layout::new(&node),
                &renderer,
                &mut final_row,
            );
            assert!(final_row.offset > penultimate.offset);
            let final_row_bounds = final_row.screen_bounds();
            let final_row_viewport = final_row.viewport.unwrap();
            assert!(final_row_bounds.y >= final_row_viewport.y + 4.0);
            assert!(
                final_row_bounds.y + final_row_bounds.height
                    <= final_row_viewport.y + final_row_viewport.height - 4.0 + SNAP
            );

            let last_cell = wizard.grid_focus_ids[WordGrid::TOTAL_CELLS - 1].clone();
            operate(
                &mut element,
                &mut tree,
                &node,
                &renderer,
                focusable::focus::<()>(last_cell.clone()),
            );
            apply_focus_task(
                action::reveal_focused(BORDER_WALLET_GRID_SCROLL_ID),
                &mut element,
                &mut tree,
                &mut node,
                &renderer,
                size,
            )
            .await;
            let mut bottom = GridViewport::new(last_cell);
            element
                .as_widget_mut()
                .operate(&mut tree, Layout::new(&node), &renderer, &mut bottom);
            assert!(bottom.offset > after.offset);
            let focused = bottom.screen_bounds();
            let viewport = bottom.viewport.unwrap();
            assert!(focused.y >= viewport.y + 4.0);
            assert!(focused.y + focused.height <= viewport.y + viewport.height - 4.0 + SNAP);
            assert!(
                focused.x + focused.width + 4.0 <= viewport.x + viewport.width - 10.0,
                "column 16 focus ring must clear the 10 px embedded scrollbar"
            );

            if let Ok(directory) = std::env::var("BORDER_WALLET_CAPTURE_DIR") {
                capture(
                    &std::path::Path::new(&directory).join(format!(
                        "border-wallet-synthetic-bottom-right-{name}-1024x768.png"
                    )),
                    &mut element,
                    &mut tree,
                    &node,
                    &mut renderer,
                    &theme,
                    size,
                );
            }

            drop(element);
            wizard.error = Some(
                "Your pattern already has all 11 cells. Deselect one before choosing another."
                    .to_string(),
            );
            let mut error_element: Element<'_, Message> = Container::new(wizard.view_grid())
                // The modal lays the sheet out in the window less its margin.
                .padding(coincube_ui::widget::modal::WINDOW_MARGIN)
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .into();
            let mut error_tree = Tree::new(error_element.as_widget());
            let error_node = error_element.as_widget_mut().layout(
                &mut error_tree,
                &renderer,
                &layout::Limits::new(Size::ZERO, size),
            );
            let error_modal = error_node.children()[0].bounds();
            assert!(error_modal.y >= 0.0 && error_modal.y + error_modal.height <= size.height);
            if let Ok(directory) = std::env::var("BORDER_WALLET_CAPTURE_DIR") {
                capture(
                    &std::path::Path::new(&directory).join(format!(
                        "border-wallet-synthetic-error-line-{name}-1024x768.png"
                    )),
                    &mut error_element,
                    &mut error_tree,
                    &error_node,
                    &mut renderer,
                    &theme,
                    size,
                );
            }

            // The full pattern enables Continue; Checksum and Confirm follow.
            drop(error_element);
            capture_completed_grid_steps(&mut wizard, &mut renderer, &theme, size, name);
        }
    }
}
