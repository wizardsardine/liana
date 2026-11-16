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

/// A connected hardware wallet, as the Keys step lists it so the user can
/// tell which device holds which key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ListedDevice {
    pub(super) name: String,
    /// Unknown while the device is locked or cannot be read.
    pub(super) fingerprint: Option<Fingerprint>,
    /// Why the device cannot be checked yet, in English.
    pub(super) problem: Option<&'static str>,
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

/// The connected hardware wallets with their fingerprints, each matched to
/// the key it holds.
fn devices_list<'a>(keys: &KeysState, devices: &[ListedDevice]) -> Element<'a, view::Message> {
    let mut list = Column::new()
        .spacing(6)
        .push(p1_bold(localized_copy("Connected hardware wallets")));
    if devices.is_empty() {
        list = list.push(
            p2_regular(localized_copy(
                "None yet. Connect and unlock a device; it appears here with its fingerprint.",
            ))
            .style(theme::text::secondary),
        );
    }
    for device in devices {
        let name = &device.name;
        let line = match (device.problem, device.fingerprint) {
            (Some(problem), _) => p2_regular(format!("{name} · {}", localized_copy(problem)))
                .style(theme::text::warning),
            (None, Some(fingerprint)) => match keys
                .slots
                .iter()
                .position(|slot| slot.account.fingerprint == fingerprint)
            {
                Some(index) => p2_regular(coincube_ui::localized_format!(
                    "{name} · fingerprint {fingerprint} · Key {} of {}",
                    "{name} · huella {fingerprint} · clave {} de {}",
                    index + 1,
                    keys.slots.len()
                ))
                .style(theme::text::success),
                None => p2_regular(coincube_ui::localized_format!(
                    "{name} · fingerprint {fingerprint} · not a signer of this wallet",
                    "{name} · huella {fingerprint} · no firma en esta cartera"
                ))
                .style(theme::text::secondary),
            },
            (None, None) => p2_regular(name.clone()).style(theme::text::secondary),
        };
        list = list.push(line);
    }
    coincube_ui::widget::Container::new(list)
        .padding(16)
        .width(Length::Fill)
        .style(theme::card::simple)
        .into()
}

