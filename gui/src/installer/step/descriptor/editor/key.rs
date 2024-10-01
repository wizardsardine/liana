use std::collections::HashSet;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use iced::{Command, Subscription};
use liana::miniscript::bitcoin::bip32::Xpub;
use liana::miniscript::{
    bitcoin::{
        bip32::{DerivationPath, Fingerprint},
        Network,
    },
    descriptor::{DerivPaths, DescriptorMultiXKey, DescriptorPublicKey, DescriptorXKey, Wildcard},
};

use liana_ui::{component::form, widget::Element};

use async_hwi::{DeviceKind, Version};

use crate::{
    hw::{HardwareWallet, HardwareWallets},
    installer::{
        message::{self, Message},
        view, Error,
    },
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

#[derive(Clone)]
pub struct Key {
    pub device_kind: Option<DeviceKind>,
    pub is_hot_signer: bool,
    pub device_version: Option<Version>,
    pub name: String,
    pub fingerprint: Fingerprint,
    pub key: DescriptorPublicKey,
}

pub fn check_key_network(key: &DescriptorPublicKey, network: Network) -> bool {
    match key {
        DescriptorPublicKey::XPub(key) => {
            if network == Network::Bitcoin {
                key.xkey.network == Network::Bitcoin
            } else {
                key.xkey.network == Network::Testnet
            }
        }
        DescriptorPublicKey::MultiXPub(key) => {
            if network == Network::Bitcoin {
                key.xkey.network == Network::Bitcoin
            } else {
                key.xkey.network == Network::Testnet
            }
        }
        _ => true,
    }
}

pub struct EditXpubModal {
    device_must_support_tapminiscript: bool,
    /// None if path is primary path
    path_index: Option<usize>,
    key_index: usize,
    network: Network,
    error: Option<Error>,
    processing: bool,

    form_name: form::Value<String>,
    form_xpub: form::Value<String>,
    edit_name: bool,

    other_path_keys: HashSet<Fingerprint>,
    duplicate_master_fg: bool,

    keys: Vec<Key>,
    hot_signer: Arc<Mutex<Signer>>,
    hot_signer_fingerprint: Fingerprint,
    chosen_signer: Option<(Fingerprint, Option<DeviceKind>, Option<Version>)>,
}

impl EditXpubModal {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        device_must_support_tapminiscript: bool,
        other_path_keys: HashSet<Fingerprint>,
        key: Option<Fingerprint>,
        path_index: Option<usize>,
        key_index: usize,
        network: Network,
        hot_signer: Arc<Mutex<Signer>>,
        hot_signer_fingerprint: Fingerprint,
        keys: Vec<Key>,
    ) -> Self {
        Self {
            device_must_support_tapminiscript,
            other_path_keys,
            form_name: form::Value {
                valid: true,
                value: key
                    .map(|fg| {
                        keys.iter()
                            .find(|k| k.fingerprint == fg)
                            .expect("must be stored")
                            .name
                            .clone()
                    })
                    .unwrap_or_default(),
            },
            form_xpub: form::Value {
                valid: true,
                value: key
                    .map(|fg| {
                        keys.iter()
                            .find(|k| k.fingerprint == fg)
                            .expect("must be stored")
                            .key
                            .to_string()
                    })
                    .unwrap_or_default(),
            },
            keys,
            path_index,
            key_index,
            processing: false,
            error: None,
            network,
            edit_name: false,
            chosen_signer: key.map(|k| (k, None, None)),
            hot_signer_fingerprint,
            hot_signer,
            duplicate_master_fg: false,
        }
    }

    pub fn load(&self) -> Command<Message> {
        Command::none()
    }
}

impl super::DescriptorEditModal for EditXpubModal {
    fn processing(&self) -> bool {
        self.processing
    }

