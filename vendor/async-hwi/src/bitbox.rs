use crate::{bip389, parse_version, AddressScript, DeviceKind, Error as HWIError, HWI};
use api::btc::make_script_config_simple;
use async_trait::async_trait;
use bitbox_api::{
    btc::KeyOriginInfo,
    error::{BitBoxError, Error},
    pb::{self, BtcScriptConfig},
    usb::UsbError,
    Keypath, PairedBitBox, PairingBitBox,
};
use bitcoin::{
    bip32::{ChildNumber, DerivationPath, Fingerprint, Xpub},
    psbt::Psbt,
};
use regex::Regex;
use std::{
    str::FromStr,
    sync::{Arc, Mutex},
};

pub use bitbox_api::{
    self as api,
    runtime::Runtime,
    usb::{get_any_bitbox02, is_bitbox02},
    ConfigError, NoiseConfig, NoiseConfigData, NoiseConfigNoCache,
};

#[derive(Clone)]
struct Cache(Arc<Mutex<Option<NoiseConfigData>>>);

impl bitbox_api::Threading for Cache {}

impl NoiseConfig for Cache {
    fn read_config(&self) -> Result<NoiseConfigData, ConfigError> {
        let noise_data = self.0.lock().map_err(|e| ConfigError(e.to_string()))?;
        if let Some(data) = noise_data.as_ref() {
            Ok(data.clone())
        } else {
            Ok(NoiseConfigData::default())
        }
    }
    fn store_config(&self, data: &NoiseConfigData) -> Result<(), ConfigError> {
        let mut noise_data = self.0.lock().map_err(|e| ConfigError(e.to_string()))?;
        *noise_data = Some(data.clone());
        Ok(())
    }
}

pub struct PairingBitbox02WithLocalCache<T: Runtime> {
    client: PairingBitBox<T>,
    local_cache: Cache,
}

impl<T: Runtime> PairingBitbox02WithLocalCache<T> {
    pub async fn connect(
        device: hidapi::HidDevice,
        pairing_data: Option<NoiseConfigData>,
    ) -> Result<Self, HWIError> {
        let local_cache = if let Some(data) = pairing_data {
            Cache(Arc::new(Mutex::new(Some(data))))
        } else {
            Cache(Arc::new(Mutex::new(None)))
        };
        let bitbox =
            bitbox_api::BitBox::<T>::from_hid_device(device, Box::new(local_cache.clone())).await?;
        let pairing_bitbox = bitbox.unlock_and_pair().await?;
        Ok(PairingBitbox02WithLocalCache {
            client: pairing_bitbox,
            local_cache,
        })
    }

    pub fn pairing_code(&self) -> Option<String> {
        self.client.get_pairing_code()
    }

    pub async fn wait_confirm(self) -> Result<(PairedBitBox<T>, NoiseConfigData), HWIError> {
        let client = self.client.wait_confirm().await?;
        let mut cache = self
            .local_cache
            .0
            .lock()
            .map_err(|e| HWIError::Device(e.to_string()))?;
        Ok((
            client,
            cache
                .take()
                .expect("noise config data must be in local cache"),
        ))
    }
}

pub struct PairingBitbox02<T: Runtime> {
    client: PairingBitBox<T>,
}

impl<T: Runtime> PairingBitbox02<T> {
    pub async fn connect(
        device: hidapi::HidDevice,
        pairing: Option<Box<dyn NoiseConfig>>,
    ) -> Result<Self, HWIError> {
        let noise_config = pairing.unwrap_or_else(|| Box::new(NoiseConfigNoCache {}));
        let bitbox = bitbox_api::BitBox::<T>::from_hid_device(device, noise_config).await?;
        let pairing_bitbox = bitbox.unlock_and_pair().await?;
        Ok(PairingBitbox02 {
            client: pairing_bitbox,
        })
    }

    pub fn pairing_code(&self) -> Option<String> {
        self.client.get_pairing_code()
    }

    pub async fn wait_confirm(self) -> Result<PairedBitBox<T>, HWIError> {
        self.client.wait_confirm().await.map_err(|e| e.into())
    }
}

pub struct BitBox02<T: Runtime> {
    pub network: bitcoin::Network,
    pub display_xpub: bool,
    pub client: PairedBitBox<T>,
    pub policy: Option<Policy>,
}

