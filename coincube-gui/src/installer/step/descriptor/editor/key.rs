use std::{
    collections::HashMap,
    str::FromStr,
    sync::{Arc, Mutex},
};

use async_hwi::{DeviceKind, Version};
use coincube_core::miniscript::{
    bitcoin::{
        bip32::{ChildNumber, DerivationPath, Fingerprint, Xpub},
        Network,
    },
    descriptor::{DerivPaths, DescriptorMultiXKey, DescriptorPublicKey, DescriptorXKey, Wildcard},
};
use iced::{
    alignment::{Horizontal, Vertical},
    clipboard,
    widget::{column, container, pick_list, row, Column, Row, Space},
    Length, Subscription, Task,
};

use coincube_ui::{
    color,
    component::{
        button, card, form,
        hw::Account,
        modal::{self, collapsible_input_button},
        text::{p1_bold, p1_regular},
        tooltip,
    },
    icon, theme,
    widget::{Button, ColumnExt, Container, Element, Text},
};

use crate::{
    app::{settings::ProviderKey, state::vault::export::VaultExportModal},
    export::{ImportExportMessage, ImportExportType},
    hw::{is_compatible_with_tapminiscript, HardwareWallet, HardwareWallets, UnsupportedReason},
    installer::{
        descriptor::{Key, KeySource, KeychainKeyOwner},
        message::{self, Message},
        Error, PathKind,
    },
    services::{
        self,
        coincube::CubeKeyRaw,
        keys::{self, api::KeyKind},
    },
    signer::Signer,
};

const MAX_ALIAS_LEN: usize = 24;

/// A `CubeKeyRaw` enriched with resolved owner identity (self vs. contact).
#[derive(Debug, Clone)]
pub struct ResolvedCubeKey {
    pub raw: CubeKeyRaw,
    pub owner: KeychainKeyOwner,
}

/// Result of fetching and resolving Cube keys.
#[derive(Debug, Clone)]
pub struct ResolvedCubeKeys {
    pub my_keys: Vec<ResolvedCubeKey>,
    pub contact_keys: Vec<ResolvedCubeKey>,
}

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

