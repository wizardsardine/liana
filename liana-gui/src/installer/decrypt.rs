use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fmt::Debug,
    str::FromStr,
    sync::Arc,
};

use async_hwi::{bitbox::api::btc::Fingerprint, DeviceKind, Version, HWI};
use bip138::{
    backend::{Active as Miniscript, Backend},
    Decrypted, EncryptedBackup, EncryptedMetadata,
};
use iced::{
    alignment::{self, Horizontal},
    clipboard,
    widget::{column, row, tooltip, Column, Space},
    Length, Task,
};
use liana::{
    bip39::Mnemonic,
    descriptors::LianaDescriptor,
    miniscript::{
        bitcoin::{
            bip32::{self, ChildNumber, DerivationPath},
            key::Secp256k1,
            secp256k1, Network,
        },
        DescriptorPublicKey,
    },
};
use liana_ui::{
    component::{
        button, card,
        form::Value,
        modal::{self, widget_style, ModalWidth, BTN_W, LIANA_ACCOUNTS},
        scrollable,
        text::{self, capitalize_first, p1_regular},
    },
    icon,
    spacing::{HSpacing, VSpacing},
    theme,
    widget::{modal::Modal, Button, Container, Element, Row, SpaceExt},
};

use crate::{
    app::state::export::ExportModal,
    backup::Backup,
    export::ImportExportType,
    hw::{HardwareWallet, HardwareWallets},
    installer,
    utils::{coin_type, default_derivation_path, derivation_path},
};

#[allow(unused, clippy::enum_variant_names)]
#[derive(Debug, Clone, Copy)]
pub enum Error {
    InvalidEncoding,
    InvalidType,
    InvalidDescriptor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    None,
    ImportXpub,
    Xpub,
    Mnemonic,
}

pub struct DecryptModal {
    network: Network,
    error: Option<Error>,
    bytes: Vec<u8>,
    /// Paths stored in the backup, or the default one when it stores none.
    derivation_paths: Vec<DerivationPath>,
    cant_fetch: BTreeMap<
        String, /* id */
        (
            String,         /* name */
            Option<String>, /* pairing code */
        ),
    >,
    fetching: BTreeMap<Fingerprint, String /* name */>,
    fetched: BTreeMap<Fingerprint, String /* name */>,
    devices: BTreeMap<Fingerprint, Arc<dyn HWI + Send + Sync>>,
    /// Devices that failed with the Liana paths, each waiting for the user to accept or decline
    /// the other paths; the first one is asked.
    other_paths_offers: VecDeque<Fingerprint>,
    other_paths_asked: BTreeSet<Fingerprint>,
    show_options: bool,
    import_xpub_error: Option<String>,
    xpub: Value<String>,
    xpub_busy: bool,
    mnemonic: Value<String>,
    mnemonic_ack: bool,
    mnemonic_busy: bool,
    focus: Focus,
    pub modal: Option<ExportModal>,
}
impl Debug for DecryptModal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecryptModal")
            .field("error", &self.error)
            .field("derivation_paths", &self.derivation_paths.len())
            .field("cant_fetch", &self.cant_fetch.len())
            .field("fetching", &self.fetching.len())
            .field("fetched", &self.fetched.len())
            .finish()
    }
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum Decrypt {
    Fetched(Fingerprint, String /* name */),
    Backup(Backup),
    Xpub(String),
    PasteXpub,
    SelectXpub,
    XpubError(DecryptWarning),
    Mnemonic(String),
    PasteMnemonic,
    SelectMnemonic,
    MnemonicStatus(Option<DecryptWarning> /* error */, Option<Fingerprint>),
    MnemonicAck(bool),
    SelectImportXpub,
    UnexpectedPayload(Decrypted),
    InvalidDescriptor,
    ContentNotSupported,
    ShowOptions(bool),
    TryOtherPaths(Fingerprint),
    SkipOtherPaths(Fingerprint),
    Close,
    CloseModal,
    None,
}

#[derive(Debug, Clone, Copy)]
pub enum DecryptWarning {
    InvalidMnemonic,
    InvalidXpub,
    MnemonicCannotDecrypt,
    XpubCannotDecrypt,
}

impl DecryptWarning {
    fn message(self) -> String {
        match self {
            Self::InvalidMnemonic => crate::t!("decrypt-invalid-mnemonic"),
            Self::InvalidXpub => crate::t!("decrypt-invalid-xpub"),
            Self::MnemonicCannotDecrypt => crate::t!("decrypt-mnemonic-cannot-decrypt"),
            Self::XpubCannotDecrypt => crate::t!("decrypt-xpub-cannot-decrypt"),
        }
    }
}

impl From<Decrypt> for installer::Message {
    fn from(value: Decrypt) -> Self {
        installer::Message::Decrypt(value)
    }
}

/// A backup can hold several items: a liana backup found among them wins over the others.
pub fn decrypt_descriptor_with_pk(bytes: &[u8], pk: secp256k1::PublicKey) -> Option<Decrypt> {
    let items = EncryptedBackup::new()
        .set_encrypted_payload(bytes)
        .expect("sanitized")
        .set_keys(vec![pk])
        .decrypt()
        .ok()?;
    let (backups, unexpected): (Vec<_>, Vec<_>) = items
        .into_iter()
        .filter_map(decrypted_to_decrypt)
        .partition(|d| matches!(d, Decrypt::Backup(_)));
    backups.into_iter().chain(unexpected).next()
}

