use base64::Engine;
use bitcoin_hashes::{sha256, HashEngine, HmacEngine, HmacSha256};
use coincube_core::{
    miniscript::bitcoin::{self, Network},
    random::{random_bytes, RandomnessError},
};
use coincube_ui::component::form;
use coincubed::config::BitcoindConfig;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::thread;
use std::time;

use tracing::{info, warn};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

use crate::dir::{BitcoindDirectory, CoincubeDirectory};
use crate::utils::now_fallible;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[cfg(target_os = "windows")]
const DETACHED_PROCESS: u32 = 0x00000008;

/// The flavour of managed Bitcoin node COINCUBE downloads, configures, and runs.
///
/// Only affects the managed local-node backend; the Esplora and Electrum
/// backends never touch a local binary. `Core` is the historical default;
/// `Knots` is opt-in and is the flavour that can enforce BIP-110 (RDTS) — see
/// [`InternalBitcoindConfig::enforce_rdts`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum NodeFlavor {
    /// Bitcoin Core, fetched from bitcoincore.org.
    #[default]
    Core,
    /// Bitcoin Knots, fetched from bitcoinknots.org. Ships RDTS (BIP-110)
    /// enforcement in mainline from `29.3.knots20260508`.
    Knots,
}

/// Current and previous managed Bitcoin Core versions, in order of descending version.
pub const CORE_VERSIONS: [&str; 7] = ["29.0", "28.0", "27.1", "26.1", "26.0", "25.1", "25.0"];

/// Current managed Bitcoin Core version for new installations.
pub const CORE_VERSION: &str = CORE_VERSIONS[0];

/// Current and previous managed Bitcoin Knots versions, in order of descending version.
///
/// RDTS (BIP-110) enforcement ships in mainline Knots from `29.3.knots20260508`;
/// older Knots builds are intentionally not offered. Pinned — bumping is a
/// deliberate follow-up (the `SHA256SUMS`-based verification in the installer
/// means a bump is not checksum-locked in code).
pub const KNOTS_VERSIONS: [&str; 1] = ["29.3.knots20260508"];

/// Current managed Bitcoin Knots version for new installations.
pub const KNOTS_VERSION: &str = KNOTS_VERSIONS[0];

// Pinned SHA-256 of the Bitcoin Core archive for the current `CORE_VERSION`, per
// platform. Knots is verified against its published `SHA256SUMS` manifest instead
// (see `installer::step::node::bitcoind`), so it needs no pinned hash here.
#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
pub const CORE_SHA256SUM: &str = "5bb824fc86a15318d6a83a1b821ff4cd4b3d3d0e1ec3d162b805ccf7cae6fca8";

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub const CORE_SHA256SUM: &str = "34431c582a0399dd42e1276d87d25306cbdde0217f6744bd55a2945986645dda";

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub const CORE_SHA256SUM: &str = "a681e4f6ce524c338a105f214613605bac6c33d58c31dc5135bbc02bc458bb6c";

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
pub const CORE_SHA256SUM: &str = "4c1780532031129fcacfc0e393c8430b3cea414c9f8c5e0c0c87ebe59a5ada1b";

/// PGP key fingerprint that signs Bitcoin Knots' `SHA256SUMS.asc`.
///
/// Confirmed from the issuer-fingerprint subpacket of the live
/// `…/29.3.knots20260508/SHA256SUMS.asc` — Luke Dashjr's canonical Knots
/// release key. Pinning the *fingerprint* lets us recognise the signing key;
/// full cryptographic verification of the detached signature additionally
/// requires vendoring the key's public material, tracked as an open item for
/// this feature (see `plans/PLAN-knots-bip110-managed-node.md`).
pub const KNOTS_SIGNING_KEY_FINGERPRINT: &str = "1A3E761F19D2CC7785C5502EA291A2C45D0C504A";

/// Vendored armored OpenPGP public key for [`KNOTS_SIGNING_KEY_FINGERPRINT`]
/// (Luke Dashjr's Knots codesigning key), used to verify `SHA256SUMS.asc`. It is
/// a minimal export (primary key + self-sig only) so it is small and needs no
/// keyserver/keyring at runtime. The fingerprint is re-derived from this key and
/// checked against the pin at verification time, so a swapped-out file cannot
/// silently change the trust anchor.
pub const KNOTS_SIGNING_KEY_ASC: &str = include_str!("../../assets/knots_signing_key.asc");

