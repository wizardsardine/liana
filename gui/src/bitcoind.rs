use base64::Engine;
use bitcoin_hashes::{sha256, Hash, HashEngine, Hmac, HmacEngine};
use liana::{
    config::BitcoindConfig,
    miniscript::bitcoin::{self, Network},
    random::{random_bytes, RandomnessError},
};
use liana_ui::component::form;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time;

use tracing::{info, warn};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

/// Current and previous managed bitcoind versions, in order of descending version.
pub const VERSIONS: [&str; 4] = ["26.1", "26.0", "25.1", "25.0"];

/// Current managed bitcoind version for new installations.
pub const VERSION: &str = VERSIONS[0];

#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
pub const SHA256SUM: &str = "acb50edd20692a9d023de12da573b64ca0fd9b4e9a2b88d1251020a3022b0f27";

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub const SHA256SUM: &str = "8a8e415763b7ffd5988153cf03967d812eca629016dd3b0ddf6da3ab6f4a3621";

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub const SHA256SUM: &str = "a5b7d206384a8100058d3f2e2f02123a8e49e83f523499e70e86e121a4897d5b";

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
pub const SHA256SUM: &str = "7bd0849e47472aeff99a0ea2c0cefd98f5be829e5a2d3b0168b5a54456cc638a";

#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
pub fn download_filename() -> String {
    format!("bitcoin-{}-x86_64-apple-darwin.tar.gz", &VERSION)
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub fn download_filename() -> String {
    format!("bitcoin-{}-arm64-apple-darwin.tar.gz", &VERSION)
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
pub fn internal_bitcoind_exe_path(liana_datadir: &PathBuf, bitcoind_version: &str) -> PathBuf {
    internal_bitcoind_directory(liana_datadir)
        .join(format!("bitcoin-{}", bitcoind_version))
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

/// Path of the cookie file used by internal bitcoind on a given network.
pub fn internal_bitcoind_debug_log_path(lianad_datadir: &PathBuf, network: Network) -> PathBuf {
    let mut debug_log_path = internal_bitcoind_datadir(lianad_datadir);
    if let Some(dir) = bitcoind_network_dir(&network) {
        debug_log_path.push(dir);
    }
    debug_log_path.push("debug.log");
    debug_log_path
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

#[derive(PartialEq, Eq, Debug, Clone)]
pub enum RpcAuthParseError {
    MissingColon,
    MissingDollarSign,
}

impl std::fmt::Display for RpcAuthParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::MissingColon => write!(
                f,
                "RPC auth string should contain colon between user and salt."
            ),
            Self::MissingDollarSign => write!(
                f,
                "RPC auth string should contain dollar sign between salt and password HMAC."
            ),
        }
    }
}

/// Represents RPC auth credentials as stored in bitcoin.conf.
#[derive(PartialEq, Eq, Debug, Clone)]
pub struct RpcAuth {
    pub user: String,
    salt: String,
    password_hmac: String,
}

impl RpcAuth {
    /// Returns a new `RpcAuth` object for the given `user` with a random salt and password.
    /// This random password is also returned.
    pub fn new(user: &str) -> Result<(Self, String), RandomnessError> {
        // RPC auth generation follows approach in
        // https://github.com/bitcoin/bitcoin/blob/master/share/rpcauth/rpcauth.py
        let password =
            random_bytes().map(|bytes| base64::prelude::BASE64_URL_SAFE_NO_PAD.encode(bytes))?;
        // As per the Python script, only use 16 bytes for the salt.
        let salt = random_bytes().map(|bytes| hex::encode(&bytes[..16]))?;
        let mut engine = HmacEngine::<sha256::Hash>::new(salt.as_bytes());
        engine.input(password.as_bytes());
        let password_hmac = Hmac::<sha256::Hash>::from_engine(engine);

        Ok((
            Self {
                user: user.to_string(),
                salt,
                password_hmac: hex::encode(&password_hmac[..]),
            },
            password,
        ))
    }
}

impl std::fmt::Display for RpcAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}:{}${}", self.user, self.salt, self.password_hmac)
    }
}

impl std::str::FromStr for RpcAuth {
    type Err = RpcAuthParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (user, salt_pw) = s.split_once(':').ok_or(RpcAuthParseError::MissingColon)?;
        let (salt, pw) = salt_pw
            .split_once('$')
            .ok_or(RpcAuthParseError::MissingDollarSign)?;
        Ok(Self {
            user: user.to_string(),
            salt: salt.to_string(),
            password_hmac: pw.to_string(),
        })
    }
}