/// The Keys step's form and action row.
pub(super) fn step<'a>(
    keys: &'a KeysState,
    adding: bool,
    hardware_connected: impl Fn(&ImportedAccount) -> bool,
    devices: &[ListedDevice],
    error: Option<&'a String>,
) -> (Element<'a, view::Message>, Element<'a, view::Message>) {
    let count = keys.slots.len();
    let lead = if adding {
        localized_copy("Enter the recovery words for any of this wallet's signers. Each set is checked against its signer's fingerprint, then stored on this device for this Cube.")
            .to_string()
    } else if keys.multisig() {
        coincube_ui::localized_format!(
            "The descriptor names {count} signers and needs {} to spend. Provide at least {} \
             of them; the others sign elsewhere. Each key is checked against its \
             fingerprint before it is accepted.",
            "El descriptor indica {count} firmantes y se necesitan {} para gastar. Aporta al \
             menos {} de sus claves; las demás firman en otro lugar. Cada clave se \
             verifica con su huella antes de aceptarla.",
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
    let awaiting_device = !adding
        && keys
            .slots
            .iter()
            .any(|slot| slot.source == KeySource::Hardware && !slot.provided());
    if awaiting_device {
        content = content.push(devices_list(keys, devices));
    }
    let busy = keys.checking();
    for (index, slot) in keys.slots.iter().enumerate() {
        let mut card = Column::new()
            .spacing(12)
            .push(p1_bold(coincube_ui::localized_format!(
                "Key {} of {count} · fingerprint {} · path {}",
                "Clave {} de {count} · huella {} · ruta {}",
                index + 1,
                slot.account.fingerprint,
                slot.account.origin
            )));
        card = card.push(match &slot.status {
            KeyStatus::Verified if slot.source == KeySource::Elsewhere => {
                p2_regular(localized_copy("Signs elsewhere")).style(theme::text::secondary)
            }
            KeyStatus::Verified => p2_regular(coincube_ui::localized_format!(
                "Verified · matches {} · {}",
                "Verificada · coincide con {} · {}",
                slot.account.fingerprint,
                localized_copy(slot.source.label()).to_lowercase()
            ))
            .style(theme::text::success),
            KeyStatus::Checking => {
                p2_regular(localized_copy("Checking…")).style(theme::text::secondary)
            }
            KeyStatus::Failed(reason) => {
                p2_regular(super::shown_notice(reason)).style(theme::text::error)
            }
            KeyStatus::Missing => {
                p2_regular(localized_copy("Not provided yet")).style(theme::text::secondary)
            }
        });
        let mut sources = Row::new().spacing(8);
        // Adding to an attached wallet only stores words; a device needs
        // nothing stored.
        let choices = if adding {
            vec![KeySource::Words]
        } else {
            let mut choices = vec![KeySource::Hardware, KeySource::Words];
            if keys.multisig() {
                choices.push(KeySource::Elsewhere);
            }
            choices
        };
        for source in choices {
            let pick = if slot.source == source {
                button::primary(None, localized_copy(source.label()))
            } else {
                button::secondary(None, localized_copy(source.label()))
            };
            sources = sources.push(pick.width(Length::Shrink).on_press_maybe(
                (!busy).then_some(message(ImportMessage::KeySourcePicked(index, source))),
            ));
        }
        card = card
            .push(p2_regular(localized_copy("Where is this key?")).style(theme::text::secondary))
            .push(sources);
        card = match slot.source {
            // Verified words leave the screen; changing them starts over.
            KeySource::Words if slot.status == KeyStatus::Verified => card.push(
                button::secondary(None, localized_copy("Enter different words"))
                    .width(Length::Shrink)
                    .on_press_maybe((!busy).then_some(message(ImportMessage::KeySourcePicked(
                        index,
                        KeySource::Words,
                    )))),
            ),
            KeySource::Words => card
                .push(p1_bold(localized_copy("Recovery words")))
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
                    caption(localized_copy("Words are checked here, then stored encrypted on this device for this Cube. They are not in the Recovery Kit yet, so keep your original backup."))
                    .style(theme::text::secondary),
                )
                .push(p1_bold(localized_copy("Passphrase (if the wallet used one)")))
                .push(
                    text_input(localized_copy("Optional"), slot.passphrase.as_str())
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
                .push(
                    button::secondary(None, localized_copy("Verify key"))
                        .width(Length::Shrink)
                        .on_press_maybe(
                            (!busy
                                && slot.status != KeyStatus::Verified
                                && !slot.words.trim().is_empty())
                            .then_some(message(ImportMessage::VerifyWords(index))),
                        ),
                ),
            KeySource::Hardware if slot.status == KeyStatus::Verified => card.push(
                caption(localized_copy(
                    "The seed stays on the device. It signs whenever this wallet spends.",
                ))
                .style(theme::text::secondary),
            ),
            // The check is offered only once the matching device shows up,
            // and then says so plainly.
            KeySource::Hardware if hardware_connected(&slot.account) => card
                .push(
                    caption(coincube_ui::localized_format!(
                        "The device with fingerprint {} is connected. Check it to confirm this key.",
                        "El dispositivo con la huella {} está conectado. Compruébalo para confirmar esta clave.",
                        slot.account.fingerprint
                    ))
                    .style(theme::text::success),
                )
                .push(
                    button::primary(None, localized_copy("Check device"))
                        .width(Length::Shrink)
                        .on_press_maybe(
                            (!busy).then_some(message(ImportMessage::CheckDevice(index))),
                        ),
                ),
            KeySource::Hardware => card.push(
                caption(coincube_ui::localized_format!(
                    "Connect and unlock the device with fingerprint {}. You can check it here once it is connected.",
                    "Conecta y desbloquea el dispositivo con la huella {}. Podrás comprobarlo aquí cuando esté conectado.",
                    slot.account.fingerprint
                ))
                .style(theme::text::secondary),
            ),
            KeySource::Elsewhere => card.push(
                caption(localized_copy(
                    "Another wallet or person signs with this key. COINCUBE Desktop saves the PSBT for them.",
                ))
                .style(theme::text::secondary),
            ),
        };
        content = content.push(
            coincube_ui::widget::Container::new(card)
                .padding(16)
                .width(Length::Fill)
                .style(theme::card::simple),
        );
    }
    if !adding && keys.multisig() && keys.provided() < keys.threshold {
        content = content.push(
            p2_regular(coincube_ui::localized_format!(
                "{} of {} keys needed to sign are provided.",
                "Se aportaron {} de las {} claves necesarias para firmar.",
                keys.provided(),
                keys.threshold
            ))
            .style(theme::text::secondary),
        );
    }
    if let Some(error) = error {
        content = content.push(p1_regular(super::shown_notice(error)).style(theme::text::error));
    }
    let actions = Row::new()
        .spacing(10)
        .align_y(Alignment::Center)
        .push((!adding).then(|| {
            button::secondary(None, localized_copy("Back"))
                .width(Length::Shrink)
                .on_press_maybe((!busy).then_some(message(ImportMessage::Edit)))
        }))
        .push(iced::widget::Space::new().width(Length::Fill))
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