/// Operating system COINCUBE builds managed-node asset names for. Kept explicit
/// (rather than only `cfg!`) so URL construction is unit-testable for every
/// `(flavor, platform)` regardless of the host running the test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeOs {
    MacOs,
    Linux,
    Windows,
}

/// CPU architecture COINCUBE builds managed-node asset names for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeArch {
    X86_64,
    Aarch64,
}

#[cfg(target_os = "macos")]
pub const HOST_OS: NodeOs = NodeOs::MacOs;
#[cfg(target_os = "linux")]
pub const HOST_OS: NodeOs = NodeOs::Linux;
#[cfg(target_os = "windows")]
pub const HOST_OS: NodeOs = NodeOs::Windows;

#[cfg(target_arch = "x86_64")]
pub const HOST_ARCH: NodeArch = NodeArch::X86_64;
#[cfg(target_arch = "aarch64")]
pub const HOST_ARCH: NodeArch = NodeArch::Aarch64;

impl NodeFlavor {
    /// Current managed version for new installations of this flavour.
    pub fn version(self) -> &'static str {
        match self {
            NodeFlavor::Core => CORE_VERSION,
            NodeFlavor::Knots => KNOTS_VERSION,
        }
    }

    /// All known managed versions for this flavour, newest first. Used to find
    /// an already-installed binary on disk.
    pub fn versions(self) -> &'static [&'static str] {
        match self {
            NodeFlavor::Core => &CORE_VERSIONS,
            NodeFlavor::Knots => &KNOTS_VERSIONS,
        }
    }

    /// Infer the flavour from a managed-binary directory `version` string.
    /// Knots version strings embed `knots`; Core's never do.
    pub fn from_version(version: &str) -> Self {
        if version.contains("knots") {
            NodeFlavor::Knots
        } else {
            NodeFlavor::Core
        }
    }

    /// Human-readable name for UI copy and logs.
    pub fn display_name(self) -> &'static str {
        match self {
            NodeFlavor::Core => "Bitcoin Core",
            NodeFlavor::Knots => "Bitcoin Knots",
        }
    }

    /// Download archive filename for `(self, version)` on the given platform.
    ///
    /// Knots reuses Core's `{arch}-{os}` suffixes on macOS/Linux, but its
    /// Windows asset carries a `-pgpverifiable` suffix Core does not.
    pub fn asset_filename(self, version: &str, os: NodeOs, arch: NodeArch) -> String {
        match (os, arch) {
            (NodeOs::MacOs, NodeArch::X86_64) => {
                format!("bitcoin-{version}-x86_64-apple-darwin.tar.gz")
            }
            (NodeOs::MacOs, NodeArch::Aarch64) => {
                format!("bitcoin-{version}-arm64-apple-darwin.tar.gz")
            }
            (NodeOs::Linux, NodeArch::X86_64) => {
                format!("bitcoin-{version}-x86_64-linux-gnu.tar.gz")
            }
            (NodeOs::Linux, NodeArch::Aarch64) => {
                format!("bitcoin-{version}-aarch64-linux-gnu.tar.gz")
            }
            (NodeOs::Windows, _) => match self {
                NodeFlavor::Core => format!("bitcoin-{version}-win64.zip"),
                NodeFlavor::Knots => format!("bitcoin-{version}-win64-pgpverifiable.zip"),
            },
        }
    }

    /// Download URL for `(self, version)` on the given platform.
    pub fn asset_url(self, version: &str, os: NodeOs, arch: NodeArch) -> String {
        let filename = self.asset_filename(version, os, arch);
        match self {
            NodeFlavor::Core => {
                format!("https://bitcoincore.org/bin/bitcoin-core-{version}/{filename}")
            }
            NodeFlavor::Knots => {
                // e.g. "29.3.knots20260508" -> major "29" -> ".../29.x/29.3.knots20260508/".
                let major = version.split('.').next().unwrap_or(version);
                format!("https://bitcoinknots.org/files/{major}.x/{version}/{filename}")
            }
        }
    }

    /// Download archive filename for this flavour's current version on the host.
    pub fn download_filename(self) -> String {
        self.asset_filename(self.version(), HOST_OS, HOST_ARCH)
    }

    /// Download URL for this flavour's current version on the host.
    pub fn download_url(self) -> String {
        self.asset_url(self.version(), HOST_OS, HOST_ARCH)
    }

    /// URLs of the release `SHA256SUMS` and `SHA256SUMS.asc` for this flavour's
    /// current version. `None` for flavours verified by a code-pinned hash
    /// (Core); `Some` for those verified against a published manifest (Knots).
    pub fn manifest_urls(self) -> Option<(String, String)> {
        match self {
            NodeFlavor::Core => None,
            NodeFlavor::Knots => {
                let version = self.version();
                let major = version.split('.').next().unwrap_or(version);
                let base = format!("https://bitcoinknots.org/files/{major}.x/{version}");
                Some((
                    format!("{base}/SHA256SUMS"),
                    format!("{base}/SHA256SUMS.asc"),
                ))
            }
        }
    }
}

