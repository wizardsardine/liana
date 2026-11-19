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
    /// The miniscript wallet [`HWI::find_wallet_registration`] last found
    /// for `wallet_name`'s policy under another name. The next
    /// [`HWI::sign_tx`] signs against it and consumes it.
    found: Arc<Mutex<Option<String>>>,
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
            if registered.is_some_and(|registered| registered_descriptor_is(&registered, policy)) {
                return Ok(Some(WalletRegistration { name: Some(other) }));
            }
        }
        // The multisig registry is not searched: `multisig_check` matches
        // only M, N and the XOR of the fingerprints, so another wallet on
        // the same devices (another account, say) would pass for this one,
        // and no silent command reads an entry's xpubs to tell them apart.
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

    /// Searches the named miniscript wallets, `name` first. A match under
    /// another name is remembered, so this handle's [`HWI::sign_tx`] signs
    /// against it.
    async fn find_wallet_registration(
        &self,
        name: &str,
        policy: &str,
    ) -> Result<Option<WalletRegistration>, HWIError> {
        // Cleared first, so a failed search never leaves an earlier match
        // for the next signature.
        *self
            .found
            .lock()
            .map_err(|_| HWIError::Unexpected("Failed to unlock"))? = None;
        let found = self.search_registrations(name, policy).await?;
        *self
            .found
            .lock()
            .map_err(|_| HWIError::Unexpected("Failed to unlock"))? = match &found {
            Some(WalletRegistration { name: Some(other) }) if other != name => Some(other.clone()),
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

        let wallet_name = found
            .or_else(|| self.wallet_name.clone())
            .map(|name| {
                DescriptorName::new(name)
                    .map_err(|_| HWIError::Unexpected("Coldcard: Invalid wallet name"))
            })
            .transpose()?;

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

/// Whether the descriptor of a miniscript wallet under another name is
/// exactly `policy`, hardened markers and checksum aside. Exact, not
/// [`registered_descriptor_matches`]'s substring test, which would take a
/// `sh(wsh(P))` wallet for `wsh(P)`. The reply is either the descriptor or
/// a JSON object carrying it as `"desc"`.
fn registered_descriptor_is(registered: &str, policy: &str) -> bool {
    let registered = registered.trim();
    let descriptor = json_string_field(registered, "desc").unwrap_or(registered);
    utils::same_descriptor(descriptor, policy)
}

/// The value of a top-level `"key": "value"` string in a flat JSON object
/// whose strings hold no escapes, as Coldcard's descriptor replies are.
fn json_string_field<'a>(json: &'a str, key: &str) -> Option<&'a str> {
    if !json.starts_with('{') {
        return None;
    }
    let after_key = &json[json.find(&format!("\"{key}\""))? + key.len() + 2..];
    let value = after_key.trim_start().strip_prefix(':')?.trim_start();
    let value = value.strip_prefix('"')?;
    value.split_once('"').map(|(value, _)| value)
}

/// Whether the descriptor Coldcard returned under COINCUBE's own name holds
/// `policy` (its checksum aside, with `h` for hardened steps, as Coldcard
/// writes them). The check COINCUBE always made on its own registration.
fn registered_descriptor_matches(registered: &str, policy: &str) -> bool {
    let policy = policy.replace('\'', "h");
    let policy = policy.split_once('#').map_or(policy.as_str(), |(p, _)| p);
    registered.contains(policy)
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
    fn another_wallet_must_be_exactly_the_policy() {
        let policy = format!(
            "wsh(sortedmulti(2,{}/<0;1>/*,{}/<0;1>/*))#abcdefgh",
            KEYS[0], KEYS[1]
        );
        let bare = policy.split_once('#').unwrap().0.replace('\'', "h");
        assert!(registered_descriptor_is(&bare, &policy));
        assert!(registered_descriptor_is(
            &format!("{{\"name\":\"Specter\",\"desc\":\"{bare}#zzzzzzzz\"}}"),
            &policy
        ));
        // Nested segwit over the same script is another wallet.
        let nested = format!("sh({bare})");
        assert!(registered_descriptor_matches(&nested, &policy));
        assert!(!registered_descriptor_is(&nested, &policy));
        assert!(!registered_descriptor_is(
            &format!("{{\"name\":\"Specter\",\"desc\":\"{nested}\"}}"),
            &policy
        ));
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
