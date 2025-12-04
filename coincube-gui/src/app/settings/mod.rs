//! Settings is the module to handle the GUI settings file.
//! The settings file is used by the GUI to store useful information.
pub mod fiat;

use std::collections::HashMap;

use async_fd_lock::LockWrite;
use coincube_core::descriptors::CoincubeDescriptor;
use std::io::SeekFrom;
use tokio::fs::OpenOptions;
use tokio::io::AsyncSeekExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use coincube_core::miniscript::bitcoin::bip32::Fingerprint;
use coincube_ui::component::form;
use serde::{Deserialize, Serialize};

use crate::{
    backup::{Key, KeyRole, KeyType},
    dir::NetworkDirectory,
    hw::HardwareWalletConfig,
    services::{self, connect::client::backend},
    utils::serde::ok_or_none,
};

use coincube_core::miniscript::bitcoin::Network;

pub const SETTINGS_FILE_NAME: &str = "settings.json";

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Settings {
    #[serde(default)]
    pub cubes: Vec<CubeSettings>,
    #[serde(default)]
    pub wallets: Vec<WalletSettings>,
}

impl Settings {
    pub fn from_file(network_dir: &NetworkDirectory) -> Result<Settings, SettingsError> {
        let mut path = network_dir.path().to_path_buf();
        path.push(SETTINGS_FILE_NAME);

        std::fs::read(path)
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => SettingsError::NotFound,
                _ => SettingsError::ReadingFile(format!("Reading settings file: {}", e)),
            })
            .and_then(|file_content| {
                serde_json::from_slice::<Settings>(&file_content).map_err(|e| {
                    SettingsError::ReadingFile(format!("Parsing settings file: {}", e))
                })
            })
    }
}

pub async fn update_settings_file<F>(
    network_dir: &NetworkDirectory,
    updater: F,
) -> Result<(), SettingsError>
where
    F: FnOnce(Settings) -> Option<Settings>,
{
    let path = network_dir.path().join(SETTINGS_FILE_NAME);
    let file_exists = tokio::fs::try_exists(&path).await.unwrap_or(false);

    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .await
        .map_err(|e| SettingsError::ReadingFile(format!("Opening file: {}", e)))?
        .lock_write()
        .await
        .map_err(|e| SettingsError::ReadingFile(format!("Locking file: {:?}", e)))?;

    let settings = if file_exists {
        let mut file_content = Vec::new();
        file.read_to_end(&mut file_content)
            .await
            .map_err(|e| SettingsError::ReadingFile(format!("Reading file content: {}", e)))?;

        serde_json::from_slice::<Settings>(&file_content)
            .map_err(|e| SettingsError::ReadingFile(e.to_string()))?
    } else {
        Settings::default()
    };

    let settings = updater(settings);

    // If updater returns None, delete the file
    let Some(settings) = settings else {
        tokio::fs::remove_file(&path)
            .await
            .map_err(|e| SettingsError::DeletingFile(e.to_string()))?;
        return Ok(());
    };

    let content = serde_json::to_vec_pretty(&settings)
        .map_err(|e| SettingsError::WritingFile(format!("Failed to serialize settings: {}", e)))?;

    file.seek(SeekFrom::Start(0)).await.map_err(|e| {
        SettingsError::WritingFile(format!("Failed to seek to start of file: {}", e))
    })?;

    file.write_all(&content).await.map_err(|e| {
        tracing::warn!("failed to write to file: {:?}", e);
        SettingsError::WritingFile(e.to_string())
    })?;

    file.inner_mut()
        .set_len(content.len() as u64)
        .await
        .map_err(|e| SettingsError::WritingFile(format!("Failed to truncate file: {}", e)))?;

    Ok(())
}

/// Cubes represent user accounts that can contain multiple features (Vault, Active wallet, etc.)
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CubeSettings {
    pub id: String,
    pub name: String,
    pub network: Network,
    pub created_at: i64,
    /// The Vault wallet for this Cube (optional - may not be set up yet)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vault_wallet_id: Option<WalletId>,
    /// Optional security PIN (stored as Argon2id hash with salt in PHC format)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security_pin_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_wallet_signer_fingerprint: Option<Fingerprint>,
}