impl<T: Runtime> std::fmt::Debug for BitBox02<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BitBox").finish()
    }
}

impl<T: Runtime> BitBox02<T> {
    pub fn from(paired_bitbox: PairedBitBox<T>) -> Self {
        BitBox02 {
            display_xpub: false,
            network: bitcoin::Network::Bitcoin,
            client: paired_bitbox,
            policy: None,
        }
    }

    pub fn with_network(mut self, network: bitcoin::Network) -> Self {
        self.network = network;
        self
    }

    pub fn display_xpub(mut self, value: bool) -> Self {
        self.display_xpub = value;
        self
    }

    pub fn with_policy(mut self, policy: &str) -> Result<Self, HWIError> {
        self.policy = Some(extract_script_config_policy(policy)?);
        Ok(self)
    }

    pub async fn is_policy_registered(&self, policy: &str) -> Result<bool, HWIError> {
        let pb_network = coin_from_network(self.network);
        let policy = extract_script_config_policy(policy)?;
        let fg = self
            .client
            .root_fingerprint()
            .await
            .map_err(|e| HWIError::Device(e.to_string()))
            .and_then(|fg| {
                Fingerprint::from_str(&fg).map_err(|e| HWIError::Device(e.to_string()))
            })?;
        let (config, keypath) = policy.script_config(fg);
        self.client
            .btc_is_script_config_registered(pb_network, &config, keypath.as_ref())
            .await
            .map_err(|e| e.into())
    }
}

#[async_trait]
impl<T: Runtime + Sync + Send> HWI for BitBox02<T> {
    fn device_kind(&self) -> DeviceKind {
        DeviceKind::BitBox02
    }

    async fn get_version(&self) -> Result<super::Version, HWIError> {
        let info = self
            .client
            .device_info()
            .await
            .map_err(|e| HWIError::Device(e.to_string()))?;
        Ok(parse_version(&info.version)?)
    }

    async fn get_master_fingerprint(&self) -> Result<Fingerprint, HWIError> {
        let fg = self
            .client
            .root_fingerprint()
            .await
            .map_err(|e| HWIError::Device(e.to_string()))?;
        Ok(Fingerprint::from_str(&fg).map_err(|e| HWIError::Device(e.to_string()))?)
    }

    async fn get_extended_pubkey(&self, path: &DerivationPath) -> Result<Xpub, HWIError> {
        let fg = self
            .client
            .btc_xpub(
                if self.network == bitcoin::Network::Bitcoin {
                    pb::BtcCoin::Btc
                } else {
                    pb::BtcCoin::Tbtc
                },
                &Keypath::from(path),
                if self.network == bitcoin::Network::Bitcoin {
                    pb::btc_pub_request::XPubType::Xpub
                } else {
                    pb::btc_pub_request::XPubType::Tpub
                },
                self.display_xpub,
            )
            .await
            .map_err(|e| HWIError::Device(e.to_string()))?;
        Ok(Xpub::from_str(&fg).map_err(|e| HWIError::Device(e.to_string()))?)
    }

