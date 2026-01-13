//! Business-specific settings implementation.
//!
//! This module provides settings types for liana-business that implement
//! the generic Settings traits from liana-gui.
//!
//! Note: Unlike liana-gui, business settings are managed by the backend server.
//! The file-based methods exist for trait compatibility but settings should be
//! fetched from and saved to the backend via the connect client.

pub mod message;
pub mod ui;
pub mod views;

pub use liana_gui::services::fiat::Currency;
pub use message::{Msg, Section};
pub use ui::BusinessSettingsUI;

use liana::miniscript::bitcoin::bip32::Fingerprint;
use liana_gui::{
    app::settings::{
        fiat, AuthConfig, KeySetting, ProviderKey, SettingsError, SettingsTrait, WalletId,
        WalletSettingsTrait, SETTINGS_FILE_NAME,
    },
    dir::NetworkDirectory,
    hw::HardwareWalletConfig,
    utils::serde::{deser_fromstr, serialize_display},
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Business fiat price setting.
///
/// Simpler than liana-gui's `PriceSetting` - no source selection since
/// the backend provides a single price feed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct FiatSetting {
    #[serde(
        deserialize_with = "deser_fromstr",
        serialize_with = "serialize_display"
    )]
    pub currency: Currency,
    pub is_enabled: bool,
}

impl FiatSetting {
    /// Convert from liana-gui's PriceSetting (ignoring source).
    pub fn from_price_setting(ps: &fiat::PriceSetting) -> Self {
        Self {
            currency: ps.currency,
            is_enabled: ps.is_enabled,
        }
    }
}

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
    type Message = Msg;
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
