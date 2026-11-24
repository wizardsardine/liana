use liana::miniscript::bitcoin::{bip32::Fingerprint, Network};
use lianad::bip329;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fmt::{Debug, Display},
};

use crate::{backup::bip139::Bip139Backup, daemon::DaemonError, VERSION};

pub mod bip139;

pub const SETTINGS_KEY: &str = "settings";

pub fn liana_version() -> String {
    strip_version_suffix(VERSION).to_string()
}

fn strip_version_suffix(version: &str) -> &str {
    version.split_once('-').map_or(version, |(v, _)| v)
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct LegacyBackup {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    pub accounts: Vec<LegacyAccount>,
    pub network: Network,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<u64>,
    /// App proprietary metadata (settings, configuration, etc..)
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub proprietary: serde_json::Map<String, serde_json::Value>,
    #[serde(default = "default_version")]
    pub version: u32,
}

fn default_version() -> u32 {
    0
}

#[derive(Debug, Clone)]
pub enum Error {
    DescriptorMissing,
    NotSingleWallet,
    Json,
    SettingsFromFile,
    Daemon(String),
    Bip139(String),
}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::DescriptorMissing => write!(f, "Backup: descriptor missing"),
            Error::NotSingleWallet => write!(f, "Backup: Zero or several wallets"),
            Error::Json => write!(f, "Backup: json error"),
            Error::SettingsFromFile => write!(f, "Backup: fail to parse setting from file"),
            Error::Daemon(e) => write!(f, "Backup daemon error: {e}"),
            Error::Bip139(e) => write!(f, "Backup: {e}"),
        }
    }
}

impl From<DaemonError> for Error {
    fn from(value: DaemonError) -> Self {
        Error::Daemon(value.to_string())
    }
}

impl Display for LegacyBackup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let str = serde_json::to_string(self).map_err(|_| std::fmt::Error)?;
        write!(f, "{str}")
    }
}

impl Debug for LegacyBackup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let str = serde_json::to_string_pretty(self).map_err(|_| std::fmt::Error)?;
        write!(f, "{str}")
    }
}