    async fn display_address(&self, script: &AddressScript) -> Result<(), HWIError> {
        match script {
            AddressScript::P2TR(path) => {
                self.client
                    .btc_address(
                        if self.network == bitcoin::Network::Bitcoin {
                            pb::BtcCoin::Btc
                        } else {
                            pb::BtcCoin::Tbtc
                        },
                        &Keypath::from(path),
                        &make_script_config_simple(pb::btc_script_config::SimpleType::P2tr),
                        true,
                    )
                    .await?;
            }
            AddressScript::Miniscript { index, change } => {
                let policy = self.policy.clone().ok_or_else(|| HWIError::MissingPolicy)?;
                let fg = self.get_master_fingerprint().await?;
                let mut path = DerivationPath::master();
                for (key_index, key) in policy.pubkeys.iter().enumerate() {
                    if Some(fg) == key.master_fingerprint {
                        if let Some(p) = &key.path {
                            path = p.clone();
                        }
                        let (appended_path, wildcard) =
                            extract_first_appended_derivation_with_some_wildcard(
                                key_index,
                                &policy.template,
                            )?;
                        if appended_path.len() >= 2 {
                            path = path.extend(if *change {
                                &appended_path[1]
                            } else {
                                &appended_path[0]
                            });
                        } else if !appended_path.is_empty() {
                            path = path.extend(&appended_path[0]);
                        }
                        if wildcard == bip389::Wildcard::Hardened {
                            let child = ChildNumber::from_hardened_idx(*index)
                                .map_err(|_| HWIError::UnsupportedInput)?;
                            path = path.extend([child]);
                        } else if wildcard == bip389::Wildcard::Unhardened {
                            let child = ChildNumber::from_normal_idx(*index)
                                .map_err(|_| HWIError::UnsupportedInput)?;
                            path = path.extend([child]);
                        }
                        break;
                    }
                }
                self.client
                    .btc_address(
                        if self.network == bitcoin::Network::Bitcoin {
                            pb::BtcCoin::Btc
                        } else {
                            pb::BtcCoin::Tbtc
                        },
                        &Keypath::from(&path),
                        &policy.script_config(fg).0,
                        true,
                    )
                    .await?;
            }
        }
        Ok(())
    }

    async fn register_wallet(
        &self,
        name: &str,
        policy: &str,
    ) -> Result<Option<[u8; 32]>, HWIError> {
        let pb_network = coin_from_network(self.network);
        let policy = extract_script_config_policy(policy)?;
        let fg = self.get_master_fingerprint().await?;
        let (config, keypath) = policy.script_config(fg);
        if self
            .client
            .btc_is_script_config_registered(pb_network, &config, keypath.as_ref())
            .await?
        {
            return Ok(None);
        }
        self.client
            .btc_register_script_config(
                pb_network,
                &config,
                keypath.as_ref(),
                pb::btc_register_script_config_request::XPubType::AutoXpubTpub,
                Some(name),
            )
            .await
            .map(|_| None)
            .map_err(|e| e.into())
    }

    async fn is_wallet_registered(&self, _name: &str, policy: &str) -> Result<bool, HWIError> {
        let pb_network = coin_from_network(self.network);
        let policy = extract_script_config_policy(policy)?;
        let fg = self.get_master_fingerprint().await?;
        let (config, keypath) = policy.script_config(fg);
        self.client
            .btc_is_script_config_registered(pb_network, &config, keypath.as_ref())
            .await
            .map_err(|e| e.into())
    }

    /// Bitbox and Coldcard sign with the first bip32_derivation that matches its fingerprint.
    /// It may be useful to user utils::Bip32DerivationFilter to filter already signed derivations
    /// and derivations collusion in case of multiple spending path per outputs.
    async fn sign_tx(&self, psbt: &mut Psbt) -> Result<(), HWIError> {
        let policy: Option<pb::BtcScriptConfigWithKeypath> =
            if let Some(policy) = self.policy.clone() {
                let mut path = DerivationPath::master();
                let fg = self.get_master_fingerprint().await?;
                for key in &policy.pubkeys {
                    if Some(fg) == key.master_fingerprint {
                        if let Some(p) = &key.path {
                            path = p.clone();
                            break;
                        }
                    }
                }
                Some(pb::BtcScriptConfigWithKeypath {
                    script_config: Some(policy.script_config(fg).0),
                    keypath: Keypath::from(&path).to_vec(),
                })
            } else {
                None
            };

        self.client
            .btc_sign_psbt(
                coin_from_network(self.network),
                psbt,
                policy,
                pb::btc_sign_init_request::FormatUnit::Default,
            )
            .await?;

        Ok(())
    }
}

fn coin_from_network(network: bitcoin::Network) -> pb::BtcCoin {
    if network == bitcoin::Network::Bitcoin {
        pb::BtcCoin::Btc
    } else {
        pb::BtcCoin::Tbtc
    }
}

impl From<UsbError> for HWIError {
    fn from(value: UsbError) -> Self {
        HWIError::Device(value.to_string())
    }
}

impl From<Error> for HWIError {
    fn from(e: Error) -> Self {
        if let Error::BitBox(BitBoxError::UserAbort) = e {
            HWIError::UserRefused
        } else {
            HWIError::Device(e.to_string())
        }
    }
}