/// Represents section for a single network in `bitcoin.conf` file.
#[derive(PartialEq, Eq, Debug, Clone)]
pub struct InternalBitcoindNetworkConfig {
    pub rpc_port: u16,
    pub p2p_port: u16,
    pub prune: u32,
    pub rpc_auth: Option<RpcAuth>,
}

/// Represents the `bitcoin.conf` file to be used by internal bitcoind.
#[derive(Debug, Clone)]
pub struct InternalBitcoindConfig {
    pub networks: BTreeMap<Network, InternalBitcoindNetworkConfig>,
}

#[derive(PartialEq, Eq, Debug, Clone)]
pub enum InternalBitcoindConfigError {
    KeyNotFound(String),
    CouldNotParseValue(String),
    UnexpectedSection(String),
    TooManyElements(String),
    FileNotFound,
    ReadingFile(String),
    WritingFile(String),
    Unexpected(String),
}

impl std::fmt::Display for InternalBitcoindConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::KeyNotFound(e) => write!(f, "Config file does not contain expected key: {}", e),
            Self::CouldNotParseValue(e) => write!(f, "Value could not be parsed: {}", e),
            Self::UnexpectedSection(e) => write!(f, "Unexpected section in file: {}", e),
            Self::TooManyElements(section) => {
                write!(f, "Section in file contains too many elements: {}", section)
            }
            Self::FileNotFound => write!(f, "File not found"),
            Self::ReadingFile(e) => write!(f, "Error while reading file: {}", e),
            Self::WritingFile(e) => write!(f, "Error while writing file: {}", e),
            Self::Unexpected(e) => write!(f, "Unexpected error: {}", e),
        }
    }
}

impl Default for InternalBitcoindConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl InternalBitcoindConfig {
    pub fn new() -> Self {
        Self {
            networks: BTreeMap::new(),
        }
    }

    pub fn from_ini(ini: &ini::Ini) -> Result<Self, InternalBitcoindConfigError> {
        let mut networks = BTreeMap::new();
        for (maybe_sec, prop) in ini {
            if let Some(sec) = maybe_sec {
                let network = Network::from_core_arg(sec)
                    .map_err(|e| InternalBitcoindConfigError::UnexpectedSection(e.to_string()))?;
                if prop.len() > 4 {
                    return Err(InternalBitcoindConfigError::TooManyElements(
                        sec.to_string(),
                    ));
                }
                let rpc_port = prop
                    .get("rpcport")
                    .ok_or_else(|| InternalBitcoindConfigError::KeyNotFound("rpcport".to_string()))?
                    .parse::<u16>()
                    .map_err(|e| InternalBitcoindConfigError::CouldNotParseValue(e.to_string()))?;
                let p2p_port = prop
                    .get("port")
                    .ok_or_else(|| InternalBitcoindConfigError::KeyNotFound("port".to_string()))?
                    .parse::<u16>()
                    .map_err(|e| InternalBitcoindConfigError::CouldNotParseValue(e.to_string()))?;
                let prune = prop
                    .get("prune")
                    .ok_or_else(|| InternalBitcoindConfigError::KeyNotFound("prune".to_string()))?
                    .parse::<u32>()
                    .map_err(|e| InternalBitcoindConfigError::CouldNotParseValue(e.to_string()))?;
                let rpc_auth = prop
                    .get("rpcauth")
                    .map(|v| {
                        v.parse::<RpcAuth>().map_err(|e| {
                            InternalBitcoindConfigError::CouldNotParseValue(e.to_string())
                        })
                    })
                    .transpose()?;

                networks.insert(
                    network,
                    InternalBitcoindNetworkConfig {
                        rpc_port,
                        p2p_port,
                        prune,
                        rpc_auth,
                    },
                );
            } else if !prop.is_empty() {
                return Err(InternalBitcoindConfigError::UnexpectedSection(
                    "General section should be empty".to_string(),
                ));
            }
        }
        Ok(Self { networks })
    }

    pub fn from_file(path: &PathBuf) -> Result<Self, InternalBitcoindConfigError> {
        if !path.exists() {
            return Err(InternalBitcoindConfigError::FileNotFound);
        }
        let conf_ini = ini::Ini::load_from_file(path)
            .map_err(|e| InternalBitcoindConfigError::ReadingFile(e.to_string()))?;

        Self::from_ini(&conf_ini)
    }

