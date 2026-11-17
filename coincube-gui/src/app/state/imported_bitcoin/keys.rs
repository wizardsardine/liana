//! Import or restore's Keys step: the descriptor names its signers and the
//! user says where each key is. Recovery words are checked against the
//! signer's fingerprint, origin and xpub in both branches and are stored in
//! this Cube once the wallet is attached. A hardware wallet is checked
//! against the connected device and keeps its seed. In a multisig wallet the
//! signers past the threshold may sign elsewhere (Robert, 2026-11-02, #914).

use coincube_core::{
    imported_keys::ImportedAccount,
    miniscript::{bitcoin::bip32::Fingerprint, DescriptorPublicKey, ForEachKey},
    wallet_descriptor::ImportedWalletDescriptor,
};
use coincube_ui::{
    component::{button, text::*},
    locale::copy as localized_copy,
    theme,
    widget::{Column, Element, Row},
};
use std::sync::atomic::{AtomicU64, Ordering};

use iced::{widget::text_input, Alignment, Length};
use zeroize::Zeroizing;

use crate::app::view;

use super::ImportMessage;

/// A fresh id for one words or device check. A result lands only on the
/// slot whose current check carries the same id.
pub(super) fn next_check() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Where the user keeps one signer's key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    Words,
    Hardware,
    /// Another wallet or person signs; Tenshu saves the PSBT for them.
    Elsewhere,
}

impl KeySource {
    fn label(self) -> &'static str {
        match self {
            Self::Words => "Recovery words",
            Self::Hardware => "Hardware wallet",
            Self::Elsewhere => "Skip this key",
        }
    }
}

/// The source menu shows each choice in the current language.
impl std::fmt::Display for KeySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(localized_copy(self.label()))
    }
}

/// The source menu's choices: a single-signer wallet has no one else to
/// sign, an n-of-n wallet needs every key here, and adding to an attached
/// wallet only stores words.
const MULTISIG_SOURCES: [KeySource; 3] =
    [KeySource::Hardware, KeySource::Words, KeySource::Elsewhere];
const SINGLE_SOURCES: [KeySource; 2] = [KeySource::Hardware, KeySource::Words];

/// A connected hardware wallet, as the Keys step lists it so the user can
/// tell which device holds which key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ListedDevice {
    pub(super) name: String,
    /// Unknown while the device is locked or cannot be read.
    pub(super) fingerprint: Option<Fingerprint>,
    /// Why the device cannot be checked yet, already localized.
    pub(super) problem: Option<String>,
    /// The code a BitBox02 shows while it waits to be paired.
    pub(super) pairing_code: Option<String>,
    /// Connected but locked, so its fingerprint is not known yet.
    pub(super) locked: bool,
}

impl ListedDevice {
    /// A device whose fingerprint is known and can be checked.
    fn readable(&self) -> Option<Fingerprint> {
        self.fingerprint.filter(|_| self.problem.is_none())
    }
}