    fn update(&mut self, hws: &mut HardwareWallets, message: Message) -> Command<Message> {
        // Reset these fields.
        // the fonction will setup them again if something is wrong
        self.duplicate_master_fg = false;
        self.error = None;
        match message {
            Message::Select(i) => {
                if let Some(HardwareWallet::Supported {
                    device,
                    fingerprint,
                    kind,
                    version,
                    ..
                }) = hws.list.get(i)
                {
                    self.chosen_signer = Some((*fingerprint, Some(*kind), version.clone()));
                    self.processing = true;
                    return Command::perform(
                        get_extended_pubkey(device.clone(), *fingerprint, self.network),
                        |res| {
                            Message::DefineDescriptor(message::DefineDescriptor::KeyModal(
                                message::ImportKeyModal::HWXpubImported(res),
                            ))
                        },
                    );
                }
            }
            Message::Reload => {
                return self.load();
            }
            Message::UseHotSigner => {
                let fingerprint = self.hot_signer.lock().unwrap().fingerprint();
                self.chosen_signer = Some((fingerprint, None, None));
                self.form_xpub.valid = true;
                if let Some(alias) = self
                    .keys
                    .iter()
                    .find(|key| key.fingerprint == fingerprint)
                    .map(|k| k.name.clone())
                {
                    self.form_name.valid = true;
                    self.form_name.value = alias;
                    self.edit_name = false;
                } else {
                    self.edit_name = true;
                    self.form_name.value = String::new();
                }
                let derivation_path = default_derivation_path(self.network);
                self.form_xpub.value = format!(
                    "[{}{}]{}",
                    fingerprint,
                    derivation_path.to_string().trim_start_matches('m'),
                    self.hot_signer
                        .lock()
                        .unwrap()
                        .get_extended_pubkey(&derivation_path)
                );
            }
            Message::DefineDescriptor(message::DefineDescriptor::KeyModal(msg)) => match msg {
                message::ImportKeyModal::HWXpubImported(res) => {
                    self.processing = false;
                    match res {
                        Ok(key) => {
                            if let Some(alias) = self
                                .keys
                                .iter()
                                .find(|k| k.fingerprint == key.master_fingerprint())
                                .map(|k| k.name.clone())
                            {
                                self.form_name.valid = true;
                                self.form_name.value = alias;
                                self.edit_name = false;
                            } else {
                                self.edit_name = true;
                                self.form_name.value = String::new();
                            }
                            self.form_xpub.valid = check_key_network(&key, self.network);
                            self.form_xpub.value = key.to_string();
                        }
                        Err(e) => {
                            self.chosen_signer = None;
                            self.error = Some(e);
                        }
                    }
                }
                message::ImportKeyModal::EditName => {
                    self.edit_name = true;
                }
                message::ImportKeyModal::NameEdited(name) => {
                    self.form_name.valid = true;
                    self.form_name.value = name;
                }
                message::ImportKeyModal::XPubEdited(s) => {
                    if let Ok(DescriptorPublicKey::XPub(key)) = DescriptorPublicKey::from_str(&s) {
                        self.chosen_signer = None;
                        if !key.derivation_path.is_master() {
                            self.form_xpub.valid = false;
                        } else if let Some((fingerprint, _)) = key.origin {
                            self.form_xpub.valid = if self.network == Network::Bitcoin {
                                key.xkey.network == Network::Bitcoin
                            } else {
                                key.xkey.network == Network::Testnet
                            };
                            if let Some(alias) = self
                                .keys
                                .iter()
                                .find(|k| k.fingerprint == fingerprint)
                                .map(|k| k.name.clone())
                            {
                                self.form_name.valid = true;
                                self.form_name.value = alias;
                                self.edit_name = false;
                            } else {
                                self.edit_name = true;
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
                    if let Ok(key) = DescriptorPublicKey::from_str(&self.form_xpub.value) {
                        let key_index = self.key_index;
                        let name = self.form_name.value.clone();
                        let (device_kind, device_version) =
                            if let Some((_, kind, version)) = &self.chosen_signer {
                                (*kind, version.clone())
                            } else {
                                (None, None)
                            };
                        if self.other_path_keys.contains(&key.master_fingerprint()) {
                            self.duplicate_master_fg = true;
                        } else if let Some(path_index) = self.path_index {
                            return Command::perform(
                                async move { (path_index, key_index, key) },
                                move |(path_index, key_index, key)| {
                                    message::DefineDescriptor::RecoveryPath(
                                        path_index,
                                        message::DefinePath::Key(
                                            key_index,
                                            message::DefineKey::Edited(
                                                name,
                                                key,
                                                device_kind,
                                                device_version,
                                            ),
                                        ),
                                    )
                                },
                            )
                            .map(Message::DefineDescriptor);
                        } else {
                            return Command::perform(
                                async move { (key_index, key) },
                                move |(key_index, key)| {
                                    message::DefineDescriptor::PrimaryPath(
                                        message::DefinePath::Key(
                                            key_index,
                                            message::DefineKey::Edited(
                                                name,
                                                key,
                                                device_kind,
                                                device_version,
                                            ),
                                        ),
                                    )
                                },
                            )
                            .map(Message::DefineDescriptor);
                        }
                    }
                }
                message::ImportKeyModal::SelectKey(i) => {
                    if let Some(key) = self.keys.get(i) {
                        self.chosen_signer =
                            Some((key.fingerprint, key.device_kind, key.device_version.clone()));
                        self.form_xpub.value = key.key.to_string();
                        self.form_xpub.valid = true;
                        self.form_name.value.clone_from(&key.name);
                        self.form_name.valid = true;
                    }
                }
            },
            _ => {}
        };
        Command::none()
    }

    fn subscription(&self, hws: &HardwareWallets) -> Subscription<Message> {
        hws.refresh().map(Message::HardwareWallets)
    }

    fn view<'a>(&'a self, hws: &'a HardwareWallets) -> Element<'a, Message> {
        let chosen_signer = self.chosen_signer.as_ref().map(|s| s.0);
        view::edit_key_modal(
            self.network,
            hws.list
                .iter()
                .enumerate()
                .filter_map(|(i, hw)| {
                    if self
                        .keys
                        .iter()
                        .any(|k| Some(k.fingerprint) == hw.fingerprint())
                    {
                        None
                    } else {
                        Some(view::hw_list_view(
                            i,
                            hw,
                            hw.fingerprint() == chosen_signer,
                            self.processing,
                            !self.processing
                                && hw.fingerprint() == chosen_signer
                                && self.form_xpub.valid
                                && !self.form_xpub.value.is_empty(),
                            self.device_must_support_tapminiscript,
                        ))
                    }
                })
                .collect(),
            self.keys
                .iter()
                .enumerate()
                .filter_map(|(i, key)| {
                    if key.fingerprint == self.hot_signer_fingerprint {
                        None
                    } else {
                        Some(view::key_list_view(
                            i,
                            &key.name,
                            &key.fingerprint,
                            key.device_kind.as_ref(),
                            key.device_version.as_ref(),
                            Some(key.fingerprint) == chosen_signer,
                            self.device_must_support_tapminiscript,
                        ))
                    }
                })
                .collect(),
            self.error.as_ref(),
            self.chosen_signer.as_ref().map(|s| s.0),
            &self.hot_signer_fingerprint,
            self.keys.iter().find_map(|k| {
                if k.fingerprint == self.hot_signer_fingerprint {
                    Some(&k.name)
                } else {
                    None
                }
            }),
            &self.form_xpub,
            &self.form_name,
            self.edit_name,
            self.duplicate_master_fg,
        )
    }
}

pub fn default_derivation_path(network: Network) -> DerivationPath {
    DerivationPath::from_str({
        if network == Network::Bitcoin {
            "m/48'/0'/0'/2'"
        } else {
            "m/48'/1'/0'/2'"
        }
    })
    .unwrap()
}

/// LIANA_STANDARD_PATH: m/48'/0'/0'/2';
/// LIANA_TESTNET_STANDARD_PATH: m/48'/1'/0'/2';
pub async fn get_extended_pubkey(
    hw: std::sync::Arc<dyn async_hwi::HWI + Send + Sync>,
    fingerprint: Fingerprint,
    network: Network,
) -> Result<DescriptorPublicKey, Error> {
    let derivation_path = default_derivation_path(network);
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
