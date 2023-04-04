use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

use liana::miniscript::bitcoin::{util::bip32::Fingerprint, Network};
use serde::{Deserialize, Serialize};

use crate::{app::wallet::Wallet, hw::HardwareWalletConfig};

///! Settings is the module to handle the GUI settings file.
///! The settings file is used by the GUI to store useful information.
pub const DEFAULT_FILE_NAME: &str = "settings.json";

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Settings {
    pub wallets: Vec<WalletSetting>,
}

impl Settings {
    pub fn from_file(datadir: PathBuf, network: Network) -> Result<Self, SettingsError> {
        let mut path = datadir;
        path.push(network.to_string());
        path.push(DEFAULT_FILE_NAME);

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

    pub fn to_file(&self, datadir: PathBuf, network: Network) -> Result<(), SettingsError> {
        let mut path = datadir;
        path.push(network.to_string());
        path.push(DEFAULT_FILE_NAME);

        let content = serde_json::to_string_pretty(&self).map_err(|e| {
            SettingsError::WritingFile(format!("Failed to serialize settings: {}", e))
        })?;

        let mut settings_file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .map_err(|e| SettingsError::WritingFile(e.to_string()))?;

        settings_file.write_all(content.as_bytes()).map_err(|e| {
            tracing::warn!("failed to write to file: {:?}", e);
            SettingsError::WritingFile(e.to_string())
        })
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
        for key in self.keys.iter().filter(|k| !k.name.is_empty()) {
            map.insert(key.master_fingerprint, key.name.clone());
        }
        map
    }
}

impl From<&Wallet> for WalletSetting {
    fn from(w: &Wallet) -> WalletSetting {
        Self {
            name: w.name.clone(),
            hardware_wallets: w.hardware_wallets.clone(),
            keys: w
                .keys_aliases
                .clone()
                .into_iter()
                .map(|(master_fingerprint, name)| KeySetting {
                    name,
                    master_fingerprint,
                })
                .collect(),
            descriptor_checksum: w.descriptor_checksum(),
        }
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
    WritingFile(String),
    Unexpected(String),
}

impl std::fmt::Display for SettingsError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "Settings file not found"),
            Self::ReadingFile(e) => write!(f, "Error while reading file: {}", e),
            Self::WritingFile(e) => write!(f, "Error while writing file: {}", e),
            Self::Unexpected(e) => write!(f, "Unexpected error: {}", e),
        }
    }
}
