use liana::{
    config::BitcoindConfig,
    miniscript::bitcoin::{self, Network},
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

use tracing::{info, warn};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

pub const VERSION: &str = "25.0";

#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
pub const SHA256SUM: &str = "5708fc639cdfc27347cccfd50db9b73b53647b36fb5f3a4a93537cbe8828c27f";

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub const SHA256SUM: &str = "33930d432593e49d58a9bff4c30078823e9af5d98594d2935862788ce8a20aec";

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
pub const SHA256SUM: &str = "7154b35ecc8247589070ae739b7c73c4dee4794bea49eb18dc66faed65b819e7";

#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
pub fn download_filename() -> String {
    format!("bitcoin-{}-x86_64-apple-darwin.tar.gz", &VERSION)
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn download_filename() -> String {
    format!("bitcoin-{}-x86_64-linux-gnu.tar.gz", &VERSION)
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
fn download_filename() -> String {
    format!("bitcoin-{}-win64.zip", &VERSION)
}

pub fn download_url() -> String {
    format!(
        "https://bitcoincore.org/bin/bitcoin-core-{}/{}",
        &VERSION,
        download_filename()
    )
}

pub fn internal_bitcoind_directory(liana_datadir: &PathBuf) -> PathBuf {
    let mut datadir = PathBuf::from(liana_datadir);
    datadir.push("bitcoind");
    datadir
}

/// Data directory used by internal bitcoind.
pub fn internal_bitcoind_datadir(liana_datadir: &PathBuf) -> PathBuf {
    let mut datadir = internal_bitcoind_directory(liana_datadir);
    datadir.push("datadir");
    datadir
}

/// Internal bitcoind executable path.
pub fn internal_bitcoind_exe_path(liana_datadir: &PathBuf) -> PathBuf {
    internal_bitcoind_directory(liana_datadir)
        .join(format!("bitcoin-{}", &VERSION))
        .join("bin")
        .join(if cfg!(target_os = "windows") {
            "bitcoind.exe"
        } else {
            "bitcoind"
        })
}

/// Path of the `bitcoin.conf` file used by internal bitcoind.
pub fn internal_bitcoind_config_path(bitcoind_datadir: &PathBuf) -> PathBuf {
    let mut config_path = PathBuf::from(bitcoind_datadir);
    config_path.push("bitcoin.conf");
    config_path
}

/// Path of the cookie file used by internal bitcoind on a given network.
pub fn internal_bitcoind_cookie_path(bitcoind_datadir: &Path, network: &Network) -> PathBuf {
    let mut cookie_path = bitcoind_datadir.to_path_buf();
    if let Some(dir) = bitcoind_network_dir(network) {
        cookie_path.push(dir);
    }
    cookie_path.push(".cookie");
    cookie_path
}

pub fn bitcoind_network_dir(network: &Network) -> Option<String> {
    let dir = match network {
        Network::Bitcoin => {
            return None;
        }
        Network::Testnet => "testnet3",
        Network::Regtest => "regtest",
        Network::Signet => "signet",
        _ => panic!("Directory required for this network is unknown."),
    };
    Some(dir.to_string())
}

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
        liana_datadir: &PathBuf,
    ) -> Result<Self, StartInternalBitcoindError> {
        let bitcoind_datadir = internal_bitcoind_datadir(liana_datadir);
        let bitcoind_exe_path = internal_bitcoind_exe_path(liana_datadir);
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
        let mut command = std::process::Command::new(bitcoind_exe_path);

        #[cfg(target_os = "windows")]
        let command = command.creation_flags(CREATE_NO_WINDOW);

        let mut process = command
            .args(&args)
            .stdout(std::process::Stdio::piped())
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