    pub fn to_ini(&self) -> ini::Ini {
        let mut conf_ini = ini::Ini::new();

        for (network, network_conf) in &self.networks {
            conf_ini
                .with_section(Some(network.to_core_arg()))
                .set("rpcport", network_conf.rpc_port.to_string())
                .set("port", network_conf.p2p_port.to_string())
                .set("prune", network_conf.prune.to_string());
            if let Some(rpc_auth) = network_conf.rpc_auth.as_ref() {
                conf_ini
                    .with_section(Some(network.to_core_arg()))
                    .set("rpcauth", rpc_auth.to_string());
            }
        }
        conf_ini
    }

    pub fn to_file(&self, path: &PathBuf) -> Result<(), InternalBitcoindConfigError> {
        std::fs::create_dir_all(
            path.parent()
                .ok_or_else(|| InternalBitcoindConfigError::Unexpected("No parent".to_string()))?,
        )
        .map_err(|e| InternalBitcoindConfigError::Unexpected(e.to_string()))?;
        info!("Writing to file {}", path.to_string_lossy());
        self.to_ini()
            .write_to_file(path)
            .map_err(|e| InternalBitcoindConfigError::WritingFile(e.to_string()))?;

        Ok(())
    }
}

/// Possible errors when starting bitcoind.
#[derive(PartialEq, Eq, Debug, Clone)]
pub enum StartInternalBitcoindError {
    CommandError(String),
    CouldNotCanonicalizeDataDir(String),
    BitcoinDError(String),
    ExecutableNotFound,
    ProcessExited(std::process::ExitStatus),
}

impl std::fmt::Display for StartInternalBitcoindError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::CommandError(e) => {
                write!(f, "Command to start bitcoind returned an error: {}", e)
            }
            Self::CouldNotCanonicalizeDataDir(e) => {
                write!(f, "Failed to canonicalize datadir: {}", e)
            }
            Self::BitcoinDError(e) => write!(f, "bitcoind connection check failed: {}", e),
            Self::ExecutableNotFound => write!(f, "bitcoind executable not found."),
            Self::ProcessExited(status) => {
                write!(f, "bitcoind process exited with status '{}'.", status)
            }
        }
    }
}
#[derive(Debug, Clone)]
pub struct Bitcoind {
    _process: Arc<std::process::Child>,
    pub config: BitcoindConfig,
}

impl Bitcoind {
    /// Start internal bitcoind for the given network.
    pub fn start(
        network: &bitcoin::Network,
        config: BitcoindConfig,
        liana_datadir: &PathBuf,
    ) -> Result<Self, StartInternalBitcoindError> {
        let bitcoind_datadir = internal_bitcoind_datadir(liana_datadir);
        // Find most recent bitcoind version available.
        let bitcoind_exe_path = VERSIONS
            .iter()
            .filter_map(|v| {
                let path = internal_bitcoind_exe_path(liana_datadir, v);
                if path.exists() {
                    Some(path)
                } else {
                    None
                }
            })
            .next()
            .ok_or(StartInternalBitcoindError::ExecutableNotFound)?;
        info!(
            "Found bitcoind executable at '{}'.",
            bitcoind_exe_path.to_string_lossy()
        );
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
            // FIXME: can we pipe stderr to our logging system somehow?
            .stdout(std::process::Stdio::null())
            .spawn()
            .map_err(|e| StartInternalBitcoindError::CommandError(e.to_string()))?;

        // We've started bitcoind in the background, however it may fail to start for whatever
        // reason. And we need its JSONRPC interface to be available to continue. Thus wait for
        // the interface to be created successfully, regularly checking it did not fail to start.
        loop {
            match process.try_wait() {
                Ok(None) => {}
                Err(e) => log::error!("Error while trying to wait for bitcoind: {}", e),
                Ok(Some(status)) => {
                    log::error!("Bitcoind exited with status '{}'", status);
                    return Err(StartInternalBitcoindError::ProcessExited(status));
                }
            }
            match liana::BitcoinD::new(&config, "internal_bitcoind_start".to_string()) {
                Ok(_) => {
                    log::info!("Bitcoind seems to have successfully started.");
                    return Ok(Self {
                        config,
                        _process: Arc::new(process),
                    });
                }
                Err(liana::BitcoindError::CookieFile(_)) => {
                    // This is only raised if we're using cookie authentication.
                    // Assume cookie file has not been created yet and try again.
                }
                Err(e) => {
                    if !e.is_transient() {
                        // Non-transient error could happen, e.g., if RPC auth credentials are wrong.
                        // Kill process now in case it's not possible to do via RPC command later.
                        if let Err(e) = process.kill() {
                            log::error!("Error trying to kill bitcoind process: '{}'", e);
                        }
                        return Err(StartInternalBitcoindError::BitcoinDError(e.to_string()));
                    }
                }
            }
            log::info!("Waiting for bitcoind to start.");
            thread::sleep(time::Duration::from_millis(500));
        }
    }

