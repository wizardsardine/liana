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

use liana::miniscript::bitcoin;
use liana::miniscript::bitcoin::bip32::Fingerprint;
use liana_gui::{
    app::{
        self,
        cache::{Cache, DaemonCache},
        settings::{
            fiat, AuthConfig, KeySetting, ProviderKey, SettingsError, SettingsTrait, WalletId,
            WalletSettingsTrait, SETTINGS_FILE_NAME,
        },
        wallet::Wallet as AppWallet,
    },
    daemon::model::ListCoinsResult,
    dir::{LianaDirectory, NetworkDirectory},
    hw::HardwareWalletConfig,
    services::connect::client::backend::{api, BackendWalletClient},
    utils::serde::{deser_fromstr, serialize_display},
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

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

    fn create_app_for_remote_backend(
        _wallet_id: WalletId,
        remote_backend: BackendWalletClient,
        wallet: api::Wallet,
        coins: ListCoinsResult,
        liana_dir: LianaDirectory,
        network: bitcoin::Network,
        config: app::Config,
    ) -> Option<Result<(app::App<Self>, iced::Task<app::message::Message>), SettingsError>> {
        Some((|| {
            let hws: Vec<HardwareWalletConfig> = wallet
                .metadata
                .ledger_hmacs
                .into_iter()
                .map(|lh| HardwareWalletConfig {
                    kind: async_hwi::DeviceKind::Ledger.to_string(),
                    fingerprint: lh.fingerprint,
                    token: lh.hmac,
                })
                .collect();

            let aliases: HashMap<Fingerprint, String> = wallet
                .metadata
                .fingerprint_aliases
                .into_iter()
                .filter_map(|a| {
                    if a.user_id == remote_backend.user_id() {
                        Some((a.fingerprint, a.alias))
                    } else {
                        None
                    }
                })
                .collect();

            let provider_keys: HashMap<_, _> = wallet
                .metadata
                .provider_keys
                .into_iter()
                .map(|pk| (pk.fingerprint, pk.into()))
                .collect();

            let auth_cfg = AuthConfig {
                email: remote_backend.user_email().to_string(),
                wallet_id: remote_backend.wallet_id(),
                refresh_token: None,
            };

            let app_wallet = Arc::new(
                AppWallet::new(wallet.descriptor)
                    .with_name(wallet.name)
                    .with_alias(wallet.metadata.wallet_alias)
                    .with_key_aliases(aliases)
                    .with_provider_keys(provider_keys)
                    .with_hardware_wallets(hws)
                    .with_remote_backend_auth(auth_cfg)
                    .load_hotsigners(&liana_dir, network)
                    .map_err(|e| SettingsError::Unexpected(e.to_string()))?,
            );

            let cache = Cache {
                network,
                datadir_path: liana_dir.clone(),
                last_poll_at_startup: None,
                daemon_cache: DaemonCache {
                    coins: coins.coins,
                    rescan_progress: None,
                    sync_progress: 1.0,
                    blockheight: wallet.tip_height.unwrap_or(0),
                    last_poll_timestamp: None,
                    last_tick: Instant::now(),
                },
                fiat_price: None,
            };

            Ok(app::App::new(
                cache,
                app_wallet,
                config,
                Arc::new(remote_backend),
                liana_dir,
                None,
                false,
            ))
        })())
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
