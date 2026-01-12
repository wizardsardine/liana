//! Settings is the module to handle the GUI settings file.
//! The settings file is used by the GUI to store useful information.
pub mod fiat;
pub mod ui;

pub use ui::SettingsUI;

use std::collections::HashMap;

use async_fd_lock::LockWrite;
use liana::descriptors::LianaDescriptor;
use serde::de::DeserializeOwned;
use std::io::SeekFrom;
use tokio::fs::OpenOptions;
use tokio::io::AsyncSeekExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use liana::miniscript::bitcoin::bip32::Fingerprint;
use liana_ui::component::form;
use serde::{Deserialize, Serialize};

use iced::Task;
use liana::miniscript::bitcoin;
use lianad::commands::ListCoinsResult;

use crate::{
    app::{self, state::State},
    backup::{Key, KeyRole, KeyType},
    dir::{LianaDirectory, NetworkDirectory},
    hw::HardwareWalletConfig,
    services::{
        self,
        connect::client::backend::{self, api, BackendWalletClient},
    },
    utils::serde::ok_or_none,
};

pub const SETTINGS_FILE_NAME: &str = "settings.json";

/// Trait for application-level settings.
///
/// This trait combines data operations with UI abstraction. Each implementation
/// specifies its own message type and UI component, following the Installer pattern.
pub trait SettingsTrait: Clone + Default + Serialize + DeserializeOwned + Send + 'static {
    /// The wallet settings type.
    type Wallet: WalletSettingsTrait;

    /// The message type used by the settings UI.
    type Message: Send + 'static;

    /// The settings UI component type.
    /// Must implement both SettingsUI (for settings-specific logic) and State
    /// (for the unified panel interface that App uses).
    type UI: SettingsUI<Self::Message> + State;

    /// Load settings from the network directory.
    fn from_file(network_dir: &NetworkDirectory) -> Result<Self, SettingsError>;

    /// Get the list of wallet settings.
    fn wallets(&self) -> &[Self::Wallet];

    /// Get mutable access to wallet settings.
    fn wallets_mut(&mut self) -> &mut Vec<Self::Wallet>;

    /// Create an App from Liana Connect remote backend data.
    ///
    /// Returns `None` if this settings type doesn't support Liana Connect login.
    /// Only `LianaSettings` implements this; other settings types return `None`.
    ///
    /// This method allows the generic `Tab<I, S, M>` to handle the Login -> App
    /// transition in a type-safe way without unsafe code.
    #[allow(unused_variables)]
    #[allow(clippy::type_complexity)]
    fn create_app_for_remote_backend(
        wallet_id: WalletId,
        remote_backend: BackendWalletClient,
        wallet: api::Wallet,
        coins: ListCoinsResult,
        liana_dir: LianaDirectory,
        network: bitcoin::Network,
        config: app::Config,
    ) -> Option<Result<(app::App<Self>, Task<app::message::Message>), SettingsError>>
    where
        Self: Sized,
    {
        // Default implementation: not supported
        None
    }
}

/// Trait for wallet-level settings.
pub trait WalletSettingsTrait: Clone + Serialize + DeserializeOwned + Send + 'static {
    /// Get the wallet's display name.
    fn name(&self) -> &str;
    /// Get an optional user-defined alias for the wallet.
    fn alias(&self) -> Option<&str>;
    /// Get the checksum portion of the wallet's descriptor.
    fn descriptor_checksum(&self) -> &str;
    /// Get the Unix timestamp when this wallet was created/pinned (used for wallet identification).
    fn pinned_at(&self) -> Option<i64>;
    /// Get the unique identifier for this wallet (combines checksum and timestamp).
    fn wallet_id(&self) -> WalletId;
    /// Get the list of key settings (aliases and provider metadata) for this wallet.
    fn keys(&self) -> &[KeySetting];
    /// Get the list of hardware wallet configurations registered with this wallet.
    fn hardware_wallets(&self) -> &[HardwareWalletConfig];
    /// Get the remote backend authentication config, if this wallet uses a remote backend.
    fn remote_backend_auth(&self) -> Option<&AuthConfig>;
    /// Get the fiat price conversion settings for this wallet.
    fn fiat_price(&self) -> Option<&fiat::PriceSetting>;
    /// Get a map of key fingerprints to their user-defined aliases.
    fn keys_aliases(&self) -> HashMap<Fingerprint, String>;
    /// Get a map of key fingerprints to their third-party provider information.
    fn provider_keys(&self) -> HashMap<Fingerprint, ProviderKey>;
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct LianaSettings {
    pub wallets: Vec<LianaWalletSettings>,
}

impl LianaSettings {
    pub fn from_file(network_dir: &NetworkDirectory) -> Result<LianaSettings, SettingsError> {
        let mut path = network_dir.path().to_path_buf();
        path.push(SETTINGS_FILE_NAME);

        std::fs::read(path)
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => SettingsError::NotFound,
                _ => SettingsError::ReadingFile(format!("Reading settings file: {}", e)),
            })
            .and_then(|file_content| {
                serde_json::from_slice::<LianaSettings>(&file_content).map_err(|e| {
                    SettingsError::ReadingFile(format!("Parsing settings file: {}", e))
                })
            })
    }
}

