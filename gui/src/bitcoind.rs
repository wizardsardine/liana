use liana::{config::BitcoindConfig, miniscript::bitcoin};

use tracing::{info, warn};

use crate::app::config::InternalBitcoindExeConfig;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

/// Possible errors when starting bitcoind.
#[derive(PartialEq, Eq, Debug, Clone)]
pub enum StartInternalBitcoindError {
    CommandError(String),
    CouldNotCanonicalizeExePath(String),
    CouldNotCanonicalizeDataDir(String),
    CouldNotCanonicalizeCookiePath(String),
    CookieFileNotFound(String),
    BitcoinDError(String),
}

impl std::fmt::Display for StartInternalBitcoindError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::CommandError(e) => {
                write!(f, "Command to start bitcoind returned an error: {}", e)
            }
            Self::CouldNotCanonicalizeExePath(e) => {
                write!(f, "Failed to canonicalize executable path: {}", e)
            }
            Self::CouldNotCanonicalizeDataDir(e) => {
                write!(f, "Failed to canonicalize datadir: {}", e)
            }
            Self::CouldNotCanonicalizeCookiePath(e) => {
                write!(f, "Failed to canonicalize cookie path: {}", e)
            }
            Self::CookieFileNotFound(path) => {
                write!(
                    f,
                    "Cookie file was not found at the expected path: {}",
                    path
                )
            }
            Self::BitcoinDError(e) => write!(f, "bitcoind connection check failed: {}", e),
        }
    }
}

/// Start internal bitcoind for the given network.
pub fn start_internal_bitcoind(
    network: &bitcoin::Network,
    exe_config: InternalBitcoindExeConfig,
) -> Result<std::process::Child, StartInternalBitcoindError> {
    let datadir_path_str = exe_config
        .data_dir
        .canonicalize()
        .map_err(|e| StartInternalBitcoindError::CouldNotCanonicalizeDataDir(e.to_string()))?
        .to_str()
        .ok_or_else(|| {
            StartInternalBitcoindError::CouldNotCanonicalizeDataDir(
                "Couldn't convert path to str.".to_string(),
            )
        })?
        .to_string();
    #[cfg(target_os = "windows")]
    // See https://github.com/rust-lang/rust/issues/42869.
    let datadir_path_str = datadir_path_str.replace("\\\\?\\", "").replace("\\\\?", "");
    let args = vec![
        format!("-chain={}", network.to_core_arg()),
        format!("-datadir={}", datadir_path_str),
    ];
    let mut command = std::process::Command::new(exe_config.exe_path);
    #[cfg(target_os = "windows")]
    let command = command.creation_flags(CREATE_NO_WINDOW);
    command
        .args(&args)
        .stdout(std::process::Stdio::null()) // We still get bitcoind's logs in debug.log.
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| StartInternalBitcoindError::CommandError(e.to_string()))
}

/// Stop (internal) bitcoind.
pub fn stop_internal_bitcoind(bitcoind_config: &BitcoindConfig) {
    match liana::BitcoinD::new(bitcoind_config, "internal_bitcoind_stop".to_string()) {
        Ok(bitcoind) => {
            info!("Stopping internal bitcoind...");
            bitcoind.stop();
            info!("Stopped liana managed bitcoind");
        }
        Err(e) => {
            warn!("Could not create interface to internal bitcoind: '{}'.", e);
        }
    }
}
