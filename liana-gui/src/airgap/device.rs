use liana::miniscript::{bitcoin::bip32::Fingerprint, descriptor::DescriptorPublicKey};
use serde::{Deserialize, Serialize};

use super::{Error, PassportAccount};

/// A persisted air-gapped signer. Only public account material is stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AirgappedSignerConfig {
    pub kind: AirgappedSignerKind,
    pub fingerprint: Fingerprint,
    pub alias: Option<String>,
    pub account: DescriptorPublicKey,
    #[serde(default)]
    pub registration: RegistrationState,
}

impl AirgappedSignerConfig {
    pub fn qr(account: PassportAccount, alias: Option<String>) -> Result<Self, Error> {
        let origin_fingerprint = match &account.account {
            DescriptorPublicKey::XPub(xpub) => {
                xpub.origin.as_ref().map(|(fingerprint, _)| *fingerprint)
            }
            _ => None,
        }
        .ok_or_else(|| Error::InvalidAccount("extended key origin is required".to_owned()))?;
        if origin_fingerprint != account.fingerprint {
            return Err(Error::InvalidFingerprint);
        }
        Ok(Self {
            kind: AirgappedSignerKind::Qr,
            fingerprint: account.fingerprint,
            alias,
            account: account.account,
            registration: RegistrationState::NotRegistered,
        })
    }

    pub fn invalidate_registration(&mut self, descriptor_checksum: &str) {
        if !self.registration.is_current(descriptor_checksum) {
            self.registration = RegistrationState::NotRegistered;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AirgappedSignerKind {
    #[serde(alias = "passport")]
    Qr,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RegistrationState {
    #[default]
    NotRegistered,
    Exported {
        descriptor_checksum: String,
    },
}

impl RegistrationState {
    pub fn is_current(&self, descriptor_checksum: &str) -> bool {
        match self {
            Self::NotRegistered => false,
            Self::Exported {
                descriptor_checksum: registered,
            } => registered == descriptor_checksum,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use liana::miniscript::bitcoin::Network;

    use super::*;

    const ACCOUNT: &str = "[9f141cf0/48'/1'/0'/2']tpubDFnReAwXvYd6RA46X55HuFpmvZsLanDrwHAUsdYEGEpNGTRnCdbDRXJGLTwDeqKURCPZUDgdkuuu9dYkuBNQHmSNBUu7V2CdLKwpJjx2JuC";

    #[test]
    fn signer_config_roundtrips_without_secret_material() {
        let account = PassportAccount::from_descriptor_key(ACCOUNT, Network::Testnet4).unwrap();
        let mut signer = AirgappedSignerConfig::qr(account, Some("Recovery".to_owned())).unwrap();
        signer.registration = RegistrationState::Exported {
            descriptor_checksum: "u768v50p".to_owned(),
        };

        let json = serde_json::to_string(&signer).unwrap();
        assert!(!json.contains("xprv"));
        assert!(!json.contains("tprv"));
        assert_eq!(
            serde_json::from_str::<AirgappedSignerConfig>(&json).unwrap(),
            signer
        );
    }

    #[test]
    fn stale_registration_is_invalidated() {
        let account = PassportAccount::from_descriptor_key(ACCOUNT, Network::Testnet4).unwrap();
        let mut signer = AirgappedSignerConfig::qr(account, None).unwrap();
        signer.registration = RegistrationState::Exported {
            descriptor_checksum: "u768v50p".to_owned(),
        };
        signer.invalidate_registration("aaaaaaaa");
        assert_eq!(signer.registration, RegistrationState::NotRegistered);
    }

    #[test]
    fn persisted_account_origin_remains_parseable() {
        let account = DescriptorPublicKey::from_str(ACCOUNT).unwrap();
        let encoded = serde_json::to_string(&account).unwrap();
        assert_eq!(
            serde_json::from_str::<DescriptorPublicKey>(&encoded).unwrap(),
            account
        );
    }

    #[test]
    fn legacy_passport_kind_migrates_to_generic_qr_kind() {
        let legacy = format!(
            r#"{{"kind":"passport","fingerprint":"9f141cf0","alias":"Cold signer","account":"{ACCOUNT}","registration":{{"state":"not_registered"}}}}"#
        );
        let signer: AirgappedSignerConfig = serde_json::from_str(&legacy).unwrap();
        assert_eq!(signer.kind, AirgappedSignerKind::Qr);
        assert!(serde_json::to_string(&signer)
            .unwrap()
            .contains(r#""kind":"qr""#));
    }
}
