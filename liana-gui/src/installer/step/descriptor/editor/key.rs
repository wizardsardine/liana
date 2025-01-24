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
    hw::{is_compatible_with_tapminiscript, HardwareWallet, HardwareWallets},
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

#[derive(Debug, Clone)]
pub struct Key {
    pub device_kind: Option<DeviceKind>,
    pub is_hot_signer: bool,
    pub device_version: Option<Version>,
    pub name: String,
    pub fingerprint: Fingerprint,
    pub key: DescriptorPublicKey,
    pub is_compatible_taproot: bool,
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
    manually_imported_xpub: bool,

    other_path_keys: HashSet<Fingerprint>,
    duplicate_master_fg: bool,

    keys: Vec<Key>,
    hot_signer: Arc<Mutex<Signer>>,
    hot_signer_fingerprint: Fingerprint,
    chosen_signer: Option<Key>,
}

impl EditXpubModal {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        device_must_support_tapminiscript: bool,
        other_path_keys: HashSet<Fingerprint>,
        key: Option<Key>,
        keys_coordinate: Vec<(usize, usize)>,
        network: Network,
        hot_signer: Arc<Mutex<Signer>>,
        hot_signer_fingerprint: Fingerprint,
        keys: Vec<Key>,
    ) -> Self {
        // The xpub is manually imported if the key is neither from a device or the hot signer.
        let manually_imported_xpub = key
            .as_ref()
            .map(|k| !k.is_hot_signer && k.device_kind.is_none())
            .unwrap_or(false);
        Self {
            device_must_support_tapminiscript,
            other_path_keys,
            form_name: form::Value {
                valid: true,
                value: key.as_ref().map(|k| k.name.clone()).unwrap_or_default(),
            },
            form_xpub: form::Value {
                valid: true,
                value: if manually_imported_xpub {
                    key.as_ref().map(|k| k.key.to_string()).unwrap_or_default()
                } else {
                    String::new()
                },
            },
            manually_imported_xpub,
            keys,
            keys_coordinate,
            processing: false,
            error: None,
            network,
            chosen_signer: key,
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
        // the function will setup them again if something is wrong
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
                    self.processing = true;
                    self.manually_imported_xpub = false;
                    let device_version = version.clone();
                    let fingerprint = *fingerprint;
                    let device_kind = *kind;
                    let network = self.network;
                    return Command::perform(
                        get_extended_pubkey(device.clone(), fingerprint, self.network),
                        move |res| {
                            Message::DefineDescriptor(message::DefineDescriptor::KeyModal(
                                message::ImportKeyModal::FetchedKey(match res {
                                    Err(e) => Err(e),
                                    Ok(key) => {
                                        if check_key_network(&key, network) {
                                            Ok(Key {
                                                is_hot_signer: false,
                                                fingerprint,
                                                name: "".to_string(),
                                                key,
                                                is_compatible_taproot:
                                                    is_compatible_with_tapminiscript(
                                                        &device_kind,
                                                        device_version.as_ref(),
                                                    ),
                                                device_kind: Some(device_kind),
                                                device_version,
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
                self.manually_imported_xpub = false;
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
                    is_hot_signer: true,
                    fingerprint,
                    name: "".to_string(),
                    key: DescriptorPublicKey::from_str(&key_str).unwrap(),
                    is_compatible_taproot: true,
                    device_kind: None,
                    device_version: None,
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
            Message::DefineDescriptor(message::DefineDescriptor::KeyModal(msg)) => match msg {
                message::ImportKeyModal::FetchedKey(res) => {
                    self.processing = false;
                    match res {
                        Ok(key) => {
                            self.form_name.valid = true;
                            self.form_name.value.clone_from(&key.name);
                            self.chosen_signer = Some(key);
                        }
                        Err(e) => {
                            self.chosen_signer = None;
                            self.error = Some(e);
                        }
                    }
                }
                message::ImportKeyModal::ManuallyImportXpub => {
                    self.chosen_signer = None;
                    self.manually_imported_xpub = true;
                    self.form_xpub = form::Value::default();
                }
                message::ImportKeyModal::NameEdited(name) => {
                    self.form_name.valid = !self.keys.iter().any(|k| {
                        Some(&k.fingerprint) != self.chosen_signer.as_ref().map(|s| &s.fingerprint)
                            && name == k.name
                    });
                    self.form_name.value = name;
                }
                message::ImportKeyModal::XPubEdited(s) => {
                    if let Ok(DescriptorPublicKey::XPub(key)) = DescriptorPublicKey::from_str(&s) {
                        self.chosen_signer = None;
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
                                    is_hot_signer: false,
                                    fingerprint,
                                    name: "".to_string(),
                                    key: DescriptorPublicKey::XPub(key),
                                    is_compatible_taproot: true,
                                    device_kind: None,
                                    device_version: None,
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
                            return Command::perform(
                                async move { (coordinate, key) },
                                move |(coordinate, key)| {
                                    message::DefineDescriptor::KeysEdited(coordinate, key)
                                },
                            )
                            .map(Message::DefineDescriptor);
                        }
                    }
                }
                message::ImportKeyModal::SelectKey(i) => {
                    if let Some(key) = self.keys.get(i) {
                        self.chosen_signer = Some(key.clone());
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
        let chosen_signer = self.chosen_signer.as_ref().map(|s| s.fingerprint);
        view::editor::edit_key_modal(
            "Set your key",
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
                            hw.fingerprint() == chosen_signer,
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
            self.manually_imported_xpub,
            self.duplicate_master_fg,
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
}