/// A device kind as people name it.
pub(super) fn device_name(kind: &async_hwi::DeviceKind) -> &'static str {
    use async_hwi::DeviceKind;
    match kind {
        DeviceKind::BitBox02 => "BitBox02",
        DeviceKind::Coldcard => "Coldcard",
        DeviceKind::Specter => "Specter",
        DeviceKind::SpecterSimulator => "Specter simulator",
        DeviceKind::Ledger => "Ledger",
        DeviceKind::LedgerSimulator => "Ledger simulator",
        DeviceKind::Jade => "Jade",
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum KeyStatus {
    Missing,
    Checking,
    Verified,
    Failed(String),
}

/// One signer from the descriptor. Words stay zeroizing and are cleared once
/// stored; nothing here reaches Cube Settings.
pub(super) struct KeySlot {
    pub(super) account: ImportedAccount,
    pub(super) source: KeySource,
    pub(super) words: Zeroizing<String>,
    pub(super) passphrase: Zeroizing<String>,
    pub(super) status: KeyStatus,
    /// The check in flight, if any; 0 when none.
    pub(super) check: u64,
    /// The connected device this key was last checked on without a click.
    /// It is checked on its own once per connection: a failed check waits
    /// for Check again, or for the device to be reconnected.
    pub(super) auto_checked: Option<String>,
}

impl KeySlot {
    /// A key this Cube can sign with.
    pub(super) fn provided(&self) -> bool {
        self.status == KeyStatus::Verified && self.source != KeySource::Elsewhere
    }
}

pub(super) struct KeysState {
    pub(super) slots: Vec<KeySlot>,
    pub(super) threshold: usize,
    /// The key whose "where is it" choices are open, if any.
    pub(super) menu: Option<usize>,
}

impl KeysState {
    /// Every signer starts on a hardware wallet, the safer place for a key
    /// (Robert, 2026-11-16).
    pub(super) fn new(descriptor: &ImportedWalletDescriptor) -> Self {
        let slots = signers(descriptor)
            .into_iter()
            .map(|account| KeySlot {
                account,
                source: KeySource::Hardware,
                words: Zeroizing::new(String::new()),
                passphrase: Zeroizing::new(String::new()),
                status: KeyStatus::Missing,
                check: 0,
                auto_checked: None,
            })
            .collect::<Vec<_>>();
        let threshold = signing_threshold(descriptor).clamp(1, slots.len().max(1));
        Self {
            slots,
            threshold,
            menu: None,
        }
    }

    /// Every signer starts on `source` instead, for when a device cannot be
    /// used: adding words to an attached wallet, or no device access.
    pub(super) fn starting_on(mut self, source: KeySource) -> Self {
        for slot in &mut self.slots {
            slot.source = source;
        }
        self
    }

    pub(super) fn multisig(&self) -> bool {
        self.slots.len() > 1
    }

    /// A key may be skipped only when the others can still reach the
    /// threshold: never in a single-signer or n-of-n wallet.
    pub(super) fn can_skip(&self) -> bool {
        self.multisig() && self.threshold < self.slots.len()
    }

    /// The choices the source menu offers for each key.
    pub(super) fn sources(&self) -> &'static [KeySource] {
        if self.can_skip() {
            &MULTISIG_SOURCES
        } else {
            &SINGLE_SOURCES
        }
    }

    pub(super) fn provided(&self) -> usize {
        self.slots.iter().filter(|slot| slot.provided()).count()
    }

    /// Enough keys to sign, and none still checking. Every signer not
    /// provided then signs elsewhere, whether or not it was marked so: the
    /// threshold is all the wallet needs (Robert, 2026-11-16).
    pub(super) fn ready(&self) -> bool {
        self.provided() >= self.threshold && !self.checking()
    }

    /// The signers this Cube cannot sign for.
    pub(super) fn signs_elsewhere(&self) -> usize {
        self.slots.iter().filter(|slot| !slot.provided()).count()
    }

    /// Adding words to an attached wallet: at least one verified set of
    /// words, nothing still checking. Other signers need no answer.
    pub(super) fn ready_to_add(&self) -> bool {
        !self.checking()
            && self
                .slots
                .iter()
                .any(|slot| slot.source == KeySource::Words && slot.provided())
    }

    /// Wipe every typed phrase and passphrase; verified words fall back to
    /// unverified, since they would have to be typed again.
    pub(super) fn forget_words(&mut self) {
        for slot in &mut self.slots {
            slot.words = Zeroizing::new(String::new());
            slot.passphrase = Zeroizing::new(String::new());
            slot.check = 0;
            // A check in flight can no longer land, so nothing may stay
            // Checking: that would hold every control disabled.
            if slot.source == KeySource::Words || slot.status == KeyStatus::Checking {
                // A cancelled device check never ran to the end: the
                // device is checked again when the step is next shown.
                if slot.status == KeyStatus::Checking {
                    slot.auto_checked = None;
                }
                slot.status = KeyStatus::Missing;
            }
        }
    }

    pub(super) fn checking(&self) -> bool {
        self.slots
            .iter()
            .any(|slot| slot.status == KeyStatus::Checking)
    }

    /// Review's one-line account of the keys.
    pub(super) fn summary(&self) -> String {
        let mut sources = Vec::new();
        for source in [KeySource::Words, KeySource::Hardware] {
            let count = self
                .slots
                .iter()
                .filter(|slot| slot.source == source && slot.provided())
                .count();
            if count > 0 {
                sources.push(if source == KeySource::Words {
                    localized_copy("recovery words")
                } else {
                    localized_copy("hardware wallet")
                });
            }
        }
        let mut line = coincube_ui::localized_format!(
            "Keys: {} of {} provided · {} · verified against the fingerprint",
            "Claves: {} de {} aportadas · {} · verificadas con la huella",
            self.provided(),
            self.slots.len(),
            sources.join(localized_copy(" and "))
        );
        let elsewhere = self.signs_elsewhere();
        if elsewhere > 0 {
            line.push_str(&coincube_ui::localized_format!(
                " · {elsewhere} not added here",
                " · {elsewhere} sin añadir aquí"
            ));
        }
        line
    }
}

/// Each xpub signer with an origin, in descriptor order, that appears in both
/// branches. The descriptor parser already refuses a key missing from a branch.
pub(super) fn signers(descriptor: &ImportedWalletDescriptor) -> Vec<ImportedAccount> {
    let mut accounts: Vec<ImportedAccount> = Vec::new();
    descriptor.receive_descriptor().for_each_key(|key| {
        if let DescriptorPublicKey::XPub(key) = key {
            if let Some((fingerprint, origin)) = &key.origin {
                let account = ImportedAccount {
                    fingerprint: *fingerprint,
                    origin: origin.clone(),
                    xpub: key.xkey,
                };
                if !accounts.contains(&account)
                    && coincube_core::imported_keys::account_in_both_branches(descriptor, &account)
                {
                    accounts.push(account);
                }
            }
        }
        true
    });
    accounts
}