impl<T: Runtime + Sync + Send + 'static> From<BitBox02<T>> for Box<dyn HWI + Sync + Send> {
    fn from(s: BitBox02<T>) -> Box<dyn HWI + Sync + Send> {
        Box::new(s)
    }
}

impl<T: Runtime + Sync + Send + 'static> From<BitBox02<T>> for Box<dyn HWI + Send> {
    fn from(s: BitBox02<T>) -> Box<dyn HWI + Send> {
        Box::new(s)
    }
}

impl<T: Runtime + Sync + Send + 'static> From<BitBox02<T>>
    for std::sync::Arc<dyn HWI + Sync + Send>
{
    fn from(s: BitBox02<T>) -> std::sync::Arc<dyn HWI + Sync + Send> {
        std::sync::Arc::new(s)
    }
}

pub fn extract_script_config_policy(policy: &str) -> Result<Policy, HWIError> {
    let re = Regex::new(r"((\[.+?\])?[xyYzZtuUvV]pub[1-9A-HJ-NP-Za-km-z]{79,108})").unwrap();
    let mut descriptor_template = policy.to_string();
    let mut pubkeys_str: Vec<&str> = Vec::new();
    for capture in re.find_iter(policy) {
        if !pubkeys_str.contains(&capture.as_str()) {
            pubkeys_str.push(capture.as_str());
        }
    }

    let mut pubkeys: Vec<KeyInfo> = Vec::new();
    for (i, key_str) in pubkeys_str.iter().enumerate() {
        descriptor_template = descriptor_template.replace(key_str, &format!("@{i}"));
        let pubkey = if let Ok(key) = Xpub::from_str(key_str) {
            KeyInfo {
                path: None,
                master_fingerprint: None,
                xpub: key,
            }
        } else {
            let (keysource_str, xpub_str) = key_str
                .strip_prefix('[')
                .and_then(|s| s.rsplit_once(']'))
                .ok_or(HWIError::InvalidParameter(
                    "policy",
                    "Invalid key source".to_string(),
                ))?;
            let (f_str, path_str) = keysource_str.split_once('/').unwrap_or((keysource_str, ""));
            let fingerprint = Fingerprint::from_str(f_str)
                .map_err(|e| HWIError::InvalidParameter("policy", e.to_string()))?;
            let derivation_path = if path_str.is_empty() {
                DerivationPath::master()
            } else {
                DerivationPath::from_str(&format!("m/{path_str}"))
                    .map_err(|e| HWIError::InvalidParameter("policy", e.to_string()))?
            };

            KeyInfo {
                xpub: Xpub::from_str(xpub_str)
                    .map_err(|e| HWIError::InvalidParameter("policy", e.to_string()))?,
                path: Some(derivation_path),
                master_fingerprint: Some(fingerprint),
            }
        };
        pubkeys.push(pubkey);
    }
    // Do not include the hash in the descriptor template.
    let descriptor_template =
        if let Some((descriptor_template, _hash)) = descriptor_template.rsplit_once('#') {
            descriptor_template
        } else {
            &descriptor_template
        };

    //Ok(
    Ok(Policy {
        template: descriptor_template.to_string(),
        pubkeys,
    })
}

pub fn extract_first_appended_derivation_with_some_wildcard(
    key_index: usize,
    template: &str,
) -> Result<(Vec<DerivationPath>, bip389::Wildcard), HWIError> {
    let re = Regex::new(r"@\d+/[^,)]+").unwrap();
    for capture in re.find_iter(template) {
        if capture.as_str().contains(&format!("@{key_index}")) {
            if let Some((_, appended)) = capture.as_str().split_once('/') {
                let (derivations, wildcard) = bip389::parse_xkey_deriv(appended)?;
                if wildcard != bip389::Wildcard::None {
                    return Ok((derivations, wildcard));
                }
            }
        }
    }
    Ok((Vec::new(), bip389::Wildcard::None))
}

#[derive(Clone)]
pub struct Policy {
    template: String,
    pubkeys: Vec<KeyInfo>,
}