pub fn internal_bitcoind_directory(coincube_datadir: &CoincubeDirectory) -> PathBuf {
    coincube_datadir.bitcoind_directory().path().to_path_buf()
}

/// Data directory used by internal bitcoind.
pub fn internal_bitcoind_datadir(coincube_datadir: &CoincubeDirectory) -> PathBuf {
    let mut datadir = internal_bitcoind_directory(coincube_datadir);
    datadir.push("datadir");
    datadir
}

/// Internal bitcoind executable path.
pub fn internal_bitcoind_exe_path(
    coincube_datadir: &CoincubeDirectory,
    bitcoind_version: &str,
) -> PathBuf {
    internal_bitcoind_directory(coincube_datadir)
        .join(format!("bitcoin-{}", bitcoind_version))
        .join("bin")
        .join(if cfg!(target_os = "windows") {
            "bitcoind.exe"
        } else {
            "bitcoind"
        })
}

/// Path of the `bitcoin.conf` file used by internal bitcoind.
pub fn internal_bitcoind_config_path(bitcoind_datadir: &Path) -> PathBuf {
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
pub fn internal_bitcoind_debug_log_path(
    coincubed_datadir: &CoincubeDirectory,
    network: Network,
) -> PathBuf {
    let mut debug_log_path = internal_bitcoind_datadir(coincubed_datadir);
    if let Some(dir) = bitcoind_network_dir(&network) {
        debug_log_path.push(dir);
    }
    debug_log_path.push("debug.log");
    debug_log_path
}

#[allow(unreachable_patterns)]
pub fn bitcoind_network_dir(network: &Network) -> Option<String> {
    let dir = match network {
        Network::Bitcoin => {
            return None;
        }
        Network::Testnet => "testnet3",
        Network::Testnet4 => "testnet4",
        Network::Regtest => "regtest",
        Network::Signet => "signet",
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
#[derive(PartialEq, Eq, Debug, Clone, Serialize, Deserialize)]
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
        let password_hmac = <HmacSha256 as bitcoin_hashes::GeneralHash>::from_engine(engine);

        Ok((
            Self {
                user: user.to_string(),
                salt,
                password_hmac: hex::encode(password_hmac.as_ref()),
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
#[derive(PartialEq, Eq, Debug, Clone, Serialize, Deserialize)]
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
    /// Which managed node flavour this config is for. Recovered on load from
    /// `enforce_rdts` (and, at runtime, from the binary's subversion); it is
    /// not written as its own key because bitcoind rejects unknown options.
    pub flavor: NodeFlavor,
    /// When true (Knots only), [`Self::to_ini`] emits `consensusrules=rdts`,
    /// making the node enforce BIP-110. This is the only persisted marker of
    /// RDTS enforcement. Never emitted for Core, which rejects the key and
    /// refuses to start.
    pub enforce_rdts: bool,
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
            flavor: NodeFlavor::Core,
            enforce_rdts: false,
        }
    }

    /// A config for the given managed-node flavour. For Knots, RDTS (BIP-110)
    /// enforcement defaults on — that is the reason a user opts into Knots —
    /// while staying a distinct field so "Knots without RDTS" remains
    /// expressible. For Core, RDTS is never enforced.
    pub fn for_flavor(flavor: NodeFlavor) -> Self {
        Self {
            networks: BTreeMap::new(),
            flavor,
            enforce_rdts: matches!(flavor, NodeFlavor::Knots),
        }
    }

    pub fn from_ini(ini: &ini::Ini) -> Result<Self, InternalBitcoindConfigError> {
        let mut networks = BTreeMap::new();
        let mut enforce_rdts = false;
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
            } else {
                // The general (section-less) part of the file. We only ever
                // write `consensusrules=rdts` here (Knots RDTS enforcement);
                // anything else is unexpected.
                for (key, value) in prop.iter() {
                    if key == "consensusrules" {
                        enforce_rdts = value.split(',').any(|rule| rule.trim() == "rdts");
                    } else {
                        return Err(InternalBitcoindConfigError::UnexpectedSection(format!(
                            "Unexpected key in general section: {key}"
                        )));
                    }
                }
            }
        }
        // A persisted `consensusrules=rdts` is the marker that this is a Knots
        // RDTS node; absent it, we assume Core. The runtime subversion is the
        // authoritative source once the node is up (see settings UI).
        let flavor = if enforce_rdts {
            NodeFlavor::Knots
        } else {
            NodeFlavor::Core
        };
        Ok(Self {
            networks,
            flavor,
            enforce_rdts,
        })
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

        // RDTS (BIP-110) enforcement is a global, non-network-scoped option and
        // is only valid on Knots — Core rejects the key and refuses to start, so
        // gating on `enforce_rdts` (only ever true for Knots) keeps Core safe.
        // We run bitcoind headless, so Knots' native GUI confirmation prompt
        // never fires; writing this line is both necessary and sufficient to
        // enforce. Written before the network sections so it lands in the
        // section-less general part of the file.
        if self.enforce_rdts {
            conf_ini
                .with_general_section()
                .set("consensusrules", "rdts");
        }

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
    Lock(String),
    CommandError(String),
    CouldNotCanonicalizeDataDir(String),
    BitcoinDError(String),
    ExecutableNotFound,
    ProcessExited(std::process::ExitStatus),
}

impl std::fmt::Display for StartInternalBitcoindError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Lock(e) => {
                write!(f, "lock file error: {}", e)
            }
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
    pub config: BitcoindConfig,
    lock: LockFile,
}

/// Pick the managed `bitcoind` binary to launch for `configured_flavor`,
/// preferring that flavour's versions (newest first) and falling back to the
/// other flavour's only if none are installed. Returns the first existing
/// `bitcoin-<version>/bin/bitcoind[.exe]` under the managed directory, or `None`
/// when nothing is installed. Preferring the configured flavour keeps the binary
/// consistent with the `bitcoin.conf` — critical because a Knots `bitcoin.conf`
/// (with `consensusrules=rdts`) cannot be started by a Core binary.
fn select_managed_bitcoind_exe(
    coincube_datadir: &CoincubeDirectory,
    configured_flavor: NodeFlavor,
) -> Option<PathBuf> {
    let (primary, secondary): (&[&str], &[&str]) = match configured_flavor {
        NodeFlavor::Knots => (&KNOTS_VERSIONS, &CORE_VERSIONS),
        NodeFlavor::Core => (&CORE_VERSIONS, &KNOTS_VERSIONS),
    };
    primary
        .iter()
        .chain(secondary.iter())
        .map(|v| internal_bitcoind_exe_path(coincube_datadir, v))
        .find(|path| path.exists())
}

impl Bitcoind {
    /// Start internal bitcoind for the given network.
    pub fn maybe_start(
        network: bitcoin::Network,
        config: BitcoindConfig,
        coincube_datadir: &CoincubeDirectory,
    ) -> Result<Self, StartInternalBitcoindError> {
        if coincubed::BitcoinD::new(&config, "internal_bitcoind_start".to_string()).is_ok() {
            info!("Internal bitcoind is already running");
            return Ok(Bitcoind {
                config,
                lock: LockFile::create(coincube_datadir.bitcoind_directory(), network)
                    .map_err(|e| StartInternalBitcoindError::Lock(format!("{:?}", e)))?,
            });
        }
        let bitcoind_datadir = internal_bitcoind_datadir(coincube_datadir);
        // Launch a binary consistent with the on-disk `bitcoin.conf`. A conf
        // carrying `consensusrules=rdts` *requires* a Knots binary — starting
        // Core against it makes Core reject the unknown option and exit — so we
        // prefer the configured flavour's binary, not just the first one we find.
        // A machine that still has Core installed after a Knots setup would
        // otherwise launch Core against a Knots conf and fail to start.
        let configured_flavor =
            InternalBitcoindConfig::from_file(&internal_bitcoind_config_path(&bitcoind_datadir))
                .map(|conf| conf.flavor)
                .unwrap_or(NodeFlavor::Core);
        let bitcoind_exe_path = select_managed_bitcoind_exe(coincube_datadir, configured_flavor)
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
        let command = command.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);

        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // Create a new session to detach the child from the main process.
            unsafe {
                command.pre_exec(|| {
                    libc::setsid();
                    Ok(())
                });
            }
        }

        let mut process = command
            .args(&args)
            // FIXME: can we pipe stderr to our logging system somehow?
            .stdout(std::process::Stdio::null())
            .spawn()
            .map_err(|e| StartInternalBitcoindError::CommandError(e.to_string()))?;

        // We've started bitcoind in the background, however it may fail to start for whatever
        // reason. And we need its JSONRPC interface to be available to continue. Thus wait for
        // the interface to be created successfully, regularly checking it did not fail to start.
        let mut try_count = 0;
        loop {
            match process.try_wait() {
                Ok(None) => {}
                Err(e) => log::error!("Error while trying to wait for bitcoind: {}", e),
                Ok(Some(status)) => {
                    log::error!("Bitcoind exited with status '{}'", status);
                    return Err(StartInternalBitcoindError::ProcessExited(status));
                }
            }
            match coincubed::BitcoinD::new(&config, "internal_bitcoind_start".to_string()) {
                Ok(_) => {
                    log::info!("Bitcoind seems to have successfully started.");
                    return Ok(Self {
                        config,
                        lock: LockFile::create(coincube_datadir.bitcoind_directory(), network)
                            .map_err(|e| StartInternalBitcoindError::Lock(format!("{:?}", e)))?,
                    });
                }
                Err(coincubed::BitcoindError::CookieFile(_)) => {
                    // This is only raised if we're using cookie authentication.
                    // Assume cookie file has not been created yet and try again.
                }
                Err(e) => {
                    if !e.is_transient() && (!e.is_unauthorized() || try_count > 10) {
                        // Non-transient error could happen, e.g., if RPC auth credentials are wrong.
                        // Kill process now in case it's not possible to do via RPC command later.
                        // If the auth credentials are wrong, it is possible that coincube-gui is
                        // reading the previous state of the .cookie file and not the new generated
                        // one.
                        if let Err(e) = process.kill() {
                            log::error!("Error trying to kill bitcoind process: '{}'", e);
                        }
                        return Err(StartInternalBitcoindError::BitcoinDError(e.to_string()));
                    }
                }
            }
            try_count += 1;
            log::info!("Waiting for bitcoind to start.");
            thread::sleep(time::Duration::from_millis(500));
        }
    }

    /// Stop (internal) bitcoind.
    pub fn stop(self) {
        match self.lock.delete() {
            Err(e) => {
                tracing::error!("Failed to release bitcoind lock: {}", e);
            }
            Ok(false) => {
                info!("Other processes are using internal bitcoind. Process lock has been deleted");
            }
            Ok(true) => {
                match coincubed::BitcoinD::new(&self.config, "internal_bitcoind_stop".to_string()) {
                    Ok(bitcoind) => {
                        info!("Stopping internal bitcoind...");
                        bitcoind.stop();
                        info!("Stopped coincube managed bitcoind");
                    }
                    Err(e) => {
                        warn!("Could not create interface to internal bitcoind: '{}'.", e);
                    }
                }
            }
        }
    }
}