/// Signatures a spend needs: `k` of a `multi`/`sortedmulti`, else one. The
/// imported shapes are `pkh`, `wpkh`, `sh(wpkh)` and `wsh((sorted)multi)`.
pub(super) fn signing_threshold(descriptor: &ImportedWalletDescriptor) -> usize {
    descriptor
        .receive_descriptor()
        .to_string()
        .split_once("multi(")
        .and_then(|(_, rest)| rest.split_once(','))
        .and_then(|(k, _)| k.trim().parse().ok())
        .unwrap_or(1)
}

fn message(message: ImportMessage) -> view::Message {
    view::Message::ImportedBitcoin(message)
}

type TextStyle = fn(&theme::Theme) -> iced::widget::text::Style;

fn fingerprint_text<'a>(fingerprint: Fingerprint, style: TextStyle) -> Element<'a, view::Message> {
    iced::widget::text(fingerprint.to_string())
        .font(coincube_ui::font::MONO_REGULAR)
        .size(13)
        .style(style)
        .into()
}

/// The look of a row's icon tile (design: Keys step, impeccable pass).
#[derive(Clone, Copy)]
enum Tone {
    Neutral,
    Accent,
    Good,
    Warn,
    Bad,
}

/// What a key row's icon says.
#[derive(Clone, Copy)]
enum Mark {
    Waiting,
    Device,
    Checking,
    Verified,
    Words,
    Elsewhere,
    Failed,
}

fn tinted(color: iced::Color, alpha: f32) -> iced::Color {
    iced::Color { a: alpha, ..color }
}

fn tone_colors(theme: &theme::Theme, tone: Tone) -> (iced::Color, iced::Color) {
    let shell = theme.colors.shell;
    match tone {
        Tone::Neutral => (shell.hover, shell.muted),
        Tone::Accent => (shell.selected, shell.accent),
        Tone::Good => (tinted(shell.good, 0.1), shell.good),
        Tone::Warn => (tinted(shell.warn, 0.1), shell.warn),
        Tone::Bad => (tinted(shell.bad, 0.1), shell.bad),
    }
}

