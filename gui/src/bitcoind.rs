use liana::{config::BitcoindConfig, miniscript::bitcoin};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;

use tracing::{info, warn};

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
#[derive(Debug, Clone)]
pub struct Bitcoind {
    _process: Arc<std::process::Child>,
    pub config: BitcoindConfig,
    pub stdout: Option<Arc<Mutex<std::process::ChildStdout>>>,
}

impl Bitcoind {
    /// Start internal bitcoind for the given network.
    pub fn start(
        network: &bitcoin::Network,
        mut config: BitcoindConfig,
        bitcoind_datadir: &Path,
        exe_path: &Path,
    ) -> Result<Self, StartInternalBitcoindError> {
        let datadir_path_str = bitcoind_datadir
            .canonicalize()
            .map_err(|e| StartInternalBitcoindError::CouldNotCanonicalizeDataDir(e.to_string()))?
            .to_str()
            .ok_or_else(|| {
                StartInternalBitcoindError::CouldNotCanonicalizeDataDir(
                    "Couldn't convert path to str.".to_string(),
                )
            })?
            .to_string();

        // See https://github.com/rust-lang/rust/issues/42869.
        #[cfg(target_os = "windows")]
        let datadir_path_str = datadir_path_str.replace("\\\\?\\", "").replace("\\\\?", "");

        let args = vec![
            format!("-chain={}", network.to_core_arg()),
            format!("-datadir={}", datadir_path_str),
        ];
        let mut command = std::process::Command::new(exe_path);

        #[cfg(target_os = "windows")]
        let command = command.creation_flags(CREATE_NO_WINDOW);

        let mut process = command
            .args(&args)
            .stdout(std::process::Stdio::piped()) // We still get bitcoind's logs in debug.log.
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| StartInternalBitcoindError::CommandError(e.to_string()))?;

        if !crate::utils::poll_for_file(&config.cookie_path, 200, 15) {
            match process.wait_with_output() {
                Err(e) => {
                    tracing::error!("Error while waiting for bitcoind to finish: {}", e)
                }
                Ok(o) => {
                    tracing::error!("Exit status: {}", o.status);
                    tracing::error!("stderr: {}", String::from_utf8_lossy(&o.stderr));
                }
            }
            return Err(StartInternalBitcoindError::CookieFileNotFound(
                config.cookie_path.to_string_lossy().into_owned(),
            ));
        }
        config.cookie_path = config.cookie_path.canonicalize().map_err(|e| {
            StartInternalBitcoindError::CouldNotCanonicalizeCookiePath(e.to_string())
        })?;

        liana::BitcoinD::new(&config, "internal_bitcoind_start".to_string())
            .map_err(|e| StartInternalBitcoindError::BitcoinDError(e.to_string()))?;

        Ok(Self {
            stdout: process.stdout.take().map(|s| Arc::new(Mutex::new(s))),
            config,
            _process: Arc::new(process),
        })
    }

    /// Stop (internal) bitcoind.
    pub fn stop(&self) {
        match liana::BitcoinD::new(&self.config, "internal_bitcoind_stop".to_string()) {
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
}