fn decrypted_to_decrypt(decrypted: Decrypted) -> Option<Decrypt> {
    match decrypted {
        Decrypted::Descriptor(d) => {
            let descriptor = d.to_string();
            Some(descriptor_to_decrypt(&descriptor, Decrypted::Descriptor(d)))
        }
        Decrypted::DescriptorBackup(b) if b.descriptor_sets.len() == 1 => {
            let descriptor = b.descriptor_sets[0].descriptor.to_string();
            Some(descriptor_to_decrypt(
                &descriptor,
                Decrypted::DescriptorBackup(b),
            ))
        }
        Decrypted::PolicyBackup(b) if b.policy_sets.len() == 1 => {
            let descriptor = b.policy_sets[0].to_descriptor().ok()?.to_string();
            Some(descriptor_to_decrypt(
                &descriptor,
                Decrypted::PolicyBackup(b),
            ))
        }
        Decrypted::WalletBackup(backup_bytes) => {
            let backup_str = String::from_utf8(backup_bytes.clone()).ok()?;
            let backup: Backup = serde_json::from_str(&backup_str).ok()?;
            if backup.accounts.len() != 1 {
                return None;
            }
            let descriptor_str = &backup.accounts.first().expect("checked").descriptor;
            let _ = match LianaDescriptor::from_str(descriptor_str) {
                Ok(descr) => descr,
                Err(_) => {
                    return Some(Decrypt::UnexpectedPayload(Decrypted::WalletBackup(
                        backup_bytes,
                    )))
                }
            };
            Some(Decrypt::Backup(backup))
        }
        decrypted => Some(Decrypt::UnexpectedPayload(decrypted)),
    }
}

fn descriptor_to_decrypt(descriptor: &str, decrypted: Decrypted) -> Decrypt {
    let descr = match LianaDescriptor::from_str(descriptor) {
        Ok(descr) => descr,
        Err(_) => return Decrypt::UnexpectedPayload(decrypted),
    };
    let network = if descr.all_xpubs_net_is(Network::Bitcoin) {
        Network::Bitcoin
    } else {
        Network::Signet
    };
    Decrypt::Backup(Backup::from_descriptor(descr, network))
}

/// As in the bip138 binary: a Ledger refuses a path it does not consider standard unless it shows
/// the xpub, so it displays it for a path outside the common ones and for purpose 87.
fn display_xpub(kind: DeviceKind, network: Network, path: &DerivationPath) -> bool {
    let common = bip138::ll::common_derivation_paths(coin_type(network))
        .iter()
        .map(Miniscript::from_ll_path)
        .any(|common| &common == path);
    let purpose_87 = path.into_iter().next() == Some(&ChildNumber::Hardened { index: 87 });
    matches!(kind, DeviceKind::Ledger | DeviceKind::LedgerSimulator) && (!common || purpose_87)
}

fn decrypt_with_seed(
    bytes: &[u8],
    seed: &[u8; 64],
    network: Network,
    deriv_paths: Vec<DerivationPath>,
) -> Decrypt {
    let xpriv = bip32::Xpriv::new_master(network, seed).expect("seed is 64 bytes");
    let secp = Secp256k1::new();
    let fingerprint = xpriv.fingerprint(&secp);

    for path in deriv_paths {
        let pk = xpriv
            .derive_priv(&secp, &path)
            .expect("cannot fail")
            .private_key
            .public_key(&secp);
        if let Some(backup @ Decrypt::Backup(_)) = decrypt_descriptor_with_pk(bytes, pk) {
            return backup;
        }
    }
    Decrypt::MnemonicStatus(
        Some(DecryptWarning::MnemonicCannotDecrypt),
        Some(fingerprint),
    )
}