/// An icon on a small rounded tile, tinted by `tone`.
fn icon_tile<'a>(
    icon: coincube_ui::widget::Text<'a>,
    tone: Tone,
    size: f32,
) -> Element<'a, view::Message> {
    coincube_ui::widget::Container::new(icon.size(size * 0.5))
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .style(move |theme: &theme::Theme| {
            let (background, color) = tone_colors(theme, tone);
            iced::widget::container::Style {
                background: Some(iced::Background::Color(background)),
                text_color: Some(color),
                border: iced::Border {
                    radius: 8.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        })
        .into()
}

fn mark_tile<'a>(mark: Mark) -> Element<'a, view::Message> {
    use coincube_ui::icon;
    match mark {
        Mark::Waiting => icon_tile(icon::plug_icon(), Tone::Neutral, 32.0),
        Mark::Device => icon_tile(icon::usb_icon(), Tone::Accent, 32.0),
        // A slow pulse while the device is asked for the key.
        Mark::Checking => coincube_ui::component::spinner::Carousel::new(
            std::time::Duration::from_millis(700),
            vec![
                icon_tile(icon::arrow_repeat(), Tone::Accent, 32.0),
                icon_tile(icon::arrow_repeat(), Tone::Neutral, 32.0),
            ],
        )
        .into(),
        Mark::Verified => icon_tile(icon::check_circle_icon(), Tone::Good, 32.0),
        Mark::Words => icon_tile(icon::key_icon(), Tone::Neutral, 32.0),
        Mark::Elsewhere => icon_tile(icon::share_icon(), Tone::Neutral, 32.0),
        Mark::Failed => icon_tile(icon::exclamation_circle_icon(), Tone::Bad, 32.0),
    }
}

/// What picking `source` for key `index` does. The current choice only
/// closes the list: picking it again would reset the key, clearing typed
/// words or a verified device.
pub(super) fn choice_message(index: usize, source: KeySource, current: KeySource) -> ImportMessage {
    if source == current {
        ImportMessage::KeySourceMenu(index)
    } else {
        ImportMessage::KeySourcePicked(index, source)
    }
}

/// A radio mark: an empty ring, filled with the accent when picked.
fn radio_mark<'a>(picked: bool) -> Element<'a, view::Message> {
    coincube_ui::widget::Container::new(
        coincube_ui::widget::Container::new(iced::widget::Space::new())
            .width(Length::Fixed(8.0))
            .height(Length::Fixed(8.0))
            .style(move |theme: &theme::Theme| iced::widget::container::Style {
                background: picked.then_some(iced::Background::Color(theme.colors.shell.accent)),
                border: iced::Border {
                    radius: 4.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            }),
    )
    .width(Length::Fixed(16.0))
    .height(Length::Fixed(16.0))
    .align_x(Alignment::Center)
    .align_y(Alignment::Center)
    .style(move |theme: &theme::Theme| {
        let shell = theme.colors.shell;
        iced::widget::container::Style {
            border: iced::Border {
                radius: 8.0.into(),
                width: 1.5,
                color: if picked { shell.accent } else { shell.muted },
            },
            ..Default::default()
        }
    })
    .into()
}

/// A row in the "where is it" list: the picked one on the selected tint,
/// the others on hover.
fn choice_style(
    theme: &theme::Theme,
    status: iced::widget::button::Status,
    picked: bool,
) -> iced::widget::button::Style {
    let shell = theme.colors.shell;
    let background = if picked {
        Some(shell.selected)
    } else if matches!(
        status,
        iced::widget::button::Status::Hovered | iced::widget::button::Status::Pressed
    ) {
        Some(shell.hover)
    } else {
        None
    };
    iced::widget::button::Style {
        background: background.map(iced::Background::Color),
        text_color: shell.ink,
        border: iced::Border {
            radius: 8.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// A fingerprint on a green chip: the key and its device match.
fn fingerprint_chip<'a>(fingerprint: Fingerprint) -> Element<'a, view::Message> {
    coincube_ui::widget::Container::new(
        iced::widget::text(fingerprint.to_string())
            .font(coincube_ui::font::MONO_REGULAR)
            .size(13),
    )
    .padding([1, 6])
    .style(|theme: &theme::Theme| {
        let (background, color) = tone_colors(theme, Tone::Good);
        iced::widget::container::Style {
            background: Some(iced::Background::Color(background)),
            text_color: Some(color),
            border: iced::Border {
                radius: 5.0.into(),
                ..Default::default()
            },
            ..Default::default()
        }
    })
    .into()
}

fn verified_pill<'a>() -> Element<'a, view::Message> {
    coincube_ui::widget::Container::new(
        Row::new()
            .spacing(4)
            .align_y(Alignment::Center)
            .push(
                coincube_ui::icon::check_icon()
                    .size(12)
                    .width(Length::Shrink),
            )
            .push(p2_bold(localized_copy("Verified"))),
    )
    .padding([1, 7])
    .style(|theme: &theme::Theme| {
        let good = theme.colors.shell.good;
        iced::widget::container::Style {
            text_color: Some(good),
            border: iced::Border {
                radius: 5.0.into(),
                width: 1.0,
                color: good,
            },
            ..Default::default()
        }
    })
    .into()
}

/// One dot per needed key, filled once it is verified.
fn pips<'a>(filled: usize, total: usize) -> Element<'a, view::Message> {
    let mut row = Row::new().spacing(4).align_y(Alignment::Center);
    for index in 0..total {
        let on = index < filled;
        row = row.push(
            coincube_ui::widget::Container::new(iced::widget::Space::new())
                .width(Length::Fixed(10.0))
                .height(Length::Fixed(10.0))
                .style(move |theme: &theme::Theme| {
                    let shell = theme.colors.shell;
                    iced::widget::container::Style {
                        background: on.then_some(iced::Background::Color(shell.good)),
                        border: iced::Border {
                            radius: 5.0.into(),
                            width: 1.5,
                            color: if on { shell.good } else { shell.line },
                        },
                        ..Default::default()
                    }
                }),
        );
    }
    row.into()
}

impl KeySource {
    /// The line under each choice in the "where is it" list.
    fn description(self) -> &'static str {
        match self {
            Self::Hardware => "Checked on the device; the seed stays there.",
            Self::Words => "Checked here, then stored encrypted for this Cube.",
            Self::Elsewhere => {
                "Another wallet or person signs; COINCUBE Desktop saves the PSBT for them."
            }
        }
    }
}

/// The readable connected device that holds signer `slot`'s key, if any.
fn matching_device<'d>(slot: &KeySlot, devices: &'d [ListedDevice]) -> Option<&'d ListedDevice> {
    devices
        .iter()
        .find(|device| device.readable() == Some(slot.account.fingerprint))
}

/// A row's status: a first line (with the matched device, if any), a
/// second line, and the action the key is waiting on.
struct KeyLine<'a> {
    mark: Mark,
    device: Option<&'a ListedDevice>,
    title: String,
    verified: bool,
    detail: String,
    detail_style: TextStyle,
    dimmed: bool,
    action: Option<(&'static str, ImportMessage)>,
}

fn key_line<'d>(
    keys: &KeysState,
    index: usize,
    slot: &KeySlot,
    devices: &'d [ListedDevice],
) -> KeyLine<'d> {
    let device = matching_device(slot, devices);
    let line = |title: String, detail: String| KeyLine {
        mark: Mark::Waiting,
        device: None,
        title,
        verified: false,
        detail,
        detail_style: theme::text::secondary,
        dimmed: false,
        action: None,
    };
    match (slot.source, &slot.status) {
        (KeySource::Elsewhere, _) => KeyLine {
            mark: Mark::Elsewhere,
            ..line(
                localized_copy("Skipped").to_owned(),
                localized_copy("Not added here. Another wallet or person signs with this key; COINCUBE Desktop saves the PSBT for them.").to_owned(),
            )
        },
        (KeySource::Words, KeyStatus::Verified) => KeyLine {
            mark: Mark::Verified,
            verified: true,
            ..line(
                localized_copy("Recovery words").to_owned(),
                localized_copy("Stored encrypted for this Cube once the wallet is saved.")
                    .to_owned(),
            )
        },
        (KeySource::Words, KeyStatus::Checking) => KeyLine {
            mark: Mark::Checking,
            ..line(
                localized_copy("Recovery words").to_owned(),
                localized_copy("Checking…").to_owned(),
            )
        },
        (KeySource::Words, KeyStatus::Failed(reason)) => KeyLine {
            mark: Mark::Failed,
            detail_style: theme::text::error,
            ..line(
                localized_copy("Recovery words").to_owned(),
                super::shown_notice(reason),
            )
        },
        (KeySource::Words, KeyStatus::Missing) => KeyLine {
            mark: Mark::Words,
            ..line(
                localized_copy("Recovery words").to_owned(),
                localized_copy("Enter them below.").to_owned(),
            )
        },
        (KeySource::Hardware, KeyStatus::Verified) => KeyLine {
            mark: Mark::Verified,
            device,
            verified: true,
            ..line(
                device.map_or_else(
                    || localized_copy("Hardware wallet").to_owned(),
                    |device| device.name.clone(),
                ),
                localized_copy("The seed stays on the device. It signs whenever this wallet spends.")
                    .to_owned(),
            )
        },
        (KeySource::Hardware, KeyStatus::Checking) => {
            let name = device.map_or_else(
                || localized_copy("Hardware wallet").to_owned(),
                |device| device.name.clone(),
            );
            KeyLine {
                mark: Mark::Checking,
                device,
                ..line(
                    name.clone(),
                    coincube_ui::localized_format!(
                        "Checking this key on the {name}… Approve on the device if it asks.",
                        "Comprobando esta clave en el {name}… Apruébalo en el dispositivo si lo pide."
                    ),
                )
            }
        }
        (KeySource::Hardware, KeyStatus::Failed(reason)) => match device {
            Some(device) => KeyLine {
                mark: Mark::Failed,
                device: Some(device),
                detail_style: theme::text::error,
                action: Some(("Check again", ImportMessage::CheckDevice(index))),
                ..line(device.name.clone(), super::shown_notice(reason))
            },
            None => KeyLine {
                mark: Mark::Failed,
                detail_style: theme::text::error,
                ..line(
                    localized_copy("Waiting for its device").to_owned(),
                    super::shown_notice(reason),
                )
            },
        },
        // The check normally starts by itself once the device appears;
        // the button covers the moment before it does.
        (KeySource::Hardware, KeyStatus::Missing) => match device {
            Some(device) => KeyLine {
                mark: Mark::Device,
                device: Some(device),
                action: Some(("Check device", ImportMessage::CheckDevice(index))),
                ..line(
                    device.name.clone(),
                    localized_copy("Connected, and its fingerprint matches this key.").to_owned(),
                )
            },
            None if keys.multisig() && keys.ready() => KeyLine {
                mark: Mark::Elsewhere,
                dimmed: true,
                ..line(
                    localized_copy("Not added — not needed to spend").to_owned(),
                    coincube_ui::localized_format!(
                        "You've verified {} keys, which is enough to spend. Connect this device to add it too.",
                        "Has verificado {} claves, suficientes para gastar. Conecta este dispositivo para añadirla también.",
                        keys.threshold
                    ),
                )
            },
            None => line(
                localized_copy("Waiting for its device").to_owned(),
                localized_copy("Connect and unlock the hardware wallet with this fingerprint.")
                    .to_owned(),
            ),
        },
    }
}