    /// Stop (internal) bitcoind.
    pub fn stop(&self) {
        stop_bitcoind(&self.config);
    }
}

pub fn stop_bitcoind(config: &BitcoindConfig) -> bool {
    match liana::BitcoinD::new(config, "internal_bitcoind_stop".to_string()) {
        Ok(bitcoind) => {
            info!("Stopping internal bitcoind...");
            bitcoind.stop();
            info!("Stopped liana managed bitcoind");
            true
        }
        Err(e) => {
            warn!("Could not create interface to internal bitcoind: '{}'.", e);
            false
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum RpcAuthType {
    CookieFile,
    UserPass,
}

impl fmt::Display for RpcAuthType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            RpcAuthType::CookieFile => write!(f, "Cookie file path"),
            RpcAuthType::UserPass => write!(f, "User and password"),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RpcAuthValues {
    pub cookie_path: form::Value<String>,
    pub user: form::Value<String>,
    pub password: form::Value<String>,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ConfigField {
    Address,
    CookieFilePath,
    User,
    Password,
}

impl fmt::Display for ConfigField {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ConfigField::Address => write!(f, "Socket address"),
            ConfigField::CookieFilePath => write!(f, "Cookie file path"),
            ConfigField::User => write!(f, "User"),
            ConfigField::Password => write!(f, "Password"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ini::Ini;
    use liana::miniscript::bitcoin::Network;

    // Test the format of the internal bitcoind configuration file.
    #[test]
    fn internal_bitcoind_config() {
        // A valid config
        let mut conf_ini = Ini::new();
        conf_ini
            .with_section(Some("main"))
            .set("rpcport", "43345")
            .set("port", "42355")
            .set("prune", "15246");
        conf_ini
            .with_section(Some("regtest"))
            .set("rpcport", "34067")
            .set("port", "45175")
            .set("prune", "2043")
            .set("rpcauth", "my_user:my_salt$my_pw_hmac");
        let conf = InternalBitcoindConfig::from_ini(&conf_ini).expect("Loading conf from ini");
        let main_conf = InternalBitcoindNetworkConfig {
            rpc_port: 43345,
            p2p_port: 42355,
            prune: 15246,
            rpc_auth: None,
        };
        let regtest_conf = InternalBitcoindNetworkConfig {
            rpc_port: 34067,
            p2p_port: 45175,
            prune: 2043,
            rpc_auth: Some(RpcAuth {
                user: "my_user".to_string(),
                salt: "my_salt".to_string(),
                password_hmac: "my_pw_hmac".to_string(),
            }),
        };
        assert_eq!(conf.networks.len(), 2);
        assert_eq!(
            conf.networks.get(&Network::Bitcoin).expect("Missing main"),
            &main_conf
        );
        assert_eq!(
            conf.networks
                .get(&Network::Regtest)
                .expect("Missing regtest"),
            &regtest_conf
        );

        let mut conf = InternalBitcoindConfig::new();
        conf.networks.insert(Network::Bitcoin, main_conf);
        conf.networks.insert(Network::Regtest, regtest_conf);
        conf_ini = conf.to_ini();
        assert_eq!(conf_ini.len(), 3); // 2 network sections plus the empty general section
        assert!(conf_ini.general_section().is_empty());
        for (sec, prop) in &conf_ini {
            if let Some(sec) = sec {
                let rpc_port = prop.get("rpcport").expect("rpcport");
                let p2p_port = prop.get("port").expect("port");
                let prune = prop.get("prune").expect("prune");
                let rpc_auth = prop.get("rpcauth");
                if sec == "main" {
                    assert_eq!(prop.len(), 3);
                    assert_eq!(rpc_port, "43345");
                    assert_eq!(p2p_port, "42355");
                    assert_eq!(prune, "15246");
                    assert!(rpc_auth.is_none());
                } else if sec == "regtest" {
                    assert_eq!(prop.len(), 4);
                    assert_eq!(rpc_port, "34067");
                    assert_eq!(p2p_port, "45175");
                    assert_eq!(prune, "2043");
                    assert_eq!(rpc_auth, Some("my_user:my_salt$my_pw_hmac"));
                } else {
                    panic!("Unexpected section");
                }
            } else {
                assert!(prop.is_empty())
            }
        }
    }
}