impl DecryptModal {
    pub fn new(bytes: Vec<u8>, network: Network) -> Self {
        let mut error = None;
        let mut derivation_paths = match EncryptedMetadata::from_encrypted_payload(&bytes) {
            Ok(metadata) => metadata.derivation_paths,
            Err(_) => {
                error = Some(Error::InvalidEncoding);
                Vec::new()
            }
        };
        if derivation_paths.is_empty() {
            derivation_paths.push(default_derivation_path(network));
        }
        Self {
            network,
            error,
            bytes,
            derivation_paths,
            cant_fetch: BTreeMap::new(),
            fetching: BTreeMap::new(),
            fetched: BTreeMap::new(),
            devices: BTreeMap::new(),
            other_paths_offers: VecDeque::new(),
            other_paths_asked: BTreeSet::new(),
            show_options: false,
            import_xpub_error: None,
            xpub: Value::default(),
            xpub_busy: false,
            mnemonic: Value::default(),
            mnemonic_busy: false,
            mnemonic_ack: false,
            focus: Focus::None,
            modal: None,
        }
    }
    fn liana_common_paths(&self) -> Vec<DerivationPath> {
        let mut paths = self.derivation_paths.clone();
        for account in 0..LIANA_ACCOUNTS {
            let path = derivation_path(self.network, ChildNumber::Hardened { index: account });
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
        paths
    }
    fn other_paths(&self) -> Vec<DerivationPath> {
        let liana_common_paths = self.liana_common_paths();
        bip138::ll::common_derivation_paths(coin_type(self.network))
            .iter()
            .map(Miniscript::from_ll_path)
            .filter(|path| !liana_common_paths.contains(path))
            .collect()
    }
    /// Every path: deriving keys locally is almost instant, so a mnemonic is never asked.
    fn mnemonic_paths(&self) -> Vec<DerivationPath> {
        let mut paths = self.liana_common_paths();
        paths.extend(self.other_paths());
        paths
    }
    fn offer_other_paths(&mut self, fingerprint: Fingerprint) {
        if self.devices.contains_key(&fingerprint)
            && !self.other_paths_asked.contains(&fingerprint)
            && !self.other_paths_offers.contains(&fingerprint)
        {
            self.other_paths_offers.push_back(fingerprint);
        }
    }
    fn try_other_paths(&mut self, fingerprint: Fingerprint) -> Task<installer::Message> {
        if self.other_paths_offers.front() != Some(&fingerprint) {
            return Task::none();
        }
        self.other_paths_offers.pop_front();
        self.other_paths_asked.insert(fingerprint);
        let (Some(device), Some(name)) = (
            self.devices.get(&fingerprint).cloned(),
            self.fetched.remove(&fingerprint),
        ) else {
            return Task::none();
        };
        self.fetching.insert(fingerprint, name.clone());
        self.fetch(device, fingerprint, name, self.other_paths())
    }
    fn skip_other_paths(&mut self, fingerprint: Fingerprint) {
        if self.other_paths_offers.front() == Some(&fingerprint) {
            self.other_paths_offers.pop_front();
            self.other_paths_asked.insert(fingerprint);
        }
    }
    pub fn update(&mut self, msg: Decrypt) -> Task<installer::Message> {
        match msg {
            Decrypt::Fetched(fg, name) => {
                self.fetching.remove(&fg);
                self.fetched.insert(fg, name);
                self.offer_other_paths(fg);
                Task::none()
            }
            Decrypt::TryOtherPaths(fg) => self.try_other_paths(fg),
            Decrypt::SkipOtherPaths(fg) => {
                self.skip_other_paths(fg);
                Task::none()
            }
            Decrypt::Backup(_) => {
                tracing::error!(
                    "DecryptModal::update(Backup), this message must have been caught early"
                );
                Task::none()
            }
            Decrypt::XpubError(s) => {
                match self.focus {
                    Focus::ImportXpub => {
                        self.import_xpub_error = Some(s.message());
                    }
                    Focus::Xpub => self.update_xpub_error(s),
                    Focus::Mnemonic | Focus::None => {}
                }
                Task::none()
            }

            Decrypt::MnemonicStatus(s, fg) => {
                self.update_mnemonic_state(s, fg);
                Task::none()
            }
            Decrypt::UnexpectedPayload(p) => match p {
                Decrypted::Descriptor(_) | Decrypted::DescriptorBackup(_) => {
                    tracing::error!("Descriptor decrypted but not a valid liana descriptor");
                    Task::done(Decrypt::InvalidDescriptor.into())
                }
                _ => {
                    tracing::error!("Content decrypted but type not supported");
                    Task::done(Decrypt::ContentNotSupported.into())
                }
            },
            Decrypt::ShowOptions(show) => {
                self.show_options = show;
                Task::none()
            }
            Decrypt::Xpub(value) => self.update_xpub(value),
            Decrypt::SelectXpub => {
                self.focus = Focus::Xpub;
                self.import_xpub_error = None;
                Task::none()
            }
            Decrypt::PasteXpub => clipboard::read().map(|m| {
                if let Some(xpub) = m {
                    Decrypt::Xpub(xpub)
                } else {
                    Decrypt::None
                }
                .into()
            }),
            Decrypt::Mnemonic(value) => self.update_mnemonic(value),
            Decrypt::SelectMnemonic => {
                self.focus = Focus::Mnemonic;
                self.import_xpub_error = None;
                Task::none()
            }
            Decrypt::PasteMnemonic => clipboard::read().map(|m| {
                if let Some(mnemo) = m {
                    Decrypt::Mnemonic(mnemo)
                } else {
                    Decrypt::None
                }
                .into()
            }),
            Decrypt::SelectImportXpub => {
                self.focus = Focus::ImportXpub;
                self.import_xpub_error = None;
                let modal = ExportModal::new(None, ImportExportType::ImportXpub(self.network));
                let launch = modal.launch(false);
                self.modal = Some(modal);
                launch
            }
            Decrypt::CloseModal => {
                self.modal = None;
                Task::none()
            }
            Decrypt::MnemonicAck(ack) => {
                self.mnemonic_ack = ack;
                Task::none()
            }
            Decrypt::None
            | Decrypt::InvalidDescriptor
            | Decrypt::ContentNotSupported
            | Decrypt::Close => Task::none(),
        }
    }
    pub fn view<'a>(
        &'a self,
        content: Element<'a, installer::Message>,
    ) -> Element<'a, installer::Message> {
        if let Some(mo) = &self.modal {
            mo.view(content)
        } else {
            // the offer sits over the decrypt card itself, so both modals show
            let offer = self
                .other_paths_offers
                .front()
                .and_then(|fg| self.fetched.get(fg).map(|name| (name, *fg)));
            let view = decrypt_view(self);
            let decrypt: Element<'a, installer::Message> = match offer {
                Some((name, fingerprint)) => {
                    // a BitBox02 confirms every unusual path, a Ledger the ones it displays
                    let confirm_on_device = self.devices.get(&fingerprint).is_some_and(|d| {
                        matches!(
                            d.device_kind(),
                            DeviceKind::BitBox02 | DeviceKind::Ledger | DeviceKind::LedgerSimulator
                        )
                    });
                    let offer = other_paths_modal(name, fingerprint, confirm_on_device);
                    Modal::new(view, offer)
                        .on_blur(Some(Decrypt::SkipOtherPaths(fingerprint).into()))
                        .into()
                }
                None => view.into(),
            };
            Modal::new(content, decrypt)
                .on_blur(Some(Decrypt::Close.into()))
                .into()
        }
    }
    #[allow(clippy::collapsible_match)]
    fn fetch(
        &self,
        device: Arc<dyn HWI + Send + Sync>,
        fingerprint: Fingerprint,
        name: String,
        derivation_paths: Vec<DerivationPath>,
    ) -> Task<installer::Message> {
        let bytes = self.bytes.clone();
        let network = self.network;
        Task::perform(
            async move {
                for path in derivation_paths {
                    device.display(display_xpub(device.device_kind(), network, &path));
                    if let Ok(xpub) = device.get_extended_pubkey(&path).await {
                        let pk = xpub.public_key;
                        if let Some(d) = decrypt_descriptor_with_pk(&bytes, pk) {
                            if let d @ Decrypt::Backup(_) | d @ Decrypt::UnexpectedPayload(_) = d {
                                return d;
                            }
                        }
                    } else {
                        // FIXME: should we retry here?
                        tracing::error!(
                            "Fail to fetch xpub for {} {}",
                            device.device_kind(),
                            fingerprint
                        );
                    }
                }
                Decrypt::Fetched(fingerprint, name)
            },
            |m| m.into(),
        )
    }
    pub fn update_devices(
        &mut self,
        devices: &mut HardwareWallets,
    ) -> Option<Task<installer::Message>> {
        fn name(kind: DeviceKind, version: Option<Version>) -> String {
            capitalize_first(&if let Some(v) = version {
                format!("{kind} {v}")
            } else {
                kind.to_string()
            })
        }

        let mut new_cant_fetch = BTreeMap::new();
        let mut to_fetch = vec![];
        for d in &devices.list {
            match d {
                HardwareWallet::Unsupported {
                    id, kind, version, ..
                } => {
                    new_cant_fetch.insert(id.clone(), (name(*kind, version.clone()), None));
                }
                HardwareWallet::Locked {
                    id,
                    kind,
                    pairing_code,
                    ..
                } => {
                    new_cant_fetch.insert(id.clone(), (name(*kind, None), pairing_code.clone()));
                }
                d => {
                    if let HardwareWallet::Supported {
                        fingerprint,
                        device,
                        ..
                    } = d
                    {
                        self.devices.insert(*fingerprint, device.clone());
                        if !self.fetched.contains_key(fingerprint)
                            && !self.fetching.contains_key(fingerprint)
                        {
                            to_fetch.push(d);
                        }
                    }
                }
            };
        }
        self.cant_fetch = new_cant_fetch;
        let plugged: BTreeSet<Fingerprint> = devices
            .list
            .iter()
            .filter_map(|d| match d {
                HardwareWallet::Supported { fingerprint, .. } => Some(*fingerprint),
                _ => None,
            })
            .collect();
        self.devices.retain(|fg, _| plugged.contains(fg));
        self.other_paths_offers.retain(|fg| plugged.contains(fg));

        let mut batch = vec![];
        for i in to_fetch {
            if let HardwareWallet::Supported {
                device,
                kind,
                fingerprint,
                version,
                ..
            } = i
            {
                let name = name(*kind, version.clone());
                self.fetching.insert(*fingerprint, name.clone());
                batch.push(self.fetch(
                    device.clone(),
                    *fingerprint,
                    name,
                    self.liana_common_paths(),
                ));
            }
        }
        (!batch.is_empty()).then_some(Task::batch(batch))
    }
    fn update_xpub(&mut self, xpub: String) -> Task<installer::Message> {
        if self.xpub_busy {
            return Task::none();
        }
        self.xpub.value = xpub.clone();
        if xpub.is_empty() {
            self.xpub.valid = true;
            self.xpub.warning = None;
            return Task::none();
        }
        if let Ok(dpk) = DescriptorPublicKey::from_str(&xpub) {
            self.xpub_busy = true;
            self.xpub.warning = None;
            self.xpub.valid = true;
            let bytes = self.bytes.clone();
            Task::perform(
                async move {
                    match bip138::descriptor::dpk_to_root_pk(&dpk) {
                        Ok(pk) => decrypt_descriptor_with_pk(&bytes, pk)
                            .unwrap_or(Decrypt::XpubError(DecryptWarning::XpubCannotDecrypt)),
                        Err(_) => Decrypt::XpubError(DecryptWarning::InvalidXpub),
                    }
                },
                |m| m.into(),
            )
        } else {
            self.xpub.warning = Some(DecryptWarning::InvalidXpub.message());
            self.xpub.valid = false;
            Task::none()
        }
    }
    fn update_xpub_error(&mut self, error: DecryptWarning) {
        self.xpub.warning = Some(error.message());
        self.xpub.valid = false;
        self.xpub_busy = false;
    }
    fn update_mnemonic(&mut self, mnemonic: String) -> Task<installer::Message> {
        if self.mnemonic_busy {
            return Task::none();
        }
        self.mnemonic.value = mnemonic.clone();
        if mnemonic.is_empty() {
            self.mnemonic.valid = true;
            self.mnemonic.warning = None;
            return Task::none();
        }
        let seed = match Mnemonic::from_str(&mnemonic) {
            Ok(m) => m,
            Err(_) => {
                self.mnemonic.valid = false;
                self.mnemonic.warning = Some(DecryptWarning::InvalidMnemonic.message());
                return Task::none();
            }
        }
        .to_seed("");
        self.mnemonic.valid = true;
        self.mnemonic.warning = None;
        let deriv_paths = self.mnemonic_paths();
        let bytes = self.bytes.clone();
        let network = self.network;
        self.mnemonic_busy = true;
        Task::perform(
            async move { decrypt_with_seed(&bytes, &seed, network, deriv_paths) },
            |m| m.into(),
        )
    }
    fn update_mnemonic_state(&mut self, error: Option<DecryptWarning>, fg: Option<Fingerprint>) {
        self.mnemonic_busy = false;
        self.mnemonic.warning = error.map(DecryptWarning::message);
        self.mnemonic.valid = false;
        if let Some(fg) = fg {
            self.fetched
                .insert(fg, crate::t!("decrypt-mnemonic-source"));
        }
    }
}