/// The devices that are connected but cannot be put on a key: locked, not
/// ready, or not a signer. A matched device is shown in its key's row.
fn unmatched_devices<'a>(keys: &KeysState, devices: &[ListedDevice]) -> Element<'a, view::Message> {
    let mut list = Column::new().spacing(10).push(
        p2_bold(localized_copy("Connected, not matched to a key")).style(theme::text::secondary),
    );
    for device in devices.iter().filter(|device| !in_a_row(keys, device)) {
        use coincube_ui::icon;
        let tile = if device.pairing_code.is_some() {
            icon_tile(icon::link_icon(), Tone::Warn, 28.0)
        } else if device.locked {
            icon_tile(icon::lock_icon(), Tone::Warn, 28.0)
        } else if device.problem.is_some() {
            icon_tile(icon::warning_icon(), Tone::Warn, 28.0)
        } else {
            icon_tile(icon::tooltip_icon(), Tone::Neutral, 28.0)
        };
        let mut name = Row::new()
            .spacing(8)
            .align_y(Alignment::Center)
            .push(p1_bold(device.name.clone()));
        if let Some(fingerprint) = device.fingerprint {
            name = name.push(fingerprint_text(fingerprint, theme::text::secondary));
        }
        // A signer's device whose key is set to words or signing elsewhere
        // is not drawn in that row, so it says which key it holds.
        let held = device.readable().and_then(|fingerprint| {
            keys.slots
                .iter()
                .position(|slot| slot.account.fingerprint == fingerprint)
        });
        let mut detail = Row::new().spacing(8).align_y(Alignment::Center);
        detail = match (&device.problem, held) {
            (Some(problem), _) => {
                detail.push(p2_regular(problem.clone()).style(theme::text::warning))
            }
            (None, Some(index)) => detail.push(
                p2_regular(coincube_ui::localized_format!(
                    "Holds Key {}, which is set to {}. Choose Hardware wallet for it to check it here.",
                    "Guarda la clave {}, configurada como {}. Elige Cartera de hardware para comprobarla aquí.",
                    index + 1,
                    keys.slots[index].source.to_string().to_lowercase()
                ))
                .style(theme::text::secondary),
            ),
            (None, None) => detail.push(
                p2_regular(localized_copy("Not a signer of this wallet."))
                    .style(theme::text::secondary),
            ),
        };
        if let Some(code) = &device.pairing_code {
            detail = detail.push(
                iced::widget::text(code.clone())
                    .font(coincube_ui::font::MONO_REGULAR)
                    .size(13),
            );
        }
        list = list.push(
            Row::new()
                .spacing(12)
                .align_y(Alignment::Center)
                .push(tile)
                .push(Column::new().spacing(2).push(name).push(detail)),
        );
    }
    list.into()
}