pub fn read(text: &str) -> Result<(Bip139Backup, Vec<String>), Error> {
    let value: Value = serde_json::from_str(text).map_err(|_| Error::Json)?;
    let bip139 = value
        .get("version")
        .and_then(Value::as_str)
        .is_some_and(|version| version.starts_with(bip139::VERSION_PREFIX));
    let mut backup = if bip139 {
        let backup: Bip139Backup =
            serde_json::from_value(value).map_err(|e| Error::Bip139(e.to_string()))?;
        backup.validate()?;
        backup
    } else {
        let backup: LegacyBackup = serde_json::from_value(value).map_err(|_| Error::Json)?;
        Bip139Backup::from_legacy(backup)
    };
    let warnings = backup.retain_usable_accounts()?;
    Ok((backup, warnings))
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct LegacyAccount {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub descriptor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receive_index: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_index: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<u64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub keys: BTreeMap<Fingerprint, Key>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<bip329::Labels>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transactions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub psbts: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub coins: BTreeMap<String, LegacyCoin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain_tip: Option<LegacyChainTip>,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub proprietary: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct LegacyChainTip {
    pub block_height: i32,
    pub block_hash: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct LegacyCoin {
    amount: u64,
    outpoint: String,
    address: String,
    block_height: Option<i32>,
    account: u32,
    derivation_index: u32,
    is_coinbase: Option<bool>,
    is_from_self: Option<bool>,
}

impl LegacyAccount {
    pub fn new(descriptor: String) -> Self {
        Self {
            name: None,
            descriptor,
            receive_index: None,
            change_index: None,
            timestamp: None,
            keys: BTreeMap::new(),
            labels: None,
            transactions: Vec::new(),
            psbts: Vec::new(),
            coins: BTreeMap::new(),
            proprietary: serde_json::Map::new(),
            chain_tip: None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct Key {
    pub key: Fingerprint,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<KeyRole>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_type: Option<KeyType>,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub proprietary: Value,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
pub enum KeyRole {
    /// Key to be used in normal spending condition
    Main,
    /// Key that will be used for recover in case loss of main key(s)
    Recovery,
    /// Key that will inherit coins if main user disappears
    Inheritance,
    /// Key that will cosign a spend in order to enforce some policy
    Cosigning,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
pub enum KeyType {
    /// Main user
    Internal,
    /// Heirs or friends
    External,
    /// Service the user pay for
    ThirdParty,
}

#[cfg(test)]
mod test {
    use super::*;

    fn round_trip(backup: &LegacyBackup) -> bool {
        let serialized = serde_json::to_string(backup).unwrap();
        let parsed: LegacyBackup = serde_json::from_str(&serialized).unwrap();
        *backup == parsed
    }

    #[test]
    fn version_suffix() {
        assert_eq!(strip_version_suffix("16.0"), "16.0");
        assert_eq!(strip_version_suffix("16.0-dev"), "16.0");
        assert_eq!(strip_version_suffix("16.0-rc1"), "16.0");
        assert_eq!(strip_version_suffix("16.0-rc10"), "16.0");
    }

    #[test]
    fn backup_serde() {
        let mut backup = LegacyBackup {
            name: None,
            alias: None,
            accounts: Vec::new(),
            network: Network::Signet,
            date: Some(0),
            proprietary: serde_json::Map::new(),
            version: 0,
        };
        let serialized = serde_json::to_string(&backup).unwrap();
        let expected = r#"{"accounts":[],"network":"signet","date":0,"version":0}"#;
        assert_eq!(expected, serialized);
        assert!(round_trip(&backup));

        backup.name = Some("Liana".into());

        let serialized = serde_json::to_string(&backup).unwrap();
        let expected = r#"{"name":"Liana","accounts":[],"network":"signet","date":0,"version":0}"#;
        assert_eq!(expected, serialized);
        assert!(round_trip(&backup));

        let descr_str = r#"wsh(or_d(pk([19608592/48'/1'/0'/2']tpubDEjf1AbrUjxnw8jg6Gi12CunPqnCobLP6Ktoy4Hd52pa65d6QRPg5CSkdFrqPDjJ8BAUuMEDVDRQVjtuWWksMqBeZCqyABFucN9ErQq8oVX/<0;1>/*),and_v(v:pkh([19608592/48'/1'/0'/2']tpubDEjf1AbrUjxnw8jg6Gi12CunPqnCobLP6Ktoy4Hd52pa65d6QRPg5CSkdFrqPDjJ8BAUuMEDVDRQVjtuWWksMqBeZCqyABFucN9ErQq8oVX/<2;3>/*),older(52596))))#x6u6lmej"#.to_string();

        let account = LegacyAccount::new(descr_str);
        backup.accounts.push(account);

        let serialized = serde_json::to_string(&backup).unwrap();
        println!("{serialized}");
        let expected = r#"{"name":"Liana","accounts":[{"descriptor":"wsh(or_d(pk([19608592/48'/1'/0'/2']tpubDEjf1AbrUjxnw8jg6Gi12CunPqnCobLP6Ktoy4Hd52pa65d6QRPg5CSkdFrqPDjJ8BAUuMEDVDRQVjtuWWksMqBeZCqyABFucN9ErQq8oVX/<0;1>/*),and_v(v:pkh([19608592/48'/1'/0'/2']tpubDEjf1AbrUjxnw8jg6Gi12CunPqnCobLP6Ktoy4Hd52pa65d6QRPg5CSkdFrqPDjJ8BAUuMEDVDRQVjtuWWksMqBeZCqyABFucN9ErQq8oVX/<2;3>/*),older(52596))))#x6u6lmej"}],"network":"signet","date":0,"version":0}"#;
        assert_eq!(expected, serialized);
        assert!(round_trip(&backup));

        // if there is no version, the default is 0
        let no_version = r#"{"name":"Liana","accounts":[{"descriptor":"wsh(or_d(pk([19608592/48'/1'/0'/2']tpubDEjf1AbrUjxnw8jg6Gi12CunPqnCobLP6Ktoy4Hd52pa65d6QRPg5CSkdFrqPDjJ8BAUuMEDVDRQVjtuWWksMqBeZCqyABFucN9ErQq8oVX/<0;1>/*),and_v(v:pkh([19608592/48'/1'/0'/2']tpubDEjf1AbrUjxnw8jg6Gi12CunPqnCobLP6Ktoy4Hd52pa65d6QRPg5CSkdFrqPDjJ8BAUuMEDVDRQVjtuWWksMqBeZCqyABFucN9ErQq8oVX/<2;3>/*),older(52596))))#x6u6lmej"}],"network":"signet","date":0}"#;
        let parsed: LegacyBackup = serde_json::from_str(no_version).unwrap();
        assert_eq!(parsed.version, 0);

        // Network is mandatory for an account
        let no_network = r#"{"name":"Liana","accounts":[{"descriptor":"wsh(or_d(pk([19608592/48'/1'/0'/2']tpubDEjf1AbrUjxnw8jg6Gi12CunPqnCobLP6Ktoy4Hd52pa65d6QRPg5CSkdFrqPDjJ8BAUuMEDVDRQVjtuWWksMqBeZCqyABFucN9ErQq8oVX/<0;1>/*),and_v(v:pkh([19608592/48'/1'/0'/2']tpubDEjf1AbrUjxnw8jg6Gi12CunPqnCobLP6Ktoy4Hd52pa65d6QRPg5CSkdFrqPDjJ8BAUuMEDVDRQVjtuWWksMqBeZCqyABFucN9ErQq8oVX/<2;3>/*),older(52596))))#x6u6lmej"}],"date":0,"version":0}"#;
        let parsed: Result<LegacyBackup, _> = serde_json::from_str(no_network);
        assert!(parsed.is_err());

        // But it's the only mandatory field, w/ accounts array
        let minimal = r#"{"network":"signet","accounts":[]}"#;
        let _parsed: LegacyBackup = serde_json::from_str(minimal).unwrap();
    }
}