impl Policy {
    /// The script config the BitBox02 registers and signs this policy with,
    /// plus the account keypath it needs alongside it (multisig only).
    ///
    /// BitBox02 firmware rejects `sortedmulti` inside a wallet *policy* config
    /// ("invalid input"): `sortedmulti` is not miniscript. Its native multisig
    /// script config is sortedmulti (BIP67 key order), so a plain
    /// `wsh(sortedmulti(..))` or `sh(wsh(sortedmulti(..)))` over `/<0;1>/*`
    /// keys goes there, keyed on the device's own account (`fingerprint`).
    /// Anything else — and a sortedmulti the device holds no key in — stays a
    /// policy config.
    pub fn script_config(&self, fingerprint: Fingerprint) -> (BtcScriptConfig, Option<Keypath>) {
        if let Some(multisig) = self.sorted_multisig(fingerprint) {
            return multisig;
        }
        let keys: Vec<KeyOriginInfo> = self.pubkeys.iter().cloned().map(|k| k.into()).collect();
        (
            bitbox_api::btc::make_script_config_policy(&self.template, &keys),
            None,
        )
    }

    fn sorted_multisig(
        &self,
        fingerprint: Fingerprint,
    ) -> Option<(BtcScriptConfig, Option<Keypath>)> {
        use pb::btc_script_config::multisig::ScriptType;
        let (script_type, inner) = if let Some(inner) = self
            .template
            .strip_prefix("sh(wsh(sortedmulti(")
            .and_then(|t| t.strip_suffix(")))"))
        {
            (ScriptType::P2wshP2sh, inner)
        } else {
            (
                ScriptType::P2wsh,
                self.template
                    .strip_prefix("wsh(sortedmulti(")?
                    .strip_suffix("))")?,
            )
        };
        let mut parts = inner.split(',');
        let threshold: u32 = parts.next()?.parse().ok()?;
        let keys: Vec<&str> = parts.collect();
        if keys.len() != self.pubkeys.len() {
            return None;
        }
        // Each key must be the next placeholder over the standard
        // receive/change branches: the only derivation BitBox02 multisig uses.
        for (i, key) in keys.iter().enumerate() {
            let rest = key.strip_prefix(&format!("@{i}"))?;
            if rest != "/**" && rest != "/<0;1>/*" {
                return None;
            }
        }
        let our_index = self
            .pubkeys
            .iter()
            .position(|k| k.master_fingerprint == Some(fingerprint) && k.path.is_some())?;
        let keypath = Keypath::from(self.pubkeys[our_index].path.as_ref()?);
        let xpubs: Vec<Xpub> = self.pubkeys.iter().map(|k| k.xpub).collect();
        Some((
            bitbox_api::btc::make_script_config_multisig(
                threshold,
                &xpubs,
                our_index as u32,
                script_type,
            ),
            Some(keypath),
        ))
    }
}

#[derive(Clone)]
pub struct KeyInfo {
    xpub: Xpub,
    path: Option<DerivationPath>,
    master_fingerprint: Option<Fingerprint>,
}

