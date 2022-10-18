use std::convert::TryFrom;

use bitcoin::Network;
use minisafe::{
    config::{BitcoinConfig, BitcoindConfig, Config as MinisafeConfig},
    descriptors::InheritanceDescriptor,
};

use serde::Serialize;
use std::{net::SocketAddr, path::PathBuf, time::Duration};

/// Static informations we require to operate
/// fields with default values are not present, see minisafe::config.
#[derive(Debug, Clone, Serialize)]
pub struct Config {
    #[serde(serialize_with = "serialize_option_to_string")]
    pub main_descriptor: Option<InheritanceDescriptor>,
    pub bitcoin_config: BitcoinConfig,
    /// Everything we need to know to talk to bitcoind
    pub bitcoind_config: BitcoindConfig,
    /// An optional custom data directory
    pub data_dir: Option<PathBuf>,
}

impl Config {
    pub const DEFAULT_FILE_NAME: &'static str = "daemon.toml";
    /// returns a minisafed config with empty or dummy values
    pub fn new() -> Config {
        Self {
            main_descriptor: None,
            bitcoin_config: BitcoinConfig {
                network: Network::Bitcoin,
                poll_interval_secs: Duration::from_secs(30),
            },
            bitcoind_config: BitcoindConfig {
                cookie_path: PathBuf::new(),
                addr: SocketAddr::new(
                    std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)),
                    8080,
                ),
            },
            data_dir: None,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self::new()
    }
}

pub fn serialize_option_to_string<T: std::fmt::Display, S: serde::Serializer>(
    field: &Option<T>,
    s: S,
) -> Result<S::Ok, S::Error> {
    match field {
        Some(field) => s.serialize_str(&field.to_string()),
        None => s.serialize_none(),
    }
}

impl TryFrom<Config> for MinisafeConfig {
    type Error = &'static str;

    fn try_from(cfg: Config) -> Result<Self, Self::Error> {
        if cfg.main_descriptor.is_none() {
            return Err("config does not have a main Descriptor");
        }
        Ok(MinisafeConfig {
            #[cfg(unix)]
            daemon: false,
            log_level: log::LevelFilter::Info,
            main_descriptor: cfg.main_descriptor.unwrap(),
            data_dir: cfg.data_dir,
            bitcoin_config: cfg.bitcoin_config,
            bitcoind_config: Some(cfg.bitcoind_config),
        })
    }
}