impl SettingsTrait for LianaSettings {
    type Wallet = LianaWalletSettings;
    type Message = crate::app::message::Message;
    type UI = crate::app::state::LianaSettingsUI;

    fn from_file(network_dir: &NetworkDirectory) -> Result<Self, SettingsError> {
        LianaSettings::from_file(network_dir)
    }

    fn wallets(&self) -> &[Self::Wallet] {
        &self.wallets
    }

    fn wallets_mut(&mut self) -> &mut Vec<Self::Wallet> {
        &mut self.wallets
    }

    fn create_app_for_remote_backend(
        wallet_id: WalletId,
        remote_backend: BackendWalletClient,
        wallet: api::Wallet,
        coins: ListCoinsResult,
        liana_dir: LianaDirectory,
        network: bitcoin::Network,
        config: app::Config,
    ) -> Option<Result<(app::App<Self>, Task<app::message::Message>), SettingsError>> {
        Some(crate::gui::tab::create_app_with_remote_backend(
            wallet_id,
            remote_backend,
            wallet,
            coins,
            liana_dir,
            network,
            config,
        ))
    }
}

/// Backward compatibility type alias.
pub type Settings = LianaSettings;

pub async fn update_settings_file<S, F>(
    network_dir: &NetworkDirectory,
    updater: F,
) -> Result<(), SettingsError>
where
    S: SettingsTrait,
    F: FnOnce(S) -> S,
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

    let settings: S = if file_exists {
        let mut file_content = Vec::new();
        file.read_to_end(&mut file_content)
            .await
            .map_err(|e| SettingsError::ReadingFile(format!("Reading file content: {}", e)))?;

        serde_json::from_slice::<S>(&file_content)
            .map_err(|e| SettingsError::ReadingFile(e.to_string()))?
    } else {
        S::default()
    };

    let settings = updater(settings);

    if settings.wallets().is_empty() {
        tokio::fs::remove_file(&path)
            .await
            .map_err(|e| SettingsError::ReadingFile(e.to_string()))?;
        return Ok(());
    }

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

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AuthConfig {
    pub email: String,
    pub wallet_id: String,
    // legacy field, refresh_token is now stored in the connect cache file
    // Keep it in case, user want to open the wallet with a previous Liana-GUI version.
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
pub struct LianaWalletSettings {
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

impl LianaWalletSettings {
    pub fn from_file<F>(
        network_dir: &NetworkDirectory,
        selecter: F,
    ) -> Result<Option<Self>, SettingsError>
    where
        F: FnMut(&LianaWalletSettings) -> bool,
    {
        LianaSettings::from_file(network_dir).map(|cache| cache.wallets.into_iter().find(selecter))
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

impl WalletSettingsTrait for LianaWalletSettings {
    fn name(&self) -> &str {
        &self.name
    }

    fn alias(&self) -> Option<&str> {
        self.alias.as_deref()
    }

    fn descriptor_checksum(&self) -> &str {
        &self.descriptor_checksum
    }

    fn pinned_at(&self) -> Option<i64> {
        self.pinned_at
    }

    fn wallet_id(&self) -> WalletId {
        LianaWalletSettings::wallet_id(self)
    }

    fn keys(&self) -> &[KeySetting] {
        &self.keys
    }

    fn hardware_wallets(&self) -> &[HardwareWalletConfig] {
        &self.hardware_wallets
    }

    fn remote_backend_auth(&self) -> Option<&AuthConfig> {
        self.remote_backend_auth.as_ref()
    }

    fn fiat_price(&self) -> Option<&fiat::PriceSetting> {
        self.fiat_price.as_ref()
    }

    fn keys_aliases(&self) -> HashMap<Fingerprint, String> {
        LianaWalletSettings::keys_aliases(self)
    }

    fn provider_keys(&self) -> HashMap<Fingerprint, ProviderKey> {
        LianaWalletSettings::provider_keys(self)
    }
}

/// Backward compatibility type alias.
pub type WalletSettings = LianaWalletSettings;

#[derive(Debug, Clone, PartialEq, Eq)]
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
    pub fn generate(descriptor: &LianaDescriptor) -> Self {
        WalletId {
            timestamp: Some(chrono::Utc::now().timestamp()),
            descriptor_checksum: descriptor
                .to_string()
                .split_once('#')
                .map(|(_, checksum)| checksum)
                .expect("LianaDescriptor.to_string() always include the checksum")
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
pub mod global {
    use crate::dir::LianaDirectory;
    use async_hwi::bitbox::{ConfigError, NoiseConfig, NoiseConfigData};
    use fs2::FileExt;
    use serde::{Deserialize, Serialize};
    use std::fs::OpenOptions;
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
        pub fn path(global_datadir: &LianaDirectory) -> PathBuf {
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
                    fs2::FileExt::unlock(&file).map_err(|e| format!("Unlocking file: {e}"))?;
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
                fs2::FileExt::unlock(&file).map_err(|e| format!("Unlocking file: {e}"))?;
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
        pub fn new(global_datadir: &LianaDirectory) -> PersistedBitboxNoiseConfig {
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
}