fn invalid_content(hint: String) -> Container<'static, installer::Message> {
    Container::new(
        Column::new()
            .spacing(5)
            .push(Space::with_height(Length::Fill))
            .push(
                row![
                    Space::with_width(Length::Fill),
                    icon::warning_icon().size(250),
                    Space::with_width(Length::Fill),
                ]
                .align_y(alignment::Vertical::Center),
            )
            .push(text::text(hint))
            .push(Space::with_height(Length::Fill)),
    )
}

fn widget_signing_device(
    name: String,
    fingerprint: Option<Fingerprint>,
    message: String,
    pairing_code: Option<String>,
) -> Button<'static, installer::Message> {
    let message = if let Some(code) = pairing_code {
        column![
            p1_regular(message),
            p1_regular(crate::t!("decrypt-pairing-code", code = code)),
        ]
    } else {
        column![p1_regular(message)]
    }
    .align_x(Horizontal::Center);
    let fg = if let Some(fg) = fingerprint {
        format!("#{fg}")
    } else {
        "   -   ".to_string()
    };
    let designation =
        column![text::p1_bold(name), text::p1_regular(fg)].align_x(alignment::Horizontal::Center);
    let row = row![
        Space::with_width(5),
        designation,
        message,
        Space::with_width(Length::Fill)
    ]
    .align_y(alignment::Vertical::Center)
    .spacing(10);
    Button::new(row).style(widget_style).width(BTN_W)
}

fn cant_fetch_device(
    name: String,
    pairing_code: Option<String>,
) -> Button<'static, installer::Message> {
    let message = crate::t!("decrypt-unlock-device");
    widget_signing_device(name, None, message, pairing_code)
}

