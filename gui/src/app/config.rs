use crate::hw::HardwareWalletConfig;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    /// Path to lianad configuration file.
    pub daemon_config_path: PathBuf,
    /// log level, can be "info", "debug", "trace".
    pub log_level: Option<String>,
    /// Use iced debug feature if true.
    pub debug: Option<bool>,
    /// hardware wallets config.
    #[serde(default)]
    pub hardware_wallets: Vec<HardwareWalletConfig>,
}

pub const DEFAULT_FILE_NAME: &str = "gui.toml";

impl Config {
    pub fn new(daemon_config_path: PathBuf, hardware_wallets: Vec<HardwareWalletConfig>) -> Self {
        Self {
            daemon_config_path,
            log_level: None,
            debug: None,
            hardware_wallets,
        }
    }

    pub fn from_file(path: &Path) -> Result<Self, ConfigError> {
        let config = std::fs::read(path)
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => ConfigError::NotFound,
                _ => ConfigError::ReadingFile(format!("Reading configuration file: {}", e)),
            })
            .and_then(|file_content| {
                toml::from_slice::<Config>(&file_content).map_err(|e| {
                    ConfigError::ReadingFile(format!("Parsing configuration file: {}", e))
                })
            })?;
        Ok(config)
    }

    pub fn default_path() -> Result<PathBuf, ConfigError> {
        let mut datadir = default_datadir().map_err(|_| {
            ConfigError::Unexpected("Could not locate the default datadir directory.".to_owned())
        })?;
        datadir.push(DEFAULT_FILE_NAME);
        Ok(datadir)
    }
}

#[derive(PartialEq, Eq, Debug, Clone)]
pub enum ConfigError {
    NotFound,
    ReadingFile(String),
    Unexpected(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "Config file not found"),
            Self::ReadingFile(e) => write!(f, "Error while reading file: {}", e),
            Self::Unexpected(e) => write!(f, "Unexpected error: {}", e),
        }
    }
}

impl std::error::Error for ConfigError {}

// Get the absolute path to the liana configuration folder.
///
/// This a "liana" directory in the XDG standard configuration directory for all OSes but
/// Linux-based ones, for which it's `~/.liana`.
/// Rationale: we want to have the database, RPC socket, etc.. in the same folder as the
/// configuration file but for Linux the XDG specify a data directory (`~/.local/share/`) different
/// from the configuration one (`~/.config/`).
pub fn default_datadir() -> Result<PathBuf, Box<dyn std::error::Error>> {
    #[cfg(target_os = "linux")]
    let configs_dir = dirs::home_dir();

    #[cfg(not(target_os = "linux"))]
    let configs_dir = dirs::config_dir();

    if let Some(mut path) = configs_dir {
        #[cfg(target_os = "linux")]
        path.push(".liana");

        #[cfg(not(target_os = "linux"))]
        path.push("Liana");

        return Ok(path);
    }

    Err("Failed to get default data directory".into())
}
