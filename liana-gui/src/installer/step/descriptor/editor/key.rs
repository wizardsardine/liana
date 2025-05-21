use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use iced::{Subscription, Task};
use liana::miniscript::bitcoin::bip32::{ChildNumber, Xpub};
use liana::miniscript::{
    bitcoin::{
        bip32::{DerivationPath, Fingerprint},
        Network,
    },
    descriptor::{DerivPaths, DescriptorMultiXKey, DescriptorPublicKey, DescriptorXKey, Wildcard},
};

use liana_ui::{component::form, widget::Element};

use crate::app::state::export::ExportModal;
use crate::export::{ImportExportMessage, ImportExportType};
use crate::{
    app::settings::ProviderKey,
    hw::{HardwareWallet, HardwareWallets},
    installer::{
        descriptor::{Key, KeySource, KeySourceKind, PathKind},
        message::{self, Message},
        view, Error,
    },
    services,
    signer::Signer,
};

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

pub struct EditXpubModal {
    device_must_support_tapminiscript: bool,
    keys_coordinate: Vec<(usize, usize)>,
    network: Network,
    error: Option<Error>,
    processing: bool,

    form_name: form::Value<String>,
    form_xpub: form::Value<String>,
    // TODO: Define new `form::Value` type with `Option<String>` instead of `bool` so that we can
    // store `form_token_warning` directly in `form_token`.
    form_token: form::Value<String>,
    form_token_warning: Option<String>,
    /// The `KeySourceKind` corresponding to the required form for entering a new key.
    form_key_source_kind: Option<KeySourceKind>,

    other_path_keys: HashSet<Fingerprint>,
    duplicate_master_fg: bool,

    path_kind: PathKind,
    keys: Vec<Key>,
    hot_signer: Arc<Mutex<Signer>>,
    hot_signer_fingerprint: Fingerprint,
    chosen_signer: Option<Key>,
    modal: Option<ExportModal>,
    accounts: HashMap<Fingerprint, ChildNumber>,
}

impl EditXpubModal {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        device_must_support_tapminiscript: bool,
        path_kind: PathKind,
        other_path_keys: HashSet<Fingerprint>,
        key: Option<Key>,
        keys_coordinate: Vec<(usize, usize)>,
        network: Network,
        hot_signer: Arc<Mutex<Signer>>,
        hot_signer_fingerprint: Fingerprint,
        keys: Vec<Key>,
    ) -> Self {
        let accounts = keys
            .iter()
            .filter_map(|k| k.account.map(|acc| (k.fingerprint, acc)))
            .collect();
        Self {
            device_must_support_tapminiscript,
            path_kind,
            other_path_keys,
            form_name: form::Value {
                valid: true,
                value: key.as_ref().map(|k| k.name.clone()).unwrap_or_default(),
            },
            form_xpub: form::Value {
                valid: true,
                value: key
                    .as_ref()
                    .filter(|k| k.source.is_manual())
                    .map(|k| k.key.to_string())
                    .unwrap_or_default(),
            },
            form_token: form::Value {
                valid: true,
                value: key
                    .as_ref()
                    .and_then(|k| k.source.token().cloned())
                    .unwrap_or_default(),
            },
            form_token_warning: None,
            form_key_source_kind: None, // no form will be shown until user clicks on required option
            keys,
            keys_coordinate,
            processing: false,
            error: None,
            network,
            chosen_signer: key,
            hot_signer_fingerprint,
            hot_signer,
            duplicate_master_fg: false,
            modal: None,
            accounts,
        }
    }

    pub fn load(&self) -> Task<Message> {
        Task::none()
    }
}

impl super::DescriptorEditModal for EditXpubModal {
    fn processing(&self) -> bool {
        self.processing
    }

