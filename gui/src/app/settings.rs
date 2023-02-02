use std::collections::HashMap;
use std::path::Path;

use liana::miniscript::bitcoin::util::bip32::Fingerprint;
use serde::{Deserialize, Serialize};

use crate::hw::HardwareWalletConfig;

///! Settings is the module to handle the GUI settings file.
///! The settings file is used by the GUI to store useful information.
pub const DEFAULT_FILE_NAME: &str = "settings.json";

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Settings {
    pub wallets: Vec<WalletSetting>,
}

impl Settings {
    pub fn from_file(path: &Path) -> Result<Self, SettingsError> {
        let config = std::fs::read(path)
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => SettingsError::NotFound,
                _ => SettingsError::ReadingFile(format!("Reading settings file: {}", e)),
            })
            .and_then(|file_content| {
                serde_json::from_slice::<Settings>(&file_content).map_err(|e| {
                    SettingsError::ReadingFile(format!("Parsing settings file: {}", e))
                })
            })?;
        Ok(config)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WalletSetting {
    pub name: String,
    pub descriptor_checksum: String,
    #[serde(default)]
    pub keys: Vec<KeySetting>,
    #[serde(default)]
    pub hardware_wallets: Vec<HardwareWalletConfig>,
}

impl WalletSetting {
    pub fn keys_aliases(&self) -> HashMap<Fingerprint, String> {
        let mut map = HashMap::new();
        for key in self.keys.clone() {
            map.insert(key.master_fingerprint, key.name);
        }
        map
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct KeySetting {
    pub name: String,
    pub master_fingerprint: Fingerprint,
}

#[derive(PartialEq, Eq, Debug, Clone)]
pub enum SettingsError {
    NotFound,
    ReadingFile(String),
    Unexpected(String),
}

impl std::fmt::Display for SettingsError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "Settings file not found"),
            Self::ReadingFile(e) => write!(f, "Error while reading file: {}", e),
            Self::Unexpected(e) => write!(f, "Unexpected error: {}", e),
        }
    }
}