/// A readable device is drawn in a key's row only when that key is on a
/// hardware wallet.
fn in_a_row(keys: &KeysState, device: &ListedDevice) -> bool {
    device.readable().is_some_and(|fingerprint| {
        keys.slots.iter().any(|slot| {
            slot.account.fingerprint == fingerprint && slot.source == KeySource::Hardware
        })
    })
}

fn has_unmatched(keys: &KeysState, devices: &[ListedDevice]) -> bool {
    devices.iter().any(|device| !in_a_row(keys, device))
}

/// The Keys step's form and action row: one row per signer, with the
/// matching device shown in its key's row, then the devices that cannot be
/// matched yet (design: Keys step, impeccable pass, 2026-11-16).
pub(super) fn step<'a>(
    keys: &'a KeysState,
    adding: bool,
    devices: &[ListedDevice],
    error: Option<&'a String>,
) -> (Element<'a, view::Message>, Element<'a, view::Message>) {
    let count = keys.slots.len();
    let lead = if adding {
        localized_copy("Enter the recovery words for any of this wallet's signers. Each set is checked against its signer's fingerprint, then stored on this device for this Cube.")
            .to_string()
    } else if keys.multisig() {
        coincube_ui::localized_format!(
            "This wallet has {count} signers and any {} can spend. Verify at least {} here. \
             You can skip the rest; they can still sign from their own device or wallet. \
             Each key is checked against its fingerprint.",
            "Esta cartera tiene {count} firmantes y bastan {} para gastar. Verifica al menos {} \
             aquí. Puedes omitir las demás; pueden seguir firmando desde su propio dispositivo \
             o cartera. Cada clave se verifica con su huella.",
            keys.threshold,
            keys.threshold
        )
    } else {
        localized_copy("The descriptor names 1 signer. Provide its key; it is checked against its fingerprint before it is accepted.")
            .to_string()
    };
    let mut content = Column::new()
        .spacing(20)
        .push(h3(localized_copy("Add the wallet's keys")))
        .push(p1_regular(lead).style(theme::text::secondary));
    let busy = keys.checking();
    let sources = keys.sources();
    let mut rows = Column::new().push(iced::widget::rule::horizontal(1));
    for (index, slot) in keys.slots.iter().enumerate() {
        let status = key_line(keys, index, slot, devices);
        let matched = status.device.is_some() || status.verified;
        let identity = Column::new()
            .spacing(2)
            .width(Length::Fixed(148.0))
            .push(
                p1_bold(coincube_ui::localized_format!(
                    "Key {}",
                    "Clave {}",
                    index + 1
                ))
                .style(if status.dimmed {
                    theme::text::secondary
                } else {
                    theme::text::default
                }),
            )
            .push(if matched {
                fingerprint_chip(slot.account.fingerprint)
            } else {
                fingerprint_text(slot.account.fingerprint, theme::text::default)
            })
            .push(
                iced::widget::text(format!("m/{}", slot.account.origin))
                    .font(coincube_ui::font::MONO_REGULAR)
                    .size(12)
                    .style(theme::text::secondary),
            );
        let mut title = Row::new().spacing(8).align_y(Alignment::Center).push(
            p1_bold(status.title.clone()).style(if status.dimmed {
                theme::text::secondary
            } else {
                theme::text::default
            }),
        );
        if let Some(device) = status.device {
            if let Some(fingerprint) = device.fingerprint {
                title = title.push(fingerprint_chip(fingerprint));
            }
        }
        if status.verified {
            title = title.push(verified_pill());
        }
        let mut row = Row::new()
            .spacing(20)
            .align_y(Alignment::Center)
            .padding([12, 0])
            .push(identity)
            .push(
                Row::new()
                    .spacing(12)
                    .align_y(Alignment::Center)
                    .width(Length::Fill)
                    .push(mark_tile(status.mark))
                    .push(
                        Column::new()
                            .spacing(2)
                            .width(Length::Fill)
                            .push(title)
                            .push(p2_regular(status.detail).style(status.detail_style)),
                    ),
            );
        if let Some((label, action)) = status.action {
            row = row.push(
                button::primary(None, localized_copy(label))
                    .width(Length::Shrink)
                    .on_press_maybe((!busy).then_some(message(action))),
            );
        }
        if slot.source == KeySource::Words && slot.status == KeyStatus::Verified {
            row = row.push(
                button::secondary(None, localized_copy("Enter different words"))
                    .width(Length::Shrink)
                    .on_press_maybe((!busy).then_some(message(ImportMessage::KeySourcePicked(
                        index,
                        KeySource::Words,
                    )))),
            );
        }
        // Adding to an attached wallet only stores words, so there is
        // nothing to choose.
        let menu_open = !adding && keys.menu == Some(index);
        if !adding {
            row = row.push(
                iced::widget::button(
                    Row::new()
                        .spacing(6)
                        .align_y(Alignment::Center)
                        .push(p2_regular(slot.source.to_string()))
                        .push(
                            if menu_open {
                                coincube_ui::icon::up_icon()
                            } else {
                                coincube_ui::icon::down_icon()
                            }
                            .size(12)
                            .width(Length::Shrink),
                        ),
                )
                .padding([6, 10])
                .style(move |theme: &theme::Theme, status| {
                    if menu_open {
                        let shell = theme.colors.shell;
                        iced::widget::button::Style {
                            background: Some(iced::Background::Color(shell.selected)),
                            text_color: shell.ink,
                            border: iced::Border {
                                radius: 8.0.into(),
                                width: 1.0,
                                color: shell.accent,
                            },
                            ..Default::default()
                        }
                    } else {
                        theme::button::secondary(theme, status)
                    }
                })
                .on_press_maybe((!busy).then_some(message(ImportMessage::KeySourceMenu(index)))),
            );
        }
        let mut key = Column::new().push(row);
        // Where the key is: each choice with what it means, opened under
        // its row so the rows below move down rather than being covered.
        if menu_open && !busy {
            let mut choices = Column::new().spacing(4).padding(iced::Padding {
                top: 0.0,
                right: 0.0,
                bottom: 12.0,
                left: 168.0,
            });
            for &source in sources {
                let picked = slot.source == source;
                choices = choices.push(
                    iced::widget::button(
                        Row::new()
                            .spacing(12)
                            .align_y(Alignment::Center)
                            .push(radio_mark(picked))
                            .push(
                                Column::new()
                                    .spacing(2)
                                    .push(p1_medium(source.to_string()))
                                    .push(
                                        p2_regular(localized_copy(source.description()))
                                            .style(theme::text::secondary),
                                    ),
                            ),
                    )
                    .width(Length::Fill)
                    .padding([8, 12])
                    .style(move |theme: &theme::Theme, status| choice_style(theme, status, picked))
                    .on_press(message(choice_message(
                        index,
                        source,
                        slot.source,
                    ))),
                );
            }
            key = key.push(choices);
        }
        if slot.source == KeySource::Words && slot.status != KeyStatus::Verified {
            key = key.push(
                Column::new()
                    .spacing(8)
                    .padding(iced::Padding {
                        top: 0.0,
                        right: 0.0,
                        bottom: 14.0,
                        left: 168.0,
                    })
                    .push(
                        text_input(
                            localized_copy("12 or 24 words, separated by spaces"),
                            slot.words.as_str(),
                        )
                        .on_input(move |value| {
                            message(ImportMessage::KeyWordsEdited(
                                index,
                                super::SecretInput::new(value),
                            ))
                        })
                        .padding(10),
                    )
                    .push(
                        Row::new()
                            .spacing(12)
                            .align_y(Alignment::Center)
                            .push(
                                text_input(
                                    localized_copy("Passphrase (if the wallet used one)"),
                                    slot.passphrase.as_str(),
                                )
                                .on_input(move |value| {
                                    message(ImportMessage::KeyPassphraseEdited(
                                        index,
                                        super::SecretInput::new(value),
                                    ))
                                })
                                .secure(true)
                                .padding(10)
                                .width(Length::Fixed(320.0)),
                            )
                            .push(iced::widget::Space::new().width(Length::Fill))
                            .push(
                                button::secondary(None, localized_copy("Verify words"))
                                    .width(Length::Shrink)
                                    .on_press_maybe(
                                        (!busy
                                            && slot.status != KeyStatus::Checking
                                            && !slot.words.trim().is_empty())
                                        .then_some(message(ImportMessage::VerifyWords(index))),
                                    ),
                            ),
                    )
                    .push(
                        caption(localized_copy("Words are checked here, then stored encrypted on this device for this Cube. They are not in the Recovery Kit yet, so keep your original backup."))
                        .style(theme::text::secondary),
                    ),
            );
        }
        rows = rows.push(key).push(iced::widget::rule::horizontal(1));
    }
    content = content.push(rows);
    if !adding {
        // Once the threshold is met no device is needed, so there is
        // nothing to ask for.
        let awaiting_device = !keys.ready()
            && keys
                .slots
                .iter()
                .any(|slot| slot.source == KeySource::Hardware && !slot.provided());
        if devices.is_empty() && awaiting_device {
            content = content.push(
                coincube_ui::widget::Container::new(
                    Row::new()
                        .spacing(14)
                        .align_y(Alignment::Center)
                        .push(coincube_ui::icon::plug_icon().style(theme::text::secondary))
                        .push(
                            p2_regular(localized_copy(
                                "No hardware wallet connected. Plug one in and unlock it; it appears beside the key it holds.",
                            ))
                            .style(theme::text::secondary),
                        ),
                )
                .padding(14)
                .width(Length::Fill)
                .style(theme::card::border),
            );
        } else if has_unmatched(keys, devices) {
            content = content.push(unmatched_devices(keys, devices));
        }
    }
    if let Some(error) = error {
        content = content.push(p1_regular(super::shown_notice(error)).style(theme::text::error));
    }
    // How far the keys are, beside the button they open.
    let progress = (!adding && keys.multisig()).then(|| {
        let provided = keys.provided().min(keys.threshold);
        let mut line = coincube_ui::localized_format!(
            "{provided} of {} needed keys verified",
            "{provided} de {} claves necesarias verificadas",
            keys.threshold
        );
        if keys.ready() {
            let elsewhere: Vec<usize> = keys
                .slots
                .iter()
                .enumerate()
                .filter(|(_, slot)| !slot.provided())
                .map(|(index, _)| index + 1)
                .collect();
            match elsewhere.as_slice() {
                [] => {}
                [only] => line.push_str(&coincube_ui::localized_format!(
                    " · Key {only} not added",
                    " · clave {only} sin añadir"
                )),
                many => line.push_str(&coincube_ui::localized_format!(
                    " · {} keys not added",
                    " · {} claves sin añadir",
                    many.len()
                )),
            }
        }
        Row::new()
            .spacing(10)
            .align_y(Alignment::Center)
            .push(pips(provided, keys.threshold))
            .push(p2_regular(line).style(if keys.ready() {
                theme::text::success
            } else {
                theme::text::secondary
            }))
    });
    let actions = Row::new()
        .spacing(10)
        .align_y(Alignment::Center)
        .push((!adding).then(|| {
            button::secondary(None, localized_copy("Back"))
                .width(Length::Shrink)
                .on_press_maybe((!busy).then_some(message(ImportMessage::Edit)))
        }))
        .push(iced::widget::Space::new().width(Length::Fill))
        .push(progress)
        .push(
            button::primary(
                None,
                if adding {
                    localized_copy("Store words")
                } else {
                    localized_copy("Review wallet")
                },
            )
            .width(Length::Shrink)
            .on_press_maybe(
                (!busy
                    && if adding {
                        keys.ready_to_add()
                    } else {
                        keys.ready()
                    })
                .then_some(message(ImportMessage::KeysDone)),
            ),
        );
    (content.into(), actions.into())
}
