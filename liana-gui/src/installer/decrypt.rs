use std::{
    collections::{BTreeMap, HashSet},
    fmt::Debug,
    str::FromStr,
    sync::Arc,
};

use async_hwi::{bitbox::api::btc::Fingerprint, DeviceKind, Version, HWI};
use encrypted_backup::{Decrypted, EncryptedBackup};
use iced::{
    alignment, clipboard,
    widget::{column, row, scrollable, Column, Space},
    Length, Task,
};
use liana::{
    bip39::Mnemonic,
    descriptors::LianaDescriptor,
    miniscript::{
        bitcoin::{
            bip32::{self, DerivationPath},
            key::Secp256k1,
            secp256k1, Network,
        },
        DescriptorPublicKey,
    },
};
use liana_ui::{
    component::{
        card,
        form::Value,
        modal::{self, widget_style, BTN_W},
        text::{self, p1_regular},
    },
    icon,
    widget::{modal::Modal, Button, Container, Element},
};

use crate::{
    app::state::export::ExportModal,
    backup::Backup,
    export::ImportExportType,
    hw::{HardwareWallet, HardwareWallets},
    installer,
    utils::{default_derivation_path, example_xpub},
};

type FnMsg = fn() -> installer::Message;

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
    derivation_paths: HashSet<DerivationPath>,
    cant_fetch: BTreeMap<String /* id */, String /* name */>,
    fetching: BTreeMap<Fingerprint, String /* name */>,
    fetched: BTreeMap<Fingerprint, String /* name */>,
    show_options: bool,
    import_xpub_error: Option<String>,
    xpub: Value<String>,
    xpub_busy: bool,
    mnemonic: Value<String>,
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