fn fetching_device(name: String, fingerprint: Fingerprint) -> Button<'static, installer::Message> {
    let message = crate::t!("decrypt-try-device");
    widget_signing_device(name, Some(fingerprint), message, None)
}

fn fetched_device(name: String, fingerprint: Fingerprint) -> Button<'static, installer::Message> {
    let message = crate::t!("decrypt-device-failed");
    widget_signing_device(name, Some(fingerprint), message, None)
}

fn valid_content(state: &DecryptModal) -> Container<'static, installer::Message> {
    let description = text::text(crate::t!("decrypt-device-description"));
    let mut devices = state
        .fetching
        .iter()
        .map(|(fg, name)| fetching_device(name.clone(), *fg))
        .collect::<Vec<_>>();
    for d in &state.cant_fetch {
        let (msg, pairing_code) = d.1.clone();
        devices.push(cant_fetch_device(msg, pairing_code));
    }
    for (fg, name) in &state.fetched {
        devices.push(fetched_device(name.clone(), *fg));
    }
    let options_btn = modal::optional_section(
        state.show_options,
        crate::t!("common-other-options"),
        || Decrypt::ShowOptions(true).into(),
        || Decrypt::ShowOptions(false).into(),
    );

    let mut col = Column::new().spacing(5).push(description);
    if devices.is_empty() {
        col = col.push(modal::modal_no_devices_placeholder());
    }
    for d in devices {
        col = col.push(d);
    }
    col = col.push(Space::with_height(10)).push(options_btn);
    if state.show_options {
        col = col.push(optional_content(state));
    }

    Container::new(col).width(BTN_W)
}

fn optional_content(state: &DecryptModal) -> Container<'static, installer::Message> {
    let mut tt = Column::new().push(text::text(crate::t!("decrypt-airgap-help")));
    for d in &state.derivation_paths {
        tt = tt.push(text::text(format!("{d}")));
    }
    let tt = Container::new(tt)
        .style(theme::card::simple)
        .padding(10)
        .width(300);

    let airgap_hint = Row::new()
        .push(p1_regular(crate::t!("decrypt-provide-xpub")))
        .push(tooltip::Tooltip::new(
            icon::tooltip_icon(),
            tt,
            tooltip::Position::Bottom,
        ))
        .spacing(5);

    let import = modal::import_xpub_entry(
        state.import_xpub_error.clone(),
        None,
        Some(|| Decrypt::SelectImportXpub.into()),
    );

    let xpub = modal::paste_xpub_entry(
        state.focus == Focus::Xpub,
        state.network,
        &state.xpub,
        None,
        Some(|s| Decrypt::Xpub(s).into()),
        Some(|| Decrypt::PasteXpub.into()),
        || Decrypt::SelectXpub.into(),
    );

    let mnemonic = modal::enter_mnemonic_entry(
        state.focus == Focus::Mnemonic,
        state.mnemonic_ack,
        &state.mnemonic,
        |ack| Decrypt::MnemonicAck(ack).into(),
        |s| Decrypt::Mnemonic(s).into(),
        || Decrypt::PasteMnemonic.into(),
        || Decrypt::SelectMnemonic.into(),
    );

    let col = column![
        airgap_hint,
        Space::with_height(modal::V_SPACING),
        import,
        Space::with_height(modal::V_SPACING),
        xpub,
        Space::with_height(modal::V_SPACING),
        mnemonic,
        Space::with_height(modal::V_SPACING),
    ];

    Container::new(col)
}

fn other_paths_modal<'a>(
    name: &str,
    fingerprint: Fingerprint,
    confirm_on_device: bool,
) -> Element<'a, installer::Message> {
    let device = format!("{name} #{fingerprint}");
    let hint = text::new::caption(crate::t!("decrypt-other-paths-hint", device = device));
    let confirm = confirm_on_device
        .then(|| text::new::caption(crate::t!("decrypt-other-paths-confirm-on-device")));
    let skip = button::btn_skip(Some(Decrypt::SkipOtherPaths(fingerprint).into()));
    let try_other_paths =
        button::btn_try_other_paths(Some(Decrypt::TryOtherPaths(fingerprint).into()));
    let buttons = row![Space::fill_width(), skip, try_other_paths].spacing(HSpacing::M);
    let content = column![hint, confirm, buttons].spacing(VSpacing::S);
    modal::modal_view(
        Some(crate::t!("decrypt-other-paths-title")),
        None,
        Some(Decrypt::SkipOtherPaths(fingerprint).into()),
        ModalWidth::M,
        content,
    )
}

/// Return the modal view for an export task
pub fn decrypt_view<'a>(state: &DecryptModal) -> Container<'a, installer::Message> {
    let header = modal::header(
        Some(crate::t!("decrypt-backup-file")),
        None,
        Some(installer::Message::Decrypt(Decrypt::Close)),
    );

    let content = match state.error {
        Some(e) => match e {
            Error::InvalidEncoding => invalid_content(crate::t!("decrypt-invalid-encoding")),
            Error::InvalidType => invalid_content(crate::t!("decrypt-invalid-type")),
            Error::InvalidDescriptor => invalid_content(crate::t!("decrypt-invalid-descriptor")),
        },
        None => valid_content(state),
    };

    let content = scrollable::vertical(
        Container::new(content)
            .padding(15)
            .align_x(Horizontal::Center),
    );

    let column = Column::new()
        .push(header)
        .push(content)
        .spacing(5)
        .align_x(alignment::Horizontal::Center);

    card::simple(column)
        .width(modal::MODAL_WIDTH as u32)
        .height(Length::Fixed(450.0))
}

#[cfg(test)]
mod tests {
    use std::{env, str::FromStr, sync::Arc};

