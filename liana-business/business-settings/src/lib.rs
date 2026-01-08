//! Business-specific settings implementation.
//!
//! This module provides settings types for liana-business that implement
//! the generic Settings traits from liana-gui but without bitcoind-specific fields.
//!
//! # Architecture
//!
//! - **BusinessSettings** / **BusinessWalletSettings**: Data layer implementing
//!   `SettingsTrait` and `WalletSettingsTrait` for settings persistence.
//! - **BusinessSettingsUI**: UI layer implementing `SettingsUI` for the settings panel.
//!   Uses monostate pattern (like business-installer) for cleaner code.
//! - **BusinessSettingsMessage**: Message enum for settings UI communication.

pub mod message;
pub mod ui;

pub use message::BusinessSettingsMessage;
pub use ui::BusinessSettingsUI;

use std::collections::HashMap;

use liana::miniscript::bitcoin::bip32::Fingerprint;
use liana_gui::{
    app::settings::{
        fiat, AuthConfig, KeySetting, ProviderKey, SettingsError, SettingsTrait, WalletId,
        WalletSettingsTrait, SETTINGS_FILE_NAME,
    },
    dir::NetworkDirectory,
    hw::HardwareWalletConfig,
};
use serde::{Deserialize, Serialize};

// ============================================================================
// BusinessSettings
// ============================================================================

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct BusinessSettings {
    pub wallets: Vec<BusinessWalletSettings>,
}

impl BusinessSettings {
    pub fn from_file(network_dir: &NetworkDirectory) -> Result<BusinessSettings, SettingsError> {
        let mut path = network_dir.path().to_path_buf();
        path.push(SETTINGS_FILE_NAME);

        std::fs::read(path)
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => SettingsError::NotFound,
                _ => SettingsError::ReadingFile(format!("Reading settings file: {}", e)),
            })
            .and_then(|file_content| {
                serde_json::from_slice::<BusinessSettings>(&file_content).map_err(|e| {
                    SettingsError::ReadingFile(format!("Parsing settings file: {}", e))
                })
            })
    }
}

impl SettingsTrait for BusinessSettings {
    type Wallet = BusinessWalletSettings;
    type Message = BusinessSettingsMessage;
    type UI = BusinessSettingsUI;

    fn from_file(network_dir: &NetworkDirectory) -> Result<Self, SettingsError> {
        BusinessSettings::from_file(network_dir)
    }

    fn wallets(&self) -> &[Self::Wallet] {
        &self.wallets
    }

    fn wallets_mut(&mut self) -> &mut Vec<Self::Wallet> {
        &mut self.wallets
    }
}

// ============================================================================
// BusinessWalletSettings
// ============================================================================

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BusinessWalletSettings {
    pub name: String,
    pub alias: Option<String>,
    pub descriptor_checksum: String,
    pub pinned_at: Option<i64>,
    #[serde(default)]
    pub keys: Vec<KeySetting>,
    #[serde(default)]
    pub hardware_wallets: Vec<HardwareWalletConfig>,
    pub remote_backend_auth: Option<AuthConfig>,
    #[serde(default)]
    pub fiat_price: Option<fiat::PriceSetting>,
    // NOTE: No start_internal_bitcoind field - business uses Liana Connect only
}

impl BusinessWalletSettings {
    pub fn from_file<F>(
        network_dir: &NetworkDirectory,
        selecter: F,
    ) -> Result<Option<Self>, SettingsError>
    where
        F: FnMut(&BusinessWalletSettings) -> bool,
    {
        BusinessSettings::from_file(network_dir)
            .map(|cache| cache.wallets.into_iter().find(selecter))
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

    pub fn wallet_id(&self) -> WalletId {
        WalletId::new(self.descriptor_checksum.clone(), self.pinned_at)
    }
}

impl WalletSettingsTrait for BusinessWalletSettings {
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
        BusinessWalletSettings::wallet_id(self)
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
        BusinessWalletSettings::keys_aliases(self)
    }

    fn provider_keys(&self) -> HashMap<Fingerprint, ProviderKey> {
        BusinessWalletSettings::provider_keys(self)
    }
}