const LOCK_DIRECTORY_NAME: &str = "locks";

#[derive(Debug, Clone)]
struct LockFile {
    path: PathBuf,
    directory: BitcoindDirectory,
    network: Network,
}

impl LockFile {
    fn create(
        directory: BitcoindDirectory,
        network: Network,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let mut path = directory.clone().path().to_path_buf();
        path.push(LOCK_DIRECTORY_NAME);
        path.push(network.to_string());
        std::fs::create_dir_all(&path)?;

        path.push(format!(
            "{}-{}.lock",
            std::process::id(),
            now_fallible()?.as_secs()
        ));

        std::fs::File::create(&path)?;
        Ok(Self {
            path,
            directory,
            network,
        })
    }

    // returns true if the lock directory is removed because empty.
    fn delete(self) -> Result<bool, Box<dyn std::error::Error>> {
        std::fs::remove_file(self.path)?;
        if std::fs::read_dir(
            self.directory
                .path()
                .join(LOCK_DIRECTORY_NAME)
                .join(self.network.to_string()),
        )?
        .next()
        .is_none()
        {
            std::fs::remove_dir(
                self.directory
                    .path()
                    .join(LOCK_DIRECTORY_NAME)
                    .join(self.network.to_string()),
            )?;

            if std::fs::read_dir(self.directory.path().join(LOCK_DIRECTORY_NAME))?
                .next()
                .is_none()
            {
                std::fs::remove_dir(self.directory.path().join(LOCK_DIRECTORY_NAME))?;
            }

            Ok(true)
        } else {
            Ok(false)
        }
    }
}

