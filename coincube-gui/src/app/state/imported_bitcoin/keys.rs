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
            Self::Elsewhere => "Signs elsewhere",
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
/// sign, and adding to an attached wallet only stores words.
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
        Self { slots, threshold }
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
                " · {elsewhere} signs elsewhere",
                " · {elsewhere} con firma externa"
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

/// The readable connected device that holds signer `slot`'s key, if any.
fn matching_device<'d>(slot: &KeySlot, devices: &'d [ListedDevice]) -> Option<&'d ListedDevice> {
    devices
        .iter()
        .find(|device| device.readable() == Some(slot.account.fingerprint))
}

/// A row's status: a first line (with the matched device, if any), a
/// second line, and the action the key is waiting on.
struct KeyLine<'a> {
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
        device: None,
        title,
        verified: false,
        detail,
        detail_style: theme::text::secondary,
        dimmed: false,
        action: None,
    };
    match (slot.source, &slot.status) {
        (KeySource::Elsewhere, _) => line(
            localized_copy("Signs elsewhere").to_owned(),
            localized_copy("Another wallet or person signs with this key. COINCUBE Desktop saves the PSBT for them.").to_owned(),
        ),
        (KeySource::Words, KeyStatus::Verified) => KeyLine {
            verified: true,
            ..line(
                localized_copy("Recovery words").to_owned(),
                localized_copy("Stored encrypted for this Cube once the wallet is saved.")
                    .to_owned(),
            )
        },
        (KeySource::Words, KeyStatus::Checking) => line(
            localized_copy("Recovery words").to_owned(),
            localized_copy("Checking…").to_owned(),
        ),
        (KeySource::Words, KeyStatus::Failed(reason)) => KeyLine {
            detail_style: theme::text::error,
            ..line(
                localized_copy("Recovery words").to_owned(),
                super::shown_notice(reason),
            )
        },
        (KeySource::Words, KeyStatus::Missing) => line(
            localized_copy("Recovery words").to_owned(),
            localized_copy("Enter them below.").to_owned(),
        ),
        (KeySource::Hardware, KeyStatus::Verified) => KeyLine {
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
                device: Some(device),
                detail_style: theme::text::error,
                action: Some(("Check again", ImportMessage::CheckDevice(index))),
                ..line(device.name.clone(), super::shown_notice(reason))
            },
            None => KeyLine {
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
                device: Some(device),
                action: Some(("Check device", ImportMessage::CheckDevice(index))),
                ..line(
                    device.name.clone(),
                    localized_copy("Connected, and its fingerprint matches this key.").to_owned(),
                )
            },
            None if keys.multisig() && keys.ready() => KeyLine {
                dimmed: true,
                ..line(
                    localized_copy("Not needed to spend").to_owned(),
                    coincube_ui::localized_format!(
                        "{} keys are verified, so this one signs elsewhere. Connect its device to add it anyway.",
                        "Hay {} claves verificadas, así que esta firma en otro lugar. Conecta su dispositivo para añadirla de todos modos.",
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
        list = list.push(Column::new().spacing(2).push(name).push(detail));
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
            "This wallet has {count} signers and any {} can spend. Verify {} of them here; \
             the others sign elsewhere. Each key is checked against its fingerprint.",
            "Esta cartera tiene {count} firmantes y bastan {} para gastar. Verifica {} de \
             ellas aquí; las demás firman en otro lugar. Cada clave se verifica con su huella.",
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
    let sources: &'static [KeySource] = if keys.multisig() {
        &MULTISIG_SOURCES
    } else {
        &SINGLE_SOURCES
    };
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
            .push(fingerprint_text(
                slot.account.fingerprint,
                if matched {
                    theme::text::success
                } else {
                    theme::text::default
                },
            ))
            .push(
                iced::widget::text(slot.account.origin.to_string())
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
                title = title.push(fingerprint_text(fingerprint, theme::text::success));
            }
        }
        if status.verified {
            title = title.push(p2_bold(localized_copy("Verified")).style(theme::text::success));
        }
        let mut row = Row::new()
            .spacing(20)
            .align_y(Alignment::Center)
            .padding([12, 0])
            .push(identity)
            .push(
                Column::new()
                    .spacing(2)
                    .width(Length::Fill)
                    .push(title)
                    .push(p2_regular(status.detail).style(status.detail_style)),
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
        if !adding {
            row = row.push(if busy {
                Element::from(p2_regular(slot.source.to_string()).style(theme::text::secondary))
            } else {
                iced::widget::pick_list(sources, Some(slot.source), move |source| {
                    message(ImportMessage::KeySourcePicked(index, source))
                })
                .style(theme::pick_list::primary)
                .padding([6, 10])
                .text_size(13)
                .into()
            });
        }
        let mut key = Column::new().push(row);
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
                                button::secondary(None, localized_copy("Verify key"))
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
                    p2_regular(localized_copy(
                        "No hardware wallet connected. Plug one in and unlock it; it appears beside the key it holds.",
                    ))
                    .style(theme::text::secondary),
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
                    " · Key {only} signs elsewhere",
                    " · la clave {only} firma en otro lugar"
                )),
                many => line.push_str(&coincube_ui::localized_format!(
                    " · {} keys sign elsewhere",
                    " · {} claves firman en otro lugar",
                    many.len()
                )),
            }
        }
        p2_regular(line).style(if keys.ready() {
            theme::text::success
        } else {
            theme::text::secondary
        })
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
