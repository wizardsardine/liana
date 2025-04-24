use serde::{de, Deserialize};

use liana::miniscript::descriptor::DescriptorPublicKey;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Copy)]
pub enum KeyKind {
    SafetyNet,
    Cosigner,
}

impl std::fmt::Display for KeyKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeyKind::SafetyNet => write!(f, "Safety Net"),
            KeyKind::Cosigner => write!(f, "Cosigner"),
        }
    }
}

impl<'de> Deserialize<'de> for KeyKind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        match s.as_str() {
            "safetynet" => Ok(KeyKind::SafetyNet),
            "cosigner" => Ok(KeyKind::Cosigner),
            s => Err(de::Error::custom(format!(
                "invalid value for KeyKind: '{}'",
                s
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyStatus {
    NotFetched,
    Fetched,
    Redeemed,
}

impl<'de> Deserialize<'de> for KeyStatus {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        match s.as_str() {
            "not-fetched" => Ok(KeyStatus::NotFetched),
            "fetched" => Ok(KeyStatus::Fetched),
            "redeemed" => Ok(KeyStatus::Redeemed),
            s => Err(de::Error::custom(format!(
                "invalid value for KeyStatus: '{}'",
                s
            ))),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Provider {
    pub uuid: String,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Key {
    pub provider: Provider,
    pub uuid: String,
    pub kind: KeyKind,
    pub status: KeyStatus,
    pub xpub: DescriptorPublicKey,
}

impl Key {
    pub fn is_redeemed(&self) -> bool {
        matches!(self.status, KeyStatus::Redeemed)
    }
}