impl From<KeyInfo> for KeyOriginInfo {
    fn from(info: KeyInfo) -> KeyOriginInfo {
        KeyOriginInfo {
            root_fingerprint: info.master_fingerprint,
            keypath: info.path.as_ref().map(Keypath::from),
            xpub: info.xpub,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_script_config_policy() {
        let policy = extract_script_config_policy("wsh(or_d(pk([f5acc2fd/49'/1'/0']tpubDCbK3Ysvk8HjcF6mPyrgMu3KgLiaaP19RjKpNezd8GrbAbNg6v5BtWLaCt8FNm6QkLseopKLf5MNYQFtochDTKHdfgG6iqJ8cqnLNAwtXuP/**),and_v(v:pkh(tpubDDtb2WPYwEWw2WWDV7reLV348iJHw2HmhzvPysKKrJw3hYmvrd4jasyoioVPdKGQqjyaBMEvTn1HvHWDSVqQ6amyyxRZ5YjpPBBGjJ8yu8S/**),older(100))))").unwrap();
        assert_eq!(2, policy.pubkeys.len());
        assert_eq!(
            "wsh(or_d(pk(@0/**),and_v(v:pkh(@1/**),older(100))))",
            policy.template
        );

        let policy = extract_script_config_policy("wsh(or_d(multi(2,[b0822927/48'/1'/0'/2']tpubDEvZxV86Br8Knbm9tWcr5Hvmg5cYTYsg92vinqH6Bie6U8ix8CsoN9W11NQygdqVwmHUJpsHXxNsi5gXn36g4xNfLWkMqPuFhRZAmMQ7jjQ/<0;1>/*,[7fc39c07/48'/1'/0'/2']tpubDEvjgXtrUuH3Qtkapny9aE8gN847xiXsf9MDM5XueGf9nrvStqAuBSva3ajGyTvtp8Ti55FvVXsgYSXuS1tQkBeopFuodx2hRUDmQbvKxbZ/<0;1>/*),and_v(v:thresh(2,pkh([b0822927/48'/1'/0'/2']tpubDEvZxV86Br8Knbm9tWcr5Hvmg5cYTYsg92vinqH6Bie6U8ix8CsoN9W11NQygdqVwmHUJpsHXxNsi5gXn36g4xNfLWkMqPuFhRZAmMQ7jjQ/<2;3>/*),a:pkh([7fc39c07/48'/1'/0'/2']tpubDEvjgXtrUuH3Qtkapny9aE8gN847xiXsf9MDM5XueGf9nrvStqAuBSva3ajGyTvtp8Ti55FvVXsgYSXuS1tQkBeopFuodx2hRUDmQbvKxbZ/<2;3>/*),a:pkh([1a1ffd98/48'/1'/0'/2']tpubDFZqzTvGijYb13BC73CkS1er8DrP5YdzMhziN3kWCKUFaW51Yj6ggvf99YpdrkTJy4RT85mxQMHXDiFAKRxzf6BykQgT4pRRBNPshSJJcKo/<0;1>/*)),older(300))))#wp0w3hlw").unwrap();
        assert_eq!(3, policy.pubkeys.len());
        assert_eq!(
                "wsh(or_d(multi(2,@0/<0;1>/*,@1/<0;1>/*),and_v(v:thresh(2,pkh(@0/<2;3>/*),a:pkh(@1/<2;3>/*),a:pkh(@2/<0;1>/*)),older(300))))",
                policy.template
            );
    }

    #[test]
    fn sortedmulti_uses_the_native_multisig_config() {
        use pb::btc_script_config::{multisig::ScriptType, Config};
        let x = [
            "[b0822927/48'/1'/0'/2']tpubDEvZxV86Br8Knbm9tWcr5Hvmg5cYTYsg92vinqH6Bie6U8ix8CsoN9W11NQygdqVwmHUJpsHXxNsi5gXn36g4xNfLWkMqPuFhRZAmMQ7jjQ",
            "[7fc39c07/48'/1'/0'/2']tpubDEvjgXtrUuH3Qtkapny9aE8gN847xiXsf9MDM5XueGf9nrvStqAuBSva3ajGyTvtp8Ti55FvVXsgYSXuS1tQkBeopFuodx2hRUDmQbvKxbZ",
            "[1a1ffd98/48'/1'/0'/2']tpubDFZqzTvGijYb13BC73CkS1er8DrP5YdzMhziN3kWCKUFaW51Yj6ggvf99YpdrkTJy4RT85mxQMHXDiFAKRxzf6BykQgT4pRRBNPshSJJcKo",
        ];
        let ours = Fingerprint::from_str("7fc39c07").unwrap();
        for (wrapped, script_type) in [
            (
                format!(
                    "wsh(sortedmulti(2,{0}/<0;1>/*,{1}/<0;1>/*,{2}/<0;1>/*))",
                    x[0], x[1], x[2]
                ),
                ScriptType::P2wsh,
            ),
            (
                format!(
                    "sh(wsh(sortedmulti(2,{0}/**,{1}/**,{2}/**)))",
                    x[0], x[1], x[2]
                ),
                ScriptType::P2wshP2sh,
            ),
        ] {
            let policy = extract_script_config_policy(&wrapped).unwrap();
            let (config, keypath) = policy.script_config(ours);
            let Some(Config::Multisig(m)) = config.config else {
                panic!("expected a multisig config for {}", wrapped);
            };
            assert_eq!(m.threshold, 2);
            assert_eq!(m.xpubs.len(), 3);
            assert_eq!(m.our_xpub_index, 1);
            assert_eq!(m.script_type, script_type as i32);
            let expected = DerivationPath::from_str("m/48'/1'/0'/2'").unwrap();
            assert_eq!(keypath.unwrap().to_vec(), Keypath::from(&expected).to_vec());
        }

        // Not one of our keys, or not sortedmulti: stays a policy config.
        let sorted = format!("wsh(sortedmulti(2,{0}/<0;1>/*,{1}/<0;1>/*))", x[0], x[2]);
        let (config, keypath) = extract_script_config_policy(&sorted)
            .unwrap()
            .script_config(ours);
        assert!(matches!(config.config, Some(Config::Policy(_))));
        assert!(keypath.is_none());
        let multi = format!("wsh(multi(2,{0}/<0;1>/*,{1}/<0;1>/*))", x[0], x[1]);
        let (config, keypath) = extract_script_config_policy(&multi)
            .unwrap()
            .script_config(ours);
        assert!(matches!(config.config, Some(Config::Policy(_))));
        assert!(keypath.is_none());
    }

    #[test]
    fn test_extract_first_appended_derivation_with_some_wildcard() {
        let (paths, wildcard) = extract_first_appended_derivation_with_some_wildcard(
            1,
            "wsh(or_d(pk(@0/**),and_v(v:pkh(@1/1/**),older(100))))",
        )
        .unwrap();
        assert_eq!(wildcard, bip389::Wildcard::Unhardened);
        assert_eq!(
            paths,
            vec![
                DerivationPath::from_str("m/1/0").unwrap(),
                DerivationPath::from_str("m/1/1").unwrap()
            ],
        );
        let (paths, wildcard) = extract_first_appended_derivation_with_some_wildcard(
            0,
            "wsh(or_d(multi(2,@0/<8;9>/*,@1/<0;1>/*),and_v(v:thresh(2,pkh(@0/<2;3>/*),a:pkh(@1/<2;3>/*),a:pkh(@2/2/<3;4;5>/*)),older(300))))",
        )
        .unwrap();
        assert_eq!(wildcard, bip389::Wildcard::Unhardened);
        assert_eq!(
            paths,
            vec![
                DerivationPath::from_str("m/8").unwrap(),
                DerivationPath::from_str("m/9").unwrap(),
            ],
        );
        let (paths, wildcard) = extract_first_appended_derivation_with_some_wildcard(
            1,
            "wsh(or_d(multi(2,@0/<8;9>/*,@1/<0;1>/*),and_v(v:thresh(2,pkh(@0/<2;3>/*),a:pkh(@1/<2;3>/*),a:pkh(@2/2/<3;4;5>/*)),older(300))))",
        )
        .unwrap();
        assert_eq!(wildcard, bip389::Wildcard::Unhardened);
        assert_eq!(
            paths,
            vec![
                DerivationPath::from_str("m/0").unwrap(),
                DerivationPath::from_str("m/1").unwrap(),
            ],
        );
        let (paths, wildcard) = extract_first_appended_derivation_with_some_wildcard(
            2,
            "wsh(or_d(multi(2,@0/<0;1>/*,@1/<0;1>/*),and_v(v:thresh(2,pkh(@0/<2;3>/*),a:pkh(@1/<2;3>/*),a:pkh(@2/2/<3;4;5>/*)),older(300))))",
        )
        .unwrap();
        assert_eq!(wildcard, bip389::Wildcard::Unhardened);
        assert_eq!(
            paths,
            vec![
                DerivationPath::from_str("m/2/3").unwrap(),
                DerivationPath::from_str("m/2/4").unwrap(),
                DerivationPath::from_str("m/2/5").unwrap()
            ],
        );
    }
}
#[cfg(test)]
mod op_return_claim_tests {
    #[test]
    fn bitbox_client_accepts_the_claim_markers_87_byte_push() {
        let mut script = vec![0x6a, 0x4c, 87];
        script.extend_from_slice(&[0x42; 87]);
        let payload = bitbox_api::btc::Payload::from_pkscript(&script).unwrap();
        assert_eq!(payload.data, vec![0x42; 87]);
        assert_eq!(payload.output_type, bitbox_api::pb::BtcOutputType::OpReturn);
    }
}