    fn update(&mut self, hws: &mut HardwareWallets, message: Message) -> Task<Message> {
        // Reset these fields.
        // the function will setup them again if something is wrong
        self.duplicate_master_fg = false;
        self.error = None;
        match message {
            Message::SelectAccount(fg, index) => {
                self.accounts.insert(fg, index);
                return Task::none();
            }
            Message::Select(i) => {
                if let Some(HardwareWallet::Supported {
                    device,
                    fingerprint,
                    kind,
                    version,
                    ..
                }) = hws.list.get(i)
                {
                    self.processing = true;
                    self.form_key_source_kind = None;
                    let device_version = version.clone();
                    let account = self
                        .accounts
                        .get(fingerprint)
                        .copied()
                        .unwrap_or(ChildNumber::from_hardened_idx(0).expect("hardcoded"));
                    let fingerprint = *fingerprint;
                    let device_kind = *kind;
                    let device_cloned = device.clone();
                    let network = self.network;
                    return Task::perform(
                        async move {
                            (
                                device_version,
                                device_kind,
                                fingerprint,
                                network,
                                get_extended_pubkey(device_cloned, fingerprint, network, account)
                                    .await,
                            )
                        },
                        move |(device_version, device_kind, fingerprint, network, res)| {
                            Message::DefineDescriptor(message::DefineDescriptor::KeyModal(
                                message::ImportKeyModal::FetchedKey(match res {
                                    Err(e) => Err(e),
                                    Ok(key) => {
                                        if check_key_network(&key, network) {
                                            Ok(Key {
                                                source: KeySource::Device(
                                                    device_kind,
                                                    device_version,
                                                ),
                                                fingerprint,
                                                name: "".to_string(),
                                                key,
                                                account: Some(account),
                                            })
                                        } else {
                                            Err(Error::Unexpected(
                                                "Fetched key does not have the correct network"
                                                    .to_string(),
                                            ))
                                        }
                                    }
                                }),
                            ))
                        },
                    );
                }
            }
            Message::Reload => {
                return self.load();
            }
            Message::UseHotSigner => {
                self.form_key_source_kind = None;
                let fingerprint = self.hot_signer.lock().unwrap().fingerprint();
                let derivation_path = default_derivation_path(self.network);
                let key_str = format!(
                    "[{}/{}]{}",
                    fingerprint,
                    derivation_path.to_string().trim_start_matches("m/"),
                    self.hot_signer
                        .lock()
                        .unwrap()
                        .get_extended_pubkey(&derivation_path)
                );
                self.chosen_signer = Some(Key {
                    source: KeySource::HotSigner,
                    fingerprint,
                    name: "".to_string(),
                    key: DescriptorPublicKey::from_str(&key_str).unwrap(),
                    account: None,
                });
                self.form_name.value = self
                    .keys
                    .iter()
                    .find_map(|k| {
                        if k.fingerprint == fingerprint {
                            Some(k.name.clone())
                        } else {
                            None
                        }
                    })
                    .unwrap_or_default();
                self.form_name.valid = true;
            }
            Message::ImportExport(import_msg) => match import_msg {
                ImportExportMessage::Close => {
                    if self.modal.is_some() {
                        self.modal = None;
                    }
                }
                ImportExportMessage::Xpub(xpub_str) => {
                    if self.modal.is_some() {
                        self.modal = None;
                        return Task::perform(async move { xpub_str }, |xpub_str| {
                            Message::DefineDescriptor(message::DefineDescriptor::KeyModal(
                                message::ImportKeyModal::XPubEdited(xpub_str),
                            ))
                        });
                    }
                }
                m => {
                    if let Some(modal) = self.modal.as_mut() {
                        return modal.update(m);
                    }
                }
            },
            Message::DefineDescriptor(message::DefineDescriptor::KeyModal(msg)) => match msg {
                message::ImportKeyModal::FetchedKey(res) => {
                    self.processing = false;
                    match res {
                        Ok(mut key) => {
                            // If it is a provider key that has just been fetched, do some additional sanity checks.
                            if let Some(key_kind) = key.source.provider_key_kind() {
                                // We don't need to check key's status as redeemed keys are not returned.
                                self.form_token_warning = if self.form_key_source_kind
                                    != Some(KeySourceKind::Token(key_kind))
                                {
                                    Some("Wrong kind of token".to_string())
                                } else if !check_key_network(&key.key, self.network) {
                                    Some(
                                        "Fetched key does not have the correct network".to_string(),
                                    )
                                }
                                // If two keys have the same fingerprint, they must both have the same provider key kind (which could be `None`).
                                // Note that this checks all keys regardless of whether they are currently being used in a path.
                                else if self.keys.iter().any(|existing| {
                                    existing.fingerprint == key.fingerprint
                                        && existing.source.provider_key_kind()
                                            != key.source.provider_key_kind()
                                }) {
                                    Some(
                                        "Fetched key has already been added to the wallet."
                                            .to_string(),
                                    )
                                } else {
                                    None
                                };
                                self.form_token.valid = self.form_token_warning.is_none();
                            }
                            key.account = self.accounts.get(&key.fingerprint).copied();
                            // User can set name for key if it is not a provider key or is a valid provider key.
                            if key.source.provider_key().is_none() || self.form_token.valid {
                                self.form_name.valid = key.name.is_empty()
                                    || !self.keys.iter().any(|k| {
                                        k.fingerprint != key.fingerprint && k.name == key.name
                                    });
                                self.form_name.value.clone_from(&key.name);
                                self.chosen_signer = Some(key);
                            } else {
                                self.chosen_signer = None;
                            }
                        }
                        Err(e) => {
                            self.chosen_signer = None;
                            self.error = Some(e);
                        }
                    }
                }
                message::ImportKeyModal::ManuallyImportXpub => {
                    self.chosen_signer = None;
                    self.form_key_source_kind = Some(KeySourceKind::Manual);
                    self.form_xpub = form::Value::default();
                }
                message::ImportKeyModal::UseToken(kind) => {
                    self.chosen_signer = None;
                    self.form_key_source_kind = Some(KeySourceKind::Token(kind));
                    self.form_token = form::Value::default();
                }
                message::ImportKeyModal::NameEdited(name) => {
                    self.form_name.valid = !self.keys.iter().any(|k| {
                        Some(&k.fingerprint) != self.chosen_signer.as_ref().map(|s| &s.fingerprint)
                            && name == k.name
                    });
                    self.form_name.value = name;
                }
                message::ImportKeyModal::TokenEdited(s) => {
                    self.chosen_signer = None;
                    // We check if the token has already been fetched and saved regardless of its kind.
                    self.form_token_warning =
                        if self.keys.iter().any(|k| k.source.token() == Some(&s)) {
                            Some("Duplicate token".to_string())
                        } else {
                            None
                        };
                    self.form_token.valid = s.is_empty() || self.form_token_warning.is_none();
                    self.form_token.value = s;
                }
                message::ImportKeyModal::ImportXpub(network) => {
                    if self.modal.is_none() {
                        let modal = ExportModal::new(None, ImportExportType::ImportXpub(network));
                        let launch = modal.launch(false);
                        self.modal = Some(modal);
                        return launch;
                    }
                }
                message::ImportKeyModal::XPubEdited(s) => {
                    self.chosen_signer = None;
                    if let Ok(DescriptorPublicKey::XPub(key)) = DescriptorPublicKey::from_str(&s) {
                        if !key.derivation_path.is_master() {
                            self.form_xpub.valid = false;
                        } else if let Some((fingerprint, _)) = key.origin {
                            self.form_xpub.valid = if self.network == Network::Bitcoin {
                                key.xkey.network == Network::Bitcoin.into()
                            } else {
                                key.xkey.network == Network::Testnet.into()
                            };
                            if self.form_xpub.valid {
                                self.chosen_signer = Some(Key {
                                    source: KeySource::Manual,
                                    fingerprint,
                                    name: "".to_string(),
                                    key: DescriptorPublicKey::XPub(key),
                                    account: None,
                                });
                                self.form_name.value = "".to_string();
                                self.form_name.valid = true;
                            }
                        } else {
                            self.form_xpub.valid = false;
                        }
                    } else {
                        self.form_xpub.valid = false;
                    }
                    self.form_xpub.value = s;
                }
                message::ImportKeyModal::ConfirmXpub => {
                    if let Some(mut key) = self.chosen_signer.clone() {
                        key.name.clone_from(&self.form_name.value);
                        if self.other_path_keys.contains(&key.fingerprint) {
                            self.duplicate_master_fg = true;
                        } else {
                            let coordinate = self.keys_coordinate.clone();
                            return Task::perform(
                                async move { (coordinate, key) },
                                move |(coordinate, key)| {
                                    message::DefineDescriptor::KeysEdited(coordinate, key)
                                },
                            )
                            .map(Message::DefineDescriptor);
                        }
                    }
                }
                message::ImportKeyModal::ConfirmToken => {
                    // We have checked that the token has not already been fetched and saved.
                    let token = self.form_token.value.clone();
                    let client = services::keys::Client::new();
                    return Task::perform(
                        async move { (token.clone(), client.get_key_by_token(token).await) },
                        |(token, res)| {
                            Message::DefineDescriptor(message::DefineDescriptor::KeyModal(
                                message::ImportKeyModal::FetchedKey(match res {
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
                                        name: format!(
                                            "{} - {}",
                                            key.provider.name.clone(),
                                            key.kind
                                        ),
                                        key: key.xpub.clone(),
                                        account: None,
                                    }),
                                }),
                            ))
                        },
                    );
                }
                message::ImportKeyModal::SelectKey(i) => {
                    if let Some(key) = self.keys.get(i) {
                        self.chosen_signer = Some(key.clone());
                        self.form_key_source_kind = None;
                        self.form_name.value.clone_from(&key.name);
                        self.form_name.valid = true;
                    }
                }
            },
            _ => {}
        };
        Task::none()
    }

    fn subscription(&self, hws: &HardwareWallets) -> Subscription<Message> {
        let hw = hws.refresh().map(Message::HardwareWallets);
        if let Some(modal) = self.modal.as_ref() {
            if let Some(sub) = modal.subscription() {
                let import = sub.map(|m| Message::ImportExport(ImportExportMessage::Progress(m)));
                return Subscription::batch(vec![hw, import]);
            }
        }
        hw
    }

    fn view<'a>(&'a self, hws: &'a HardwareWallets) -> Element<'a, Message> {
        // For provider keys, include the chosen signer in case this is a provider key
        // and has not yet been saved, i.e. if it's not in `self.keys`. An unsaved provider
        // key will be displayed in a similar way to saved ones.
        let provider_keys: Vec<_> = self
            .keys
            .iter()
            .enumerate()
            .map(|(i, k)| (Some(i), k))
            .chain((self.chosen_signer).iter().filter_map(|cs| {
                (!self.keys.iter().any(|k| k.fingerprint == cs.fingerprint)).then_some((None, cs))
            }))
            .filter(|(_, k)| {
                k.source.is_token() && self.path_kind.can_choose_key_source_kind(&k.source.kind())
            })
            .collect();
        let chosen_signer = self.chosen_signer.as_ref().map(|s| s.fingerprint);
        let content = view::editor::edit_key_modal(
            "Set your key",
            self.network,
            self.path_kind,
            hws.list
                .iter()
                .enumerate()
                .filter_map(|(i, hw)| {
                    if self
                        .keys
                        .iter()
                        .any(|k| Some(k.fingerprint) == hw.fingerprint())
                        || !self
                            .path_kind
                            .can_choose_key_source_kind(&KeySourceKind::Device)
                    {
                        None
                    } else {
                        Some(view::hw_list_view(
                            i,
                            hw,
                            hw.fingerprint() == chosen_signer,
                            self.processing,
                            hw.fingerprint() == chosen_signer,
                            None,
                            self.device_must_support_tapminiscript,
                            Some(&self.accounts),
                            true,
                        ))
                    }
                })
                .collect(),
            self.keys
                .iter()
                .enumerate()
                .filter_map(|(i, key)| {
                    // ignore hot signers and provider keys.
                    if key.fingerprint == self.hot_signer_fingerprint
                        || key.source.is_token()
                        || !self
                            .path_kind
                            .can_choose_key_source_kind(&key.source.kind())
                    {
                        None
                    } else {
                        Some(view::key_list_view(
                            i,
                            &key.name,
                            &key.fingerprint,
                            key.source.device_kind(),
                            key.source.device_version(),
                            Some(key.fingerprint) == chosen_signer,
                            self.device_must_support_tapminiscript,
                            &self.accounts,
                        ))
                    }
                })
                .collect(),
            provider_keys
                .iter()
                .map(|(i, pk)| {
                    view::provider_key_list_view(*i, pk, Some(pk.fingerprint) == chosen_signer)
                })
                .collect(),
            self.error.as_ref(),
            self.chosen_signer.as_ref().map(|s| s.fingerprint),
            &self.hot_signer_fingerprint,
            self.keys.iter().find_map(|k| {
                if k.fingerprint == self.hot_signer_fingerprint {
                    Some(&k.name)
                } else {
                    None
                }
            }),
            &self.form_name,
            &self.form_xpub,
            &self.form_token,
            self.form_token_warning.as_ref(),
            self.form_key_source_kind.as_ref(),
            self.duplicate_master_fg,
        );
        if let Some(modal) = &self.modal {
            modal.view(content)
        } else {
            content
        }
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