    use async_hwi::{
        bitbox::api::btc::Fingerprint, AddressScript, DeviceKind, Error as HWIError, Version, HWI,
    };
    use async_trait::async_trait;
    use bip138::{
        descriptor::dpk_to_root_pk, Decrypted, DescriptorBackup, DescriptorSet, EncryptedBackup,
        ToPayload,
    };
    use liana::{
        bip39::Mnemonic,
        descriptors::LianaDescriptor,
        miniscript::{
            bitcoin::{
                bip32::{self, DerivationPath, Xpub},
                key::Secp256k1,
                secp256k1::{self, SecretKey},
                Network, Psbt,
            },
            Descriptor, DescriptorPublicKey,
        },
    };

    use crate::{
        dir::LianaDirectory,
        export::tests::{test_asset, ENCRYPTED_ASSETS_DESCRIPTOR},
        hw::{HardwareWallet, HardwareWallets},
        installer::decrypt::{
            decrypt_descriptor_with_pk, decrypt_with_seed, display_xpub, Decrypt, DecryptModal,
            DecryptWarning, Error,
        },
        utils::default_derivation_path,
    };

    fn descriptor() -> LianaDescriptor {
        LianaDescriptor::from_str(ENCRYPTED_ASSETS_DESCRIPTOR).unwrap()
    }

    fn spendable_pks() -> Vec<secp256k1::PublicKey> {
        descriptor()
            .spendable_keys()
            .iter()
            .map(|k| dpk_to_root_pk(k).unwrap())
            .collect()
    }

    fn unrelated_pk() -> secp256k1::PublicKey {
        SecretKey::from_slice(&[7; 32])
            .unwrap()
            .public_key(&Secp256k1::new())
    }

    fn encrypt(payloads: &[&dyn ToPayload]) -> Vec<u8> {
        EncryptedBackup::new()
            .set_payloads(payloads)
            .unwrap()
            .encrypt()
            .unwrap()
            .bytes
    }

    fn assert_backup_of_descriptor(decrypt: Option<Decrypt>) {
        match decrypt {
            Some(Decrypt::Backup(backup)) => {
                assert_eq!(backup.accounts.len(), 1);
                assert_eq!(backup.accounts[0].descriptor, ENCRYPTED_ASSETS_DESCRIPTOR);
            }
            other => panic!("not a backup: {:?}", other),
        }
    }

    #[test]
    fn decrypt_assets() {
        // `policy-descriptor.bip138` holds another descriptor after the policy
        for name in [
            "v0.bed",
            "v1.bip138",
            "string-descriptor.bip138",
            "policy.bip138",
            "policy-descriptor.bip138",
        ] {
            let bytes = test_asset(name);
            for pk in spendable_pks() {
                assert_backup_of_descriptor(decrypt_descriptor_with_pk(&bytes, pk));
            }
            assert!(decrypt_descriptor_with_pk(&bytes, unrelated_pk()).is_none());
        }
    }

    #[test]
    fn decrypt_descriptor_after_a_note() {
        let note = "backup 2026".to_string();
        let bytes = encrypt(&[&note, descriptor().descriptor()]);
        for pk in spendable_pks() {
            assert_backup_of_descriptor(decrypt_descriptor_with_pk(&bytes, pk));
        }
    }

    #[test]
    fn decrypt_descriptor_backup_document() {
        let set = |descriptor| DescriptorSet {
            descriptor,
            change_descriptor: None,
            archived: false,
            range: None,
            birth_time: None,
        };
        let one_set = DescriptorBackup {
            version: 1,
            descriptor_sets: vec![set(descriptor().descriptor().clone())],
        };
        let bytes = encrypt(&[&one_set]);
        for pk in spendable_pks() {
            assert_backup_of_descriptor(decrypt_descriptor_with_pk(&bytes, pk));
        }

        // with several sets there is no single wallet to import
        let two_sets = DescriptorBackup {
            version: 1,
            descriptor_sets: vec![
                set(descriptor().descriptor().clone()),
                set(descriptor().descriptor().clone()),
            ],
        };
        let bytes = encrypt(&[&two_sets]);
        assert!(matches!(
            decrypt_descriptor_with_pk(&bytes, spendable_pks()[0]),
            Some(Decrypt::UnexpectedPayload(Decrypted::DescriptorBackup(_)))
        ));
    }

    #[test]
    fn decrypt_non_liana_descriptor() {
        let key = DescriptorPublicKey::from_str("[bf3891f9/48'/1'/0'/2']tpubDF8FX2wbUi7rFS4xC8BfYDu6AScYFvRQBwouJeu2DzE55gJgQk2mSa56D9VbyU9YVdWNqWdFBqhWtV9ixZGbd83SRhr7EZUvGJ8QfYazwks/<0;1>/*").unwrap();
        let wpkh = Descriptor::new_wpkh(key.clone()).unwrap();
        let bytes = encrypt(&[&wpkh]);
        assert!(matches!(
            decrypt_descriptor_with_pk(&bytes, dpk_to_root_pk(&key).unwrap()),
            Some(Decrypt::UnexpectedPayload(Decrypted::Descriptor(_)))
        ));
    }

    #[test]
    fn decrypt_unsupported_content() {
        let note = "backup 2026".to_string();
        let bytes = EncryptedBackup::new()
            .set_payload(&note)
            .unwrap()
            .set_keys(spendable_pks())
            .encrypt()
            .unwrap()
            .bytes;
        assert!(matches!(
            decrypt_descriptor_with_pk(&bytes, spendable_pks()[0]),
            Some(Decrypt::UnexpectedPayload(Decrypted::String(n))) if n == note
        ));
    }

    #[test]
    fn modal_derivation_paths() {
        let signet_default = vec![default_derivation_path(Network::Signet)];

        // the old format stores every path, common ones included
        let modal = DecryptModal::new(test_asset("v0.bed"), Network::Signet);
        assert!(modal.error.is_none());
        assert_eq!(
            modal.derivation_paths,
            vec![DerivationPath::from_str("m/48'/1'/0'/2'").unwrap()]
        );

        // the encoder leaves out the common paths
        let modal = DecryptModal::new(test_asset("v1.bip138"), Network::Signet);
        assert!(modal.error.is_none());
        assert_eq!(modal.derivation_paths, signet_default);
        let bytes = encrypt(&[descriptor().descriptor()]);
        let modal = DecryptModal::new(bytes, Network::Signet);
        assert!(modal.error.is_none());
        assert_eq!(modal.derivation_paths, signet_default);

        let modal = DecryptModal::new(b"not a backup".to_vec(), Network::Signet);
        assert!(matches!(modal.error, Some(Error::InvalidEncoding)));
        assert_eq!(modal.derivation_paths, signet_default);
    }

