use std::{
    str::FromStr,
    sync::{Arc, Mutex, MutexGuard},
};

use async_trait::async_trait;
use bitcoin::{
    bip32::{DerivationPath, Fingerprint, Xpub},
    psbt::Psbt,
};
use coldcard::protocol::DescriptorName;
use hidapi::DeviceInfo;

use crate::{
    parse_version, utils, AddressScript, DeviceKind, Error as HWIError, Version,
    WalletRegistration, HWI,
};
pub use coldcard as api;

#[derive(Debug)]
pub struct Coldcard {
    device: Arc<Mutex<coldcard::Coldcard>>,
    wallet_name: Option<String>,
    /// The registration [`HWI::find_wallet_registration`] last found for
    /// `wallet_name`'s policy, when it is not the one under `wallet_name`:
    /// `Some(Some(name))` signs against that miniscript wallet,
    /// `Some(None)` against the multisig registry (no name). The next
    /// [`HWI::sign_tx`] consumes it.
    found: Arc<Mutex<Option<Option<String>>>>,
}

impl Coldcard {
    pub fn with_wallet_name(mut self, wallet_name: String) -> Self {
        self.wallet_name = Some(wallet_name);
        self
    }

    async fn search_registrations(
        &self,
        name: &str,
        policy: &str,
    ) -> Result<Option<WalletRegistration>, HWIError> {
        if self.is_wallet_registered(name, policy).await? {
            return Ok(Some(WalletRegistration {
                name: Some(name.to_string()),
            }));
        }
        // The wider search is best effort: firmware without these commands
        // just finds nothing more, and the caller registers as before.
        let names = self.device()?.miniscript_list().unwrap_or_default();
        for other in names.into_iter().filter(|other| other != name) {
            let Ok(descriptor_name) = DescriptorName::new(&other) else {
                continue;
            };
            let registered = self
                .device()?
                .miniscript_get(descriptor_name)
                .ok()
                .flatten();
            if registered
                .is_some_and(|registered| registered_descriptor_matches(&registered, policy))
            {
                return Ok(Some(WalletRegistration { name: Some(other) }));
            }
        }
        if let Some((m, n, xfp_xor)) = multisig_check_args(policy) {
            if self
                .device()?
                .multisig_check(m, n, xfp_xor)
                .unwrap_or(false)
            {
                return Ok(Some(WalletRegistration { name: None }));
            }
        }
        Ok(None)
    }

    fn device(&self) -> Result<MutexGuard<'_, coldcard::Coldcard>, HWIError> {
        self.device
            .lock()
            .map_err(|_| HWIError::Unexpected("Failed to unlock"))
    }
}

impl From<coldcard::Coldcard> for Coldcard {
    fn from(cc: coldcard::Coldcard) -> Self {
        Coldcard {
            device: Arc::new(Mutex::new(cc)),
            wallet_name: None,
            found: Arc::new(Mutex::new(None)),
        }
    }
}

#[async_trait]
impl HWI for Coldcard {
    fn device_kind(&self) -> DeviceKind {
        DeviceKind::Coldcard
    }

    /// The first semver version returned by coldcard is the firmware version.
    async fn get_version(&self) -> Result<Version, HWIError> {
        let s = self.device()?.version()?;
        for line in s.split('\n') {
            if let Ok(version) = parse_version(line) {
                return Ok(version);
            }
        }
        Err(HWIError::UnsupportedVersion)
    }

    async fn get_master_fingerprint(&self) -> Result<Fingerprint, HWIError> {
        let s = self.device()?.xpub(None)?;
        let xpub = Xpub::from_str(&s).map_err(|e| HWIError::Device(e.to_string()))?;
        Ok(xpub.fingerprint())
    }

    async fn get_extended_pubkey(&self, path: &DerivationPath) -> Result<Xpub, HWIError> {
        let path = path.to_string();
        let path = if path.starts_with("m/") {
            path
        } else {
            format!("m/{path}")
        };
        let path = coldcard::protocol::DerivationPath::new(&path)
            .map_err(|e| HWIError::InvalidParameter("path", format!("{e:?}")))?;
        let s = self.device()?.xpub(Some(path))?;
        Xpub::from_str(&s).map_err(|e| HWIError::Device(e.to_string()))
    }