/// Top-level navigation state inside the "Select key source" modal.
///
/// After the card-grid redesign (2026-04-18) the picker is organised
/// around a 3×2 grid of key-source cards. Three cards open dedicated
/// sub-screens (`HardwareListen`, `KeychainKeys`, `PasteXpubEntry`)
/// with a Back button; the others trigger existing flows that land at
/// `Details` for alias entry.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Step {
    /// The 3×2 card grid — modal entry point.
    Grid,
    /// Sub-screen: USB prompt + detected hardware signers.
    HardwareListen,
    /// Sub-screen: My Keychain Keys + Contact Keychain Keys.
    KeychainKeys,
    /// Sub-screen: paste-an-xpub entry form.
    PasteXpubEntry,
    /// Alias-entry step after a key has been selected.
    Details,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Focus {
    None,
    Key(Fingerprint),
    Device(Fingerprint),
    EnterXpub,
    LoadXpubFromFile,
    GenerateMasterKey,
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
    SelectGenerateMasterKey,
    FetchFromMasterSigner(ChildNumber),
    SelectEnterSafetyNetToken,
    SelectEnterCosignerToken,
    SelectBorderWalletSafetyNet,
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
    // Keychain key messages
    FetchCubeKeys,
    CubeKeysLoaded(Result<ResolvedCubeKeys, String>),
    SelectKeychainKey(ResolvedCubeKey),
    // Grid sub-screen navigation (2026-04-18 redesign)
    ShowHardwareListen,
    ShowKeychainKeys,
    BackToGrid,
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
    master_signer: Arc<Mutex<Signer>>,
    /// Cube UUID for fetching Keychain keys from the API.
    cube_id: Option<String>,
    /// Authenticated coincube-api client for fetching Keychain keys.
    coincube_client: Option<crate::services::coincube::CoincubeClient>,
    /// Resolved Keychain keys owned by the current user.
    my_keychain_keys: Vec<ResolvedCubeKey>,
    /// Resolved Keychain keys owned by contacts (Keyholder role only).
    contact_keychain_keys: Vec<ResolvedCubeKey>,
    /// Whether we are currently loading Keychain keys from the API.
    keychain_keys_loading: bool,
    /// Error from the last Keychain keys fetch attempt.
    keychain_keys_error: Option<String>,
    /// Whether the initial fetch has been triggered.
    keychain_keys_fetched: bool,
    /// The currently selected key.
    selected_key: SelectedKey,
    step: Step,
    focus: Focus,
    modal: Option<VaultExportModal>,
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
        master_signer: Arc<Mutex<Signer>>,
        cube_id: Option<String>,
        coincube_client: Option<crate::services::coincube::CoincubeClient>,
    ) -> Self {
        Self {
            network,
            taproot,
            keys,
            accounts,
            actual_path,
            master_signer,
            cube_id,
            coincube_client,
            my_keychain_keys: Vec::new(),
            contact_keychain_keys: Vec::new(),
            keychain_keys_loading: false,
            keychain_keys_error: None,
            keychain_keys_fetched: false,
            selected_key: SelectedKey::None,
            step: Step::Grid,
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
            let modal = VaultExportModal::new(None, ImportExportType::ImportXpub(self.network));
            let launch = modal.launch(false);
            self.modal = Some(modal);
            return launch;
        }
        Task::none()
    }
    fn on_select_enter_xpub(&mut self) -> Task<Message> {
        self.focus = Focus::EnterXpub;
        // Card-grid redesign: route into the dedicated paste-entry
        // sub-screen (previously this just flipped a focus flag that
        // revealed a collapsible input inside the old flat layout).
        self.step = Step::PasteXpubEntry;
        self.form_xpub = form::Value {
            value: String::new(),
            valid: true,
            warning: None,
        };
        self.import_xpub_error = None;
        Task::none()
    }
    fn on_select_generate_hot_key(&mut self) -> Task<Message> {
        self.focus = Focus::GenerateMasterKey;
        let _ = self.on_next();
        self.processing = true;
        Task::done(Self::route(SelectKeySourceMessage::Account(
            ChildNumber::from_hardened_idx(0).expect("hardcoded"),
        )))
    }
    fn on_fetch_from_hotsigner(&mut self, account: ChildNumber) -> Task<Message> {
        self.processing = false;
        let fingerprint = self.master_signer.lock().unwrap().fingerprint();

        if self.keys.contains_key(&fingerprint) {
            self.selected_key = SelectedKey::Existing(fingerprint);
            return Task::none();
        }

        self.form_alias.value = "Master Signer".to_string();
        self.form_alias.valid = true;

        let derivation_path = derivation_path(self.network, account);
        let key_str = format!(
            "[{}/{}]{}",
            fingerprint,
            derivation_path.to_string().trim_start_matches("m/"),
            self.master_signer
                .lock()
                .expect("poisoned")
                .get_extended_pubkey(&derivation_path)
        );

        let key = DescriptorPublicKey::from_str(&key_str).expect("always ok");
        let key = Key {
            source: KeySource::MasterSigner,
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
        self.form_alias.warning = None;
        self.form_alias.valid = true;

        if let Some(fg) = match &self.selected_key {
            SelectedKey::None => None,
            SelectedKey::Existing(fg) => Some(*fg),
            SelectedKey::New(k) => Some(k.fingerprint),
        } {
            if alias_already_exists(&alias, fg, &self.keys) {
                self.form_alias.warning = Some("This alias is already used for another key");
                self.form_alias.valid = false;
            }
        }

        if alias.chars().count() <= MAX_ALIAS_LEN {
            self.form_alias.value = alias;
        }
        Task::none()
    }
    fn on_account(&mut self, index: ChildNumber) -> Task<Message> {
        self.form_account = Some(index);
        match self.focus {
            Focus::Device(fg) => Task::done(Self::route(SelectKeySourceMessage::FetchFromDevice(
                fg, index,
            ))),
            Focus::GenerateMasterKey => self.on_fetch_from_hotsigner(index),
            _ => Task::none(),
        }
    }
    fn on_next(&mut self) -> Task<Message> {
        if !self.processing {
            match self.step {
                // All picker-style steps (Grid, HardwareListen,
                // KeychainKeys, PasteXpubEntry) behave the same on Next:
                // if an already-placed key was chosen we forward it to
                // the descriptor, otherwise we advance to the alias-entry
                // Details step.
                Step::Grid | Step::HardwareListen | Step::KeychainKeys | Step::PasteXpubEntry => {
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
            // Pop back to the sub-screen the user came from: HW device
            // picker when the selection came from a hardware signer,
            // otherwise the main grid.
            self.step = if matches!(self.focus, Focus::Device(_)) {
                Step::HardwareListen
            } else {
                Step::Grid
            };
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
            Focus::GenerateMasterKey => Task::done(Self::route(
                SelectKeySourceMessage::FetchFromMasterSigner(account),
            )),
            _ => Task::none(),
        }
    }
    // ── Keychain key handlers ─────────────────────────────────────────

    fn on_fetch_cube_keys(&mut self) -> Task<Message> {
        let (Some(cube_id), Some(client)) = (self.cube_id.clone(), self.coincube_client.clone())
        else {
            return Task::none();
        };
        self.keychain_keys_loading = true;
        self.keychain_keys_error = None;
        self.keychain_keys_fetched = true;

        Task::perform(
            async move {
                let raw_keys = client
                    .get_cube_keys(&cube_id)
                    .await
                    .map_err(|e| e.to_string())?;
                let contacts = client.get_contacts().await.map_err(|e| e.to_string())?;
                let user = client.get_user().await.map_err(|e| e.to_string())?;
                let current_user_id: u64 = user.id.into();

                let mut my_keys = Vec::new();
                let mut contact_keys = Vec::new();

                for key in raw_keys {
                    let owner_id = key.effective_owner_user_id();
                    // Prefer the server's viewer-relative `is_own_key` when
                    // it's set; fall back to a local id comparison for
                    // pre-W3 backends where the field is always false.
                    let is_own = key.is_own_key || owner_id == current_user_id;
                    if is_own {
                        my_keys.push(ResolvedCubeKey {
                            raw: key,
                            owner: KeychainKeyOwner::SelfUser {
                                primary_owner_id: owner_id,
                            },
                        });
                    } else if let Some(contact) = contacts.iter().find(|c| {
                        // Backend's lean `ContactResponse` doesn't include
                        // a flat `contact_user_id` — fall back to the
                        // nested `contact_user.id` via the helper.
                        c.effective_contact_user_id() == Some(owner_id)
                            && c.role == crate::services::coincube::ContactRole::Keyholder
                    }) {
                        // Prefer the server-supplied `ownerEmail` when the
                        // W3 backend populated it; the contact-list lookup
                        // still runs because we need `contact_id` for the
                        // keychain-key `KeySource` enum.
                        let contact_email = if !key.owner_email.is_empty() {
                            key.owner_email.clone()
                        } else if let Some(user) = contact.contact_user.as_ref() {
                            user.email.clone()
                        } else {
                            // Contact with no linked user — render a
                            // placeholder rather than failing.
                            "unknown contact".to_string()
                        };
                        contact_keys.push(ResolvedCubeKey {
                            raw: key,
                            owner: KeychainKeyOwner::Contact {
                                primary_owner_id: owner_id,
                                contact_id: contact.id,
                                contact_email,
                            },
                        });
                    }
                    // Keys from non-Keyholder contacts are silently discarded.
                }

                Ok(ResolvedCubeKeys {
                    my_keys,
                    contact_keys,
                })
            },
            |result| Self::route(SelectKeySourceMessage::CubeKeysLoaded(result)),
        )
    }

    fn on_cube_keys_loaded(&mut self, result: Result<ResolvedCubeKeys, String>) -> Task<Message> {
        self.keychain_keys_loading = false;
        match result {
            Ok(resolved) => {
                self.my_keychain_keys = resolved.my_keys;
                self.contact_keychain_keys = resolved.contact_keys;
                self.keychain_keys_error = None;
            }
            Err(e) => {
                tracing::warn!("Failed to fetch Cube keys: {}", e);
                self.keychain_keys_error = Some(e);
            }
        }
        Task::none()
    }

    fn on_select_keychain_key(&mut self, resolved: ResolvedCubeKey) -> Task<Message> {
        let fingerprint_str = &resolved.raw.fingerprint;
        let xpub_str = &resolved.raw.xpub;
        let derivation_str = &resolved.raw.derivation_path;

        let Ok(fingerprint) = Fingerprint::from_str(fingerprint_str) else {
            self.error = Some(format!("Invalid fingerprint: {}", fingerprint_str));
            return Task::none();
        };
        let Ok(xpub) = xpub_str.parse::<Xpub>() else {
            self.error = Some(format!("Invalid xpub: {}", xpub_str));
            return Task::none();
        };
        let Ok(derivation_path) = DerivationPath::from_str(derivation_str) else {
            self.error = Some(format!("Invalid derivation path: {}", derivation_str));
            return Task::none();
        };

        let descriptor_key = DescriptorPublicKey::XPub(DescriptorXKey {
            origin: Some((fingerprint, derivation_path)),
            xkey: xpub,
            derivation_path: DerivationPath::master(),
            wildcard: Wildcard::Unhardened,
        });

        if !check_key_network(&descriptor_key, self.network) {
            self.error = Some("Key network does not match".to_string());
            return Task::none();
        }

        if self.owner_placed_elsewhere(resolved.owner.primary_owner_id(), fingerprint) {
            self.error =
                Some("This owner already has a Keychain key placed in this Vault.".to_string());
            return Task::none();
        }

        if self.keys.contains_key(&fingerprint) {
            self.selected_key = SelectedKey::Existing(fingerprint);
        } else {
            let key = Key {
                source: KeySource::KeychainKey {
                    owner: resolved.owner,
                    key_id: resolved.raw.id,
                    name: resolved.raw.name.clone(),
                },
                name: resolved.raw.name.clone(),
                fingerprint,
                key: descriptor_key,
                account: None,
            };
            self.selected_key = SelectedKey::New(Box::new(key));
        }
        self.form_alias.value = resolved.raw.name;
        self.form_alias.valid = true;
        self.focus = Focus::None;
        self.step = Step::Details;
        Task::none()
    }

    /// Whether the Keychain key sections should be shown.
    fn keychain_available(&self) -> bool {
        self.cube_id.is_some() && self.coincube_client.is_some()
    }

    /// Backstop for `on_select_keychain_key`: returns true if accepting
    /// the candidate Keychain key would violate "one Keychain key per
    /// owner per Vault".  A conflict exists when a *different* Keychain
    /// key from the same owner is placed at coordinates outside the
    /// currently-edited slot (those can't be overwritten by this
    /// selection).  Replacing the key at the currently-edited slot is
    /// allowed.
    fn owner_placed_elsewhere(
        &self,
        primary_owner_id: u64,
        candidate_fingerprint: Fingerprint,
    ) -> bool {
        self.keys.values().any(|(coords, k)| {
            if k.fingerprint == candidate_fingerprint {
                return false;
            }
            let KeySource::KeychainKey { owner, .. } = &k.source else {
                return false;
            };
            if owner.primary_owner_id() != primary_owner_id {
                return false;
            }
            coords.is_empty()
                || coords
                    .iter()
                    .any(|c| !self.actual_path.coordinates.contains(c))
        })
    }

    // ── Keychain key views ──────────────────────────────────────────

    fn view_my_keychain_keys(&self) -> Element<Message> {
        let mut col = Column::new().spacing(modal::V_SPACING).width(modal::BTN_W);
        col = col.push(p1_bold("My Keychain Keys"));

        // Treat "not yet fetched" as loading — the auto-fetch fires on
        // the first update() call, leaving a brief pre-fetch window
        // where the lists are empty without the empty state being real.
        if (!self.keychain_keys_fetched || self.keychain_keys_loading)
            && self.my_keychain_keys.is_empty()
            && self.keychain_keys_error.is_none()
        {
            col = col.push(p1_regular("Fetching Keychain keys…"));
            return col.into();
        }
        if let Some(err) = &self.keychain_keys_error {
            col = col.push(p1_regular(format!("Failed to load keys: {}", err)));
            col = col.push(
                button::secondary(Some(icon::reload_icon()), "Retry")
                    .on_press(Self::route(SelectKeySourceMessage::FetchCubeKeys)),
            );
            return col.into();
        }
        if self.my_keychain_keys.is_empty() {
            col = col.push(p1_regular(
                "No Keychain keys. Add one in the COINCUBE mobile app.",
            ));
            return col.into();
        }

        for rk in &self.my_keychain_keys {
            let owner_id = rk.raw.effective_owner_user_id();
            // Match the submit-side `on_select_keychain_key` backstop:
            // a row is "owner-blocked" only when a DIFFERENT key from
            // the same owner occupies coordinates outside the
            // currently-edited slot. Replacing the key at the active
            // slot is allowed. An unparseable fingerprint disables the
            // row defensively — the submit path would also reject it.
            let candidate_fp = Fingerprint::from_str(&rk.raw.fingerprint).ok();
            let owner_blocked = match candidate_fp {
                Some(fp) => self.owner_placed_elsewhere(owner_id, fp),
                None => true,
            };
            // W9 pre-check: reject keys that another Vault already claims.
            let used_elsewhere = rk.raw.used_by_vault;
            let disabled = owner_blocked || used_elsewhere;
            let fp_short: String = rk.raw.fingerprint.chars().take(8).collect();
            let fingerprint = Some(format!("#{}", fp_short));
            let msg = if disabled {
                None
            } else {
                let rk_clone = rk.clone();
                Some(move || {
                    Self::route(SelectKeySourceMessage::SelectKeychainKey(rk_clone.clone()))
                })
            };
            // Surface the more specific reason when both apply: once a key
            // is in another Vault, that's the blocking constraint even if
            // the owner is also placed elsewhere in this build.
            let warning = if used_elsewhere {
                Some("Used by another Vault".to_string())
            } else if owner_blocked {
                Some("Already selected".to_string())
            } else {
                None
            };
            col = col.push(modal::key_entry(
                Some(icon::round_key_icon()),
                rk.raw.name.clone(),
                fingerprint,
                None,
                None,
                warning,
                msg,
            ));
        }
        col.into()
    }

    fn view_contact_keychain_keys(&self) -> Element<Message> {
        let mut col = Column::new().spacing(modal::V_SPACING).width(modal::BTN_W);
        col = col.push(p1_bold("Contact Keychain Keys"));

        // Treat "not yet fetched" as loading (see view_my_keychain_keys).
        if (!self.keychain_keys_fetched || self.keychain_keys_loading)
            && self.contact_keychain_keys.is_empty()
            && self.keychain_keys_error.is_none()
        {
            col = col.push(p1_regular("Fetching contact keys…"));
            return col.into();
        }
        if let Some(err) = &self.keychain_keys_error {
            col = col.push(p1_regular(format!("Failed to load keys: {}", err)));
            col = col.push(
                button::secondary(Some(icon::reload_icon()), "Retry")
                    .on_press(Self::route(SelectKeySourceMessage::FetchCubeKeys)),
            );
            return col.into();
        }
        if self.contact_keychain_keys.is_empty() {
            col = col.push(p1_regular("None of your contacts have shared keys yet."));
            return col.into();
        }

        // Group keys by owner (BTreeMap for stable render order)
        let mut seen_contacts: std::collections::BTreeMap<u64, Vec<&ResolvedCubeKey>> =
            std::collections::BTreeMap::new();
        for rk in &self.contact_keychain_keys {
            seen_contacts
                .entry(rk.raw.effective_owner_user_id())
                .or_default()
                .push(rk);
        }
        for keys in seen_contacts.values() {
            if let Some(first) = keys.first() {
                let contact_label = match &first.owner {
                    KeychainKeyOwner::Contact { contact_email, .. } => {
                        format!("{} [Keyholder]", contact_email)
                    }
                    _ => "Contact".to_string(),
                };
                col = col.push(p1_bold(contact_label));
                for rk in keys {
                    let owner_id = rk.raw.effective_owner_user_id();
                    // See `view_my_keychain_keys` above — we mirror the
                    // coordinate-aware `owner_placed_elsewhere` check
                    // used by the submit-side backstop so row-disabled
                    // state matches what clicking actually rejects.
                    let candidate_fp = Fingerprint::from_str(&rk.raw.fingerprint).ok();
                    let owner_blocked = match candidate_fp {
                        Some(fp) => self.owner_placed_elsewhere(owner_id, fp),
                        None => true,
                    };
                    let used_elsewhere = rk.raw.used_by_vault;
                    let disabled = owner_blocked || used_elsewhere;
                    let fp = &rk.raw.fingerprint;
                    let fingerprint = Some(format!("#{}", &fp[..fp.len().min(8)]));
                    let msg = if disabled {
                        None
                    } else {
                        let rk_clone = (*rk).clone();
                        Some(move || {
                            Self::route(SelectKeySourceMessage::SelectKeychainKey(rk_clone.clone()))
                        })
                    };
                    let warning = if used_elsewhere {
                        Some("Used by another Vault".to_string())
                    } else if owner_blocked {
                        Some("Already selected".to_string())
                    } else {
                        None
                    };
                    col = col.push(modal::key_entry(
                        Some(icon::round_key_icon()),
                        rk.raw.name.clone(),
                        fingerprint,
                        None,
                        None,
                        warning,
                        msg,
                    ));
                }
            }
        }
        col.into()
    }

    // ── Card-grid entry point (2026-04-18 redesign) ───────────────────
    //
    // The picker is organised around a 3×2 grid of key-source cards.
    // `view_grid` is the modal's default landing screen; two cards drill
    // into `view_hardware_listen` and `view_keychain_keys_screen`, the
    // rest fire their existing selection flows directly and end up at
    // `details_view` for alias entry.

    fn view_grid(
        &self,
        // `hws` is unused on the grid screen — hardware listing lives
        // on the dedicated Hardware Device sub-screen. Kept as a
        // parameter because the view dispatcher already resolves it.
        _hws: Vec<(
            String, /* alias */
            Option<Fingerprint>,
            HwState,
            bool, /* support taproot */
        )>,
    ) -> Element<Message> {
        let only_safety_net = self.actual_path.token_kind.contains(&KeyKind::SafetyNet)
            && self.actual_path.token_kind.len() == 1;

        let header = modal::header(
            Some("Select key source".to_string()),
            Some(|| Message::Close),
        );

        // If the path is "safety-net-only" there's nothing to pick
        // *but* a safety-net token — surface just that widget and
        // skip the grid entirely (matches the pre-redesign flow).
        if only_safety_net {
            let col = Column::new()
                .spacing(10)
                .push(header)
                .push(self.widget_paste_safety_net_token())
                .align_x(Horizontal::Center)
                .width(modal::MODAL_WIDTH);
            return Container::new(col)
                .padding(15)
                .style(theme::card::modal)
                .into();
        }

        let already_used = (!self.keys.is_empty()).then(|| self.view_keys());

        // Row 1: Hardware Device · Keychain Keys · Cube Key
        let row1 = Row::new()
            .spacing(modal::V_SPACING)
            .push(self.view_card(
                icon::usb_icon(),
                "Hardware Device",
                Some(
                    "Use a plugged-in hardware signer. Supported: Ledger Nano S/S+/X, \
                     Coldcard Mk3/Mk4/Q, Jade, BitBox02, Trezor, Specter-DIY.",
                ),
                Some(SelectKeySourceMessage::ShowHardwareListen),
            ))
            .push({
                let enabled = self.keychain_available();
                let on_press = enabled.then_some(SelectKeySourceMessage::ShowKeychainKeys);
                let tip: Option<&str> = if enabled {
                    None
                } else {
                    Some("Sign in to Connect to use Keychain keys.")
                };
                self.view_card(icon::key_icon(), "Keychain Keys", tip, on_press)
            })
            .push({
                // "Cube Key" replaces the old Developer-mode "Generate
                // hot key" button. Disabled when the master signer is
                // already placed in the vault (matches the old gate).
                let master_fg = self.master_signer.lock().expect("poisoned").fingerprint();
                let enabled = !self.keys.contains_key(&master_fg);
                let on_press = enabled.then_some(SelectKeySourceMessage::SelectGenerateMasterKey);
                let tip = if enabled {
                    Some(
                        "A key generated on this device, stored either encrypted with your \
                         Cube PIN or derived from a Passkey.",
                    )
                } else {
                    Some("Your Cube Key is already placed in this vault.")
                };
                self.view_card(icon::cube_icon(), "Cube Key", tip, on_press)
            });

        // Row 2: Border Wallet · Import xpub · Paste xpub
        let row2 = Row::new()
            .spacing(modal::V_SPACING)
            .push(self.view_card(
                // Placeholder icon — replace with a grid-pattern SVG in
                // a follow-up.
                icon::grid_icon(),
                "Border Wallet",
                Some(
                    "A deterministic key derived from a Border Wallet Grid Generation Seed — \
                     a visual 2048-cell grid you memorise or back up. The Grid Generation Seed \
                     itself is derived from your encrypted local seed or Passkey.",
                ),
                Some(SelectKeySourceMessage::SelectBorderWalletSafetyNet),
            ))
            .push(self.view_card(
                icon::import_icon(),
                "Import xpub File",
                None,
                Some(SelectKeySourceMessage::SelectLoadXpub),
            ))
            .push(self.view_card(
                icon::clipboard_icon(),
                "Paste xpub",
                None,
                Some(SelectKeySourceMessage::SelectEnterXpub),
            ));

        let advanced = self.view_advanced_options();

        let column = Column::new()
            .spacing(10)
            .push(header)
            .push(already_used)
            .push(row1)
            .push(row2)
            .push(advanced)
            .align_x(Horizontal::Center)
            .width(modal::MODAL_WIDTH);
        Container::new(column)
            .padding(15)
            .style(theme::card::modal)
            .into()
    }

    /// Render a single grid card.
    ///
    /// Visuals:
    ///   * Active — theme-aware card fill, muted border, primary text.
    ///   * Hovered / pressed — border and text (+ inherited icon colour)
    ///     flip to `ORANGE`. Fill stays flat (no "orange flood" on
    ///     hover).
    ///   * Disabled (`on_press == None`) — same fill, muted `text.secondary`
    ///     border and text so the shape is still legible but clearly
    ///     non-interactive.
    ///
    /// Colours are read from the active theme so light mode gets the
    /// warm-paper card tone and a soft taupe border automatically.
    fn view_card<'a>(
        &'a self,
        icon_el: Text<'static>,
        title: &'static str,
        tooltip_copy: Option<&'static str>,
        on_press: Option<SelectKeySourceMessage>,
    ) -> Element<'a, Message> {
        let enabled = on_press.is_some();
        let icon_size: f32 = 42.0;

        // Top row — pushes the ⓘ icon to the right when present.
        let tip_row: Element<Message> = if let Some(copy) = tooltip_copy {
            Row::new()
                .push(Space::new().width(Length::Fill))
                .push(tooltip::tooltip(copy))
                .into()
        } else {
            Space::new().height(Length::Fixed(18.0)).into()
        };

        // Icon + title: no explicit colour — they inherit the button's
        // `text_color`, which flips to ORANGE on hover and to the
        // theme's secondary (muted) text colour when disabled.
        let inner = Column::new()
            .push(tip_row)
            .push(Space::new().height(Length::Fill))
            .push(icon_el.size(icon_size))
            .push(Space::new().height(Length::Fixed(8.0)))
            .push(p1_bold(title.to_string()))
            .push(Space::new().height(Length::Fill))
            .align_x(Horizontal::Center)
            .width(Length::Fill)
            .height(Length::Fill);

        let mut btn: Button<'a, Message> = Button::new(inner)
            .width(Length::FillPortion(1))
            .height(Length::Fixed(170.0))
            .padding(12)
            .style(move |theme: &theme::Theme, status| {
                use iced::widget::button::Status;
                let bg = theme.colors.cards.simple.background;
                // `cards.border.border` is the palette entry that's
                // guaranteed-visible in both themes (GREY_7 dark /
                // LIGHT_BORDER light); `cards.simple.border` is
                // transparent by design.
                let default_border = theme.colors.cards.border.border.unwrap_or(color::GREY_3);
                let (border_color, text_color) = if !enabled {
                    // `text.secondary` is actually quite bright in dark
                    // mode (`GREY_2 = #CCCCCC`), which makes the
                    // disabled card read as "selected". Use `GREY_3`
                    // (#717171) for both border and text — a true
                    // midtone that looks muted on the dark card
                    // (lighter than bg, darker than primary text) and
                    // still-legible-but-clearly-muted on the light
                    // card.
                    (color::GREY_3, color::GREY_3)
                } else {
                    match status {
                        Status::Hovered | Status::Pressed => (color::ORANGE, color::ORANGE),
                        Status::Active | Status::Disabled => {
                            (default_border, theme.colors.text.primary)
                        }
                    }
                };
                iced::widget::button::Style {
                    background: Some(iced::Background::Color(bg)),
                    text_color,
                    border: iced::Border {
                        color: border_color,
                        width: 1.5,
                        radius: 16.0.into(),
                    },
                    ..Default::default()
                }
            });
        if let Some(msg) = on_press {
            btn = btn.on_press(Self::route(msg));
        }
        btn.into()
    }

    fn view_hardware_listen(
        &self,
        hws: Vec<(
            String, /* alias */
            Option<Fingerprint>,
            HwState,
            bool, /* support taproot */
        )>,
    ) -> Element<Message> {
        let header = modal::header(Some("Hardware Device".to_string()), Some(|| Message::Close));

        let listening = column![
            icon::usb_icon().size(100),
            p1_regular("Plug in a hardware device ..."),
        ]
        .align_x(Horizontal::Center)
        .spacing(20);

        // Reuse the existing detected-devices rendering.
        let devices =
            (!hws.is_empty() || !self.keys.is_empty()).then(|| self.view_signing_devices(&hws));
        let already_used = (!self.keys.is_empty()).then(|| self.view_keys());

        // Footer Back button — left-aligned, standalone (no primary
        // action on this screen; the user picks a device from the list
        // above to proceed).
        let footer = Row::new()
            .push(modal::back_button(|| {
                Self::route(SelectKeySourceMessage::BackToGrid)
            }))
            .push(Space::new().width(Length::Fill))
            .align_y(Vertical::Center);

        let column = Column::new()
            .spacing(20)
            .push(header)
            .push(listening)
            .push(devices)
            .push(already_used)
            .push(footer)
            .align_x(Horizontal::Center)
            .width(modal::MODAL_WIDTH);
        Container::new(column)
            .padding(15)
            .style(theme::card::modal)
            .into()
    }

    fn view_keychain_keys_screen(&self) -> Element<Message> {
        let header = modal::header(Some("Keychain Keys".to_string()), Some(|| Message::Close));

        let footer = Row::new()
            .push(modal::back_button(|| {
                Self::route(SelectKeySourceMessage::BackToGrid)
            }))
            .push(Space::new().width(Length::Fill))
            .align_y(Vertical::Center);

        let column = Column::new()
            .spacing(15)
            .push(header)
            .push(self.view_my_keychain_keys())
            .push(self.view_contact_keychain_keys())
            .push(footer)
            .align_x(Horizontal::Center)
            .width(modal::MODAL_WIDTH);
        Container::new(column)
            .padding(15)
            .style(theme::card::modal)
            .into()
    }

    /// Dedicated paste-xpub sub-screen, opened from the grid's "Paste
    /// xpub" card. On successful parse, `on_update_xpub` advances the
    /// modal to `Step::Details` for alias entry. Parse errors stay on
    /// this screen with `import_xpub_error` rendered below the input.
    fn view_paste_xpub_screen(&self) -> Element<Message> {
        let header = modal::header(Some("Paste xpub".to_string()), Some(|| Message::Close));

        let input = iced::widget::TextInput::new("xpub…", &self.form_xpub.value)
            .on_input(|s| Self::route(SelectKeySourceMessage::Xpub(s)))
            .on_submit(Self::route(SelectKeySourceMessage::Xpub(
                self.form_xpub.value.clone(),
            )))
            .size(16)
            .padding(12);

        let paste_btn = button::secondary(Some(icon::paste_icon()), "Paste")
            .on_press(Self::route(SelectKeySourceMessage::PasteXpub));

        let error = self
            .import_xpub_error
            .as_ref()
            .map(|e| p1_regular(e.clone()).color(color::RED));

        // Footer: [Back] [spacer] [Paste]. Paste is the primary action
        // on this screen (it pulls clipboard contents into the text
        // input and kicks the parse).
        let footer = Row::new()
            .push(modal::back_button(|| {
                Self::route(SelectKeySourceMessage::BackToGrid)
            }))
            .push(Space::new().width(Length::Fill))
            .push(paste_btn)
            .align_y(Vertical::Center);

        let column = Column::new()
            .spacing(12)
            .push(header)
            .push(p1_regular(
                "Paste an extended public key (xpub) to add it as a signer.",
            ))
            .push(input)
            .push_maybe(error)
            .push(footer)
            .align_x(Horizontal::Center)
            .width(modal::MODAL_WIDTH);

        Container::new(column)
            .padding(15)
            .style(theme::card::modal)
            .into()
    }

    /// Conditional section rendered below the grid when the current
    /// descriptor path enables safety-net or cosigner tokens. The rest
    /// of the old `view_other_options` content moved into the grid.
    fn view_advanced_options(&self) -> Option<Element<Message>> {
        if !self.safety_net_enabled() && !self.cosigner_enabled() {
            return None;
        }

        let header = modal::optional_section(
            self.options_collapsed,
            "Advanced options".into(),
            || Self::route(SelectKeySourceMessage::Collapse(true)),
            || Self::route(SelectKeySourceMessage::Collapse(false)),
        );

        let mut col = Column::new()
            .push(header)
            .spacing(modal::V_SPACING)
            .width(modal::BTN_W);

        if self.options_collapsed {
            if self.safety_net_enabled() {
                col = col.push(self.widget_paste_safety_net_token());
            }
            if self.cosigner_enabled() {
                col = col.push(self.widget_paste_cosigner_token());
            }
        }
        Some(col.into())
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
            Focus::GenerateMasterKey => self.master_signer.lock().expect("poisoned").fingerprint(),
            _ => match &self.selected_key {
                SelectedKey::Existing(fg) => *fg,
                SelectedKey::New(key) => key.fingerprint,
                SelectedKey::None => unreachable!(),
            },
        };
        let header = modal::header(None, Some(|| Message::Close));

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
            Some(Self::route(SelectKeySourceMessage::Previous)),
        )
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
                Space::new().width(Length::Fill),
                p1_regular("- No other sources detected -"),
                Space::new().width(Length::Fill)
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
                        Some(format!("Device version not supported, upgrade to version > {minimal_supported_version}"))
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
        modal::key_entry(
            Some(icon::usb_drive_icon()),
            alias,
            fingerprint,
            None,
            None,
            message,
            msg,
        )
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
            KeySource::MasterSigner => icon::round_key_icon().color(color::RED),
            KeySource::Manual => icon::round_key_icon(),
            KeySource::Token(..) => icon::hdd_icon(),
            KeySource::BorderWallet => icon::round_key_icon(),
            KeySource::KeychainKey { .. } => icon::round_key_icon(),
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
        // step back if selected device disconnected — pop back into
        // the HW listening sub-screen rather than all the way to the
        // grid so the user sees "Plug in a hardware device…" again.
        if self.step == Step::Details {
            if let Focus::Device(fg) = self.focus {
                if !hws.list.iter().any(|d| d.fingerprint() == Some(fg)) {
                    self.step = Step::HardwareListen;
                    self.focus = Focus::None;
                    self.selected_key = SelectedKey::None;
                }
            }
        }
        match message {
            Message::ImportExport(ImportExportMessage::Close) => {
                self.modal = None;
                if self.step == Step::Grid {
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
                SelectKeySourceMessage::SelectGenerateMasterKey => {
                    // Card-grid redesign: "Cube Key" is a first-class
                    // user-facing card (previously gated behind
                    // `developer_mode`). The hot-key payload is stored
                    // encrypted with the Cube PIN or derived from a
                    // Passkey; see the card's info tooltip.
                    self.on_select_generate_hot_key()
                }
                SelectKeySourceMessage::FetchFromMasterSigner(account) => {
                    self.on_fetch_from_hotsigner(account)
                }
                SelectKeySourceMessage::SelectEnterCosignerToken => {
                    self.on_select_enter_cosigner_token()
                }
                SelectKeySourceMessage::SelectEnterSafetyNetToken => {
                    self.on_select_enter_safety_net_token()
                }
                SelectKeySourceMessage::SelectBorderWalletSafetyNet => {
                    // Emit message to DefineDescriptor to swap modal to BorderWalletWizard
                    Task::done(Message::DefineDescriptor(
                        message::DefineDescriptor::OpenBorderWalletWizard(
                            self.actual_path.coordinates.clone(),
                        ),
                    ))
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
                SelectKeySourceMessage::FetchCubeKeys => self.on_fetch_cube_keys(),
                SelectKeySourceMessage::CubeKeysLoaded(result) => self.on_cube_keys_loaded(result),
                SelectKeySourceMessage::SelectKeychainKey(resolved) => {
                    self.on_select_keychain_key(resolved)
                }
                SelectKeySourceMessage::ShowHardwareListen => {
                    self.step = Step::HardwareListen;
                    Task::none()
                }
                SelectKeySourceMessage::ShowKeychainKeys => {
                    self.step = Step::KeychainKeys;
                    // Lazy fetch: only hit the API the first time the
                    // user opens this sub-screen. Replaces the old
                    // always-on trigger that fired on every `update()`.
                    if !self.keychain_keys_fetched && self.keychain_available() {
                        self.on_fetch_cube_keys()
                    } else {
                        Task::none()
                    }
                }
                SelectKeySourceMessage::BackToGrid => {
                    self.step = Step::Grid;
                    self.focus = Focus::None;
                    self.error = None;
                    self.details_error = None;
                    self.import_xpub_error = None;
                    Task::none()
                }
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
            Step::Grid => self.view_grid(detected_hws),
            Step::HardwareListen => self.view_hardware_listen(detected_hws),
            Step::KeychainKeys => self.view_keychain_keys_screen(),
            Step::PasteXpubEntry => self.view_paste_xpub_screen(),
            Step::Details => self.details_view(),
        };
        let content = Column::new()
            .push(self.error.clone().map(|e| card::error("Error", e)))
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
    // Optional Back-button message. When present, a standard
    // `modal::back_button` is rendered at the left of the footer row
    // next to the Apply/Retry primary action. Pass `None` when the
    // modal has no back navigation.
    back_msg: Option<Message>,
) -> Element<'a, Message>
where
    Alias: 'static + Fn(String) -> Message,
{
    let pick_account = pick_account
        .map(|pick_account| row![pick_account, Space::new().width(Length::Fill)].spacing(5));
    let info = "Switch account if you already uses the same hardware in other configurations";

    let error = error.clone().map(|e| p1_regular(e).color(color::ORANGE));

    let spacer = replace_message.is_some().then(|| Space::new().width(10));
    let replace_btn = replace_message.map(|m| {
        let mut btn = button::secondary(None, "Replace");
        if alias.valid {
            btn = btn.on_press(m);
        }
        btn
    });

    // Optional left-aligned Back button shared across all three btn_row
    // layouts. When absent the row layout is unchanged (spacer + Apply).
    let back = back_msg.map(|m| modal::back_button(move || m.clone()));

    let btn_row = if error.is_none() {
        Row::new()
            .push(back)
            .push(Space::new().width(Length::Fill))
            .push(replace_btn)
            .push(spacer)
            .push(button::primary(None, "Apply").on_press_maybe(apply_msg))
            .align_y(Vertical::Center)
    } else if let Some(retry_msg) = retry_msg {
        Row::new()
            .push(back)
            .push(Space::new().width(Length::Fill))
            .push(button::primary(None, "Retry").on_press(retry_msg))
            .push(Space::new().width(5))
            .push(button::secondary(None, "Apply"))
            .spacing(5)
            .align_y(Vertical::Center)
    } else {
        Row::new()
            .push(back)
            .push(Space::new().width(Length::Fill))
            .push(replace_btn)
            .push(spacer)
            .push(button::primary(None, "Apply"))
            .align_y(Vertical::Center)
    };
    let column = Column::new()
        .spacing(5)
        .push(header)
        .push(row![
            p1_bold("Key name (alias):"),
            Space::new().width(Length::Fill)
        ])
        .push(row![
            p1_regular("Give this key a friendly name. It will help you identify it later:"),
            Space::new().width(Length::Fill)
        ])
        .push(
            container(form::Form::new("E.g. My Hardware Wallet", alias, alias_msg).padding(10))
                .width(300),
        )
        .push(Space::new().height(10))
        .push(if pick_account.is_some() {
            Some(row![p1_bold("Key path account:"), tooltip(info)].align_y(Vertical::Center))
        } else {
            None
        })
        .push(pick_account)
        .push(error)
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
    keys: HashMap<Fingerprint, (Vec<(usize, usize)>, Key)>,
    fingerprint: Fingerprint,
    form_alias: form::Value<String>,
    path_kind: PathKind,
    coordinates: Vec<(usize, usize)>,
}

impl EditKeyAlias {
    pub fn new(
        keys: HashMap<Fingerprint, (Vec<(usize, usize)>, Key)>,
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
            keys,
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
                EditKeyAliasMessage::Alias(alias) => {
                    self.form_alias.warning = None;
                    self.form_alias.valid = true;

                    if alias_already_exists(&alias, self.fingerprint, &self.keys) {
                        self.form_alias.warning =
                            Some("This alias is already used for another key");
                        self.form_alias.valid = false;
                    }
                    if alias.chars().count() <= MAX_ALIAS_LEN {
                        self.form_alias.value = alias
                    }
                }
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
        let header = modal::header(None, Some(|| Message::Close));
        details_view(
            header,
            None,
            &self.form_alias,
            None,
            |s| Message::EditKeyAlias(EditKeyAliasMessage::Alias(s)),
            Some(Message::EditKeyAlias(EditKeyAliasMessage::Save)),
            None,
            Some(Message::EditKeyAlias(EditKeyAliasMessage::Replace)),
            None, // no Back — edit-alias modal only has an X
        )
    }
}

#[allow(clippy::type_complexity)]
fn alias_already_exists(
    alias: &str,
    fingerprint: Fingerprint,
    keys: &HashMap<Fingerprint, (Vec<(usize, usize)>, Key)>,
) -> bool {
    for (fg, (_, key)) in keys {
        if *fg != fingerprint && alias == key.name {
            return true;
        }
    }
    false
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

/// COINCUBE_STANDARD_PATH: m/48'/0'/0'/2';
/// COINCUBE_TESTNET_STANDARD_PATH: m/48'/1'/0'/2';
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
    use crate::utils::default_derivation_path;

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
            default_derivation_path(Network::Testnet4).to_string(),
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

    // ── Card-grid redesign state tests (2026-04-18) ──────────────────
    //
    // These exercise `Step` transitions driven by the three new
    // navigation messages (`ShowHardwareListen`, `ShowKeychainKeys`,
    // `BackToGrid`) on a freshly-constructed `SelectKeySource`. We
    // ignore the returned `Task<Message>` — the state transitions are
    // synchronous and what the tests care about.

    use super::super::DescriptorEditModal;
    use crate::dir::CoincubeDirectory;
    use crate::hw::HardwareWallets;
    use crate::signer::Signer;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::str::FromStr;
    use std::sync::{Arc, Mutex};

    fn empty_picker() -> SelectKeySource {
        SelectKeySource::new(
            Network::Signet,
            false,
            PathData {
                coordinates: vec![],
                keys: vec![],
                token_kind: vec![],
            },
            HashMap::new(),
            HashMap::new(),
            Arc::new(Mutex::new(Signer::generate(Network::Signet).unwrap())),
            None,
            None,
        )
    }

    fn sandbox_hws() -> HardwareWallets {
        HardwareWallets::new(
            CoincubeDirectory::new(PathBuf::from_str("/").unwrap()),
            Network::Bitcoin,
        )
    }

    #[test]
    fn default_step_is_grid() {
        let picker = empty_picker();
        assert_eq!(picker.step, Step::Grid);
    }

    #[test]
    fn show_hardware_listen_transitions_step() {
        let mut picker = empty_picker();
        let mut hws = sandbox_hws();
        let _ = picker.update(
            &mut hws,
            SelectKeySource::route(SelectKeySourceMessage::ShowHardwareListen),
        );
        assert_eq!(picker.step, Step::HardwareListen);
    }

    #[test]
    fn show_keychain_keys_transitions_step() {
        let mut picker = empty_picker();
        let mut hws = sandbox_hws();
        let _ = picker.update(
            &mut hws,
            SelectKeySource::route(SelectKeySourceMessage::ShowKeychainKeys),
        );
        assert_eq!(picker.step, Step::KeychainKeys);
    }

    #[test]
    fn back_to_grid_returns_and_clears_errors() {
        let mut picker = empty_picker();
        let mut hws = sandbox_hws();

        // Jump into a sub-screen and pre-populate some error state so
        // we can prove `BackToGrid` clears it.
        picker.step = Step::HardwareListen;
        picker.focus = Focus::EnterXpub;
        picker.error = Some("stale".to_string());
        picker.details_error = Some("stale".to_string());
        picker.import_xpub_error = Some("stale".to_string());

        let _ = picker.update(
            &mut hws,
            SelectKeySource::route(SelectKeySourceMessage::BackToGrid),
        );
        assert_eq!(picker.step, Step::Grid);
        assert_eq!(picker.focus, Focus::None);
        assert!(picker.error.is_none());
        assert!(picker.details_error.is_none());
        assert!(picker.import_xpub_error.is_none());
    }

    #[test]
    fn keychain_fetch_not_triggered_on_construction() {
        // Pre-redesign regression: the fetch used to fire on the first
        // `update()` call regardless of user action. After the
        // redesign it only fires when the Keychain sub-screen is
        // opened.
        let picker = empty_picker();
        assert!(!picker.keychain_keys_fetched);
        assert!(!picker.keychain_keys_loading);
    }

    #[test]
    fn keychain_fetch_skipped_when_connect_unavailable() {
        // No `coincube_client` / `cube_id` on the picker — Connect
        // isn't signed in. ShowKeychainKeys should still flip the
        // step but must not attempt the fetch.
        let mut picker = empty_picker();
        let mut hws = sandbox_hws();
        let _ = picker.update(
            &mut hws,
            SelectKeySource::route(SelectKeySourceMessage::ShowKeychainKeys),
        );
        assert_eq!(picker.step, Step::KeychainKeys);
        assert!(!picker.keychain_keys_fetched);
    }
}