    fn paths(paths: &[&str]) -> Vec<DerivationPath> {
        paths
            .iter()
            .map(|p| DerivationPath::from_str(p).unwrap())
            .collect()
    }

    #[test]
    fn modal_paths() {
        let custom = DerivationPath::from_str("m/48'/0'/100'/2'").unwrap();
        let bytes = EncryptedBackup::new()
            .set_payload(descriptor().descriptor())
            .unwrap()
            .set_derivation_paths(vec![custom.clone()])
            .encrypt()
            .unwrap()
            .bytes;
        let modal = DecryptModal::new(bytes, Network::Bitcoin);
        assert_eq!(modal.derivation_paths, vec![custom.clone()]);

        // the stored paths first, then the 10 Liana accounts
        let liana_common_paths = modal.liana_common_paths();
        let mut expected = vec![custom];
        expected.extend(paths(&[
            "m/48'/0'/0'/2'",
            "m/48'/0'/1'/2'",
            "m/48'/0'/2'/2'",
            "m/48'/0'/3'/2'",
            "m/48'/0'/4'/2'",
            "m/48'/0'/5'/2'",
            "m/48'/0'/6'/2'",
            "m/48'/0'/7'/2'",
            "m/48'/0'/8'/2'",
            "m/48'/0'/9'/2'",
        ]));
        assert_eq!(liana_common_paths, expected);

        // then, on request, the common paths not tried yet
        let other_paths = modal.other_paths();
        assert_eq!(other_paths.len(), 60);
        assert!(other_paths.iter().all(|p| !liana_common_paths.contains(p)));
        for path in paths(&[
            "m/48'/0'/0'/1'",
            "m/48'/0'/9'/1'",
            "m/44'/0'/0'",
            "m/87'/0'/9'",
        ]) {
            assert!(other_paths.contains(&path));
        }
        assert!(!other_paths.contains(&DerivationPath::from_str("m/48'/1'/0'/1'").unwrap()));
    }

    #[test]
    fn modal_paths_stored_liana_path() {
        // a stored Liana path is not tried twice
        let modal = DecryptModal::new(test_asset("v0.bed"), Network::Signet);
        let liana_common_paths = modal.liana_common_paths();
        assert_eq!(liana_common_paths.len(), 10);
        assert_eq!(
            liana_common_paths[0],
            DerivationPath::from_str("m/48'/1'/0'/2'").unwrap()
        );
        assert_eq!(modal.other_paths().len(), 60);
    }

    const UNRELATED_MNEMONIC: &str =
        "video cupboard ketchup sock armed color mobile rhythm marble credit until mosquito";

    #[derive(Debug)]
    struct FakeDevice;

    #[async_trait]
    impl HWI for FakeDevice {
        fn device_kind(&self) -> DeviceKind {
            DeviceKind::Ledger
        }
        async fn get_version(&self) -> Result<Version, HWIError> {
            unimplemented!()
        }
        async fn get_master_fingerprint(&self) -> Result<Fingerprint, HWIError> {
            unimplemented!()
        }
        async fn get_extended_pubkey(&self, _path: &DerivationPath) -> Result<Xpub, HWIError> {
            unimplemented!()
        }
        async fn register_wallet(
            &self,
            _name: &str,
            _policy: &str,
        ) -> Result<Option<[u8; 32]>, HWIError> {
            unimplemented!()
        }
        async fn is_wallet_registered(&self, _name: &str, _policy: &str) -> Result<bool, HWIError> {
            unimplemented!()
        }
        async fn display_address(&self, _script: &AddressScript) -> Result<(), HWIError> {
            unimplemented!()
        }
        async fn sign_tx(&self, _tx: &mut Psbt) -> Result<(), HWIError> {
            unimplemented!()
        }
    }

    #[test]
    fn display_xpub_on_ledger_for_uncommon_paths() {
        let display = |kind, path: &str| {
            display_xpub(
                kind,
                Network::Signet,
                &DerivationPath::from_str(path).unwrap(),
            )
        };
        assert!(!display(DeviceKind::Ledger, "m/48'/1'/0'/2'"));
        assert!(!display(DeviceKind::Ledger, "m/84'/1'/9'"));
        assert!(display(DeviceKind::Ledger, "m/87'/1'/0'"));
        assert!(display(DeviceKind::Ledger, "m/48'/1'/10'/2'"));
        assert!(display(DeviceKind::LedgerSimulator, "m/48'/0'/0'/2'"));
        assert!(!display(DeviceKind::BitBox02, "m/87'/1'/0'"));
        assert!(!display(DeviceKind::BitBox02, "m/48'/1'/10'/2'"));
    }

    fn modal_with_device(fingerprint: Fingerprint) -> DecryptModal {
        let mut modal = DecryptModal::new(test_asset("v1.bip138"), Network::Signet);
        modal.devices.insert(fingerprint, Arc::new(FakeDevice));
        modal
    }

    #[test]
    fn modal_other_paths_offer() {
        let fingerprint = Fingerprint::from([1, 2, 3, 4]);
        let mut modal = modal_with_device(fingerprint);

        // a device that failed with the Liana paths is offered once, with its name
        let _ = modal.update(Decrypt::Fetched(fingerprint, "Ledger".into()));
        let _ = modal.update(Decrypt::Fetched(fingerprint, "Ledger".into()));
        assert_eq!(modal.other_paths_offers, [fingerprint]);
        assert_eq!(modal.fetched.get(&fingerprint).unwrap(), "Ledger");

        // accepted: the device is fetched again, and never offered again
        let _ = modal.update(Decrypt::TryOtherPaths(fingerprint));
        assert!(modal.other_paths_offers.is_empty());
        assert!(modal.other_paths_asked.contains(&fingerprint));
        assert!(!modal.fetched.contains_key(&fingerprint));
        assert!(modal.fetching.contains_key(&fingerprint));
        let _ = modal.update(Decrypt::Fetched(fingerprint, "Ledger".into()));
        assert!(modal.other_paths_offers.is_empty());
    }

