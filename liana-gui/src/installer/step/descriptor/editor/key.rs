use std::{
    collections::HashMap,
    str::FromStr,
    sync::{Arc, Mutex},
};

use async_hwi::{DeviceKind, Version};
use iced::{
    alignment::{Horizontal, Vertical},
    clipboard,
    widget::{column, container, pick_list, row, Column, Row, Space},
    Length, Subscription, Task,
};
use liana::miniscript::{
    bitcoin::{
        bip32::{ChildNumber, DerivationPath, Fingerprint, Xpub},
        Network,
    },
    descriptor::{DerivPaths, DescriptorMultiXKey, DescriptorPublicKey, DescriptorXKey, Wildcard},
};

use liana_ui::{
    color,
    component::{
        button, card, form,
        hw::Account,
        modal::{self, collapsible_input_button},
        text::{p1_bold, p1_regular},
        tooltip,
    },
    icon, theme,
    widget::{Container, Element},
};

use crate::{
    app::{settings::ProviderKey, state::export::ExportModal},
    export::{ImportExportMessage, ImportExportType},
    hw::{is_compatible_with_tapminiscript, HardwareWallet, HardwareWallets, UnsupportedReason},
    installer::{
        descriptor::{Key, KeySource},
        message::{self, Message},
        view::editor::example_xpub,
        Error, PathKind,
    },
    services::{
        self,
        keys::{self, api::KeyKind},
    },
    signer::Signer,
};

pub type FnMsg = fn() -> Message;

pub fn new_multixkey_from_xpub(
    xpub: DescriptorXKey<Xpub>,
    derivation_index: usize,
) -> DescriptorMultiXKey<Xpub> {
    DescriptorMultiXKey {
        origin: xpub.origin,
        xkey: xpub.xkey,
        derivation_paths: DerivPaths::new(vec![
            DerivationPath::from_str(&format!("m/{}", 2 * derivation_index)).unwrap(),
            DerivationPath::from_str(&format!("m/{}", 2 * derivation_index + 1)).unwrap(),
        ])
        .unwrap(),
        wildcard: Wildcard::Unhardened,
    }
}

