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
        coincube::{classify_cube_key_ownership, Contact, CubeKeyOwnership, CubeKeyRaw},
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
    /// Connect's numeric id for this Cube, resolved in the same round-trip.
    /// Bound into every xpub envelope's AAD, so a blinded key can't be opened
    /// without it (PR D3). `None` when the lookup failed — blinded keys then
    /// report as unreadable rather than being silently skipped.
    pub server_cube_id: Option<u64>,
}

/// Splits a Cube's key list into the viewer's own keys and its contacts' keys.
///
/// Pure so the classification is unit-testable independently of the API calls
/// in [`SelectKeySource::on_fetch_cube_keys`], which is its only caller.
fn resolve_cube_keys(
    raw_keys: Vec<CubeKeyRaw>,
    contacts: &[Contact],
    current_user_id: u64,
) -> ResolvedCubeKeys {
    let mut my_keys = Vec::new();
    let mut contact_keys = Vec::new();

    for key in raw_keys {
        // Shared classification (identity-only contact match, never on
        // `ContactRole`) lives in `services::coincube` so this picker and the
        // sign-flow reconcile can't drift apart. See
        // [`classify_cube_key_ownership`] for the full rationale.
        let ownership = classify_cube_key_ownership(&key, contacts, current_user_id);
        match ownership {
            CubeKeyOwnership::SelfOwned { owner_id } => {
                my_keys.push(ResolvedCubeKey {
                    raw: key,
                    owner: KeychainKeyOwner::SelfUser {
                        primary_owner_id: owner_id,
                    },
                });
            }
            CubeKeyOwnership::ContactOwned { owner_id, contact } => {
                // Prefer the server-supplied `ownerEmail` when the W3 backend
                // populated it; the contact match still ran because we need
                // `contact_id` for the keychain-key `KeySource` enum.
                let contact_email = if !key.owner_email.is_empty() {
                    key.owner_email.clone()
                } else if let Some(user) = contact.contact_user.as_ref() {
                    user.email.clone()
                } else {
                    // Contact with no linked user — render a placeholder rather
                    // than failing.
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
            // Owner is neither us nor any contact of ours: we'd have no
            // `contact_id` to address them with and `AddVaultMember` would
            // reject the key. Only reachable for a viewer who is not the Cube
            // owner, and only the owner builds a Vault.
            CubeKeyOwnership::Unresolved { .. } => continue,
        }
    }

    ResolvedCubeKeys {
        server_cube_id: None,
        my_keys,
        contact_keys,
    }
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

/// One device the hardware poller has surfaced, reduced to what the key
/// picker renders: display alias, fingerprint (when the device reported one),
/// connection state, whether it can sign tap-miniscript, and any firmware
/// advisory covering it.
pub type DetectedHw = (
    String, /* alias */
    Option<Fingerprint>,
    HwState,
    bool, /* support taproot */
    Option<crate::hw_advisory::AdvisoryHit>,
);

pub enum HwState {
    Supported,
    Locked { pairing_code: Option<String> },
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
    /// This Cube's Connect-blinding encryption key — what opens the xpub
    /// envelopes Connect serves in place of plaintext keys (PR D3). `None` on a
    /// fresh install or watch-only restore, in which case blinded keys can't be
    /// selected and say so.
    cube_encryption_key:
        Option<std::sync::Arc<crate::services::connect::crypto::CubeEncryptionKey>>,
    /// Connect's **numeric** id for this Cube, resolved alongside the key
    /// fetch. Required because it is bound into each envelope's AAD; without it
    /// a blinded key can't be opened at all.
    cube_server_id: Option<u64>,
    /// Resolved Keychain keys owned by the current user.
    my_keychain_keys: Vec<ResolvedCubeKey>,
    /// Resolved Keychain keys owned by contacts.
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
    /// Firmware advisory for the signer an imported xpub file came from. Set
    /// while the file is parsed and kept after the import modal closes, so the
    /// notice survives onto the key-details screen where the user finishes
    /// adding the key. Never blocks the import.
    import_advisory: Option<&'static crate::hw_advisory::Advisory>,

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
        cube_encryption_key: Option<
            std::sync::Arc<crate::services::connect::crypto::CubeEncryptionKey>,
        >,
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
            cube_encryption_key,
            cube_server_id: None,
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
            import_advisory: None,
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
        Option<String>, /* why it can't be picked, if it can't */
    )> {
        self.keys
            .iter()
            .map(|(fg, (_, key))| {
                let unavailable = self.key_unavailable_reason(*fg, &key.source);
                (key.source.clone(), key.name.clone(), *fg, unavailable)
            })
            .collect()
    }
    /// Why an entry of the "already used sources" list can't be picked for
    /// the slot being edited, or `None` when it can. Single source of truth
    /// for both the row's disabled state and the caption explaining it —
    /// deriving those separately let them drift apart.
    fn key_unavailable_reason(&self, fg: Fingerprint, source: &KeySource) -> Option<String> {
        if let KeySource::Token(kind, _) = source {
            if !self.actual_path.token_kind.contains(kind) {
                return Some("Token type not allowed in this path".to_string());
            }
        }
        if self.actual_path.keys.iter().any(|key_fg| key_fg == &fg) {
            return Some("Key already used in this path".to_string());
        }
        // "One Keychain key per owner per Vault" is a Keychain-only rule, so
        // only Keychain keys are barred from a second spending path. Every
        // other source may legitimately be reused across paths — the
        // expanding-multisig inheritance template is built on exactly that,
        // mirroring the primary keys into the 2-of-6 recovery path.
        if matches!(source, KeySource::KeychainKey { .. }) && self.key_placed_elsewhere(fg) {
            return Some("This Keychain key is already used elsewhere in this Vault.".to_string());
        }
        None
    }
    fn detected_hws(&self, hws: &HardwareWallets) -> Vec<DetectedHw> {
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
                        HardwareWallet::Locked {
                            kind, pairing_code, ..
                        } => (
                            kind.to_string(),
                            None,
                            HwState::Locked {
                                pairing_code: pairing_code.clone(),
                            },
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

                    Some((
                        out.0,
                        out.1,
                        out.2,
                        out.3,
                        crate::hw_advisory::view::hit(hw),
                    ))
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
        self.import_advisory = None;
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
        self.import_advisory = None;
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

            // The advisory belongs to the key the file import produced. Leaving
            // Details abandons that key, so the notice goes with it — otherwise
            // it would follow the user onto whatever source they pick next,
            // which may not be a file at all.
            self.import_advisory = None;

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
            // Arrives just before `Xpub`, and is handled whether or not the
            // modal is still up so the notice can't be lost in the handover.
            ImportExportMessage::DeviceAdvisory(kind) => {
                self.import_advisory = crate::hw_advisory::evaluate_file_import(&kind);
                if let Some(modal) = self.modal.as_mut() {
                    return modal.update(ImportExportMessage::DeviceAdvisory(kind));
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

                // The picker addresses this Cube by UUID, but an xpub envelope
                // is bound to the *numeric* Connect id (PR D3). Resolve it here
                // rather than plumbing it through every installer entry point.
                // A failure isn't fatal to the fetch: plaintext keys still work
                // and blinded ones surface a clear per-key error.
                let server_cube_id = match client.list_cubes().await {
                    Ok(cubes) => cubes.iter().find(|c| c.uuid == cube_id).map(|c| c.id),
                    Err(e) => {
                        tracing::warn!("Failed to resolve Connect cube id for {cube_id}: {e}");
                        None
                    }
                };

                let mut resolved = resolve_cube_keys(raw_keys, &contacts, current_user_id);
                resolved.server_cube_id = server_cube_id;
                Ok(resolved)
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
                self.cube_server_id = resolved.server_cube_id;
                self.keychain_keys_error = None;
            }
            Err(e) => {
                tracing::warn!("Failed to fetch Cube keys: {}", e);
                self.keychain_keys_error = Some(e);
            }
        }
        Task::none()
    }

    /// Reports an unreadable Keychain key to Connect so its owner gets a
    /// re-enrol prompt (api PR A4).
    ///
    /// Fire-and-forget: the builder has already shown the user why this key
    /// can't be used, and a failed report just means the owner finds out at the
    /// next attempt. [`KeyResolveError::should_report_invalid`] filters out
    /// local conditions — a device that simply can't read envelopes must not
    /// invalidate other people's keys — and `report_reason` maps the failure to
    /// the API's closed reason set.
    ///
    /// Needs the numeric Cube id: the endpoint is Cube-scoped so an owner can
    /// only flag keys attached to their own Cube. Without it (the id lookup
    /// failed) the report is skipped rather than guessed at.
    fn report_envelope_invalid(
        &self,
        resolved: &ResolvedCubeKey,
        error: &crate::services::connect::crypto::KeyResolveError,
    ) -> Task<Message> {
        if !error.should_report_invalid() {
            return Task::none();
        }
        let (Some(client), Some(cube_id)) = (self.coincube_client.clone(), self.cube_server_id)
        else {
            return Task::none();
        };
        let key_id = resolved.raw.id;
        let reason = error.report_reason();
        Task::future(async move {
            if let Err(e) = client
                .report_key_envelope_invalid(cube_id, key_id, reason)
                .await
            {
                tracing::warn!("Failed to report key {key_id} as envelope_invalid: {e}");
            }
        })
        .discard()
    }

    fn on_select_keychain_key(&mut self, resolved: ResolvedCubeKey) -> Task<Message> {
        // I2 backstop: an owner-self recovery key restores this Cube but must
        // never be a Vault signer. The row is rendered disabled, but view state
        // can lag a re-fetch, so refuse it here too — sealing a descriptor with
        // it would only be rejected by the server's I2 guard later (PR 3).
        if resolved.raw.is_owner_self_recovery() {
            self.error = Some(
                "This is a recovery key — it restores this Cube but can never be a Vault signer."
                    .to_string(),
            );
            return Task::none();
        }
        // Already reported unopenable (api PR A4): the server dropped the stale
        // ciphertext along with the flag, so there is nothing left to decrypt.
        // Short-circuit before `resolve_key_xpub` so we don't re-report a
        // failure this owner has already reported — the keyholder has one
        // re-enrol prompt pending, and a second adds nothing.
        if resolved.raw.is_envelope_invalid() {
            self.error = Some(format!(
                "“{}” is waiting to be re-shared. Its owner has been asked to share it again \
                 from their Keychain app.",
                resolved.raw.name
            ));
            return Task::none();
        }
        let fingerprint_str = &resolved.raw.fingerprint;
        let derivation_str = &resolved.raw.derivation_path;

        let Ok(fingerprint) = Fingerprint::from_str(fingerprint_str) else {
            self.error = Some(format!("Invalid fingerprint: {}", fingerprint_str));
            return Task::none();
        };
        // Connect blinding (PR D3): the key arrives as an envelope sealed to
        // this Cube, so this is where it gets opened — and where the format /
        // network / fingerprint validation the server used to do now runs. A
        // legacy plaintext row takes the same path and the same checks.
        // Everything below (descriptor assembly, quorum rules) is unchanged: it
        // operates on the decrypted xpub.
        let xpub = match crate::services::connect::crypto::resolve_key_xpub(
            &resolved.raw,
            self.cube_encryption_key.as_deref(),
            // No numeric cube id means we can't rebuild the envelope AAD.
            // Passing a sentinel would "fail closed" only by luck, so treat it
            // as the fetch problem it is — 0 is never a real Connect cube id,
            // so a blinded key deterministically fails the tag check here.
            self.cube_server_id.unwrap_or(0),
            self.network,
        ) {
            Ok(xpub) => xpub,
            Err(e) => {
                tracing::warn!(
                    "Keychain key {} ({}) could not be resolved: {}",
                    resolved.raw.id,
                    resolved.raw.name,
                    e
                );
                self.error = Some(e.user_message(&resolved.raw.name));
                // Tell Connect the envelope is unusable so it can push a
                // re-enrol prompt to the key's owner (api PR A4). Local
                // conditions (this device simply can't read it) are excluded.
                return self.report_envelope_invalid(&resolved, &e);
            }
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

        // Check exact-key reuse before owner reuse so this backstop's error
        // matches the row-warning priority in the views (which surface
        // "already used in this Vault" ahead of "already selected" when both
        // an identical key and another key from the same owner are placed).
        if self.key_placed_elsewhere(fingerprint) {
            self.error =
                Some("This Keychain key is already used elsewhere in this Vault.".to_string());
            return Task::none();
        }

        if self.owner_placed_elsewhere(resolved.owner.primary_owner_id(), fingerprint) {
            self.error =
                Some("This owner already has a Keychain key placed in this Vault.".to_string());
            return Task::none();
        }

        // Fingerprint alone doesn't uniquely identify a Keychain key —
        // two distinct keys can share a master fingerprint if they come
        // from the same seed. Compare `key_id` to tell whether
        // this is genuinely the same key already placed,
        // vs. a different key that happens to collide on fingerprint.
        let existing_same_key = self.keys.get(&fingerprint).is_some_and(|(_, k)| {
            matches!(&k.source, KeySource::KeychainKey { key_id, .. } if *key_id == resolved.raw.id)
        });

        if existing_same_key {
            self.selected_key = SelectedKey::Existing(fingerprint);
        } else if self.keys.contains_key(&fingerprint) {
            // A different key (Keychain or otherwise) already occupies this
            // fingerprint slot. Reject instead of letting a `SelectedKey::New`
            // reach the KeysEdited handler and silently overwrite it.
            self.error = Some(
                "A different key with the same master fingerprint is already in this Vault."
                    .to_string(),
            );
            return Task::none();
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
            coords
                .iter()
                .any(|c| !self.actual_path.coordinates.contains(c))
        })
    }

    /// Companion to `owner_placed_elsewhere`: returns true when *this
    /// exact* key is already placed at coordinates outside the
    /// currently-edited slot, i.e. it's already used elsewhere in this
    /// Vault's quorum. Selecting it again would put one key at two
    /// positions, so we disable the row. Re-selecting the key at the
    /// active slot stays allowed — its coordinates are the current slot,
    /// so this returns false.
    fn key_placed_elsewhere(&self, candidate_fingerprint: Fingerprint) -> bool {
        self.keys.values().any(|(coords, k)| {
            if k.fingerprint != candidate_fingerprint {
                return false;
            }
            coords
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
            // Disable a key that's already placed in this quorum so it
            // can't be selected into a second slot.
            let key_reused = candidate_fp.is_some_and(|fp| self.key_placed_elsewhere(fp));
            // W9 pre-check: reject keys that another Vault already claims.
            let used_elsewhere = rk.raw.used_by_vault;
            // I2: the owner-self recovery key restores this Cube but is never a
            // signer. Show it — it teaches the model — but disabled, and let
            // its caption win over the reuse/selection reasons below.
            let is_recovery = rk.raw.is_owner_self_recovery();
            let disabled = is_recovery || owner_blocked || key_reused || used_elsewhere;
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
            // Surface the most specific reason when several apply: the recovery
            // caption first (it's the whole point of the row), then a key
            // claimed by another Vault, then an exact reuse in this quorum,
            // then the owner being placed elsewhere.
            let warning = if is_recovery {
                Some("Recovery key — restores this Cube, never signs".to_string())
            } else if used_elsewhere {
                Some("Used by another Vault".to_string())
            } else if key_reused {
                Some("Already used in this Vault".to_string())
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
                    // No "[Keyholder]" suffix: the contact row's role is not a
                    // per-cube fact (see `on_fetch_cube_keys`), so labelling
                    // every contact key with it was misleading.
                    KeychainKeyOwner::Contact { contact_email, .. } => contact_email.clone(),
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
                    let key_reused = candidate_fp.is_some_and(|fp| self.key_placed_elsewhere(fp));
                    let used_elsewhere = rk.raw.used_by_vault;
                    let disabled = owner_blocked || key_reused || used_elsewhere;
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
                    } else if key_reused {
                        Some("Already used in this Vault".to_string())
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
        _hws: Vec<DetectedHw>,
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

    fn view_hardware_listen(&self, hws: Vec<DetectedHw>) -> Element<Message> {
        let header = modal::header(Some("Hardware Device".to_string()), Some(|| Message::Close));

        // Present the "waiting for a device" prompt as its own full-width
        // card so it reads as a peer of the device rows below rather than a
        // loose icon floating above them.
        let listening = Container::new(
            column![
                icon::usb_icon().size(60),
                p1_regular("Plug in a hardware device ..."),
            ]
            .align_x(Horizontal::Center)
            .width(Length::Fill)
            .spacing(15),
        )
        .width(Length::Fill)
        .padding(25)
        .style(theme::card::border);

        // Reuse the existing detected-devices rendering. Gated on `hws`
        // alone: this screen no longer carries the "Already used sources"
        // section, and with no device plugged in the listening prompt above
        // already is the empty state.
        let devices = (!hws.is_empty()).then(|| self.view_signing_devices(&hws));

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

        // Parse errors live in two places depending on entry path:
        //   * `form_xpub.warning` — set by `on_update_xpub` for manual
        //     paste errors ("Invalid Xpub", "Wrong network", "Wrong
        //     derivation path", "Origin missing", "Key already used").
        //   * `import_xpub_error` — set by `on_import_xpub` on the
        //     file-picker path.
        // Surface whichever is present; prefer the inline manual-paste
        // warning because it reflects the user's latest action.
        let error_text: Option<String> = self
            .form_xpub
            .warning
            .map(|w| w.to_string())
            .or_else(|| self.import_xpub_error.clone());
        let error = error_text.map(|e| p1_regular(e).color(color::RED));

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

        let details = details_view(
            header,
            pick_account,
            &self.form_alias,
            self.details_error.clone(),
            |s| Self::route(SelectKeySourceMessage::Alias(s)),
            apply,
            Some(Self::route(SelectKeySourceMessage::Retry)),
            None,
            Some(Self::route(SelectKeySourceMessage::Previous)),
        );

        // Advisory for a key that arrived from a signer's export file. It sits
        // above the details card rather than inside it — the key is already
        // parsed and Apply stays enabled; this is something to act on later,
        // not a condition on finishing here.
        match self.import_advisory {
            Some(advisory) => Column::new()
                .spacing(10)
                .push(
                    Container::new(coincube_ui::component::hw::advisory_panel(
                        advisory.headline,
                        None,
                        advisory.file_import,
                        advisory.guide_label,
                        Some(Message::OpenUrl(advisory.url.to_string())),
                        None,
                    ))
                    .width(modal::MODAL_WIDTH),
                )
                .push(details)
                .align_x(Horizontal::Center)
                .into(),
            None => details,
        }
    }
    fn view_signing_devices(&self, hws: &[DetectedHw]) -> Element<Message> {
        // Full width so the heading and device rows sit flush-left rather
        // than shrinking to the button width and being centred by the
        // parent modal column's `align_x(Center)`.
        let mut col = column![p1_bold("Detected hardware")]
            .spacing(5)
            .width(Length::Fill);
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
        // Full width so the section sits flush-left with the card grid below,
        // rather than shrinking to the button width and being centred by the
        // parent modal column's `align_x(Center)`.
        let mut col = column![p1_bold("Already used sources")]
            .spacing(5)
            .width(Length::Fill);
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
    fn widget_signing_device(&self, device: &DetectedHw) -> Element<Message> {
        let alias = device.0.clone();
        let fg = device.1;
        let state = &device.2;
        let support_taproot = device.3;
        let mut enabled = true;
        let message = match (state, support_taproot, self.taproot) {
            (HwState::Locked { pairing_code }, _, _) => Some(match pairing_code {
                Some(code) => format!("Pairing code: {code}"),
                None => "Please unlock the device".to_string(),
            }),
            (_, false, true) => Some("This device does not support taproot".to_string()),
            (HwState::Unsupported(ur), _, _) => {
                enabled = false;
                match ur {
                    UnsupportedReason::Version {
                        minimal_supported_version,
                        note,
                    } => {
                        enabled = true;
                        let mut msg = format!(
                            "Device version not supported, upgrade to version > {minimal_supported_version}"
                        );
                        if let Some(note) = note {
                            msg.push_str(". ");
                            msg.push_str(note);
                        }
                        Some(msg)
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
        let entry = modal::key_entry(
            Some(icon::usb_drive_icon()),
            alias,
            fingerprint,
            None,
            None,
            message,
            msg,
        );
        // Firmware advisory, if this device has one. It sits under the entry
        // and changes nothing about it: `enabled` above is untouched, so a
        // flagged device is picked exactly like any other.
        match &device.4 {
            Some(hit) => Column::new()
                .push(entry)
                .push(crate::hw_advisory::view::section(
                    hit,
                    fg,
                    Message::OpenUrl(hit.url().to_string()),
                    None,
                ))
                .into(),
            None => entry,
        }
    }
    fn widget_key(
        &self,
        key: (
            KeySource,
            String, /* alias */
            Fingerprint,
            Option<String>, /* why it can't be picked, if it can't */
        ),
    ) -> Element<Message> {
        let (source, alias, fg, message) = key;
        let icon = match source {
            KeySource::Device(..) => icon::usb_drive_icon(),
            KeySource::MasterSigner => icon::round_key_icon().color(color::RED),
            KeySource::Manual => icon::round_key_icon(),
            KeySource::Token(..) => icon::hdd_icon(),
            KeySource::BorderWallet { .. } => icon::round_key_icon(),
            KeySource::KeychainKey { .. } => icon::round_key_icon(),
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

    const TESTNET_XPUB: &str = "tpubD6NzVbkrYhZ4XHQ1pLJ7pdpEGWCVbSUEaUakxnrtENzaZaDp4vL6gBgGH7n983ZPgsVe5G2JEAM2oYZkEPCNrfo9XLq8nHFhp9GzFjGc1uQ";
    /// A real BIP-48 testnet **account** xpub (`m/48'/1'/0'/2'`, depth 4) —
    /// the shape Connect actually serves for a Keychain key, and consistent
    /// with `raw_key`'s declared `derivation_path`. Distinct from
    /// [`TESTNET_XPUB`] above, which is a depth-0 master.
    const TESTNET_ACCOUNT_XPUB: &str = "tpubDFH9dgzveyD8zTbPUFuLrGmCydNvxehyNdUXKJAQN8x4aZ4j6UZqGfnqFrD4NqyaTVGKbvEW54tsvPTK2UoSbCC1PJY8iCNiwTL3RWZEheQ";
    const TESTNET_DESCRIPTOR_KEY: &str = "[8a550171/48'/1'/0'/2']tpubD6NzVbkrYhZ4XHQ1pLJ7pdpEGWCVbSUEaUakxnrtENzaZaDp4vL6gBgGH7n983ZPgsVe5G2JEAM2oYZkEPCNrfo9XLq8nHFhp9GzFjGc1uQ";
    const MAINNET_DESCRIPTOR_KEY: &str = "[abcdef01/48'/0'/0'/2']xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW";

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

    // ── key_placed_elsewhere: exact-key reuse across quorum slots ─────
    //
    // `key_placed_elsewhere` disables selecting a key already placed at
    // coordinates outside the currently-edited slot, so the same key
    // can't be used twice in a quorum — while still allowing re-selecting
    // the key at the active slot. `keys` is `Fingerprint -> (coords, Key)`
    // and `actual_path.coordinates` is the slot being edited.

    // Any parseable descriptor key works — `key_placed_elsewhere` only
    // reads `Key::fingerprint`, never the descriptor key itself.
    fn manual_key(fingerprint: Fingerprint) -> Key {
        let key = DescriptorPublicKey::from_str(
            "tpubD6NzVbkrYhZ4XHQ1pLJ7pdpEGWCVbSUEaUakxnrtENzaZaDp4vL6gBgGH7n983ZPgsVe5G2JEAM2oYZkEPCNrfo9XLq8nHFhp9GzFjGc1uQ",
        )
        .unwrap();
        Key {
            source: KeySource::Manual,
            name: "test".to_string(),
            fingerprint,
            key,
            account: None,
        }
    }

    fn picker_with_placed_key(
        active: Vec<(usize, usize)>,
        placed_at: Vec<(usize, usize)>,
        placed_fg: Fingerprint,
    ) -> SelectKeySource {
        let mut picker = empty_picker();
        picker.actual_path.coordinates = active;
        picker
            .keys
            .insert(placed_fg, (placed_at, manual_key(placed_fg)));
        picker
    }

    #[test]
    fn key_placed_elsewhere_blocks_reuse_at_other_slot() {
        let fg = Fingerprint::from_str("8a550171").unwrap();
        // Editing slot (0,0); the same key is already placed at (1,0).
        let picker = picker_with_placed_key(vec![(0, 0)], vec![(1, 0)], fg);
        assert!(picker.key_placed_elsewhere(fg));
    }

    #[test]
    fn key_placed_elsewhere_allows_reselect_at_active_slot() {
        let fg = Fingerprint::from_str("8a550171").unwrap();
        // The key is placed only at the slot currently being edited.
        let picker = picker_with_placed_key(vec![(0, 0)], vec![(0, 0)], fg);
        assert!(!picker.key_placed_elsewhere(fg));
    }

    #[test]
    fn key_placed_elsewhere_false_for_unplaced_key() {
        let placed = Fingerprint::from_str("8a550171").unwrap();
        let other = Fingerprint::from_str("c658b283").unwrap();
        let picker = picker_with_placed_key(vec![(0, 0)], vec![(1, 0)], placed);
        // A different, not-yet-placed key is free to select.
        assert!(!picker.key_placed_elsewhere(other));
    }

    #[test]
    fn key_placed_elsewhere_blocks_when_spanning_active_and_other_slot() {
        let fg = Fingerprint::from_str("8a550171").unwrap();
        // Placed at the active slot AND another — the other placement blocks.
        let picker = picker_with_placed_key(vec![(0, 0)], vec![(0, 0), (1, 0)], fg);
        assert!(picker.key_placed_elsewhere(fg));
    }

    #[test]
    fn key_placed_elsewhere_allows_unplaced_key() {
        let fg = Fingerprint::from_str("8a550171").unwrap();
        // A key with no coordinates has not been placed, so it remains available.
        let picker = picker_with_placed_key(vec![(0, 0)], vec![], fg);
        assert!(!picker.key_placed_elsewhere(fg));
    }

    // ── key_unavailable_reason: what the "already used sources" rows say ──

    #[test]
    fn a_key_already_in_the_edited_path_is_blocked_whatever_its_source() {
        let fg = Fingerprint::from_str("8a550171").unwrap();
        let mut picker = picker_with_placed_key(vec![(0, 0)], vec![(0, 0)], fg);
        picker.actual_path.keys = vec![fg];
        assert_eq!(
            picker.key_unavailable_reason(fg, &KeySource::Manual),
            Some("Key already used in this path".to_string())
        );
    }

    #[test]
    fn a_non_keychain_key_may_be_reused_in_another_path() {
        // Reuse across spending paths is a supported descriptor shape — the
        // expanding-multisig inheritance template mirrors the primary keys
        // into its recovery path — so only Keychain keys are barred from it.
        let fg = Fingerprint::from_str("8a550171").unwrap();
        // Placed at (1, 0); we're editing (0, 0), a slot in another path.
        let picker = picker_with_placed_key(vec![(0, 0)], vec![(1, 0)], fg);
        assert!(picker.key_placed_elsewhere(fg));
        for source in [
            KeySource::Manual,
            KeySource::MasterSigner,
            KeySource::BorderWallet {
                grid_seed_source: crate::app::settings::GridSeedSource::Independent,
            },
        ] {
            assert_eq!(picker.key_unavailable_reason(fg, &source), None);
        }
    }

    #[test]
    fn a_keychain_key_placed_in_another_path_is_blocked() {
        let fg = Fingerprint::from_str("8a550171").unwrap();
        let mut picker = picker_with_placed_key(vec![(0, 0)], vec![(1, 0)], fg);
        let source = keychain_key(fg, 42, 7).source;
        picker
            .keys
            .insert(fg, (vec![(1, 0)], keychain_key(fg, 42, 7)));
        assert_eq!(
            picker.key_unavailable_reason(fg, &source),
            Some("This Keychain key is already used elsewhere in this Vault.".to_string())
        );
    }

    #[test]
    fn a_token_kind_the_path_forbids_outranks_the_reuse_reasons() {
        let fg = Fingerprint::from_str("8a550171").unwrap();
        let picker = picker_with_placed_key(vec![(0, 0)], vec![], fg);
        // `actual_path.token_kind` is empty, so no token kind is allowed here.
        let token = KeySource::Token(
            KeyKind::Cosigner,
            ProviderKey {
                uuid: "uuid".to_string(),
                token: "token".to_string(),
                provider: crate::app::settings::Provider {
                    uuid: "provider-uuid".to_string(),
                    name: "Provider".to_string(),
                },
            },
        );
        assert_eq!(
            picker.key_unavailable_reason(fg, &token),
            Some("Token type not allowed in this path".to_string())
        );
    }

    fn raw_key(id: u64, fingerprint: &str, owner_id: u64) -> CubeKeyRaw {
        CubeKeyRaw {
            xpub_envelope: None,
            id,
            name: format!("Key {id}"),
            xpub: TESTNET_ACCOUNT_XPUB.to_string(),
            fingerprint: fingerprint.to_string(),
            derivation_path: "m/48'/1'/0'/2'".to_string(),
            network: "signet".to_string(),
            status: "active".to_string(),
            primary_owner_id: owner_id,
            keychain_id: Some(1),
            curve: "secp256k1".to_string(),
            taproot: true,
            cube_id: 1,
            created_at: "2026-07-20T00:00:00Z".to_string(),
            updated_at: "2026-07-20T00:00:00Z".to_string(),
            owner_user_id: owner_id,
            owner_email: format!("owner{owner_id}@example.com"),
            is_own_key: true,
            used_by_vault: false,
            recovery_role: String::new(),
        }
    }

    fn resolved_key(id: u64, fingerprint: &str, owner_id: u64) -> ResolvedCubeKey {
        ResolvedCubeKey {
            raw: raw_key(id, fingerprint, owner_id),
            owner: KeychainKeyOwner::SelfUser {
                primary_owner_id: owner_id,
            },
        }
    }

    /// A resolved owner-self recovery key — the `recoveryRole: "owner-self"`
    /// annotation the API stamps on the Cube's recovery recipient (PR 2).
    fn resolved_recovery_key(id: u64, fingerprint: &str, owner_id: u64) -> ResolvedCubeKey {
        let mut rk = resolved_key(id, fingerprint, owner_id);
        rk.raw.recovery_role = "owner-self".to_string();
        rk
    }

    fn keychain_key(fingerprint: Fingerprint, owner_id: u64, key_id: u64) -> Key {
        let mut key = manual_key(fingerprint);
        key.source = KeySource::KeychainKey {
            owner: KeychainKeyOwner::SelfUser {
                primary_owner_id: owner_id,
            },
            key_id,
            name: format!("Key {key_id}"),
        };
        key
    }

    #[test]
    fn selected_key_fingerprint_and_network_checks_cover_key_shapes() {
        assert!(SelectedKey::None.fingerprint().is_none());
        let fg = Fingerprint::from_str("8a550171").unwrap();
        assert_eq!(SelectedKey::Existing(fg).fingerprint(), Some(fg));
        assert_eq!(
            SelectedKey::New(Box::new(manual_key(fg))).fingerprint(),
            Some(fg)
        );

        let testnet_key = DescriptorPublicKey::from_str(TESTNET_DESCRIPTOR_KEY).unwrap();
        assert!(check_key_network(&testnet_key, Network::Signet));
        assert!(!check_key_network(&testnet_key, Network::Bitcoin));

        let mainnet_key = DescriptorPublicKey::from_str(MAINNET_DESCRIPTOR_KEY).unwrap();
        assert!(check_key_network(&mainnet_key, Network::Bitcoin));
        assert!(!check_key_network(&mainnet_key, Network::Regtest));

        let DescriptorPublicKey::XPub(xpub) = testnet_key else {
            panic!("fixture should be a descriptor xpub");
        };
        let multixkey = new_multixkey_from_xpub(xpub, 3);
        assert_eq!(
            multixkey
                .derivation_paths
                .paths()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            vec!["6".to_string(), "7".to_string()]
        );
        assert_eq!(multixkey.wildcard, Wildcard::Unhardened);
    }

    #[test]
    fn xpub_entry_validates_empty_invalid_origin_network_duplicate_and_imported_keys() {
        let mut picker = empty_picker();

        let _ = picker.on_select_enter_xpub();
        assert_eq!(picker.step, Step::PasteXpubEntry);
        assert!(picker.form_xpub.valid);

        let _ = picker.on_update_xpub(String::new());
        assert!(picker.form_xpub.valid);

        let _ = picker.on_update_xpub("not an xpub".to_string());
        assert!(!picker.form_xpub.valid);
        assert_eq!(picker.form_xpub.warning, Some("Invalid Xpub"));

        let _ = picker.on_update_xpub(TESTNET_XPUB.to_string());
        assert!(!picker.form_xpub.valid);
        assert_eq!(picker.form_xpub.warning, Some("Origin missing"));

        let _ = picker.on_update_xpub(format!("{TESTNET_DESCRIPTOR_KEY}/0/*"));
        assert!(!picker.form_xpub.valid);
        assert_eq!(picker.form_xpub.warning, Some("Wrong derivation path"));

        picker.network = Network::Bitcoin;
        let _ = picker.on_update_xpub(TESTNET_DESCRIPTOR_KEY.to_string());
        assert!(!picker.form_xpub.valid);
        assert_eq!(picker.form_xpub.warning, Some("Wrong network"));

        picker.network = Network::Signet;
        let fg = Fingerprint::from_str("8a550171").unwrap();
        picker.keys.insert(fg, (vec![(0, 0)], manual_key(fg)));
        let _ = picker.on_update_xpub(TESTNET_DESCRIPTOR_KEY.to_string());
        assert!(!picker.form_xpub.valid);
        assert_eq!(picker.form_xpub.warning, Some("Key already used"));

        picker.keys.clear();
        let _ = picker.on_update_xpub(TESTNET_DESCRIPTOR_KEY.to_string());
        assert!(picker.form_xpub.valid);
        assert!(matches!(picker.selected_key, SelectedKey::New(_)));
        assert_eq!(picker.step, Step::Details);

        picker.step = Step::Grid;
        picker.selected_key = SelectedKey::None;
        picker.keys.insert(fg, (vec![(0, 0)], manual_key(fg)));
        let _ = picker.on_import_xpub(TESTNET_DESCRIPTOR_KEY.to_string());
        assert_eq!(
            picker.import_xpub_error.as_deref(),
            Some("Imported key already used")
        );
        assert_eq!(picker.focus, Focus::None);
    }

    #[test]
    fn alias_previous_next_retry_and_collapse_state_are_synchronous() {
        let fg = Fingerprint::from_str("8a550171").unwrap();
        let mut picker = picker_with_placed_key(vec![(0, 0)], vec![(1, 0)], fg);
        let new_fg = Fingerprint::from_str("c658b283").unwrap();
        picker.selected_key = SelectedKey::New(Box::new(manual_key(new_fg)));
        picker.form_alias.value = "Existing".to_string();
        picker.keys.get_mut(&fg).unwrap().1.name = "Existing".to_string();

        let _ = picker.on_update_alias("Existing".to_string());
        assert!(!picker.form_alias.valid);
        assert_eq!(
            picker.form_alias.warning,
            Some("This alias is already used for another key")
        );

        let long_alias = "abcdefghijklmnopqrstuvwxyz".to_string();
        let before = picker.form_alias.value.clone();
        let _ = picker.on_update_alias(long_alias);
        assert_eq!(picker.form_alias.value, before);

        let _ = picker.on_update_alias("Fresh".to_string());
        assert!(picker.form_alias.valid);
        assert_eq!(picker.form_alias.value, "Fresh");

        picker.step = Step::Details;
        picker.focus = Focus::Device(fg);
        picker.form_xpub.value = "stale".to_string();
        picker.form_xpub.valid = false;
        picker.form_safety_net_token.value = "stale".to_string();
        picker.form_safety_net_token.valid = false;
        let _ = picker.on_previous();
        assert_eq!(picker.step, Step::HardwareListen);
        assert_eq!(picker.focus, Focus::None);
        assert!(picker.form_xpub.value.is_empty());
        assert!(picker.form_xpub.valid);
        assert!(picker.form_safety_net_token.value.is_empty());
        assert!(picker.form_safety_net_token.valid);

        let _ = picker.on_collapse(true);
        assert!(picker.options_collapsed);
        picker.focus = Focus::GenerateMasterKey;
        picker.form_account = Some(ChildNumber::from_hardened_idx(7).unwrap());
        let _ = picker.on_retry();
        assert!(picker.details_error.is_none());
    }

    #[test]
    fn import_advisory_does_not_outlive_the_details_step() {
        let mut picker = empty_picker();

        // An xpub file exported from a Coldcard: the advisory is set while the
        // file is parsed and deliberately outlives the import modal, so it is
        // still there on the details screen.
        let _ = picker.on_import_message(ImportExportMessage::DeviceAdvisory(DeviceKind::Coldcard));
        assert!(picker.import_advisory.is_some());
        picker.step = Step::Details;

        // Back: the imported key is abandoned, and its advisory with it.
        let _ = picker.on_previous();
        assert_eq!(picker.step, Step::Grid);
        assert!(picker.import_advisory.is_none());

        // A source that isn't a file must not inherit the file-import notice —
        // a connected device carries its own, tiered on the firmware it reports.
        let fg = Fingerprint::from_str("8a550171").unwrap();
        let _ = picker.on_select_device(fg);
        assert_eq!(picker.step, Step::Details);
        assert!(picker.import_advisory.is_none());
    }

    #[test]
    fn keychain_key_selection_accepts_valid_keys_and_rejects_conflicts() {
        let mut picker = empty_picker();
        let fg = Fingerprint::from_str("8a550171").unwrap();

        let _ = picker.on_select_keychain_key(resolved_key(1, "8a550171", 7));
        assert!(matches!(picker.selected_key, SelectedKey::New(_)));
        assert_eq!(picker.form_alias.value, "Key 1");
        assert_eq!(picker.step, Step::Details);

        let mut invalid = resolved_key(2, "not-hex", 7);
        let _ = picker.on_select_keychain_key(invalid.clone());
        assert_eq!(
            picker.error.as_deref(),
            Some("Invalid fingerprint: not-hex")
        );

        // Under Connect blinding the xpub is resolved (and validated) by
        // `resolve_key_xpub`, so an unusable key surfaces the shared re-share
        // wording rather than a raw parse error.
        invalid.raw.fingerprint = "8a550171".to_string();
        invalid.raw.xpub = "not-xpub".to_string();
        let _ = picker.on_select_keychain_key(invalid.clone());
        assert_eq!(
            picker.error.as_deref(),
            Some(
                "\u{201c}Key 2\u{201d} couldn't be read and needs re-sharing. Ask its owner to \
                 re-share the key from their Keychain app."
            )
        );

        invalid.raw.xpub = TESTNET_ACCOUNT_XPUB.to_string();
        invalid.raw.derivation_path = "not-path".to_string();
        let _ = picker.on_select_keychain_key(invalid);
        assert_eq!(
            picker.error.as_deref(),
            Some("Invalid derivation path: not-path")
        );

        let mut bitcoin_picker = empty_picker();
        bitcoin_picker.network = Network::Bitcoin;
        let _ = bitcoin_picker.on_select_keychain_key(resolved_key(3, "8a550171", 7));
        // The network check now runs inside `resolve_key_xpub` — one place for
        // it, whether the key arrived blinded or as legacy plaintext.
        assert_eq!(
            bitcoin_picker.error.as_deref(),
            Some("\u{201c}Key 3\u{201d} is for a different Bitcoin network.")
        );

        let mut elsewhere_picker = picker_with_placed_key(vec![(0, 0)], vec![(1, 0)], fg);
        let _ = elsewhere_picker.on_select_keychain_key(resolved_key(4, "8a550171", 7));
        assert_eq!(
            elsewhere_picker.error.as_deref(),
            Some("This Keychain key is already used elsewhere in this Vault.")
        );

        let owner_fg = Fingerprint::from_str("c658b283").unwrap();
        let mut owner_picker = empty_picker();
        owner_picker.actual_path.coordinates = vec![(0, 0)];
        owner_picker
            .keys
            .insert(owner_fg, (vec![(1, 0)], keychain_key(owner_fg, 42, 99)));
        let _ = owner_picker.on_select_keychain_key(resolved_key(5, "8a550171", 42));
        assert_eq!(
            owner_picker.error.as_deref(),
            Some("This owner already has a Keychain key placed in this Vault.")
        );

        let mut collision_picker = empty_picker();
        collision_picker.actual_path.coordinates = vec![(0, 0)];
        collision_picker
            .keys
            .insert(fg, (vec![(0, 0)], keychain_key(fg, 7, 99)));
        let _ = collision_picker.on_select_keychain_key(resolved_key(6, "8a550171", 7));
        assert_eq!(
            collision_picker.error.as_deref(),
            Some("A different key with the same master fingerprint is already in this Vault.")
        );
    }

    #[test]
    fn owner_self_recovery_key_selection_is_a_no_op() {
        // I2: an owner-self recovery key restores the Cube but can never be a
        // Vault signer. The view renders it disabled; the submit-side backstop
        // must refuse it too — an otherwise-valid key (right network, no
        // conflicts) is rejected purely on the recovery annotation, with no
        // selection made and no step advance.
        let mut picker = empty_picker();
        let _ = picker.on_select_keychain_key(resolved_recovery_key(1, "8a550171", 7));
        assert!(
            matches!(picker.selected_key, SelectedKey::None),
            "a recovery key must not be selected"
        );
        assert_eq!(
            picker.step,
            Step::Grid,
            "selection must not advance the step"
        );
        assert_eq!(
            picker.error.as_deref(),
            Some("This is a recovery key — it restores this Cube but can never be a Vault signer.")
        );

        // The same key without the annotation is accepted — proving the refusal
        // keys off `recoveryRole` alone, not some other property of the fixture.
        let mut ok_picker = empty_picker();
        let _ = ok_picker.on_select_keychain_key(resolved_key(1, "8a550171", 7));
        assert!(matches!(ok_picker.selected_key, SelectedKey::New(_)));
        assert_eq!(ok_picker.step, Step::Details);
    }

    // ── Connect blinding: the builder opens envelopes (PR D3) ────────
    //
    // The key picker is where a Contact's blinded xpub is decrypted. These
    // pin the two outcomes that matter: a good envelope selects exactly as a
    // plaintext key used to, and an unopenable one lands in the "needs
    // re-sharing" state instead of crashing or silently selecting nothing.

    /// The `SPEC-cube-xpub-envelope-v1` §8 vector — `TESTNET_ACCOUNT_XPUB`
    /// sealed to the Cube key derived from the vector mnemonic, cube id 42.
    const BLIND_MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
    const BLIND_E: &str = "032c0b7cf95324a07d05398b240174dc0c2be444d96b159aa6c7f7b1e668680991";
    const BLIND_NONCE: &str = "0000000000000000cafebabe";
    const BLIND_CT: &str = "fc13b1b9639e00e163b3664b62f516ad49d7f19c5383a758706ca813fa8e236cf14a4189aa61ee94801d31cb26a14a999eb5ea2c90a53bc704c5b262ff2b4cf984e97d7c92d13069b829b972c501190db9eaba00b8df84a25c78125e602cff3b037c7db65974b063084596a64667d5f92d647067c3c5453237d7e9e3573a57";
    const BLIND_CUBE_ID: u64 = 42;
    /// The AAD binds the key id, so the fixture row must BE key 7.
    const BLIND_KEY_ID: u64 = 7;

    /// A picker on testnet holding the vector Cube's encryption key, with the
    /// numeric cube id already resolved (as `on_cube_keys_loaded` sets it).
    fn blinded_picker() -> SelectKeySource {
        use coincube_core::signer::MasterSigner;
        let mut picker = empty_picker();
        picker.network = Network::Testnet;
        let signer = MasterSigner::from_str(Network::Testnet, BLIND_MNEMONIC).unwrap();
        picker.cube_encryption_key = Some(Arc::new(
            crate::services::connect::crypto::CubeEncryptionKey::derive(&signer, Network::Testnet),
        ));
        picker.cube_server_id = Some(BLIND_CUBE_ID);
        picker
    }

    fn blinded_key(id: u64, ciphertext: &str) -> ResolvedCubeKey {
        let mut resolved = resolved_key(id, "8a550171", 7);
        // Envelope-mode rows carry no plaintext column at all.
        resolved.raw.xpub = String::new();
        resolved.raw.xpub_envelope = Some(crate::services::connect::crypto::XpubEnvelope {
            scheme: crate::services::connect::crypto::XPUB_ENVELOPE_SCHEME.to_string(),
            recipient: crate::services::connect::crypto::RECIPIENT_CUBE_OWNER.to_string(),
            aad_key_id_bound: true,
            ephemeral_pubkey: BLIND_E.to_string(),
            nonce: BLIND_NONCE.to_string(),
            ciphertext: ciphertext.to_string(),
        });
        resolved
    }

    #[test]
    fn blinded_keychain_key_is_decrypted_and_selected() {
        let mut picker = blinded_picker();
        let _ = picker.on_select_keychain_key(blinded_key(BLIND_KEY_ID, BLIND_CT));

        assert!(picker.error.is_none(), "error: {:?}", picker.error);
        let SelectedKey::New(key) = &picker.selected_key else {
            panic!("blinded key should select: {:?}", picker.selected_key);
        };
        // The descriptor key is built from the *decrypted* xpub, with the
        // row's plaintext origin metadata — exactly as before blinding.
        assert!(key.key.to_string().contains(TESTNET_ACCOUNT_XPUB));
        assert_eq!(picker.step, Step::Details);
    }

    #[test]
    fn an_already_reported_key_is_refused_without_re_reporting() {
        // After an A4 report the server clears the ciphertext and flags the
        // row, so it comes back with neither xpub nor envelope. Selecting it
        // must say "waiting to be re-shared", not run the resolver and file a
        // second report against a keyholder who already has one pending.
        let mut picker = blinded_picker();
        let mut row = blinded_key(BLIND_KEY_ID, BLIND_CT);
        row.raw.status = crate::services::coincube::KEY_STATUS_ENVELOPE_INVALID.to_string();
        row.raw.xpub_envelope = None;
        row.raw.xpub = String::new();

        let _ = picker.on_select_keychain_key(row);

        assert!(matches!(picker.selected_key, SelectedKey::None));
        assert!(picker
            .error
            .as_deref()
            .unwrap()
            .contains("waiting to be re-shared"));
        assert_eq!(picker.step, Step::Grid);
    }

    #[test]
    fn a_blinded_key_without_the_cube_key_reports_locked_not_bad() {
        // Watch-only / no-seed install: the key is fine, this device just
        // can't read it. The message must not tell the user to go bother the
        // key's owner.
        let mut picker = blinded_picker();
        picker.cube_encryption_key = None;
        let _ = picker.on_select_keychain_key(blinded_key(BLIND_KEY_ID, BLIND_CT));

        assert!(matches!(picker.selected_key, SelectedKey::None));
        assert!(picker
            .error
            .as_deref()
            .unwrap()
            .contains("isn't available on this device"));
    }

    #[test]
    fn an_unopenable_envelope_surfaces_the_re_enrol_state() {
        let mut picker = blinded_picker();
        let mut tampered = hex::decode(BLIND_CT).unwrap();
        tampered[0] ^= 0x01;
        let _ = picker.on_select_keychain_key(blinded_key(BLIND_KEY_ID, &hex::encode(tampered)));

        assert!(matches!(picker.selected_key, SelectedKey::None));
        assert!(picker
            .error
            .as_deref()
            .unwrap()
            .contains("needs re-sharing"));
        assert_eq!(picker.step, Step::Grid, "a bad key must not advance");
    }

    #[test]
    fn a_blinded_key_cannot_be_opened_without_the_numeric_cube_id() {
        // The cube id is bound into the AAD, so a failed id lookup must fail
        // the open rather than quietly resolving to something.
        let mut picker = blinded_picker();
        picker.cube_server_id = None;
        let _ = picker.on_select_keychain_key(blinded_key(BLIND_KEY_ID, BLIND_CT));

        assert!(matches!(picker.selected_key, SelectedKey::None));
        assert!(picker
            .error
            .as_deref()
            .unwrap()
            .contains("needs re-sharing"));
    }

    // ── resolve_cube_keys: self/contact classification ───────────────
    //
    // The picker used to require `contact.role == Keyholder` before it would
    // surface a contact's Cube key. That role belongs to the contact
    // *relationship*, not to the Cube — the API instant-adds an existing
    // contact to a Cube without re-stamping it, and writes `owner` on the
    // reciprocal row — so a real Cube keyholder's key was silently dropped
    // and the picker showed "None of your contacts have shared keys yet."
    // while the Flutter app, which does no contact join, listed it.

    use crate::services::coincube::{Contact, ContactRole, ContactUser};

    /// A key owned by someone other than the viewer, as the W3 backend
    /// serves it (`isOwnKey: false`, `ownerUserId`/`ownerEmail` populated).
    fn contact_owned_raw_key(id: u64, owner_id: u64) -> CubeKeyRaw {
        let mut key = raw_key(id, "c658b283", owner_id);
        key.is_own_key = false;
        key
    }

    fn contact_with_role(contact_id: u64, contact_user_id: u64, role: ContactRole) -> Contact {
        Contact {
            id: contact_id,
            user_id: 0,
            contact_user_id: 0,
            invite_id: None,
            role,
            contact_user: Some(ContactUser {
                id: contact_user_id,
                email: format!("contact{contact_user_id}@example.com"),
                email_verified: None,
            }),
            created_at: "2026-07-20T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn contact_cube_key_resolves_whatever_the_contact_role_is() {
        for role in [
            ContactRole::Keyholder,
            ContactRole::Beneficiary,
            ContactRole::Observer,
            ContactRole::Owner,
            ContactRole::Unknown,
        ] {
            let contacts = vec![contact_with_role(11, 42, role)];
            let resolved = resolve_cube_keys(vec![contact_owned_raw_key(5, 42)], &contacts, 7);

            assert!(
                resolved.my_keys.is_empty(),
                "{}: a contact's key is not the viewer's own",
                role
            );
            assert_eq!(
                resolved.contact_keys.len(),
                1,
                "{}: contact key must be offered regardless of contact role",
                role
            );
            assert_eq!(
                resolved.contact_keys[0].owner,
                KeychainKeyOwner::Contact {
                    primary_owner_id: 42,
                    contact_id: 11,
                    // Server-supplied `ownerEmail` wins over the contact row.
                    contact_email: "owner42@example.com".to_string(),
                },
                "{}: owner must resolve to the addressable contact id",
                role
            );
        }
    }

    #[test]
    fn contact_email_falls_back_to_the_contact_row_when_server_omits_it() {
        // Pre-W3 backend: `ownerEmail` empty, so the contact row supplies it.
        let mut key = contact_owned_raw_key(5, 42);
        key.owner_email = String::new();
        let contacts = vec![contact_with_role(11, 42, ContactRole::Owner)];

        let resolved = resolve_cube_keys(vec![key], &contacts, 7);

        let KeychainKeyOwner::Contact { contact_email, .. } = &resolved.contact_keys[0].owner
        else {
            panic!("expected a contact-owned key");
        };
        assert_eq!(contact_email, "contact42@example.com");
    }

    #[test]
    fn own_keys_are_classified_by_id_when_the_server_flag_is_unset() {
        // Pre-W3 backend: `isOwnKey` always false, so the id comparison
        // against the authenticated user has to carry the classification.
        let resolved = resolve_cube_keys(vec![contact_owned_raw_key(5, 7)], &[], 7);

        assert_eq!(resolved.my_keys.len(), 1);
        assert!(resolved.contact_keys.is_empty());
        assert_eq!(
            resolved.my_keys[0].owner,
            KeychainKeyOwner::SelfUser {
                primary_owner_id: 7
            }
        );
    }

    #[test]
    fn a_key_whose_owner_is_not_a_contact_is_dropped() {
        // No contact row means no `contact_id` to send to `AddVaultMember`,
        // so the key is unusable and must not be offered.
        let contacts = vec![contact_with_role(11, 99, ContactRole::Keyholder)];
        let resolved = resolve_cube_keys(vec![contact_owned_raw_key(5, 42)], &contacts, 7);

        assert!(resolved.my_keys.is_empty());
        assert!(resolved.contact_keys.is_empty());
    }
}