impl CubeSettings {
    pub fn new(name: String, network: Network) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            network,
            created_at: chrono::Utc::now().timestamp(),
            vault_wallet_id: None,
            security_pin_hash: None,
            active_wallet_signer_fingerprint: None,
        }
    }

    pub fn with_vault(mut self, wallet_id: WalletId) -> Self {
        self.vault_wallet_id = Some(wallet_id);
        self
    }

    pub fn with_pin(mut self, pin: &str) -> Result<Self, Box<dyn std::error::Error>> {
        self.security_pin_hash = Some(Self::hash_pin(pin)?);
        Ok(self)
    }

    pub fn has_pin(&self) -> bool {
        self.security_pin_hash.is_some()
    }

    pub fn verify_pin(&self, pin: &str) -> bool {
        if let Some(stored_hash) = &self.security_pin_hash {
            Self::verify_argon2_pin(pin, stored_hash)
        } else {
            // No PIN set, allow access
            true
        }
    }

    /// Hash PIN using Argon2id with a random salt
    fn hash_pin(pin: &str) -> Result<String, Box<dyn std::error::Error>> {
        use argon2::{
            password_hash::{rand_core::OsRng, PasswordHasher, SaltString},
            Argon2, Params,
        };

        // Generate a random salt
        let salt = SaltString::generate(&mut OsRng);

        // Configure Argon2id with reasonable parameters
        // m_cost: 19456 KiB (19 MiB), t_cost: 2 iterations, p_cost: 1 thread
        let params = Params::new(19456, 2, 1, None)?;
        let argon2 = Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);

        // Hash the PIN
        let password_hash = argon2.hash_password(pin.as_bytes(), &salt)?;

        // Return the PHC string format
        Ok(password_hash.to_string())
    }

    /// Verify PIN against Argon2id hash
    fn verify_argon2_pin(pin: &str, hash: &str) -> bool {
        use argon2::{
            password_hash::{PasswordHash, PasswordVerifier},
            Argon2,
        };

        // Parse the PHC string
        let parsed_hash = match PasswordHash::new(hash) {
            Ok(h) => h,
            Err(_) => return false,
        };

        // Verify the PIN
        Argon2::default()
            .verify_password(pin.as_bytes(), &parsed_hash)
            .is_ok()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AuthConfig {
    pub email: String,
    pub wallet_id: String,
    // legacy field, refresh_token is now stored in the connect cache file
    // Keep it in case, user want to open the wallet with a previous Coincube-GUI version.
    // Field cannot be ignored as the settings file is override during settings update.
    // TODO: remove later after multiple versions.
    pub refresh_token: Option<String>,
}

impl AuthConfig {
    pub fn new(email: String, wallet_id: String) -> Self {
        Self {
            email,
            wallet_id,
            refresh_token: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WalletSettings {
    pub name: String,
    pub alias: Option<String>,
    pub descriptor_checksum: String,
    pub pinned_at: Option<i64>,
    // if wallet is using remote backend, then this information is stored on the remote backend
    // wallet metadata
    #[serde(default)]
    pub keys: Vec<KeySetting>,
    // if wallet is using remote backend, then this information is stored on the remote backend
    // wallet metadata
    #[serde(default)]
    pub hardware_wallets: Vec<HardwareWalletConfig>,
    pub remote_backend_auth: Option<AuthConfig>,
    /// Start internal bitcoind executable.
    /// if None, the app must refer to the gui.toml start_internal_bitcoind field.
    pub start_internal_bitcoind: Option<bool>,
    // If the settings file contains a currency or source that is no longer supported, the price
    // setting will be set to None during deserialization and the user will need to reconfigure it.
    #[serde(default, deserialize_with = "ok_or_none")]
    pub fiat_price: Option<fiat::PriceSetting>,
}

impl WalletSettings {
    pub fn from_file<F>(
        network_dir: &NetworkDirectory,
        selecter: F,
    ) -> Result<Option<Self>, SettingsError>
    where
        F: FnMut(&WalletSettings) -> bool,
    {
        Settings::from_file(network_dir).map(|cache| cache.wallets.into_iter().find(selecter))
    }

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

    pub fn update_alias(&mut self, key: &Fingerprint, alias: &str) {
        let key_aliases = self.keys_aliases();
        if key_aliases.contains_key(key) {
            self.keys = self
                .keys
                .clone()
                .into_iter()
                .map(|mut ks| {
                    if ks.master_fingerprint == *key {
                        ks.name = alias.into();
                        ks
                    } else {
                        ks
                    }
                })
                .collect();
        }
    }

    pub fn wallet_id(&self) -> WalletId {
        WalletId {
            timestamp: self.pinned_at,
            descriptor_checksum: self.descriptor_checksum.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalletId {
    pub timestamp: Option<i64>,
    pub descriptor_checksum: String,
}

impl WalletId {
    pub fn new(descriptor_checksum: String, timestamp: Option<i64>) -> Self {
        WalletId {
            timestamp,
            descriptor_checksum,
        }
    }
    pub fn generate(descriptor: &CoincubeDescriptor) -> Self {
        WalletId {
            timestamp: Some(chrono::Utc::now().timestamp()),
            descriptor_checksum: descriptor
                .to_string()
                .split_once('#')
                .map(|(_, checksum)| checksum)
                .expect("CoincubeDescriptor.to_string() always include the checksum")
                .to_string(),
        }
    }
    pub fn is_legacy(&self) -> bool {
        self.timestamp.is_none()
    }
}

impl std::fmt::Display for WalletId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(t) = self.timestamp {
            write!(f, "{}-{}", self.descriptor_checksum, t)
        } else {
            write!(f, "{}", self.descriptor_checksum)
        }
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

impl From<services::keys::api::Provider> for Provider {
    fn from(provider: services::keys::api::Provider) -> Self {
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

impl KeySetting {
    pub fn to_backup(&self) -> Key {
        if let Some(provider_key) = &self.provider_key {
            if let Ok(metadata) = serde_json::to_value(provider_key) {
                return Key {
                    key: self.master_fingerprint,
                    alias: Some(self.name.clone()),
                    role: None,
                    key_type: Some(KeyType::ThirdParty),
                    proprietary: metadata,
                };
            }
        }
        Key {
            key: self.master_fingerprint,
            alias: Some(self.name.clone()),
            role: None,
            key_type: None,
            proprietary: serde_json::Value::Null,
        }
    }

    pub fn from_backup(
        name: String,
        fg: Fingerprint,
        _role: Option<KeyRole>,
        key_type: Option<KeyType>,
        metadata: serde_json::Value,
    ) -> Option<Self> {
        if let Some(KeyType::ThirdParty) = key_type {
            let provider_key = serde_json::from_value(metadata).ok();
            Some(Self {
                name,
                master_fingerprint: fg,
                provider_key,
            })
        } else {
            Some(Self {
                name,
                master_fingerprint: fg,
                provider_key: None,
            })
        }
    }

    pub fn to_form(&self) -> form::Value<String> {
        form::Value {
            value: self.name.clone(),
            warning: None,
            valid: true,
        }
    }

    pub fn name(&self) -> String {
        self.name.clone()
    }

    pub fn has_name(&self) -> bool {
        !self.name.is_empty()
    }
}

#[derive(PartialEq, Eq, Debug, Clone)]
pub enum SettingsError {
    NotFound,
    ReadingFile(String),
    DeletingFile(String),
    WritingFile(String),
    Unexpected(String),
}
impl std::fmt::Display for SettingsError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "Settings file not found"),
            Self::ReadingFile(e) => write!(f, "Error while reading file: {}", e),
            Self::DeletingFile(e) => write!(f, "Error while deleting file: {}", e),
            Self::WritingFile(e) => write!(f, "Error while writing file: {}", e),
            Self::Unexpected(e) => write!(f, "Unexpected error: {}", e),
        }
    }
}

/// global settings.
#[allow(unstable_name_collisions)]
pub mod global {
    use crate::dir::CoincubeDirectory;
    use async_hwi::bitbox::{ConfigError, NoiseConfig, NoiseConfigData};
    use fs4::fs_std::FileExt;
    use serde::{Deserialize, Serialize};
    use std::fs::{File, OpenOptions};
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::path::PathBuf;

    pub const DEFAULT_FILE_NAME: &str = "global_settings.json";

    #[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
    pub struct WindowConfig {
        pub width: f32,
        pub height: f32,
    }

    #[derive(Debug, Deserialize, Serialize, Default)]
    pub struct GlobalSettings {
        pub bitbox: Option<BitboxSettings>,
        pub window_config: Option<WindowConfig>,
    }

    impl GlobalSettings {
        pub fn path(global_datadir: &CoincubeDirectory) -> PathBuf {
            global_datadir.path().join(DEFAULT_FILE_NAME)
        }

        pub fn load_window_config(path: &PathBuf) -> Option<WindowConfig> {
            let mut ret = None;
            if let Err(e) = Self::update(path, |s| ret = s.window_config.clone(), false) {
                tracing::error!("Failed to load window config: {e}");
            }
            ret
        }

        pub fn update_window_config(
            path: &PathBuf,
            window_config: &WindowConfig,
        ) -> Result<(), String> {
            Self::update(
                path,
                |s| s.window_config = Some(window_config.clone()),
                true,
            )
        }

        pub fn load_bitbox_settings(path: &PathBuf) -> Result<Option<BitboxSettings>, String> {
            let mut ret = None;
            Self::update(path, |s| ret = s.bitbox.clone(), false)?;
            Ok(ret)
        }

        pub fn update_bitbox_settings(
            path: &PathBuf,
            bitbox: &BitboxSettings,
        ) -> Result<(), String> {
            Self::update(path, |s| s.bitbox = Some(bitbox.clone()), true)
        }

        pub fn update<F>(path: &PathBuf, mut update: F, mut write: bool) -> Result<(), String>
        where
            F: FnMut(&mut GlobalSettings),
        {
            let exists = path.is_file();

            let (mut global_settings, file) = if exists {
                let mut file = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .open(path)
                    .map_err(|e| format!("Opening file: {e}"))?;

                file.lock_exclusive()
                    .map_err(|e| format!("Locking file: {e}"))?;

                let mut content = String::new();
                file.read_to_string(&mut content)
                    .map_err(|e| format!("Reading file: {e}"))?;

                if !write {
                    File::unlock(&file).map_err(|e| format!("Unlocking file: {e}"))?;
                }

                (
                    serde_json::from_str::<GlobalSettings>(&content).map_err(|e| e.to_string())?,
                    Some(file),
                )
            } else {
                (GlobalSettings::default(), None)
            };

            update(&mut global_settings);

            if !exists
                && global_settings.bitbox.is_none()
                && global_settings.window_config.is_none()
            {
                write = false;
            }

            if write {
                let mut file = if let Some(file) = file {
                    file
                } else {
                    let file = OpenOptions::new()
                        .read(true)
                        .write(true)
                        .create(true)
                        .truncate(false)
                        .open(path)
                        .map_err(|e| format!("Opening file: {e}"))?;

                    file.lock_exclusive()
                        .map_err(|e| format!("Locking file: {e}"))?;
                    file
                };
                let content = serde_json::to_vec_pretty(&global_settings)
                    .map_err(|e| format!("Failed to serialize GlobalSettings: {e}"))?;

                file.seek(SeekFrom::Start(0))
                    .map_err(|e| format!("Failed to seek file: {e}"))?;

                file.write_all(&content)
                    .map_err(|e| format!("Failed to write file: {e}"))?;
                file.set_len(content.len() as u64)
                    .map_err(|e| format!("Failed to truncate file: {e}"))?;
                File::unlock(&file).map_err(|e| format!("Unlocking file: {e}"))?;
            }

            Ok(())
        }
    }

    #[derive(Debug, Deserialize, Serialize, Clone)]
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
        pub fn new(global_datadir: &CoincubeDirectory) -> PersistedBitboxNoiseConfig {
            PersistedBitboxNoiseConfig {
                file_path: GlobalSettings::path(global_datadir),
            }
        }
    }

    impl NoiseConfig for PersistedBitboxNoiseConfig {
        fn read_config(&self) -> Result<NoiseConfigData, ConfigError> {
            let res = GlobalSettings::load_bitbox_settings(&self.file_path)
                .map_err(ConfigError)?
                .map(|s| s.noise_config)
                .unwrap_or_else(NoiseConfigData::default);
            Ok(res)
        }

        fn store_config(&self, conf: &NoiseConfigData) -> Result<(), ConfigError> {
            GlobalSettings::update(
                &self.file_path,
                |s| {
                    if let Some(bitbox) = s.bitbox.as_mut() {
                        bitbox.noise_config = conf.clone();
                    } else {
                        s.bitbox = Some(BitboxSettings {
                            noise_config: conf.clone(),
                        });
                    }
                },
                true,
            )
            .map_err(ConfigError)
        }
    }
}

#[cfg(test)]
mod test {
    use super::global::{GlobalSettings, WindowConfig};
    use std::env;

    const RAW_GLOBAL_SETTINGS: &str = r#"{
          "bitbox": {
            "noise_config": {
              "app_static_privkey": [
                84,
                118,
                69,
                7,
                5,
                246,
                50,
                252,
                79,
                62,
                233,
                118,
                54,
                46,
                247,
                143,
                255,
                152,
                11,
                96,
                7,
                213,
                209,
                42,
                219,
                58,
                237,
                22,
                53,
                221,
                227,
                228
              ],
              "device_static_pubkeys": [
                [
                  252,
                  78,
                  254,
                  112,
                  62,
                  72,
                  220,
                  22,
                  23,
                  147,
                  205,
                  166,
                  248,
                  39,
                  97,
                  46,
                  32,
                  255,
                  132,
                  125,
                  97,
                  142,
                  31,
                  146,
                  44,
                  186,
                  231,
                  1,
                  12,
                  190,
                  105,
                  11
                ]
              ]
            }
          },
          "window_config": {
            "width": 1248.0,
            "height": 688.0
          }
        }"#;

    #[test]
    fn test_parse_global_config() {
        let _ = serde_json::from_str::<GlobalSettings>(RAW_GLOBAL_SETTINGS).unwrap();
    }

    #[test]
    fn test_update_global_config() {
        let path = env::current_dir()
            .unwrap()
            .join("test_assets")
            .join("global_settings.json");
        assert!(path.exists());

        // read global config file
        GlobalSettings::update(
            &path,
            |s| {
                assert_eq!(
                    *s.window_config.as_ref().unwrap(),
                    WindowConfig {
                        width: 1248.0,
                        height: 688.0
                    }
                );
                assert!(s.bitbox.is_some());
                // this must not be written to the file as write == false
                s.window_config.as_mut().unwrap().height = 0.0;
            },
            false,
        )
        .unwrap();

        // re-read the global config file
        GlobalSettings::update(
            &path,
            |s| {
                // change have not been written
                assert_eq!(
                    *s.window_config.as_ref().unwrap(),
                    WindowConfig {
                        width: 1248.0,
                        height: 688.0
                    }
                );
            },
            true,
        )
        .unwrap();

        // edit the global config file
        GlobalSettings::update(
            &path,
            |s| {
                assert_eq!(
                    *s.window_config.as_ref().unwrap(),
                    WindowConfig {
                        width: 1248.0,
                        height: 688.0
                    }
                );
                assert!(s.bitbox.is_some());
                // this must be written to the file as write == true
                s.window_config.as_mut().unwrap().height = 0.0;
            },
            true,
        )
        .unwrap();

        // re-read the global config file
        GlobalSettings::update(
            &path,
            |s| {
                // change have been written
                assert_eq!(
                    *s.window_config.as_ref().unwrap(),
                    WindowConfig {
                        width: 1248.0,
                        height: 0.0
                    }
                );
                s.window_config.as_mut().unwrap().height = 688.0;
            },
            true,
        )
        .unwrap()
    }

    #[test]
    fn test_pin_hashing_argon2id() {
        use super::CubeSettings;
        use coincube_core::miniscript::bitcoin::Network;

        let pin = "1234";

        // Create a cube with a PIN
        let cube = CubeSettings::new("Test Cube".to_string(), Network::Bitcoin)
            .with_pin(pin)
            .expect("PIN hashing should succeed in tests");

        // Verify the hash is in Argon2id format
        let hash = cube.security_pin_hash.as_ref().unwrap();
        assert!(
            hash.starts_with("$argon2id$"),
            "Hash should be in Argon2id format"
        );

        // Verify correct PIN works
        assert!(cube.verify_pin("1234"), "Correct PIN should verify");

        // Verify incorrect PIN fails
        assert!(!cube.verify_pin("4321"), "Incorrect PIN should fail");
        assert!(!cube.verify_pin("0000"), "Incorrect PIN should fail");
    }

    #[test]
    fn test_pin_hashing_unique_salts() {
        use super::CubeSettings;
        use coincube_core::miniscript::bitcoin::Network;

        let pin = "1234";

        // Create two cubes with the same PIN
        let cube1 = CubeSettings::new("Cube 1".to_string(), Network::Bitcoin)
            .with_pin(pin)
            .expect("PIN hashing should succeed in tests");
        let cube2 = CubeSettings::new("Cube 2".to_string(), Network::Bitcoin)
            .with_pin(pin)
            .expect("PIN hashing should succeed in tests");

        let hash1 = cube1.security_pin_hash.as_ref().unwrap();
        let hash2 = cube2.security_pin_hash.as_ref().unwrap();

        // Hashes should be different due to unique salts
        assert_ne!(
            hash1, hash2,
            "Same PIN should produce different hashes with unique salts"
        );

        // Both should verify correctly
        assert!(cube1.verify_pin(pin));
        assert!(cube2.verify_pin(pin));
    }

    #[test]
    fn test_no_pin_always_allows() {
        use super::CubeSettings;
        use coincube_core::miniscript::bitcoin::Network;

        // Create a cube without a PIN
        let cube = CubeSettings::new("No PIN Cube".to_string(), Network::Bitcoin);

        // Any PIN should "verify" (allow access)
        assert!(cube.verify_pin("1234"));
        assert!(cube.verify_pin("0000"));
        assert!(cube.verify_pin("9999"));
    }
}
