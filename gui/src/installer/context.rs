use std::path::PathBuf;
use std::time::Duration;

use crate::{
    app::{
        settings::{KeySetting, Settings, WalletSetting},
        wallet::DEFAULT_WALLET_NAME,
    },
    hw::HardwareWalletConfig,
};
use async_hwi::DeviceKind;
use liana::{
    config::Config,
    config::{BitcoinConfig, BitcoindConfig},
    descriptors::MultipathDescriptor,
    miniscript::bitcoin,
};

#[derive(Clone)]
pub struct Context {
    pub bitcoin_config: BitcoinConfig,
    pub bitcoind_config: Option<BitcoindConfig>,
    pub descriptor: Option<MultipathDescriptor>,
    pub keys: Vec<KeySetting>,
    pub hws: Vec<(
        DeviceKind,
        bitcoin::util::bip32::Fingerprint,
        Option<[u8; 32]>,
    )>,
    pub data_dir: PathBuf,
}

impl Context {
    pub fn new(network: bitcoin::Network, data_dir: PathBuf) -> Self {
        Self {
            bitcoin_config: BitcoinConfig {
                network,
                poll_interval_secs: Duration::from_secs(30),
            },
            hws: Vec::new(),
            keys: Vec::new(),
            bitcoind_config: None,
            descriptor: None,
            data_dir,
        }
    }

    pub fn extract_gui_settings(&self) -> Settings {
        let hardware_wallets = self
            .hws
            .iter()
            .filter_map(|(kind, fingerprint, token)| {
                token
                    .as_ref()
                    .map(|token| HardwareWalletConfig::new(kind, fingerprint, token))
            })
            .collect();
        Settings {
            wallets: vec![WalletSetting {
                name: DEFAULT_WALLET_NAME.to_string(),
                descriptor_checksum: self
                    .descriptor
                    .as_ref()
                    .unwrap()
                    .to_string()
                    .split_once('#')
                    .map(|(_, checksum)| checksum)
                    .unwrap()
                    .to_string(),
                keys: self.keys.clone(),
                hardware_wallets,
            }],
        }
    }

    pub fn extract_daemon_config(&self) -> Config {
        Config {
            #[cfg(unix)]
            daemon: false,
            log_level: log::LevelFilter::Info,
            main_descriptor: self.descriptor.clone().unwrap(),
            data_dir: Some(self.data_dir.clone()),
            bitcoin_config: self.bitcoin_config.clone(),
            bitcoind_config: self.bitcoind_config.clone(),
        }
    }
}
