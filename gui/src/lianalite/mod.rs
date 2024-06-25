pub mod client;

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

use liana::miniscript::bitcoin::Network;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    auth: HashMap<String, NetworkAuthConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct NetworkAuthConfig {
    email: String,
    access_token: String,
    expires_at: i64,
    refresh_token: String,
}

pub const DEFAULT_FILE_NAME: &str = "lite.json";

impl Config {
    pub fn file_path(datadir: PathBuf, network: Network) -> PathBuf {
        let mut path = datadir;
        path.push(network.to_string());
        path.push(DEFAULT_FILE_NAME);
        path
    }
    pub fn from_file(datadir: PathBuf, network: Network) -> Result<Self, ConfigError> {
        let path = Self::file_path(datadir, network);

        let config = std::fs::read(path)
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => ConfigError::NotFound,
                _ => ConfigError::ReadingFile(format!("Reading settings file: {}", e)),
            })
            .and_then(|file_content| {
                serde_json::from_slice::<Config>(&file_content)
                    .map_err(|e| ConfigError::ReadingFile(format!("Parsing settings file: {}", e)))
            })?;
        Ok(config)
    }

    pub fn to_file(&self, datadir: PathBuf, network: Network) -> Result<(), ConfigError> {
        let path = Self::file_path(datadir, network);

        let content = serde_json::to_string_pretty(&self).map_err(|e| {
            ConfigError::WritingFile(format!("Failed to serialize settings: {}", e))
        })?;

        let mut settings_file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .map_err(|e| ConfigError::WritingFile(e.to_string()))?;

        settings_file.write_all(content.as_bytes()).map_err(|e| {
            tracing::warn!("failed to write to file: {:?}", e);
            ConfigError::WritingFile(e.to_string())
        })
    }
}

#[derive(PartialEq, Eq, Debug, Clone)]
pub enum ConfigError {
    NotFound,
    ReadingFile(String),
    WritingFile(String),
    Unexpected(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "Settings file not found"),
            Self::ReadingFile(e) => write!(f, "Error while reading file: {}", e),
            Self::WritingFile(e) => write!(f, "Error while writing file: {}", e),
            Self::Unexpected(e) => write!(f, "Unexpected error: {}", e),
        }
    }
}
