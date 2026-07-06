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

#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
pub const CORE_SHA256SUM: &str = "7922ac99363dd28f79e57ef7098581fd48ebd1119b412b07e73b1fd19fd0443f";

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

/// Current managed Tor version, sourced from the Tor Project's Tor Expert Bundle
/// (the same `tor` daemon shipped inside Tor Browser). Pinned like
/// [`KNOTS_VERSIONS`] — bumps are a deliberate follow-up, since the bundle is
/// verified against its published `sha256sums-unsigned-build.txt` manifest and a
/// bump is therefore not checksum-locked in code.
pub const TOR_VERSION: &str = "15.0.17";

/// PGP key fingerprint of the "Tor Browser Developers (signing key)"
/// (`torbrowser@torproject.org`) that signs the Tor Expert Bundle's
/// `sha256sums-unsigned-build.txt.asc`. Canonical, long-published fingerprint;
/// pinning it lets us recognise the signing key. Full verification of the
/// detached signature additionally uses the vendored public material below.
pub const TOR_SIGNING_KEY_FINGERPRINT: &str = "EF6E286DDA85EA2A4BA7DE684E2C6E8793298290";

/// Vendored armored OpenPGP public key for [`TOR_SIGNING_KEY_FINGERPRINT`] (the
/// Tor Browser Developers signing key), used to verify the Tor Expert Bundle's
/// `sha256sums-unsigned-build.txt.asc`. The fingerprint is re-derived from this
/// key and checked against the pin at verification time, so a swapped-out file
/// cannot silently change the trust anchor.
pub const TOR_SIGNING_KEY_ASC: &str = include_str!("../../assets/tor_signing_key.asc");

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

    /// Infer the flavour from a running node's `getnetworkinfo.subversion`
    /// (e.g. `/Satoshi:29.3.0(knots20260508)/`). Knots embeds `knots`; Core
    /// never does. Used to decide whether a reachable managed node already
    /// matches the configured flavour or must be replaced.
    pub fn from_subversion(subversion: &str) -> Self {
        if subversion.to_lowercase().contains("knots") {
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

impl std::fmt::Display for NodeFlavor {
    /// Human-readable name, so `NodeFlavor` can back a `pick_list`.
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

/// Tor Expert Bundle archive filename for `version` on the given platform, e.g.
/// `tor-expert-bundle-macos-aarch64-15.0.17.tar.gz`.
///
/// Returns `None` for platforms the Tor Project does not publish an Expert
/// Bundle for — there is no Linux or Windows **aarch64** build. On those hosts
/// inbound-over-Tor is simply unavailable and the managed node runs
/// outbound-only (fail-safe); nothing else about the wallet changes. Kept
/// explicit (not `cfg!`) so URL construction is unit-testable for every
/// platform regardless of the host running the test.
pub fn tor_asset_filename(version: &str, os: NodeOs, arch: NodeArch) -> Option<String> {
    let platform = match (os, arch) {
        (NodeOs::MacOs, NodeArch::X86_64) => "macos-x86_64",
        (NodeOs::MacOs, NodeArch::Aarch64) => "macos-aarch64",
        (NodeOs::Linux, NodeArch::X86_64) => "linux-x86_64",
        (NodeOs::Windows, NodeArch::X86_64) => "windows-x86_64",
        // No Tor Expert Bundle is published for these.
        (NodeOs::Linux, NodeArch::Aarch64) | (NodeOs::Windows, NodeArch::Aarch64) => return None,
    };
    Some(format!("tor-expert-bundle-{platform}-{version}.tar.gz"))
}

/// Base URL of the Tor Project's package archive for `version`.
fn tor_release_base_url(version: &str) -> String {
    format!("https://archive.torproject.org/tor-package-archive/torbrowser/{version}")
}

/// Download URL for the Tor Expert Bundle for `version` on the given platform,
/// or `None` where no Expert Bundle is published (see [`tor_asset_filename`]).
pub fn tor_asset_url(version: &str, os: NodeOs, arch: NodeArch) -> Option<String> {
    let filename = tor_asset_filename(version, os, arch)?;
    Some(format!("{}/{filename}", tor_release_base_url(version)))
}

/// URLs of the Tor release `sha256sums-unsigned-build.txt` and its detached
/// `.asc`, which the Expert Bundle archive is verified against (reusing the
/// same signed-manifest path as Knots).
pub fn tor_manifest_urls(version: &str) -> (String, String) {
    let base = tor_release_base_url(version);
    (
        format!("{base}/sha256sums-unsigned-build.txt"),
        format!("{base}/sha256sums-unsigned-build.txt.asc"),
    )
}

/// Tor Expert Bundle download filename for the current pinned version on this
/// host, or `None` if Tor is unavailable for the host platform.
pub fn tor_download_filename() -> Option<String> {
    tor_asset_filename(TOR_VERSION, HOST_OS, HOST_ARCH)
}

/// Tor Expert Bundle download URL for the current pinned version on this host,
/// or `None` if Tor is unavailable for the host platform.
pub fn tor_download_url() -> Option<String> {
    tor_asset_url(TOR_VERSION, HOST_OS, HOST_ARCH)
}

/// Whether a managed Tor daemon can be installed on this host. `false` on
/// Linux/Windows aarch64 (no published Expert Bundle) — the settings UI hides
/// or disables inbound-over-Tor there.
pub fn tor_supported_on_host() -> bool {
    tor_download_filename().is_some()
}

pub fn internal_bitcoind_directory(coincube_datadir: &CoincubeDirectory) -> PathBuf {
    coincube_datadir.bitcoind_directory().path().to_path_buf()
}

/// Directory the managed Tor Expert Bundle for `version` is unpacked into. Sits
/// alongside the managed `bitcoin-<version>` install so both share the managed
/// bitcoind directory (and the duress wipe that covers it). Unpacking preserves
/// the bundle's own layout, so the contents are `tor/tor[.exe]`,
/// `tor/lib…`, `data/geoip…`, etc. underneath.
pub fn internal_tor_directory(coincube_datadir: &CoincubeDirectory, version: &str) -> PathBuf {
    internal_bitcoind_directory(coincube_datadir).join(format!("tor-{version}"))
}

/// Path of the managed `tor` executable for `version`.
pub fn internal_tor_exe_path(coincube_datadir: &CoincubeDirectory, version: &str) -> PathBuf {
    internal_tor_directory(coincube_datadir, version)
        .join("tor")
        .join(if cfg!(target_os = "windows") {
            "tor.exe"
        } else {
            "tor"
        })
}

/// Directory holding the bundle's geoip databases (`data/geoip`, `data/geoip6`)
/// for `version`, passed to tor via `GeoIPFile`/`GeoIPv6File`.
pub fn internal_tor_geoip_dir(coincube_datadir: &CoincubeDirectory, version: &str) -> PathBuf {
    internal_tor_directory(coincube_datadir, version).join("data")
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

/// Default daily upload cap (`maxuploadtarget`) when inbound-over-Tor is on,
/// in MiB — ~1 GB/day. Always set by default so a metered connection is never
/// surprised by unbounded upload (see `PLAN-inbound-tor-connectivity.md`).
pub const MAX_UPLOAD_TARGET_MB_DAY_DEFAULT: u32 = 1000;

/// Default total connection cap (`maxconnections`) when inbound-over-Tor is on.
/// bitcoind reserves 10 outbound (8 full + 2 block-only); the remainder are
/// inbound slots.
pub const MAX_CONNECTIONS_DEFAULT: u16 = 20;

/// Loopback host bitcoind uses to reach the co-located managed `tor` daemon's
/// control and SOCKS ports (`torcontrol`/`proxy`).
const TOR_LOOPBACK_HOST: &str = "127.0.0.1";

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
    /// Opt-in inbound connectivity over Tor. When true, [`Self::to_ini`] emits
    /// `listen=1`, `listenonion=1`, `discover=0` (and `torcontrol` once
    /// [`Self::tor_control_port`] is known), so bitcoind advertises itself as a
    /// v3 onion service and accepts inbound peers. The persisted marker is
    /// `listenonion=1`. Off by default — absent keys parse back to all-off, so
    /// existing datadirs are unchanged.
    pub inbound_tor: bool,
    /// Route *outbound* peer connections through Tor too, via `proxy=<socks>`.
    /// Only meaningful alongside `inbound_tor`. The persisted marker is the
    /// presence of a `proxy=` line.
    pub outbound_via_tor: bool,
    /// Daily upload cap emitted as `maxuploadtarget` (MiB). `None` = unlimited
    /// (key omitted; bitcoind then defaults to no cap). Only emitted when
    /// `inbound_tor` is set.
    pub max_upload_target_mb_day: Option<u32>,
    /// Total connection cap emitted as `maxconnections`. `None` = bitcoind's
    /// own default (key omitted). Only emitted when `inbound_tor` is set.
    pub max_connections: Option<u16>,
    /// Local control port of the managed `tor` daemon. Injected by the Tor
    /// lifecycle manager once Tor is up (see `node/tor.rs`); needed to emit
    /// `torcontrol=127.0.0.1:<port>`. Runtime-only — re-derived each start.
    pub tor_control_port: Option<u16>,
    /// Local SOCKS port of the managed `tor` daemon. Injected by the Tor
    /// lifecycle manager once Tor is up; needed to emit `proxy=127.0.0.1:<port>`
    /// when `outbound_via_tor` is set. Runtime-only — re-derived each start.
    pub tor_socks_port: Option<u16>,
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

/// Parse the port out of a `torcontrol`/`proxy` value such as
/// `127.0.0.1:9151`. bitcoind always writes these as `host:port`; we split on
/// the last colon so an IPv6 host (unused here, but harmless) still parses.
fn parse_loopback_port(value: &str) -> Result<u16, InternalBitcoindConfigError> {
    let port_str = value.rsplit_once(':').map(|(_, p)| p).unwrap_or(value);
    port_str
        .trim()
        .parse::<u16>()
        .map_err(|e| InternalBitcoindConfigError::CouldNotParseValue(e.to_string()))
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
            inbound_tor: false,
            outbound_via_tor: false,
            max_upload_target_mb_day: None,
            max_connections: None,
            tor_control_port: None,
            tor_socks_port: None,
        }
    }

    /// A config for the given managed-node flavour. For Knots, RDTS (BIP-110)
    /// enforcement defaults on — that is the reason a user opts into Knots —
    /// while staying a distinct field so "Knots without RDTS" remains
    /// expressible. For Core, RDTS is never enforced.
    ///
    /// Inbound-over-Tor stays off here: the config-layer default is all-off for
    /// backward compatibility. The product default (ON for new installs) is
    /// applied at node setup, where the user is shown the disclosure and a
    /// one-click opt-out (see [`Self::with_inbound_tor_defaults`]).
    pub fn for_flavor(flavor: NodeFlavor) -> Self {
        Self {
            networks: BTreeMap::new(),
            flavor,
            enforce_rdts: matches!(flavor, NodeFlavor::Knots),
            inbound_tor: false,
            outbound_via_tor: false,
            max_upload_target_mb_day: None,
            max_connections: None,
            tor_control_port: None,
            tor_socks_port: None,
        }
    }

    /// Turn inbound-over-Tor on with the product defaults: outbound-via-Tor
    /// enabled (Decision 2 — nearly free once Tor runs), the always-on ~1 GB/day
    /// upload cap, and the default connection cap. Ports are left `None`; the
    /// Tor lifecycle manager injects them once the managed `tor` is up. Does not
    /// touch the ports or per-network sections.
    pub fn with_inbound_tor_defaults(mut self) -> Self {
        self.inbound_tor = true;
        self.outbound_via_tor = true;
        self.max_upload_target_mb_day = Some(MAX_UPLOAD_TARGET_MB_DAY_DEFAULT);
        self.max_connections = Some(MAX_CONNECTIONS_DEFAULT);
        self
    }

    pub fn from_ini(ini: &ini::Ini) -> Result<Self, InternalBitcoindConfigError> {
        let mut networks = BTreeMap::new();
        let mut enforce_rdts = false;
        let mut inbound_tor = false;
        let mut outbound_via_tor = false;
        let mut max_upload_target_mb_day = None;
        let mut max_connections = None;
        let mut tor_control_port = None;
        let mut tor_socks_port = None;
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
                // The general (section-less) part of the file. We write
                // `consensusrules=rdts` (Knots RDTS enforcement) and, when
                // inbound-over-Tor is on, the global listen/proxy/bandwidth
                // options. Recover each preference from its persisted marker;
                // anything else is unexpected.
                for (key, value) in prop.iter() {
                    match key {
                        "consensusrules" => {
                            enforce_rdts = value.split(',').any(|rule| rule.trim() == "rdts");
                        }
                        // `listenonion=1` is the marker that inbound-over-Tor is
                        // enabled; `listen`/`discover` are implied companions.
                        "listenonion" => inbound_tor = value.trim() == "1",
                        "listen" | "discover" => {}
                        // A `proxy=` line means outbound is routed through Tor;
                        // recover the SOCKS port (runtime value, re-derived on
                        // the next start but round-tripped for losslessness).
                        "proxy" => {
                            outbound_via_tor = true;
                            tor_socks_port = Some(parse_loopback_port(value)?);
                        }
                        "torcontrol" => {
                            tor_control_port = Some(parse_loopback_port(value)?);
                        }
                        "maxuploadtarget" => {
                            max_upload_target_mb_day = Some(value.parse::<u32>().map_err(|e| {
                                InternalBitcoindConfigError::CouldNotParseValue(e.to_string())
                            })?);
                        }
                        "maxconnections" => {
                            max_connections = Some(value.parse::<u16>().map_err(|e| {
                                InternalBitcoindConfigError::CouldNotParseValue(e.to_string())
                            })?);
                        }
                        _ => {
                            return Err(InternalBitcoindConfigError::UnexpectedSection(format!(
                                "Unexpected key in general section: {key}"
                            )));
                        }
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
            inbound_tor,
            outbound_via_tor,
            max_upload_target_mb_day,
            max_connections,
            tor_control_port,
            tor_socks_port,
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

        // Inbound-over-Tor. All of these are global (non-network-scoped)
        // bitcoind options, so they belong in the section-less general part of
        // the file, like `consensusrules`. Emitted only when the feature is on;
        // when off, the general section is untouched (so existing datadirs, and
        // the default no-op state, produce a byte-identical file).
        if self.inbound_tor {
            let mut general = conf_ini.with_general_section();
            // Advertise + accept inbound peers as a v3 onion service. `discover=0`
            // keeps bitcoind from leaking a clearnet address; the onion address
            // is the only one published.
            general.set("listen", "1");
            general.set("listenonion", "1");
            general.set("discover", "0");
            // Bandwidth guards — always set when inbound is on (the metered-data
            // protection). `None` upload target means the user chose "unlimited",
            // so the key is omitted and bitcoind applies no cap.
            if let Some(mb) = self.max_upload_target_mb_day {
                general.set("maxuploadtarget", mb.to_string());
            }
            if let Some(n) = self.max_connections {
                general.set("maxconnections", n.to_string());
            }
            // `torcontrol` lets bitcoind own the ephemeral onion service via the
            // managed `tor` daemon's control port. The port is a runtime value
            // injected once Tor is up; absent it (e.g. a config loaded before
            // Tor starts), bitcoind falls back to no onion service — fail-safe.
            if let Some(control_port) = self.tor_control_port {
                general.set(
                    "torcontrol",
                    format!("{TOR_LOOPBACK_HOST}:{control_port}"),
                );
            }
            // Route outbound peer connections through Tor too, when requested and
            // the SOCKS port is known.
            if self.outbound_via_tor {
                if let Some(socks_port) = self.tor_socks_port {
                    general.set("proxy", format!("{TOR_LOOPBACK_HOST}:{socks_port}"));
                }
            }
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

/// Block until a managed bitcoind we just asked to `stop` is no longer reachable
/// (it has shut down and released the datadir lock), so a replacement can start
/// on the same datadir without a lock conflict. Bounded, so a node that refuses
/// to exit can't hang startup forever; the brief grace after the RPC closes
/// covers the gap before the `.lock` file is actually released.
fn wait_for_internal_bitcoind_shutdown(config: &BitcoindConfig) {
    let deadline = time::Instant::now() + time::Duration::from_secs(90);
    while time::Instant::now() < deadline {
        if coincubed::BitcoinD::new(config, "internal_bitcoind_stop_wait".to_string()).is_err() {
            // RPC is down; give the OS a moment to release the datadir `.lock`.
            thread::sleep(time::Duration::from_millis(750));
            return;
        }
        thread::sleep(time::Duration::from_millis(500));
    }
    log::warn!(
        "Timed out waiting for the previous managed node to stop; \
         the replacement may fail to acquire the datadir lock"
    );
}

/// Stop the managed bitcoind reachable at `config` (if one is running) and block
/// until it has released the datadir, so a replacement can start cleanly on the
/// same port. Used to force a same-flavour restart that applies new config
/// (e.g. inbound-over-Tor toggled in settings) — [`Bitcoind::maybe_start`] would
/// otherwise reuse the running node and never pick the change up. No-op when no
/// managed node is reachable.
pub fn stop_and_wait_managed_bitcoind(config: &BitcoindConfig) {
    if let Ok(running) = coincubed::BitcoinD::new(config, "managed_restart_stop".to_string()) {
        info!("Stopping managed bitcoind to apply new config...");
        running.stop();
        wait_for_internal_bitcoind_shutdown(config);
    }
}

impl Bitcoind {
    /// Start internal bitcoind for the given network.
    pub fn maybe_start(
        network: bitcoin::Network,
        config: BitcoindConfig,
        coincube_datadir: &CoincubeDirectory,
    ) -> Result<Self, StartInternalBitcoindError> {
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

        // Is a managed node already running on this RPC endpoint?
        if let Ok(running) =
            coincubed::BitcoinD::new(&config, "internal_bitcoind_start".to_string())
        {
            // The managed node is shared by every Vault, so flavour is global.
            // If the running node already matches the configured flavour, reuse
            // it. If it doesn't (a global flavour switch — e.g. Core is up for
            // existing Vaults and the user just picked Knots), stop it so we can
            // relaunch the configured binary on the same datadir/port; every
            // Vault then reconnects to the new flavour on the same RPC port.
            let running_flavor = running
                .subversion()
                .map(|sv| NodeFlavor::from_subversion(&sv))
                .unwrap_or(configured_flavor);
            if running_flavor == configured_flavor {
                info!("Internal bitcoind is already running ({running_flavor:?})");
                return Ok(Bitcoind {
                    config,
                    lock: LockFile::create(coincube_datadir.bitcoind_directory(), network)
                        .map_err(|e| StartInternalBitcoindError::Lock(format!("{:?}", e)))?,
                });
            }
            info!(
                "Managed node flavour switch {running_flavor:?} → {configured_flavor:?}; \
                 stopping the running node so the configured binary can take over"
            );
            running.stop();
            wait_for_internal_bitcoind_shutdown(&config);
        }
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

    // Inbound-over-Tor global options are emitted only when enabled, and every
    // preference round-trips through `to_ini`/`from_ini`. Mirrors
    // `rdts_consensusrules_emission`.
    #[test]
    fn tor_inbound_emission() {
        let net = InternalBitcoindNetworkConfig {
            rpc_port: 12345,
            p2p_port: 12346,
            prune: 15000,
            rpc_auth: None,
        };

        // Off by default: a Knots config emits none of the Tor keys, and the
        // general section holds only `consensusrules`.
        let mut off = InternalBitcoindConfig::for_flavor(NodeFlavor::Knots);
        off.networks.insert(Network::Bitcoin, net.clone());
        assert!(!off.inbound_tor);
        let off_ini = off.to_ini();
        for key in [
            "listen",
            "listenonion",
            "discover",
            "torcontrol",
            "proxy",
            "maxuploadtarget",
            "maxconnections",
        ] {
            assert!(
                off_ini.general_section().get(key).is_none(),
                "{} should not be emitted when inbound is off",
                key
            );
        }

        // On with the product defaults + injected runtime ports: every key is
        // present with the expected value.
        let mut on = InternalBitcoindConfig::for_flavor(NodeFlavor::Knots).with_inbound_tor_defaults();
        on.tor_control_port = Some(9151);
        on.tor_socks_port = Some(9150);
        on.networks.insert(Network::Bitcoin, net.clone());
        let on_ini = on.to_ini();
        let general = on_ini.general_section();
        assert_eq!(general.get("listen"), Some("1"));
        assert_eq!(general.get("listenonion"), Some("1"));
        assert_eq!(general.get("discover"), Some("0"));
        assert_eq!(general.get("maxuploadtarget"), Some("1000"));
        assert_eq!(general.get("maxconnections"), Some("20"));
        assert_eq!(general.get("torcontrol"), Some("127.0.0.1:9151"));
        assert_eq!(general.get("proxy"), Some("127.0.0.1:9150"));

        // Round-trip recovers every preference (and the runtime ports).
        let parsed = InternalBitcoindConfig::from_ini(&on_ini).expect("parse inbound conf");
        assert!(parsed.inbound_tor);
        assert!(parsed.outbound_via_tor);
        assert_eq!(parsed.max_upload_target_mb_day, Some(1000));
        assert_eq!(parsed.max_connections, Some(20));
        assert_eq!(parsed.tor_control_port, Some(9151));
        assert_eq!(parsed.tor_socks_port, Some(9150));
        // RDTS still round-trips alongside the new keys.
        assert!(parsed.enforce_rdts);
        assert_eq!(parsed.flavor, NodeFlavor::Knots);

        // "Unlimited" upload omits the cap key and parses back to `None`.
        let mut unlimited =
            InternalBitcoindConfig::for_flavor(NodeFlavor::Knots).with_inbound_tor_defaults();
        unlimited.max_upload_target_mb_day = None;
        unlimited.tor_control_port = Some(9151);
        unlimited.networks.insert(Network::Bitcoin, net.clone());
        let unlimited_ini = unlimited.to_ini();
        assert!(unlimited_ini
            .general_section()
            .get("maxuploadtarget")
            .is_none());
        assert_eq!(
            InternalBitcoindConfig::from_ini(&unlimited_ini)
                .expect("parse unlimited conf")
                .max_upload_target_mb_day,
            None
        );

        // Inbound on but outbound off: no `proxy` line even with a SOCKS port.
        let mut no_outbound =
            InternalBitcoindConfig::for_flavor(NodeFlavor::Knots).with_inbound_tor_defaults();
        no_outbound.outbound_via_tor = false;
        no_outbound.tor_control_port = Some(9151);
        no_outbound.tor_socks_port = Some(9150);
        no_outbound.networks.insert(Network::Bitcoin, net);
        let no_outbound_ini = no_outbound.to_ini();
        assert!(no_outbound_ini.general_section().get("proxy").is_none());
        assert!(general_marker_inbound(&no_outbound_ini));
        assert!(
            !InternalBitcoindConfig::from_ini(&no_outbound_ini)
                .expect("parse no-outbound conf")
                .outbound_via_tor
        );
    }

    // Helper: does an emitted config carry the inbound marker?
    fn general_marker_inbound(ini: &ini::Ini) -> bool {
        ini.general_section().get("listenonion") == Some("1")
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