    #[test]
    fn modal_other_paths_skip() {
        let fingerprint = Fingerprint::from([1, 2, 3, 4]);
        let mut modal = modal_with_device(fingerprint);

        let _ = modal.update(Decrypt::Fetched(fingerprint, "Ledger".into()));
        let _ = modal.update(Decrypt::SkipOtherPaths(fingerprint));
        assert!(modal.other_paths_offers.is_empty());
        assert!(modal.other_paths_asked.contains(&fingerprint));
        assert!(modal.fetched.contains_key(&fingerprint));
        assert!(!modal.fetching.contains_key(&fingerprint));
        let _ = modal.update(Decrypt::Fetched(fingerprint, "Ledger".into()));
        assert!(modal.other_paths_offers.is_empty());

        // nothing to retry without the device
        let other = Fingerprint::from([5, 6, 7, 8]);
        let _ = modal.update(Decrypt::Fetched(other, "Ledger".into()));
        assert!(modal.other_paths_offers.is_empty());
    }

    #[test]
    fn modal_other_paths_answer_its_offer_only() {
        let first = Fingerprint::from([1, 2, 3, 4]);
        let second = Fingerprint::from([5, 6, 7, 8]);
        let mut modal = modal_with_device(first);
        modal.devices.insert(second, Arc::new(FakeDevice));
        let _ = modal.update(Decrypt::Fetched(first, "Ledger".into()));
        let _ = modal.update(Decrypt::Fetched(second, "Jade".into()));

        // a repeated answer to the first offer does not answer the next one
        let _ = modal.update(Decrypt::SkipOtherPaths(first));
        let _ = modal.update(Decrypt::SkipOtherPaths(first));
        let _ = modal.update(Decrypt::TryOtherPaths(first));
        assert_eq!(modal.other_paths_offers, [second]);
        assert!(!modal.other_paths_asked.contains(&second));
    }

    #[test]
    fn modal_follows_plugged_devices() {
        let fingerprint = Fingerprint::from([1, 2, 3, 4]);
        let plugged = |device: Arc<dyn HWI + Send + Sync>| HardwareWallet::Supported {
            id: "fake".into(),
            device,
            kind: DeviceKind::Ledger,
            fingerprint,
            version: None,
            registered: None,
            alias: None,
        };
        let mut modal = DecryptModal::new(test_asset("v1.bip138"), Network::Signet);
        let mut hws = HardwareWallets::new(LianaDirectory::new(env::temp_dir()), Network::Signet);
        hws.list.push(plugged(Arc::new(FakeDevice)));
        let _ = modal.update_devices(&mut hws);
        let _ = modal.update(Decrypt::Fetched(fingerprint, "Ledger".into()));
        assert_eq!(modal.other_paths_offers, [fingerprint]);

        // replugged: the offer stays, with the new handle
        let replugged: Arc<dyn HWI + Send + Sync> = Arc::new(FakeDevice);
        hws.list = vec![plugged(replugged.clone())];
        let _ = modal.update_devices(&mut hws);
        assert!(Arc::ptr_eq(&modal.devices[&fingerprint], &replugged));
        assert_eq!(modal.other_paths_offers, [fingerprint]);

        // unplugged: nothing is left to offer
        hws.list.clear();
        let _ = modal.update_devices(&mut hws);
        assert!(modal.devices.is_empty());
        assert!(modal.other_paths_offers.is_empty());
    }

    #[test]
    fn mnemonic_not_offered() {
        let fingerprint = Fingerprint::from([1, 2, 3, 4]);
        let mut modal = DecryptModal::new(test_asset("v1.bip138"), Network::Signet);
        modal.mnemonic.value = UNRELATED_MNEMONIC.into();
        let _ = modal.update(Decrypt::MnemonicStatus(
            Some(DecryptWarning::MnemonicCannotDecrypt),
            Some(fingerprint),
        ));
        assert!(modal.fetched.contains_key(&fingerprint));
        assert!(modal.other_paths_offers.is_empty());
    }

    #[test]
    fn mnemonic_tries_every_path() {
        // the primary key of this wallet derives from the mnemonic at m/84'/1'/0', not a Liana path
        let seed = Mnemonic::from_str(UNRELATED_MNEMONIC).unwrap().to_seed("");
        let secp = Secp256k1::new();
        let master = bip32::Xpriv::new_master(Network::Signet, &seed).unwrap();
        let path = DerivationPath::from_str("m/84'/1'/0'").unwrap();
        let xpub = Xpub::from_priv(&secp, &master.derive_priv(&secp, &path).unwrap());
        let descriptor = LianaDescriptor::from_str(&format!(
            "wsh(or_d(pk([{}/84'/1'/0']{}/<0;1>/*),and_v(v:pkh([6abb52a9/48'/1'/0'/2']tpubDFZTCVU1Sa9nJXCxx97UFvGausHQPFjJyaiDbdr8GNqjCLKwYc8ihegK7yJdcizs9HMbiGA7ke1HiCENVHaERvNANHW7U2Wo2qnRsuqB52r/<2;3>/*),older(65535))))",
            master.fingerprint(&secp),
            xpub
        ))
        .unwrap();
        let bytes = encrypt(&[descriptor.descriptor()]);
        let modal = DecryptModal::new(bytes.clone(), Network::Signet);

        assert!(matches!(
            decrypt_with_seed(&bytes, &seed, Network::Signet, modal.liana_common_paths()),
            Decrypt::MnemonicStatus(Some(DecryptWarning::MnemonicCannotDecrypt), Some(_))
        ));
        match decrypt_with_seed(&bytes, &seed, Network::Signet, modal.mnemonic_paths()) {
            Decrypt::Backup(backup) => {
                assert_eq!(backup.accounts[0].descriptor, descriptor.to_string())
            }
            other => panic!("not a backup: {:?}", other),
        }
    }
}