pub fn check_key_network(key: &DescriptorPublicKey, network: Network) -> bool {
    match key {
        DescriptorPublicKey::XPub(key) => {
            if network == Network::Bitcoin {
                key.xkey.network == Network::Bitcoin.into()
            } else {
                key.xkey.network == Network::Testnet.into()
            }
        }
        DescriptorPublicKey::MultiXPub(key) => {
            if network == Network::Bitcoin {
                key.xkey.network == Network::Bitcoin.into()
            } else {
                key.xkey.network == Network::Testnet.into()
            }
        }
        _ => true,
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Step {
    Select,
    Details,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Focus {
    None,
    Key(Fingerprint),
    Device(Fingerprint),
    EnterXpub,
    LoadXpubFromFile,
    GenerateHotKey,
    EnterSafetyNetToken,
    EnterCosignerToken,
}

#[derive(Debug, Clone)]
pub enum SelectKeySourceMessage {
    SelectDevice(Fingerprint),
    FetchFromDevice(Fingerprint, ChildNumber),
    SelectKey(Fingerprint),
    SelectLoadXpub,
    SelectEnterXpub,
    PasteXpub,
    Xpub(String),
    SelectGenerateHotKey,
    FetchFromHotSigner(ChildNumber),
    SelectEnterSafetyNetToken,
    SelectEnterCosignerToken,
    PasteToken,
    Token(String),
    Previous,
    Next,
    Alias(String),
    LoadKey(Result<Key, Error>),
    ProviderKey(Result<Key, Error>),
    ImportExport(ImportExportMessage),
    Account(ChildNumber),
    Collapse(bool),
    Retry,
    None,
}

/// This struct represent metadata about a spending path, including whether it's
/// a primary path or a timelocked recovery path, keys used
/// in this path, if safety-net feature is allowed for this path.
pub struct PathData {
    /// Coordinate of the key to edit/insert
    pub coordinates: Vec<(usize, usize)>,
    /// List of keys already used in this path
    pub keys: Vec<Fingerprint>,
    /// Whether safety-net or cosigner features are enabled for this path
    pub token_kind: Vec<KeyKind>,
}

pub enum HwState {
    Supported,
    Locked,
    Unsupported(UnsupportedReason),
}

#[derive(Debug, Clone)]
pub enum SelectedKey {
    None,
    Existing(Fingerprint),
    New(Box<Key>),
}

impl SelectedKey {
    pub fn fingerprint(&self) -> Option<Fingerprint> {
        match self {
            SelectedKey::None => None,
            SelectedKey::Existing(fg) => Some(*fg),
            SelectedKey::New(key) => Some(key.fingerprint),
        }
    }
}

pub struct SelectKeySource {
    // state
    network: Network,
    /// Whether keys must support tap-miniscript signing.
    taproot: bool,
    /// List of keys already in use, including metadata about spending
    /// path they are used in.
    keys: HashMap<Fingerprint, (Vec<(usize, usize)>, Key)>,
    /// Accounts that are used for deriving keys
    accounts: HashMap<Fingerprint, ChildNumber>,
    /// Informations about the actual spending path.
    actual_path: PathData,
    hot_signer: Arc<Mutex<Signer>>,
    /// The currently selected key.
    selected_key: SelectedKey,
    step: Step,
    focus: Focus,
    modal: Option<ExportModal>,
    processing: bool,
    error: Option<String>,
    details_error: Option<String>,
    import_xpub_error: Option<String>,

    // fields
    form_alias: form::Value<String>,
    form_xpub: form::Value<String>,
    form_safety_net_token: form::Value<String>,
    form_cosigner_token: form::Value<String>,
    form_account: Option<ChildNumber>,

    options_collapsed: bool,
}

impl SelectKeySource {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        network: Network,
        taproot: bool,
        actual_path: PathData,
        keys: HashMap<Fingerprint, (Vec<(usize, usize)>, Key)>,
        accounts: HashMap<Fingerprint, ChildNumber>,
        hot_signer: Arc<Mutex<Signer>>,
    ) -> Self {
        Self {
            network,
            taproot,
            keys,
            accounts,
            actual_path,
            hot_signer,
            selected_key: SelectedKey::None,
            step: Step::Select,
            focus: Focus::None,
            modal: None,
            processing: false,
            error: None,
            details_error: None,
            import_xpub_error: None,
            form_alias: Default::default(),
            form_xpub: Default::default(),
            form_safety_net_token: Default::default(),
            form_cosigner_token: Default::default(),
            form_account: None,
            options_collapsed: false,
        }
    }
    fn already_used_keys(
        &self,
    ) -> Vec<(
        KeySource,
        String, /* alias */
        Fingerprint,
        bool, /* available */
    )> {
        self.keys
            .iter()
            .map(|(fg, (_, key))| {
                let source = key.source.clone();
                let alias = key.name.clone();
                let mut available = true;
                if self.actual_path.keys.iter().any(|key_fg| key_fg == fg) {
                    available = false;
                }
                if let KeySource::Token(kind, _) = key.source {
                    if !self.actual_path.token_kind.contains(&kind) {
                        available = false;
                    }
                }
                (source, alias, *fg, available)
            })
            .collect()
    }
    fn detected_hws(
        &self,
        hws: &HardwareWallets,
    ) -> Vec<(
        String, /* alias */
        Option<Fingerprint>,
        HwState,
        bool, /* support taproot */
    )> {
        hws.list
            .iter()
            .filter_map(|hw| {
                let registered = if let Some(fg) = hw.fingerprint() {
                    self.keys.contains_key(&fg)
                } else {
                    false
                };
                if !registered {
                    let mut out = match hw {
                        HardwareWallet::Unsupported {
                            kind,
                            version,
                            reason,
                            ..
                        } => match version {
                            Some(v) => (
                                format!("{kind} {v}"),
                                None,
                                HwState::Unsupported(reason.clone()),
                                is_compatible_with_tapminiscript(kind, Some(v)),
                            ),
                            None => (
                                kind.to_string(),
                                None,
                                HwState::Unsupported(reason.clone()),
                                is_compatible_with_tapminiscript(kind, None),
                            ),
                        },
                        HardwareWallet::Locked { kind, .. } => (
                            kind.to_string(),
                            None,
                            HwState::Locked,
                            is_compatible_with_tapminiscript(kind, None),
                        ),
                        HardwareWallet::Supported {
                            kind,
                            fingerprint,
                            version,
                            ..
                        } => match version {
                            Some(v) => (
                                format!("{kind} {v}"),
                                Some(*fingerprint),
                                HwState::Supported,
                                is_compatible_with_tapminiscript(kind, Some(v)),
                            ),
                            None => (
                                kind.to_string(),
                                Some(*fingerprint),
                                HwState::Supported,
                                is_compatible_with_tapminiscript(kind, None),
                            ),
                        },
                    };

                    // Capitalize first letter
                    let alias = &mut out.0;
                    if let Some(first) = alias.get_mut(0..1) {
                        first.make_ascii_uppercase();
                    }

                    Some(out)
                } else {
                    None
                }
            })
            .collect()
    }
    pub fn route(msg: SelectKeySourceMessage) -> Message {
        Message::SelectKeySource(msg)
    }
    fn fetch_xpub(
        hw: std::sync::Arc<dyn async_hwi::HWI + Send + Sync>,
        device_version: Option<Version>,
        device_kind: DeviceKind,
        fingerprint: Fingerprint,
        network: Network,
        account: ChildNumber,
    ) -> Task<Message> {
        Task::perform(
            async move {
                (
                    device_version,
                    device_kind,
                    fingerprint,
                    network,
                    get_extended_pubkey(hw, fingerprint, network, account).await,
                )
            },
            move |(device_version, device_kind, fingerprint, network, res)| {
                let r = match res {
                    Err(e) => Err(e),
                    Ok(key) => {
                        if check_key_network(&key, network) {
                            Ok(Key {
                                source: KeySource::Device(device_kind, device_version),
                                fingerprint,
                                name: "".to_string(),
                                key,
                                account: Some(account),
                            })
                        } else {
                            Err(Error::Unexpected(
                                "Fetched key does not have the correct network".to_string(),
                            ))
                        }
                    }
                };
                Self::route(SelectKeySourceMessage::LoadKey(r))
            },
        )
    }
    fn on_select_device(&mut self, fingerprint: Fingerprint) -> Task<Message> {
        self.focus = Focus::Device(fingerprint);
        let _ = self.on_next();
        self.processing = true;
        Task::done(Self::route(SelectKeySourceMessage::Account(
            ChildNumber::from_hardened_idx(0).expect("hardcoded"),
        )))
    }
    fn on_fetch_from_device(
        &mut self,
        fingerprint: Fingerprint,
        account: ChildNumber,
        hws: &mut HardwareWallets,
    ) -> Task<Message> {
        let hw_list = &hws.list;
        let mut i = None;
        for (i_hw, hw) in hw_list.iter().enumerate() {
            if hw.fingerprint() == Some(fingerprint) {
                i = Some(i_hw);
            }
        }
        let i = match i {
            None => {
                tracing::error!("SelectKeySource::on_select_device(): device with fingerprint {fingerprint} not found.");
                return Task::none();
            }
            Some(i) => i,
        };
        if let Some(HardwareWallet::Supported {
            device,
            fingerprint,
            kind,
            version,
            ..
        }) = hw_list.get(i)
        {
            self.processing = true;
            let device_version = version.clone();
            if self.accounts.contains_key(fingerprint) {
                // FIXME: here we're gonna overwrite an actual selected account, we should only
                // allow this if the key is only present in the current account.
            }
            let fingerprint = *fingerprint;
            let device_kind = *kind;
            let device_cloned = device.clone();
            let network = self.network;
            return Self::fetch_xpub(
                device_cloned,
                device_version,
                device_kind,
                fingerprint,
                network,
                account,
            );
        }
        Task::none()
    }
    fn fetch_provider(&mut self, token: String) -> Task<Message> {
        self.processing = true;
        let client = services::keys::Client::new();
        Task::perform(
            async move { (token.clone(), client.get_key_by_token(token).await) },
            |(token, res)| {
                Self::route(SelectKeySourceMessage::ProviderKey(match res {
                    Err(e) => Err(Error::Services(e)),
                    Ok(ref key) => Ok(Key {
                        source: KeySource::Token(
                            key.kind,
                            ProviderKey {
                                uuid: key.uuid.clone(),
                                token,
                                provider: key.provider.clone().into(),
                            },
                        ),
                        fingerprint: key.xpub.master_fingerprint(),
                        name: format!("{} - {}", key.provider.name.clone(), key.kind),
                        key: key.xpub.clone(),
                        account: None,
                    }),
                }))
            },
        )
    }
    fn on_select_key(&mut self, fingerprint: Fingerprint) -> Task<Message> {
        self.focus = Focus::Key(fingerprint);
        self.selected_key = SelectedKey::Existing(fingerprint);
        self.on_next()
    }
    fn on_select_load_xpub(&mut self) -> Task<Message> {
        self.focus = Focus::LoadXpubFromFile;
        self.import_xpub_error = None;
        if self.modal.is_none() {
            let modal = ExportModal::new(None, ImportExportType::ImportXpub(self.network));
            let launch = modal.launch(false);
            self.modal = Some(modal);
            return launch;
        }
        Task::none()
    }
    fn on_select_enter_xpub(&mut self) -> Task<Message> {
        self.focus = Focus::EnterXpub;
        Task::none()
    }
    fn on_select_generate_hot_key(&mut self) -> Task<Message> {
        self.focus = Focus::GenerateHotKey;
        let _ = self.on_next();
        self.processing = true;
        Task::done(Self::route(SelectKeySourceMessage::Account(
            ChildNumber::from_hardened_idx(0).expect("hardcoded"),
        )))
    }
    fn on_fetch_from_hotsigner(&mut self, account: ChildNumber) -> Task<Message> {
        self.processing = false;
        let fingerprint = self.hot_signer.lock().unwrap().fingerprint();

        if self.keys.contains_key(&fingerprint) {
            self.selected_key = SelectedKey::Existing(fingerprint);
            return Task::none();
        }

        self.form_alias.value = "Hot Signer".to_string();
        self.form_alias.valid = true;

        let derivation_path = derivation_path(self.network, account);
        let key_str = format!(
            "[{}/{}]{}",
            fingerprint,
            derivation_path.to_string().trim_start_matches("m/"),
            self.hot_signer
                .lock()
                .expect("poisoned")
                .get_extended_pubkey(&derivation_path)
        );

        let key = DescriptorPublicKey::from_str(&key_str).expect("always ok");
        let key = Key {
            source: KeySource::HotSigner,
            name: self.form_alias.value.clone(),
            fingerprint,
            key,
            account: Some(account),
        };
        self.selected_key = SelectedKey::New(Box::new(key));
        Task::none()
    }
    fn on_select_enter_safety_net_token(&mut self) -> Task<Message> {
        self.focus = Focus::EnterSafetyNetToken;
        Task::none()
    }
    fn on_select_enter_cosigner_token(&mut self) -> Task<Message> {
        self.focus = Focus::EnterCosignerToken;
        Task::none()
    }
    fn on_provider_key(&mut self, key: Result<Key, Error>) -> Task<Message> {
        self.processing = false;
        let (warning, valid) = match self.focus {
            Focus::EnterSafetyNetToken => (
                &mut self.form_safety_net_token.warning,
                &mut self.form_safety_net_token.valid,
            ),
            Focus::EnterCosignerToken => (
                &mut self.form_cosigner_token.warning,
                &mut self.form_cosigner_token.valid,
            ),
            _ => return Task::none(),
        };
        match key {
            Ok(k) => {
                // If it is a provider key that has just been fetched, do some additional sanity checks.
                if let Some(key_kind) = k.source.provider_key_kind() {
                    // We don't need to check key's status as redeemed keys are not returned.
                    *warning = if !check_key_network(&k.key, self.network) {
                        Some("Fetched key does not have the correct network")
                    } else if !self.actual_path.token_kind.contains(&key_kind) {
                        let warn = match key_kind {
                            KeyKind::SafetyNet => {
                                "SafetyNet kind of token is not allowed for this path"
                            }
                            KeyKind::Cosigner => {
                                "Cosigner kind of token is not allowed for this path"
                            }
                        };
                        Some(warn)
                    }
                    // If two keys have the same fingerprint, they must both have the same provider key kind (which could be `None`).
                    // Note that this checks all keys regardless of whether they are currently being used in a path.
                    else if self.keys.iter().any(|(fg, (_, key))| {
                        *fg == key.fingerprint
                            && key.source.provider_key_kind() != key.source.provider_key_kind()
                    }) {
                        Some("Fetched key has already been added to the wallet.")
                    } else {
                        None
                    };
                    *valid = warning.is_none();
                    if *valid {
                        self.selected_key = SelectedKey::New(Box::new(k.clone()));
                        if let Some(kind) = k.source.provider_key_kind() {
                            self.form_alias.value = format!("{:?}", kind);
                        }
                        let _ = self.on_next();
                    }
                }
            }
            Err(e) => {
                self.error = Some(e.to_string());
            }
        }
        Task::none()
    }
    fn on_load_key(&mut self, key: Result<Key, Error>) -> Task<Message> {
        self.processing = false;
        match key {
            Ok(mut key) => {
                key.account = self.accounts.get(&key.fingerprint).copied();
                self.selected_key = SelectedKey::New(Box::new(key));
                self.details_error = None;
            }
            Err(e) => {
                self.details_error = match e {
                    Error::Unexpected(u) => match u {
                        u if u == "Fetched key does not have the correct network" => Some(
                            "Failed to fetch key. Switch network on device and retry".to_string(),
                        ),
                        u => Some(u),
                    },
                    Error::HardwareWallet(eh) => match eh {
                        // error returned by ledger on wrong network
                        async_hwi::Error::Device(d)
                            if d == "Device {\n    command: 0,\n    status: NotSupported,\n}" =>
                        {
                            Some(
                                "Failed to fetch key. Switch network on device and retry"
                                    .to_string(),
                            )
                        }
                        _ => Some(eh.to_string()),
                    },
                    _ => None,
                };
            }
        }
        Task::none()
    }
    fn on_update_xpub(&mut self, xpub: String) -> Task<Message> {
        self.form_xpub.warning = None;
        self.selected_key = SelectedKey::None;
        self.form_xpub.value = xpub.clone();
        if let Ok(DescriptorPublicKey::XPub(key)) = DescriptorPublicKey::from_str(&xpub) {
            if !key.derivation_path.is_master() {
                self.form_xpub.valid = false;
                self.form_xpub.warning = Some("Wrong derivation path");
            } else if let Some((fingerprint, _)) = key.origin {
                self.form_xpub.valid = if self.network == Network::Bitcoin {
                    key.xkey.network == Network::Bitcoin.into()
                } else {
                    key.xkey.network == Network::Testnet.into()
                };
                if !self.form_xpub.valid {
                    self.form_xpub.warning = Some("Wrong network");
                    self.form_xpub.valid = false;
                }
                if self.keys.contains_key(&fingerprint) {
                    self.form_xpub.warning = Some("Key already used");
                    self.form_xpub.valid = false;
                }

                if self.form_xpub.valid {
                    self.xpub_valid(fingerprint, key);
                }
            } else {
                self.form_xpub.valid = false;
                self.form_xpub.warning = Some("Origin missing");
            }
        } else {
            self.form_xpub.valid = xpub.is_empty();
            if !self.form_xpub.valid {
                self.form_xpub.warning = Some("Invalid Xpub");
            }
        }
        Task::none()
    }
    fn on_import_xpub(&mut self, xpub: String) -> Task<Message> {
        if let Ok(DescriptorPublicKey::XPub(key)) = DescriptorPublicKey::from_str(&xpub) {
            if let Some((fingerprint, _)) = key.origin {
                if self.keys.contains_key(&fingerprint) {
                    self.import_xpub_error = Some("Imported key already used".to_string());
                    self.focus = Focus::None;
                } else {
                    self.xpub_valid(fingerprint, key)
                }
            }
        }
        Task::none()
    }
    fn xpub_valid(&mut self, fingerprint: Fingerprint, key: DescriptorXKey<Xpub>) {
        let key = Key {
            source: KeySource::Manual,
            fingerprint,
            name: "".to_string(),
            key: DescriptorPublicKey::XPub(key),
            account: None,
        };
        if self.keys.contains_key(&fingerprint) {
            self.selected_key = SelectedKey::Existing(fingerprint);
        } else {
            self.selected_key = SelectedKey::New(Box::new(key));
        }
        self.form_alias.value = "".to_string();
        self.form_alias.valid = true;
        let _ = self.on_next();
    }
    fn on_paste_xpub(&mut self) -> Task<Message> {
        clipboard::read().map(|t| {
            Self::route(match t {
                Some(xpub) => SelectKeySourceMessage::Xpub(xpub),
                None => SelectKeySourceMessage::None,
            })
        })
    }
    fn on_update_token(&mut self, token: String) -> Task<Message> {
        let token = token.trim().to_string();
        self.selected_key = SelectedKey::None;
        let value = {
            let (value, valid, warning) = match self.focus {
                Focus::EnterSafetyNetToken => (
                    &mut self.form_safety_net_token.value,
                    &mut self.form_safety_net_token.valid,
                    &mut self.form_safety_net_token.warning,
                ),
                Focus::EnterCosignerToken => (
                    &mut self.form_cosigner_token.value,
                    &mut self.form_cosigner_token.valid,
                    &mut self.form_cosigner_token.warning,
                ),
                _ => {
                    log::error!(
                        "SelectKeySource.on_update_token() call with focus on {:?}",
                        self.focus
                    );
                    return Task::none();
                }
            };
            *value = token.clone();

            if keys::token::Token::from_str(&token).is_ok() {
                // We check if the token has already been fetched and saved regardless of its kind
                *warning = if self
                    .keys
                    .iter()
                    .any(|(_, (_, k))| k.source.token() == Some(&token))
                {
                    Some("Duplicate token")
                } else {
                    None
                };
                *valid = token.is_empty() || warning.is_none();
                if !*valid {
                    return Task::none();
                }
            } else {
                *valid = value.is_empty();
                *warning = if !*valid {
                    Some("Invalid token!")
                } else {
                    None
                };
                return Task::none();
            }
            value.clone()
        };
        self.fetch_provider(value)
    }
    fn on_paste_token(&mut self) -> Task<Message> {
        clipboard::read().map(|t| {
            Self::route(match t {
                Some(token) => SelectKeySourceMessage::Token(token),
                None => SelectKeySourceMessage::None,
            })
        })
    }
    fn on_update_alias(&mut self, alias: String) -> Task<Message> {
        // We do not allow editing of existing key
        if let SelectedKey::Existing(_) = self.selected_key {
            tracing::error!(
                "SelectKeySource::on_update_alias(): alias of existing key cannot be edited"
            );
            return Task::none();
        }
        // TODO: which max length for an alias?
        self.form_alias.valid = alias.len() < 30;
        self.form_alias.value = alias;
        Task::none()
    }
    fn on_account(&mut self, index: ChildNumber) -> Task<Message> {
        self.form_account = Some(index);
        match self.focus {
            Focus::Device(fg) => Task::done(Self::route(SelectKeySourceMessage::FetchFromDevice(
                fg, index,
            ))),
            Focus::GenerateHotKey => self.on_fetch_from_hotsigner(index),
            _ => Task::none(),
        }
    }
    fn on_next(&mut self) -> Task<Message> {
        if !self.processing {
            match self.step {
                Step::Select => {
                    if let SelectedKey::Existing(_) = &self.selected_key {
                        return Task::done(Message::DefineDescriptor(
                            message::DefineDescriptor::KeysEdited(
                                self.actual_path.coordinates.clone(),
                                self.selected_key.clone(),
                            ),
                        ));
                    } else {
                        self.step = Step::Details;
                    }
                }
                Step::Details => {
                    if !self.form_alias.value.is_empty() {
                        if let SelectedKey::New(k) = &mut self.selected_key {
                            k.name = self.form_alias.value.clone();
                        }
                        return Task::done(Message::DefineDescriptor(
                            message::DefineDescriptor::KeysEdited(
                                self.actual_path.coordinates.clone(),
                                self.selected_key.clone(),
                            ),
                        ));
                    }
                }
            }
        }
        Task::none()
    }
    fn on_previous(&mut self) -> Task<Message> {
        if self.step == Step::Details {
            self.step = Step::Select;
            self.focus = Focus::None;

            self.form_safety_net_token.value = "".to_string();
            self.form_safety_net_token.valid = true;
            self.form_safety_net_token.warning = None;

            self.form_xpub.value = "".to_string();
            self.form_xpub.valid = true;
            self.form_xpub.warning = None;
        }
        Task::none()
    }
    fn on_import_message(&mut self, msg: ImportExportMessage) -> Task<Message> {
        match msg {
            ImportExportMessage::Close => {
                if self.modal.is_some() {
                    self.modal = None;
                }
            }
            ImportExportMessage::Xpub(xpub_str) => {
                if self.modal.is_some() {
                    self.modal = None;
                    return Task::perform(async move { xpub_str }, |xpub_str| {
                        Self::route(SelectKeySourceMessage::Xpub(xpub_str))
                    });
                }
            }
            m => {
                if let Some(modal) = self.modal.as_mut() {
                    return modal.update(m);
                }
            }
        }
        Task::none()
    }
    fn on_collapse(&mut self, collapse: bool) -> Task<Message> {
        self.options_collapsed = collapse;
        Task::none()
    }
    fn on_retry(&mut self) -> Task<Message> {
        self.details_error = None;
        let account = self
            .form_account
            .unwrap_or(ChildNumber::from_hardened_idx(0).expect("hardcoded"));
        match self.focus {
            Focus::Device(fg) => Task::done(Self::route(SelectKeySourceMessage::FetchFromDevice(
                fg, account,
            ))),
            Focus::GenerateHotKey => Task::done(Self::route(
                SelectKeySourceMessage::FetchFromHotSigner(account),
            )),
            _ => Task::none(),
        }
    }
    fn main_view(
        &self,
        hws: Vec<(
            String, /* alias */
            Option<Fingerprint>,
            HwState,
            bool, /* support taproot */
        )>,
    ) -> Element<Message> {
        let only_safety_net = self.actual_path.token_kind.contains(&KeyKind::SafetyNet)
            && self.actual_path.token_kind.len() == 1;

        let no_devices = (hws.is_empty() && self.keys.is_empty() && !only_safety_net)
            .then_some(self.view_no_devices());

        let devices = ((!hws.is_empty() || !self.keys.is_empty()) && !only_safety_net)
            .then_some(self.view_signing_devices(&hws));

        let keys = (!self.keys.is_empty() && !only_safety_net).then_some(self.view_keys());

        let header = modal::header(
            Some("Select key source".to_string()),
            None::<FnMsg>,
            Some(|| Message::Close),
        );

        let column = Column::new()
            .spacing(10)
            .push(header)
            .push_maybe(no_devices)
            .push_maybe(devices)
            .push_maybe(keys)
            .push(self.view_other_options())
            .align_x(Horizontal::Center)
            .width(modal::MODAL_WIDTH);
        let cont = Container::new(column).padding(15).style(theme::card::modal);
        cont.into()
    }
    fn details_view(&self) -> Element<Message> {
        let apply = match (
            &self.selected_key,
            !self.processing && self.form_alias.valid && !self.form_alias.value.is_empty(),
        ) {
            (SelectedKey::None, _) => None,
            (_, true) => Some(Self::route(SelectKeySourceMessage::Next)),
            _ => None,
        };
        let fingerprint = match self.focus {
            Focus::Key(fg) | Focus::Device(fg) => fg,
            Focus::GenerateHotKey => self.hot_signer.lock().expect("poisoned").fingerprint(),
            _ => match &self.selected_key {
                SelectedKey::Existing(fg) => *fg,
                SelectedKey::New(key) => key.fingerprint,
                SelectedKey::None => unreachable!(),
            },
        };
        let header = modal::header(
            None,
            Some(|| Self::route(SelectKeySourceMessage::Previous)),
            Some(|| Message::Close),
        );

        let accounts: Vec<_> = (0..10)
            .map(|i| {
                Account::new(
                    ChildNumber::from_hardened_idx(i).expect("hardcoded"),
                    fingerprint,
                )
            })
            .collect();
        let child = self
            .form_account
            .unwrap_or(ChildNumber::Hardened { index: 0 });
        let account = Account::new(child, fingerprint);

        let pick_enabled = !self.processing && matches!(self.focus, Focus::Device(_));

        let pick_account: Container<_> = if pick_enabled {
            container(pick_list(accounts, Some(account.clone()), move |a| {
                Self::route(SelectKeySourceMessage::Account(a.index))
            }))
        } else {
            container(p1_regular(account.to_string()))
        };
        let edit_account = matches!(self.focus, Focus::Device(_));

        let pick_account = edit_account.then_some(pick_account);

        details_view(
            header,
            pick_account,
            &self.form_alias,
            self.details_error.clone(),
            |s| Self::route(SelectKeySourceMessage::Alias(s)),
            apply,
            Some(Self::route(SelectKeySourceMessage::Retry)),
            None,
        )
    }
    fn view_no_devices(&self) -> Element<Message> {
        column![
            icon::usb_icon().size(100),
            p1_regular("Plug in a hardware device ...")
        ]
        .align_x(Horizontal::Center)
        .spacing(20)
        .into()
    }
    fn view_signing_devices(
        &self,
        hws: &Vec<(
            String, /* alias */
            Option<Fingerprint>,
            HwState,
            bool, /* support taproot */
        )>,
    ) -> Element<Message> {
        let mut col = column![p1_bold("Detected hardware")]
            .spacing(5)
            .width(modal::BTN_W);
        for hw in hws {
            col = col.push(self.widget_signing_device(hw));
        }
        if hws.is_empty() {
            col = col.push(row![
                Space::with_width(Length::Fill),
                p1_regular("- No other sources detected -"),
                Space::with_width(Length::Fill)
            ])
        }
        col.into()
    }
    fn view_keys(&self) -> Element<Message> {
        let keys = self.already_used_keys();
        let mut col = column![p1_bold("Already used sources")].spacing(5);
        for key in keys {
            col = col.push(self.widget_key(key));
        }
        col.into()
    }
    fn safety_net_enabled(&self) -> bool {
        self.actual_path.token_kind.contains(&KeyKind::SafetyNet)
    }
    fn cosigner_enabled(&self) -> bool {
        self.actual_path.token_kind.contains(&KeyKind::Cosigner)
    }
    fn view_other_options(&self) -> Element<Message> {
        let safety_net_token = self
            .safety_net_enabled()
            .then_some(self.widget_paste_safety_net_token());

        let cosigner_token = self
            .cosigner_enabled()
            .then_some(self.widget_paste_cosigner_token());

        let paste_xpub = safety_net_token
            .is_none()
            .then_some(self.widget_paste_xpub());

        let collapsed = self.options_collapsed || safety_net_token.is_some();

        let option_section = modal::optional_section(
            collapsed,
            "Other options".into(),
            || Self::route(SelectKeySourceMessage::Collapse(true)),
            || Self::route(SelectKeySourceMessage::Collapse(false)),
        );

        let hot_signer_fg = self.hot_signer.lock().expect("poisoned").fingerprint();
        let hot_signer = (!self.keys.contains_key(&hot_signer_fg) && safety_net_token.is_none())
            .then_some(self.widget_generate_hot_key());

        let load_key = safety_net_token.is_none().then_some(self.widget_load_key());

        let mut col = Column::new()
            .push(option_section)
            .spacing(modal::V_SPACING)
            .width(modal::BTN_W);
        if collapsed {
            col = col
                .push_maybe(load_key)
                .push_maybe(paste_xpub)
                .push_maybe(hot_signer)
                .push_maybe(cosigner_token)
                .push_maybe(safety_net_token);
        }
        col.into()
    }
    fn widget_signing_device(
        &self,
        device: &(
            String, /* alias */
            Option<Fingerprint>,
            HwState,
            bool, /* support taproot */
        ),
    ) -> Element<Message> {
        let alias = device.0.clone();
        let fg = device.1;
        let state = &device.2;
        let support_taproot = device.3;
        let mut enabled = true;
        let message = match (state, support_taproot, self.taproot) {
            (_, false, true) => Some("This device do not support taproot".to_string()),
            (HwState::Locked, _, _) => Some("Please unlock the device".to_string()),
            (HwState::Unsupported(ur), _, _) => {
                enabled = false;
                match ur {
                    UnsupportedReason::Version {
                        minimal_supported_version,
                    } => {
                        enabled = true;
                        Some(format!("Device version not supported, you must upgrate to version > {minimal_supported_version}"))
                    }
                    UnsupportedReason::Method(m) => {
                        Some(format!("Device not supported, method: {m}"))
                    }
                    UnsupportedReason::NotPartOfWallet(_) => None, // unreachable
                    UnsupportedReason::WrongNetwork => {
                        Some("The device is configured on wrong network".to_string())
                    }
                    UnsupportedReason::AppIsNotOpen => {
                        Some("Please open the app on device".to_string())
                    }
                }
            }
            _ => None,
        };
        enabled = enabled && fg.is_some();

        let mut msg = None;
        if enabled {
            if let Some(fg) = fg {
                msg = Some(move || Self::route(SelectKeySourceMessage::SelectDevice(fg)));
            }
        }
        let fingerprint = fg.map(|fg| format!("#{fg}"));
        modal::key_entry(None, alias, fingerprint, None, None, message, msg)
    }
    fn widget_key(
        &self,
        key: (
            KeySource,
            String, /* alias */
            Fingerprint,
            bool, /* available */
        ),
    ) -> Element<Message> {
        let (source, alias, fg, available) = key;
        let icon = match source {
            KeySource::Device(..) => icon::usb_drive_icon(),
            KeySource::HotSigner => icon::round_key_icon().color(color::RED),
            KeySource::Manual => icon::round_key_icon(),
            KeySource::Token(..) => icon::hdd_icon(),
        };
        let message = if let KeySource::Token(kind, _) = source {
            if !self.actual_path.token_kind.contains(&kind) {
                Some("Token type not allowed in this path".to_string())
            } else {
                None
            }
        } else {
            (!available).then_some("Key already used in this path".to_string())
        };
        let fg_str = format!("#{}", fg);
        let on_press = message
            .is_none()
            .then_some(move || Self::route(SelectKeySourceMessage::SelectKey(fg)));
        modal::key_entry(
            Some(icon),
            alias,
            Some(fg_str),
            None,
            None,
            message,
            on_press,
        )
    }
    fn widget_load_key(&self) -> Element<Message> {
        modal::button_entry(
            Some(icon::import_icon()),
            "Import extended public key file",
            None,
            self.import_xpub_error.clone(),
            Some(|| Self::route(SelectKeySourceMessage::SelectLoadXpub)),
        )
    }
    fn widget_generate_hot_key(&self) -> Element<Message> {
        modal::button_entry(
            Some(icon::round_key_icon().color(color::RED)),
            "Generate hot key stored on this computer",
            Some("We recommend to use this option only for test purposes"),
            None,
            Some(|| Self::route(SelectKeySourceMessage::SelectGenerateHotKey)),
        )
    }
    fn widget_paste_xpub(&self) -> Element<Message> {
        collapsible_input_button(
            self.focus == Focus::EnterXpub,
            Some(icon::paste_icon()),
            "Paste extended public key".to_string(),
            example_xpub(self.network),
            &self.form_xpub,
            Some(|xpub| Self::route(SelectKeySourceMessage::Xpub(xpub))),
            Some(|| Self::route(SelectKeySourceMessage::PasteXpub)),
            || Self::route(SelectKeySourceMessage::SelectEnterXpub),
        )
    }
    fn widget_paste_safety_net_token(&self) -> Element<Message> {
        collapsible_input_button(
            self.focus == Focus::EnterSafetyNetToken,
            Some(icon::enter_box_icon()),
            "Enter a Safety Net token".to_string(),
            "aaaa-bbbb-cccc".to_string(),
            &self.form_safety_net_token,
            Some(|token| Self::route(SelectKeySourceMessage::Token(token))),
            Some(|| Self::route(SelectKeySourceMessage::PasteToken)),
            || Self::route(SelectKeySourceMessage::SelectEnterSafetyNetToken),
        )
    }
    fn widget_paste_cosigner_token(&self) -> Element<Message> {
        collapsible_input_button(
            self.focus == Focus::EnterCosignerToken,
            Some(icon::enter_box_icon()),
            "Enter a Cosigner token".to_string(),
            "aaaa-bbbb-cccc".to_string(),
            &self.form_cosigner_token,
            Some(|token| Self::route(SelectKeySourceMessage::Token(token))),
            Some(|| Self::route(SelectKeySourceMessage::PasteToken)),
            || Self::route(SelectKeySourceMessage::SelectEnterCosignerToken),
        )
    }
}

impl super::DescriptorEditModal for SelectKeySource {
    fn processing(&self) -> bool {
        self.processing
    }
    fn update(&mut self, hws: &mut HardwareWallets, message: Message) -> Task<Message> {
        // step back if selected device disconnected
        if self.step == Step::Details {
            if let Focus::Device(fg) = self.focus {
                if !hws.list.iter().any(|d| d.fingerprint() == Some(fg)) {
                    self.step = Step::Select;
                    self.focus = Focus::None;
                    self.selected_key = SelectedKey::None;
                }
            }
        }
        match message {
            Message::ImportExport(ImportExportMessage::Close) => {
                self.modal = None;
                if self.step == Step::Select {
                    self.focus = Focus::None;
                }
                Task::none()
            }
            Message::ImportExport(ImportExportMessage::Xpub(xpub)) => {
                self.modal = None;
                self.on_import_xpub(xpub)
            }
            Message::ImportExport(iem) => {
                if let Some(modal) = &mut self.modal {
                    modal.update(iem)
                } else {
                    Task::none()
                }
            }
            Message::SelectKeySource(sksm) => match sksm {
                SelectKeySourceMessage::SelectDevice(fingerprint) => {
                    self.on_select_device(fingerprint)
                }
                SelectKeySourceMessage::FetchFromDevice(fingerprint, account) => {
                    self.on_fetch_from_device(fingerprint, account, hws)
                }
                SelectKeySourceMessage::SelectKey(fingerprint) => self.on_select_key(fingerprint),
                SelectKeySourceMessage::SelectLoadXpub => self.on_select_load_xpub(),
                SelectKeySourceMessage::LoadKey(key) => self.on_load_key(key),
                SelectKeySourceMessage::SelectEnterXpub => self.on_select_enter_xpub(),
                SelectKeySourceMessage::PasteXpub => self.on_paste_xpub(),
                SelectKeySourceMessage::Xpub(xpub) => self.on_update_xpub(xpub),
                SelectKeySourceMessage::SelectGenerateHotKey => self.on_select_generate_hot_key(),
                SelectKeySourceMessage::FetchFromHotSigner(account) => {
                    self.on_fetch_from_hotsigner(account)
                }
                SelectKeySourceMessage::SelectEnterCosignerToken => {
                    self.on_select_enter_cosigner_token()
                }
                SelectKeySourceMessage::SelectEnterSafetyNetToken => {
                    self.on_select_enter_safety_net_token()
                }
                SelectKeySourceMessage::PasteToken => self.on_paste_token(),
                SelectKeySourceMessage::Token(token) => self.on_update_token(token),
                SelectKeySourceMessage::Next => self.on_next(),
                SelectKeySourceMessage::Previous => self.on_previous(),
                SelectKeySourceMessage::Alias(alias) => self.on_update_alias(alias),
                SelectKeySourceMessage::ImportExport(msg) => self.on_import_message(msg),
                SelectKeySourceMessage::Account(index) => self.on_account(index),
                SelectKeySourceMessage::ProviderKey(key) => self.on_provider_key(key),
                SelectKeySourceMessage::Collapse(collapse) => self.on_collapse(collapse),
                SelectKeySourceMessage::Retry => self.on_retry(),
                SelectKeySourceMessage::None => Task::none(),
            },
            _ => Task::none(),
        }
    }
    fn subscription(&self, hws: &HardwareWallets) -> Subscription<Message> {
        let hw = hws.refresh().map(Message::HardwareWallets);
        if let Some(modal) = self.modal.as_ref() {
            if let Some(sub) = modal.subscription() {
                let import = sub.map(|m| {
                    Self::route(SelectKeySourceMessage::ImportExport(
                        ImportExportMessage::Progress(m),
                    ))
                });
                return Subscription::batch(vec![hw, import]);
            }
        }
        hw
    }
    fn view<'a>(&'a self, hws: &'a HardwareWallets) -> Element<'a, Message> {
        let detected_hws = self.detected_hws(hws);
        let content = match self.step {
            Step::Select => self.main_view(detected_hws),
            Step::Details => self.details_view(),
        };
        let content = Column::new()
            .push_maybe(self.error.clone().map(|e| card::error("Error", e)))
            .push(content)
            .into();
        if let Some(modal) = &self.modal {
            modal.view(content)
        } else {
            content
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn details_view<'a, Alias>(
    header: Element<'a, Message>,
    pick_account: Option<Container<'a, Message>>,
    alias: &'a form::Value<String>,
    error: Option<String>,
    alias_msg: Alias,
    apply_msg: Option<Message>,
    retry_msg: Option<Message>,
    replace_message: Option<Message>,
) -> Element<'a, Message>
where
    Alias: 'static + Fn(String) -> Message,
{
    let pick_account = pick_account
        .map(|pick_account| row![pick_account, Space::with_width(Length::Fill)].spacing(5));
    let info = "Switch account if you already uses the same hardware in other configurations";

    let error = error.clone().map(|e| p1_regular(e).color(color::ORANGE));

    let spacer = replace_message.is_some().then(|| Space::with_width(10));
    let replace_btn = replace_message.map(|m| {
        let mut btn = button::secondary(None, "Replace");
        if alias.valid {
            btn = btn.on_press(m);
        }
        btn
    });

    let btn_row = if error.is_none() {
        Row::new()
            .push(Space::with_width(Length::Fill))
            .push_maybe(replace_btn)
            .push_maybe(spacer)
            .push(button::primary(None, "Apply").on_press_maybe(apply_msg))
    } else if let Some(retry_msg) = retry_msg {
        row![
            Space::with_width(Length::Fill),
            button::primary(None, "Retry").on_press(retry_msg),
            button::secondary(None, "Apply")
        ]
        .spacing(5)
        .align_y(Vertical::Center)
    } else {
        Row::new()
            .push(Space::with_width(Length::Fill))
            .push_maybe(replace_btn)
            .push_maybe(spacer)
            .push(button::primary(None, "Apply"))
    };
    let column = Column::new()
        .spacing(5)
        .push(header)
        .push(row![
            p1_bold("Key name (alias):"),
            Space::with_width(Length::Fill)
        ])
        .push(row![
            p1_regular("Give this key a friendly name. it helps you identify it later:"),
            Space::with_width(Length::Fill)
        ])
        .push(
            container(form::Form::new("E.g. My Hardware Wallet", alias, alias_msg).padding(10))
                .width(300),
        )
        .push(Space::with_height(10))
        .push_maybe(if pick_account.is_some() {
            Some(row![p1_bold("Key path account:"), tooltip(info)].align_y(Vertical::Center))
        } else {
            None
        })
        .push_maybe(pick_account)
        .push_maybe(error)
        .push(btn_row)
        .width(410);
    card::modal(column).into()
}

#[derive(Debug, Clone)]
pub enum EditKeyAliasMessage {
    Alias(String),
    Save,
    Replace,
    DoReplace {
        path_kind: PathKind,
        coordinates: Vec<(usize, usize)>,
    },
    Close,
}

pub struct EditKeyAlias {
    fingerprint: Fingerprint,
    form_alias: form::Value<String>,
    path_kind: PathKind,
    coordinates: Vec<(usize, usize)>,
}

impl EditKeyAlias {
    pub fn new(
        fingerprint: Fingerprint,
        alias: String,
        path_kind: PathKind,
        coordinates: Vec<(usize, usize)>,
    ) -> Self {
        let form_alias = form::Value {
            value: alias,
            warning: None,
            valid: true,
        };
        Self {
            fingerprint,
            form_alias,
            path_kind,
            coordinates,
        }
    }
}

impl super::DescriptorEditModal for EditKeyAlias {
    fn update(&mut self, _hws: &mut HardwareWallets, message: Message) -> Task<Message> {
        if let Message::EditKeyAlias(msg) = message {
            match msg {
                EditKeyAliasMessage::Alias(alias) => self.form_alias.value = alias,
                EditKeyAliasMessage::Save => {
                    return Task::done(Message::DefineDescriptor(
                        message::DefineDescriptor::AliasEdited(
                            self.fingerprint,
                            self.form_alias.value.clone(),
                        ),
                    ))
                }
                EditKeyAliasMessage::Replace => {
                    return Task::done(Message::EditKeyAlias(EditKeyAliasMessage::DoReplace {
                        path_kind: self.path_kind,
                        coordinates: self.coordinates.clone(),
                    }))
                }
                EditKeyAliasMessage::DoReplace { .. } | EditKeyAliasMessage::Close => { /* unreachable  */
                }
            }
        }
        Task::none()
    }

    fn view<'a>(&'a self, _hws: &'a HardwareWallets) -> Element<'a, Message> {
        let header = modal::header(None, None::<FnMsg>, Some(|| Message::Close));
        details_view(
            header,
            None,
            &self.form_alias,
            self.form_alias.warning.map(|s| s.to_string()),
            |s| Message::EditKeyAlias(EditKeyAliasMessage::Alias(s)),
            Some(Message::EditKeyAlias(EditKeyAliasMessage::Save)),
            None,
            Some(Message::EditKeyAlias(EditKeyAliasMessage::Replace)),
        )
    }
}

pub fn default_derivation_path(network: Network) -> DerivationPath {
    // Note that "m" is ignored when parsing string and could be removed:
    // https://github.com/rust-bitcoin/rust-bitcoin/pull/2677
    DerivationPath::from_str({
        if network == Network::Bitcoin {
            "m/48'/0'/0'/2'"
        } else {
            "m/48'/1'/0'/2'"
        }
    })
    .unwrap()
}

pub fn derivation_path(network: Network, account: ChildNumber) -> DerivationPath {
    assert!(account.is_hardened());
    let network = if network == Network::Bitcoin {
        ChildNumber::Hardened { index: 0 }
    } else {
        ChildNumber::Hardened { index: 1 }
    };
    vec![
        ChildNumber::Hardened { index: 48 },
        network,
        account,
        ChildNumber::Hardened { index: 2 },
    ]
    .into()
}

/// LIANA_STANDARD_PATH: m/48'/0'/0'/2';
/// LIANA_TESTNET_STANDARD_PATH: m/48'/1'/0'/2';
pub async fn get_extended_pubkey(
    hw: std::sync::Arc<dyn async_hwi::HWI + Send + Sync>,
    fingerprint: Fingerprint,
    network: Network,
    account: ChildNumber,
) -> Result<DescriptorPublicKey, Error> {
    let derivation_path = derivation_path(network, account);
    let xkey = hw
        .get_extended_pubkey(&derivation_path)
        .await
        .map_err(Error::from)?;
    Ok(DescriptorPublicKey::XPub(DescriptorXKey {
        origin: Some((fingerprint, derivation_path)),
        derivation_path: DerivationPath::master(),
        wildcard: Wildcard::None,
        xkey,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_derivation_path() {
        assert_eq!(
            default_derivation_path(Network::Bitcoin).to_string(),
            "48'/0'/0'/2'"
        );
        assert_eq!(
            default_derivation_path(Network::Testnet).to_string(),
            "48'/1'/0'/2'"
        );
        assert_eq!(
            default_derivation_path(Network::Signet).to_string(),
            "48'/1'/0'/2'"
        );
        assert_eq!(
            default_derivation_path(Network::Regtest).to_string(),
            "48'/1'/0'/2'"
        );
    }

    #[test]
    fn test_derivation_path() {
        assert_eq!(
            derivation_path(Network::Bitcoin, ChildNumber::Hardened { index: 0 }).to_string(),
            "48'/0'/0'/2'"
        );
        assert_eq!(
            derivation_path(Network::Regtest, ChildNumber::Hardened { index: 0 }).to_string(),
            "48'/1'/0'/2'"
        );
        assert_eq!(
            derivation_path(Network::Bitcoin, ChildNumber::Hardened { index: 1 }).to_string(),
            "48'/0'/1'/2'"
        );
        assert_eq!(
            derivation_path(Network::Regtest, ChildNumber::Hardened { index: 1 }).to_string(),
            "48'/1'/1'/2'"
        );
    }

    #[test]
    #[should_panic]
    fn unhardened_derivation_path() {
        derivation_path(Network::Bitcoin, ChildNumber::Normal { index: 0 }).to_string();
    }
}