// In case of panic, we remove all the bitcoind locks created by the process.
pub fn delete_all_bitcoind_locks_for_process(
    directory: BitcoindDirectory,
) -> Result<(), Box<dyn std::error::Error>> {
    let locks_directory = directory.path().join(LOCK_DIRECTORY_NAME);
    if !locks_directory.exists() {
        tracing::debug!("No internal bitcoind locks for the current process");
        return Ok(());
    }
    tracing::info!("Deleting all internal bitcoind locks for the current process");
    let process_prefix = format!("{}-", std::process::id());
    for network_dir in std::fs::read_dir(&locks_directory)? {
        let dir = network_dir?.path();
        for lock_file in std::fs::read_dir(&dir)? {
            let file = lock_file?.path();
            if let Some(name) = file.file_name().and_then(|n| n.to_str()) {
                if name.starts_with(&process_prefix) {
                    std::fs::remove_file(file)?;
                }
            }
        }
        if std::fs::read_dir(&dir)?.next().is_none() {
            std::fs::remove_dir(dir)?;
        }
    }
    if std::fs::read_dir(&locks_directory)?.next().is_none() {
        std::fs::remove_dir(locks_directory)?;
    }
    Ok(())
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
    use coincube_core::miniscript::bitcoin::Network;
    use ini::Ini;

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

    // Exact download URLs per (flavour, platform). Runs the same on any host
    // because `asset_url` takes the platform explicitly rather than via `cfg!`.
    #[test]
    fn node_flavor_asset_urls() {
        // Knots, arm64 macOS.
        assert_eq!(
            NodeFlavor::Knots.asset_url(KNOTS_VERSION, NodeOs::MacOs, NodeArch::Aarch64),
            "https://bitcoinknots.org/files/29.x/29.3.knots20260508/\
             bitcoin-29.3.knots20260508-arm64-apple-darwin.tar.gz"
        );
        // Knots, x86_64 Linux.
        assert_eq!(
            NodeFlavor::Knots.asset_url(KNOTS_VERSION, NodeOs::Linux, NodeArch::X86_64),
            "https://bitcoinknots.org/files/29.x/29.3.knots20260508/\
             bitcoin-29.3.knots20260508-x86_64-linux-gnu.tar.gz"
        );
        // Knots, Windows — note the `-pgpverifiable` suffix that Core lacks.
        assert_eq!(
            NodeFlavor::Knots.asset_url(KNOTS_VERSION, NodeOs::Windows, NodeArch::X86_64),
            "https://bitcoinknots.org/files/29.x/29.3.knots20260508/\
             bitcoin-29.3.knots20260508-win64-pgpverifiable.zip"
        );
        // Core path is byte-for-byte the historical shape.
        assert_eq!(
            NodeFlavor::Core.asset_url(CORE_VERSION, NodeOs::MacOs, NodeArch::Aarch64),
            format!(
                "https://bitcoincore.org/bin/bitcoin-core-{CORE_VERSION}/\
                 bitcoin-{CORE_VERSION}-arm64-apple-darwin.tar.gz"
            )
        );
        assert_eq!(
            NodeFlavor::Core.asset_url(CORE_VERSION, NodeOs::Windows, NodeArch::X86_64),
            format!(
                "https://bitcoincore.org/bin/bitcoin-core-{CORE_VERSION}/\
                 bitcoin-{CORE_VERSION}-win64.zip"
            )
        );
        // Flavour is recoverable from a managed-binary directory name.
        assert_eq!(
            NodeFlavor::from_version("29.3.knots20260508"),
            NodeFlavor::Knots
        );
        assert_eq!(NodeFlavor::from_version("29.0"), NodeFlavor::Core);
    }

    // `consensusrules=rdts` is emitted for Knots-with-enforcement only, and
    // round-trips through `to_ini`/`from_ini`.
    #[test]
    fn rdts_consensusrules_emission() {
        let net = InternalBitcoindNetworkConfig {
            rpc_port: 12345,
            p2p_port: 12346,
            prune: 15000,
            rpc_auth: None,
        };

        // Core: never emits consensusrules.
        let mut core = InternalBitcoindConfig::for_flavor(NodeFlavor::Core);
        core.networks.insert(Network::Bitcoin, net.clone());
        assert!(core
            .to_ini()
            .general_section()
            .get("consensusrules")
            .is_none());

        // Knots with enforcement (the default for the flavour): emits the line.
        let mut knots = InternalBitcoindConfig::for_flavor(NodeFlavor::Knots);
        assert!(knots.enforce_rdts);
        knots.networks.insert(Network::Bitcoin, net.clone());
        let knots_ini = knots.to_ini();
        assert_eq!(
            knots_ini.general_section().get("consensusrules"),
            Some("rdts")
        );

        // Round-trip preserves the flag and recovers the flavour.
        let parsed = InternalBitcoindConfig::from_ini(&knots_ini).expect("parse rdts conf");
        assert!(parsed.enforce_rdts);
        assert_eq!(parsed.flavor, NodeFlavor::Knots);

        // "Knots without RDTS" stays expressible and emits nothing.
        let mut knots_off = InternalBitcoindConfig::for_flavor(NodeFlavor::Knots);
        knots_off.enforce_rdts = false;
        knots_off.networks.insert(Network::Bitcoin, net);
        let off_ini = knots_off.to_ini();
        assert!(off_ini.general_section().get("consensusrules").is_none());
        assert!(
            !InternalBitcoindConfig::from_ini(&off_ini)
                .expect("parse non-rdts conf")
                .enforce_rdts
        );
    }

    // When both flavours are installed, the launched binary must match the
    // configured flavour — a Knots conf (`consensusrules=rdts`) cannot be
    // started by a Core binary, so Core must never be preferred over Knots.
    #[test]
    fn managed_binary_prefers_configured_flavor() {
        use std::fs;

        let base =
            std::env::temp_dir().join(format!("coincube-knots-bin-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let datadir = CoincubeDirectory::new(base.clone());

        // Install BOTH a Core and a Knots binary (the dual-install case).
        for v in [CORE_VERSION, KNOTS_VERSION] {
            let exe = internal_bitcoind_exe_path(&datadir, v);
            fs::create_dir_all(exe.parent().unwrap()).unwrap();
            fs::write(&exe, b"fake bitcoind").unwrap();
        }

        // Knots conf -> Knots binary, even though Core is also installed.
        assert_eq!(
            select_managed_bitcoind_exe(&datadir, NodeFlavor::Knots),
            Some(internal_bitcoind_exe_path(&datadir, KNOTS_VERSION))
        );
        // Core conf -> Core binary.
        assert_eq!(
            select_managed_bitcoind_exe(&datadir, NodeFlavor::Core),
            Some(internal_bitcoind_exe_path(&datadir, CORE_VERSION))
        );

        // Fallback: with only Knots installed, a Core conf still finds the
        // Knots binary rather than failing to locate any executable.
        let core_install = internal_bitcoind_exe_path(&datadir, CORE_VERSION)
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        fs::remove_dir_all(&core_install).unwrap();
        assert_eq!(
            select_managed_bitcoind_exe(&datadir, NodeFlavor::Core),
            Some(internal_bitcoind_exe_path(&datadir, KNOTS_VERSION))
        );

        let _ = fs::remove_dir_all(&base);
    }
}