    async fn display_address(&self, script: &AddressScript) -> Result<(), HWIError> {
        if let Some(name) = &self.wallet_name {
            let descriptor_name = coldcard::protocol::DescriptorName::new(name)
                .map_err(|_| HWIError::UnsupportedInput)?;
            if let AddressScript::Miniscript { index, change } = script {
                self.device()?
                    .miniscript_address(descriptor_name, *change, *index)?;
                Ok(())
            } else {
                Err(HWIError::UnimplementedMethod)
            }
        } else {
            Err(HWIError::UnimplementedMethod)
        }
    }

    async fn register_wallet(
        &self,
        name: &str,
        policy: &str,
    ) -> Result<Option<[u8; 32]>, HWIError> {
        let payload = format!("{{\"name\":\"{name}\",\"desc\":\"{policy}\"}}");
        let _ = self.device()?.miniscript_enroll(payload.as_bytes())?;
        Ok(None)
    }

    async fn is_wallet_registered(&self, name: &str, policy: &str) -> Result<bool, HWIError> {
        let descriptor_name = coldcard::protocol::DescriptorName::new(name)
            .map_err(|_| HWIError::UnsupportedInput)?;
        let desc = self.device()?.miniscript_get(descriptor_name)?;
        Ok(desc.is_some_and(|desc| registered_descriptor_matches(&desc, policy)))
    }

    /// Searches the named miniscript wallets (`name` first), then, for a
    /// sortedmulti policy, the multisig registry. A match under another name
    /// is remembered, so this handle's [`HWI::sign_tx`] signs against it.
    async fn find_wallet_registration(
        &self,
        name: &str,
        policy: &str,
    ) -> Result<Option<WalletRegistration>, HWIError> {
        let found = self.search_registrations(name, policy).await?;
        *self
            .found
            .lock()
            .map_err(|_| HWIError::Unexpected("Failed to unlock"))? = match &found {
            Some(registration) if registration.name.as_deref() != Some(name) => {
                Some(registration.name.clone())
            }
            _ => None,
        };
        Ok(found)
    }

    async fn sign_tx(&self, psbt: &mut Psbt) -> Result<(), HWIError> {
        // Used for this signature only: the next signature on this handle
        // must be preceded by its own lookup.
        let found = self
            .found
            .lock()
            .map_err(|_| HWIError::Unexpected("Failed to unlock"))?
            .take();
        let mut cc = self.device()?;

        let wallet_name = if let Some(found) = found {
            match found {
                Some(name) => Some(
                    DescriptorName::new(name)
                        .map_err(|_| HWIError::Unexpected("Coldcard: Invalid wallet name"))?,
                ),
                None => None,
            }
        } else if let Some(name) = self.wallet_name.clone() {
            Some(
                DescriptorName::new(name)
                    .map_err(|_| HWIError::Unexpected("Coldcard: Invalid wallet name"))?,
            )
        } else {
            None
        };

        let _ = cc.sign_psbt_miniscript(&psbt.serialize(), api::SignMode::Signed, wallet_name)?;

        let tx = loop {
            if let Some(tx) = cc.get_signed_tx()? {
                break tx;
            }
        };

        let mut new_psbt = Psbt::deserialize(&tx).map_err(|e| HWIError::Device(e.to_string()))?;

        for i in 0..new_psbt.inputs.len() {
            psbt.inputs[i]
                .partial_sigs
                .append(&mut new_psbt.inputs[i].partial_sigs);
            psbt.inputs[i]
                .tap_script_sigs
                .append(&mut new_psbt.inputs[i].tap_script_sigs);
            if let Some(sig) = new_psbt.inputs[i].tap_key_sig {
                psbt.inputs[i].tap_key_sig = Some(sig);
            }
        }

        Ok(())
    }
}

impl From<api::Error> for HWIError {
    fn from(e: api::Error) -> Self {
        if let api::Error::UnexpectedResponse(api::protocol::Response::Refused) = e {
            HWIError::UserRefused
        } else {
            HWIError::Device(e.to_string())
        }
    }
}