#[derive(Debug, Clone)]
pub enum Decrypt {
    Fetched(Fingerprint, String /* name */),
    Backup(Backup),
    Xpub(String),
    PasteXpub,
    SelectXpub,
    XpubError(&'static str),
    Mnemonic(String),
    PasteMnemonic,
    SelectMnemonic,
    MnemonicStatus(Option<&'static str> /* error */, Option<Fingerprint>),
    SelectImportXpub,
    UnexpectedPayload(Decrypted),
    InvalidDescriptor,
    ContentNotSupported,
    ShowOptions(bool),
    Close,
    CloseModal,
    None,
}

impl From<Decrypt> for installer::Message {
    fn from(value: Decrypt) -> Self {
        installer::Message::Decrypt(value)
    }
}

pub fn decrypt_descriptor_with_pk(bytes: &[u8], pk: secp256k1::PublicKey) -> Option<Decrypt> {
    match EncryptedBackup::new()
        .set_encrypted_payload(bytes)
        .expect("sanitized")
        .set_keys(vec![pk])
        .decrypt()
    {
        Ok(dec) => match dec {
            Decrypted::Descriptor(d) => {
                let descr = match LianaDescriptor::from_str(&d.to_string()) {
                    Ok(descr) => descr,
                    Err(_) => return Some(Decrypt::UnexpectedPayload(Decrypted::Descriptor(d))),
                };
                let network = if descr.all_xpubs_net_is(Network::Bitcoin) {
                    Network::Bitcoin
                } else {
                    Network::Signet
                };
                Some(Decrypt::Backup(Backup::from_descriptor(descr, network)))
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
        },
        Err(_) => None,
    }
}

impl DecryptModal {
    pub fn new(bytes: Vec<u8>, network: Network) -> Self {
        let mut error = None;
        let derivation_paths = if let Some(backup) =
            match EncryptedBackup::new().set_encrypted_payload(&bytes) {
                Ok(b) => Some(b),
                Err(_) => {
                    error = Some(Error::InvalidEncoding);
                    None
                }
            } {
            backup.get_derivation_paths().into_iter().collect()
        } else {
            let mut h = HashSet::new();
            h.insert(default_derivation_path(Network::Bitcoin));
            h.insert(default_derivation_path(Network::Signet));
            h
        };
        Self {
            network,
            error,
            bytes,
            derivation_paths,
            cant_fetch: BTreeMap::new(),
            fetching: BTreeMap::new(),
            fetched: BTreeMap::new(),
            show_options: false,
            import_xpub_error: None,
            xpub: Value::default(),
            xpub_busy: false,
            mnemonic: Value::default(),
            mnemonic_busy: false,
            focus: Focus::None,
            modal: None,
        }
    }
    pub fn update(&mut self, msg: Decrypt) -> Task<installer::Message> {
        match msg {
            Decrypt::Fetched(fg, name) => {
                self.fetching.remove(&fg);
                self.fetched.insert(fg, name);
                Task::none()
            }
            Decrypt::Backup(_) => {
                tracing::error!(
                    "DecryptModal::update(Backup), this message must have been catched early"
                );
                Task::none()
            }
            Decrypt::XpubError(s) => {
                match self.focus {
                    Focus::ImportXpub => {
                        self.import_xpub_error = Some(s.to_string());
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
                Decrypted::Descriptor(_) => {
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
            let modal = Modal::new(content, decrypt_view(self));
            modal.on_blur(Some(Decrypt::Close.into())).into()
        }
    }
    #[allow(clippy::collapsible_match)]
    fn fetch(
        &self,
        device: Arc<dyn HWI + Send + Sync>,
        fingerprint: Fingerprint,
        name: String,
    ) -> Task<installer::Message> {
        let derivation_paths = self.derivation_paths.clone();
        let bytes = self.bytes.clone();
        Task::perform(
            async move {
                for path in derivation_paths {
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
            // FIXME: Capitalize first letter
            if let Some(v) = version {
                format!("{kind} {v}")
            } else {
                kind.to_string()
            }
        }

        let mut new_cant_fetch = BTreeMap::new();
        let mut to_fetch = vec![];
        for d in &devices.list {
            match d {
                HardwareWallet::Unsupported {
                    id, kind, version, ..
                } => {
                    new_cant_fetch.insert(id.clone(), name(*kind, version.clone()));
                }
                HardwareWallet::Locked { id, kind, .. } => {
                    new_cant_fetch.insert(id.clone(), name(*kind, None));
                }
                d => {
                    if let HardwareWallet::Supported { fingerprint, .. } = d {
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
                batch.push(self.fetch(device.clone(), *fingerprint, name));
            }
        }
        (!batch.is_empty()).then(|| Task::batch(batch))
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
                    let pk = encrypted_backup::descriptor::dpk_to_pk(&dpk);
                    decrypt_descriptor_with_pk(&bytes, pk).unwrap_or(Decrypt::XpubError(
                        "Xpub is valid but cannot decrypt this file",
                    ))
                },
                |m| m.into(),
            )
        } else {
            self.xpub.warning = Some("Invalid xpub");
            self.xpub.valid = false;
            Task::none()
        }
    }
    fn update_xpub_error(&mut self, error: &'static str) {
        self.xpub.warning = Some(error);
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
        let bytes = self.bytes.clone();
        let deriv_paths = self.derivation_paths.clone();
        let network = self.network;
        let seed = match Mnemonic::from_str(&mnemonic) {
            Ok(m) => m,
            Err(_) => {
                self.mnemonic.valid = false;
                self.mnemonic.warning = Some("Invalid mnemonic");
                return Task::none();
            }
        }
        .to_seed("");
        self.mnemonic.valid = true;
        self.mnemonic.warning = None;
        self.mnemonic_busy = true;
        Task::perform(
            async move {
                let xpriv = bip32::Xpriv::new_master(network, &seed).expect("seed is 64 bytes");
                let secp = Secp256k1::new();
                let fingerprint = xpriv.fingerprint(&secp);

                let mut backup = None;
                for path in deriv_paths {
                    let pk = xpriv
                        .derive_priv(&secp, &path)
                        .expect("cannot fail")
                        .private_key
                        .public_key(&secp);
                    if let Some(Decrypt::Backup(b)) = decrypt_descriptor_with_pk(&bytes, pk) {
                        backup = Some(Decrypt::Backup(b));
                    }
                }
                backup.unwrap_or(Decrypt::MnemonicStatus(
                    Some("Mnemonic is valid but cannot decrypt the file"),
                    Some(fingerprint),
                ))
            },
            |m| m.into(),
        )
    }
    fn update_mnemonic_state(&mut self, error: Option<&'static str>, fg: Option<Fingerprint>) {
        self.mnemonic_busy = false;
        self.mnemonic.warning = error;
        self.mnemonic.valid = false;
        if let Some(fg) = fg {
            self.fetched.insert(fg, "Mnemonic".to_string());
        }
        self.mnemonic.warning = error;
    }
}

fn invalid_content(hint: &str) -> Container<'_, installer::Message> {
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
    message: &str,
) -> Button<'_, installer::Message> {
    let message = p1_regular(message);
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

fn cant_fetch_device(name: String) -> Button<'static, installer::Message> {
    let message = "Please unlock or open app on the device";
    widget_signing_device(name, None, message)
}

fn fetching_device(name: String, fingerprint: Fingerprint) -> Button<'static, installer::Message> {
    let message = "Try to decrypt with this device...";
    widget_signing_device(name, Some(fingerprint), message)
}

fn fetched_device(name: String, fingerprint: Fingerprint) -> Button<'static, installer::Message> {
    let message = "Failed to decrypt file with this device";
    widget_signing_device(name, Some(fingerprint), message)
}

fn valid_content(state: &DecryptModal) -> Container<'static, installer::Message> {
    let description = text::text("Plug in and unlock a hardware device belonging to this setup to automatically decrypt the backup");
    let mut devices = state
        .fetching
        .iter()
        .map(|(fg, name)| fetching_device(name.clone(), *fg))
        .collect::<Vec<_>>();
    for d in &state.cant_fetch {
        devices.push(cant_fetch_device(d.1.clone()));
    }
    for (fg, name) in &state.fetched {
        devices.push(fetched_device(name.clone(), *fg));
    }
    let options_btn = modal::optional_section(
        state.show_options,
        "Other options".to_string(),
        || Decrypt::ShowOptions(true).into(),
        || Decrypt::ShowOptions(false).into(),
    );

    let mut col = Column::new().spacing(5).push(description);
    for d in devices {
        col = col.push(d);
    }
    col = col.push(Space::with_height(10)).push(options_btn);
    if state.show_options {
        col = col.push(optional_content(state));
    }

    Container::new(col)
}

fn optional_content(state: &DecryptModal) -> Container<'static, installer::Message> {
    let import = modal::button_entry(
        Some(icon::import_icon()),
        "Upload extended public key file",
        None,
        state.import_xpub_error.clone(),
        Some(|| Decrypt::SelectImportXpub.into()),
    );

    let xpub = modal::collapsible_input_button(
        state.focus == Focus::Xpub,
        Some(icon::round_key_icon()),
        "Paste an extended public key".to_string(),
        example_xpub(state.network),
        &state.xpub,
        Some(|s| Decrypt::Xpub(s).into()),
        Some(|| Decrypt::PasteXpub.into()),
        || Decrypt::SelectXpub.into(),
    );

    let mnemonic = modal::collapsible_input_button(
        state.focus == Focus::Mnemonic,
        Some(icon::pencil_icon()),
        "Enter mnemonic of one of the keys".to_string(),
        "code code code code code code code code code code code brave".to_string(),
        &state.mnemonic,
        Some(|s| Decrypt::Mnemonic(s).into()),
        Some(|| Decrypt::PasteMnemonic.into()),
        || Decrypt::SelectMnemonic.into(),
    );

    let col = column![
        import,
        Space::with_height(modal::V_SPACING),
        xpub,
        Space::with_height(modal::V_SPACING),
        mnemonic
    ];

    Container::new(col)
}

/// Return the modal view for an export task
pub fn decrypt_view<'a>(state: &DecryptModal) -> Container<'a, installer::Message> {
    let header = modal::header(
        Some("Decrypt backup file".to_string()),
        None::<FnMsg>,
        Some(|| installer::Message::Decrypt(Decrypt::Close)),
    );

    let content = match state.error {
        Some(e) => match e {
            Error::InvalidEncoding => invalid_content(
                "The file cannot be decoded properly, it seems no be an encrypted backup.",
            ),
            Error::InvalidType => invalid_content(
                "The file have been decrypted but the content type is not supported.",
            ),
            Error::InvalidDescriptor => invalid_content(
                "The file have been decrypted but the descriptor is not a valid Liana descriptor.",
            ),
        },
        None => valid_content(state),
    };

    let content = scrollable(content);

    let content = row![Space::with_width(50), content,];

    let column = Column::new()
        .push(header)
        .push(content)
        .spacing(5)
        .align_x(alignment::Horizontal::Center);

    card::simple(column)
        .width(Length::Fixed(modal::MODAL_WIDTH as f32))
        .height(Length::Fixed(450.0))
}
