//! Settings is the module to handle the GUI settings file.
//! The settings file is used by the GUI to store useful information.
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

use liana::miniscript::bitcoin::{bip32::Fingerprint, Network};
use serde::{Deserialize, Serialize};

use crate::{hw::HardwareWalletConfig, lianalite::client::backend, services};

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
pub struct AuthConfig {
    pub email: String,
    pub wallet_id: String,
    pub refresh_token: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WalletSetting {
    pub name: String,
    pub descriptor_checksum: String,
    // if wallet is using remote backend, then this information is stored on the remote backend
    // wallet metadata
    #[serde(default)]
    pub keys: Vec<KeySetting>,
    // if wallet is using remote backend, then this information is stored on the remote backend
    // wallet metadata
    #[serde(default)]
    pub hardware_wallets: Vec<HardwareWalletConfig>,
    pub remote_backend_auth: Option<AuthConfig>,
}

impl WalletSetting {
    pub fn keys_aliases(&self) -> HashMap<Fingerprint, String> {
        let mut map = HashMap::new();
        for key in self.keys.iter().filter(|k| !k.name.is_empty()) {
            map.insert(key.master_fingerprint, key.name.clone());
        }
        map
    }

    pub fn provider_keys(&self) -> HashMap<Fingerprint, ProviderKey> {
        let mut map = HashMap::new();
        for (fingerprint, provider_key) in self
            .keys
            .iter()
            .filter_map(|k| k.provider_key.as_ref().map(|pk| (k.master_fingerprint, pk)))
        {
            map.insert(fingerprint, provider_key.clone());
        }
        map
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Hash)]
pub struct Provider {
    pub uuid: String,
    pub name: String,
}

impl From<backend::api::Provider> for Provider {
    fn from(provider: backend::api::Provider) -> Self {
        Self {
            uuid: provider.uuid,
            name: provider.name,
        }
    }
}

impl From<services::api::Provider> for Provider {
    fn from(provider: services::api::Provider) -> Self {
        Self {
            uuid: provider.uuid,
            name: provider.name,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Hash)]
pub struct ProviderKey {
    pub uuid: String,
    pub token: String,
    pub provider: Provider,
}

impl From<backend::api::ProviderKey> for ProviderKey {
    fn from(pk: backend::api::ProviderKey) -> Self {
        Self {
            uuid: pk.uuid.clone(),
            token: pk.token.clone(),
            provider: pk.provider.into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct KeySetting {
    pub name: String,
    pub master_fingerprint: Fingerprint,
    pub provider_key: Option<ProviderKey>,
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

/// global settings.
pub mod global {
    use async_hwi::bitbox::{ConfigError, NoiseConfig, NoiseConfigData};
    use serde::{Deserialize, Serialize};
    use std::io::{Read, Write};
    use std::path::{Path, PathBuf};

    pub const DEFAULT_FILE_NAME: &str = "global_settings.json";

    #[derive(Debug, Deserialize, Serialize)]
    pub struct Settings {
        pub bitbox: Option<BitboxSettings>,
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub struct BitboxSettings {
        pub noise_config: NoiseConfigData,
    }

    pub struct PersistedBitboxNoiseConfig {
        file_path: PathBuf,
    }

    impl async_hwi::bitbox::api::Threading for PersistedBitboxNoiseConfig {}

    impl PersistedBitboxNoiseConfig {
        /// Creates a new persisting noise config, which stores the pairing information in "bitbox.json"
        /// in the provided directory.
        pub fn new(global_datadir: &Path) -> PersistedBitboxNoiseConfig {
            PersistedBitboxNoiseConfig {
                file_path: global_datadir.join(DEFAULT_FILE_NAME),
            }
        }
    }

    impl NoiseConfig for PersistedBitboxNoiseConfig {
        fn read_config(&self) -> Result<NoiseConfigData, async_hwi::bitbox::api::ConfigError> {
            if !self.file_path.exists() {
                return Ok(NoiseConfigData::default());
            }

            let mut file =
                std::fs::File::open(&self.file_path).map_err(|e| ConfigError(e.to_string()))?;

            let mut contents = String::new();
            file.read_to_string(&mut contents)
                .map_err(|e| ConfigError(e.to_string()))?;

            let settings = serde_json::from_str::<Settings>(&contents)
                .map_err(|e| ConfigError(e.to_string()))?;

            Ok(settings
                .bitbox
                .map(|s| s.noise_config)
                .unwrap_or_else(NoiseConfigData::default))
        }

        fn store_config(&self, conf: &NoiseConfigData) -> Result<(), ConfigError> {
            let data = if self.file_path.exists() {
                let mut file =
                    std::fs::File::open(&self.file_path).map_err(|e| ConfigError(e.to_string()))?;

                let mut contents = String::new();
                file.read_to_string(&mut contents)
                    .map_err(|e| ConfigError(e.to_string()))?;

                let mut settings = serde_json::from_str::<Settings>(&contents)
                    .map_err(|e| ConfigError(e.to_string()))?;

                settings.bitbox = Some(BitboxSettings {
                    noise_config: conf.clone(),
                });

                serde_json::to_string_pretty(&settings).map_err(|e| ConfigError(e.to_string()))?
            } else {
                serde_json::to_string_pretty(&Settings {
                    bitbox: Some(BitboxSettings {
                        noise_config: conf.clone(),
                    }),
                })
                .map_err(|e| ConfigError(e.to_string()))?
            };

            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&self.file_path)
                .map_err(|e| ConfigError(e.to_string()))?;

            file.write_all(data.as_bytes())
                .map_err(|e| ConfigError(e.to_string()))
        }
    }
}