impl From<Coldcard> for Box<dyn HWI + Send> {
    fn from(s: Coldcard) -> Box<dyn HWI + Send> {
        Box::new(s)
    }
}

impl From<Coldcard> for Arc<dyn HWI + Sync + Send> {
    fn from(s: Coldcard) -> Arc<dyn HWI + Sync + Send> {
        Arc::new(s)
    }
}

/// Whether a descriptor the Coldcard returned is `policy` (its checksum
/// aside, with `h` for hardened steps, as Coldcard writes them).
fn registered_descriptor_matches(registered: &str, policy: &str) -> bool {
    let policy = policy.replace('\'', "h");
    let policy = policy.split_once('#').map_or(policy.as_str(), |(p, _)| p);
    registered.contains(policy)
}

/// `M`, `N` and the XOR of the key fingerprints, for a sortedmulti policy
/// whose keys all carry an origin: what the Coldcard multisig registry
/// (sortedmulti only, BIP67) is looked up by. Coldcard reads each 4-byte
/// fingerprint as a little-endian `u32`. The device checks the full xpubs
/// again when it signs.
fn multisig_check_args(policy: &str) -> Option<(u32, u32, u32)> {
    let multisig = utils::MultisigPolicy::parse(policy)?;
    if !multisig.sorted {
        return None;
    }
    let mut xfp_xor = 0;
    for (fingerprint, _) in &multisig.keys {
        xfp_xor ^= u32::from_le_bytes(fingerprint.as_ref()?.to_bytes());
    }
    Some((multisig.threshold, multisig.keys.len() as u32, xfp_xor))
}

pub fn is_coldcard(device_info: &DeviceInfo) -> bool {
    device_info.vendor_id() == api::COINKITE_VID && device_info.product_id() == api::CKCC_PID
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEYS: [&str; 3] = [
        "[b0822927/48'/1'/0'/2']tpubDEvZxV86Br8Knbm9tWcr5Hvmg5cYTYsg92vinqH6Bie6U8ix8CsoN9W11NQygdqVwmHUJpsHXxNsi5gXn36g4xNfLWkMqPuFhRZAmMQ7jjQ",
        "[7fc39c07/48'/1'/0'/2']tpubDEvjgXtrUuH3Qtkapny9aE8gN847xiXsf9MDM5XueGf9nrvStqAuBSva3ajGyTvtp8Ti55FvVXsgYSXuS1tQkBeopFuodx2hRUDmQbvKxbZ",
        "[1a1ffd98/48'/1'/0'/2']tpubDFZqzTvGijYb13BC73CkS1er8DrP5YdzMhziN3kWCKUFaW51Yj6ggvf99YpdrkTJy4RT85mxQMHXDiFAKRxzf6BykQgT4pRRBNPshSJJcKo",
    ];

    #[test]
    fn multisig_check_uses_little_endian_fingerprint_xor() {
        let policy = format!(
            "wsh(sortedmulti(2,{}/<0;1>/*,{}/<0;1>/*,{}/<0;1>/*))",
            KEYS[0], KEYS[1], KEYS[2]
        );
        let xor = u32::from_le_bytes([0xb0, 0x82, 0x29, 0x27])
            ^ u32::from_le_bytes([0x7f, 0xc3, 0x9c, 0x07])
            ^ u32::from_le_bytes([0x1a, 0x1f, 0xfd, 0x98]);
        assert_eq!(multisig_check_args(&policy), Some((2, 3, xor)));
        // Coldcard's multisig registry is sortedmulti only.
        assert_eq!(
            multisig_check_args(&policy.replace("sortedmulti", "multi")),
            None
        );
    }

    #[test]
    fn registered_descriptor_match_ignores_marker_and_checksum() {
        let policy = format!(
            "wsh(sortedmulti(2,{}/<0;1>/*,{}/<0;1>/*))#abcdefgh",
            KEYS[0], KEYS[1]
        );
        let registered = policy.replace('\'', "h").replace("#abcdefgh", "#zzzzzzzz");
        assert!(registered_descriptor_matches(&registered, &policy));
        assert!(!registered_descriptor_matches(
            &registered,
            &policy.replace("2,", "1,")
        ));
    }
}
