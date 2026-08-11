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

use crate::dir::{BitcoindDirectory, CoincubeDirectory};
use crate::utils::now_fallible;

/// The flavour of managed Bitcoin node COINCUBE downloads, configures, and runs.
///
/// Only affects the managed local-node backend; the Esplora and Electrum
/// backends never touch a local binary. Both flavours follow the same consensus
/// rules; they differ in *relay policy* — Knots ships stricter data-carrier
/// defaults. Knots is the default for new setups.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum NodeFlavor {
    /// Bitcoin Core, fetched from bitcoincore.org.
    #[default]
    Core,
    /// Bitcoin Knots, fetched from bitcoinknots.org.
    Knots,
}

/// Current and previous managed Bitcoin Core versions, in order of descending version.
pub const CORE_VERSIONS: [&str; 7] = ["29.0", "28.0", "27.1", "26.1", "26.0", "25.1", "25.0"];

/// Current managed Bitcoin Core version for new installations.
pub const CORE_VERSION: &str = CORE_VERSIONS[0];

/// Current and previous managed Bitcoin Knots versions, in order of descending version.
///
/// Deliberately pinned to the **last non-enforcing** Knots release. Builds from
/// `29.3.knots20260508` on enforce BIP-110 (RDTS) on their own deployment
/// schedule — enforcement is a property of the build, with no runtime
/// off-switch — and that fork stalled two blocks in. A node running one of those
/// builds follows a dead chain, so they are not offered: see
/// [`RDTS_ENFORCING_KNOTS_BUILD`] and `node::revalidate` for the repair that
/// un-strands datadirs they left behind.
///
/// Listing only the pinned build is also what makes an update *replace* an
/// installed enforcing binary rather than reuse it: every "is it installed?"
/// check keys on this list (or on [`NodeFlavor::version`]), so a
/// `29.3.knots20260508` directory on disk no longer satisfies the Knots flavour
/// and the pinned build is downloaded instead.
///
/// Bumping is a deliberate follow-up (the `SHA256SUMS`-based verification in the
/// installer means a bump is not checksum-locked in code).
pub const KNOTS_VERSIONS: [&str; 1] = ["29.3.knots20260507"];

/// Current managed Bitcoin Knots version for new installations.
pub const KNOTS_VERSION: &str = KNOTS_VERSIONS[0];

/// First Knots build date that enforces BIP-110 (RDTS).
///
/// Knots subversions carry a `knots<YYYYMMDD>` build tag
/// (`/Satoshi:29.3.0(knots20260508)/`). Enforcement shipped in mainline from
/// `knots20260508`; every earlier build — including the pinned
/// [`KNOTS_VERSION`] — and every Bitcoin Core build ignore RDTS entirely.
///
/// This is the *observable* property the chain-repair planner keys on, because
/// it, not the flavour, decides whether a node trailing the most-work chain is
/// doing so deliberately. See [`build_enforces_rdts`].
pub const RDTS_ENFORCING_KNOTS_BUILD: u32 = 20_260_508;

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
/// `…/29.3.knots20260507/SHA256SUMS.asc` — Luke Dashjr's canonical Knots
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
    /// (e.g. `/Satoshi:29.3.0(knots20260507)/`). Knots embeds `knots`; Core
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
                // e.g. "29.3.knots20260507" -> major "29" -> ".../29.x/29.3.knots20260507/".
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

/// Whether the build behind `subversion` enforces BIP-110 (RDTS).
///
/// Enforcement is a build property, not a configuration one: `consensusrules=rdts`
/// only ever recorded the user's consent, and an enforcing build enforces with or
/// without it. So the only honest way to ask the question of a *running* node is
/// to read the build tag out of its `getnetworkinfo.subversion` —
/// `/Satoshi:29.3.0(knots20260508)/` → `20260508` → enforcing.
///
/// Core is never enforcing. A Knots build whose tag we cannot parse is treated as
/// enforcing, which is the conservative answer: the caller uses this to decide
/// whether a node trailing the most-work chain should be dragged back onto it, and
/// doing that to a genuinely enforcing node just re-rejects the same blocks on
/// every start, forever.
pub fn build_enforces_rdts(subversion: &str) -> bool {
    let subversion = subversion.to_lowercase();
    let Some(tag) = subversion.split_once("knots") else {
        return false;
    };
    let build: String = tag.1.chars().take_while(char::is_ascii_digit).collect();
    match build.parse::<u32>() {
        Ok(build) => build >= RDTS_ENFORCING_KNOTS_BUILD,
        // A Knots build we can't date. Assume the worst and leave its chain alone.
        Err(_) => true,
    }
}

/// What a running managed node actually *is*, as opposed to what it was
/// configured to be: its flavour and whether its build enforces BIP-110.
///
/// The two travel together because the chain-repair planner needs both and they
/// come from the same one source of truth — the node's own subversion. Splitting
/// them across parameters is how a caller ends up pairing one node's flavour with
/// another's enforcement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObservedBuild {
    pub flavor: NodeFlavor,
    /// Whether this build enforces RDTS. See [`build_enforces_rdts`].
    pub enforces_rdts: bool,
}

impl ObservedBuild {
    /// Read both facts off a node's `getnetworkinfo.subversion`.
    pub fn from_subversion(subversion: &str) -> Self {
        Self {
            flavor: NodeFlavor::from_subversion(subversion),
            enforces_rdts: build_enforces_rdts(subversion),
        }
    }

    /// Fall back to the configured flavour when the node would not tell us what it
    /// is. Knots is assumed enforcing here for the same reason an undatable Knots
    /// build is: with no evidence, the option that cannot loop is to leave the
    /// chain alone.
    pub fn assumed(flavor: NodeFlavor) -> Self {
        Self {
            flavor,
            enforces_rdts: matches!(flavor, NodeFlavor::Knots),
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

/// Give the managed node's datadir an identity, unless it already has one.
///
/// Written beside the cookie file, i.e. inside the node's own network datadir, and
/// read back by `coincubed` as part of [`coincubed::BackendId`]. That placement is the
/// whole point: a chain repair authorises a deep rollback on one specific node, and
/// the things that would otherwise identify it — the RPC port, the cookie path — both
/// outlive the datadir being deleted and recreated beneath them. An authorisation from
/// the old datadir would then be honoured against the new one. This marker goes with
/// the datadir, so the replacement gets a fresh identity and the stale authorisation
/// stops matching.
///
/// Generated once and never rewritten, so it is stable across restarts, node upgrades
/// and flavour switches — all of which leave the datadir in place.
///
/// Installed atomically, and that matters more than it looks. Creating the final name
/// and *then* writing into it leaves a window in which the marker exists but is empty:
/// a second caller sees it, reports success, and connects — so a repair can be
/// recorded against an identity derived from an empty marker while every later reader
/// derives a different one from the finished file, and the authorisation stops
/// matching for good. So the contents are staged under a private name, flushed, and
/// linked into place in one step. A reader sees either no marker or a complete one.
///
/// Returns the identity now in force, which may be another caller's if it got there
/// first. An incomplete marker — an empty file from a crash, or one left by an earlier
/// version of this function — is discarded and replaced rather than trusted forever.
/// How long to keep trying for the marker lock, as (attempts, delay between them).
///
/// Short in tests so the timeout path is exercisable without a two-second wait; the
/// behaviour either side of it is what the tests are about, not the duration.
fn lock_acquisition_bound() -> (u32, std::time::Duration) {
    #[cfg(not(test))]
    {
        (40, std::time::Duration::from_millis(50))
    }
    #[cfg(test)]
    (30, std::time::Duration::from_millis(10))
}

/// Whether the managed node's durable identity can be relied on yet.
///
/// A `BitcoinD` reads the datadir's instance marker once, at construction, and caches
/// the identity it derives. That is fine when the marker is there, and fine when none is
/// expected — but not when one is expected and merely *late*: everything built in the
/// meantime reports the endpoint-and-cookie-path identity, and everything built after the
/// marker lands reports a different one. A repair recorded in that window names an
/// identity that no later client agrees with, and the authorisation it depends on stops
/// matching — which is the failure the marker was introduced to prevent, reached by
/// another route.
///
/// So the answer to a failed marker install is not "carry on with the weaker identity".
/// It is to let the node start and sync, and to refuse anything that would write a repair
/// down until the identity is settled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeIdentity {
    /// Settled: either the marker is installed and validated, or none is expected here,
    /// so what a `BitcoinD` reports now is what it will keep reporting.
    Stable,
    /// A marker is expected and is not there. Any identity derived right now is
    /// provisional, so no chain repair may be started or recorded against it.
    Unstable,
}

impl NodeIdentity {
    /// Whether a chain repair may be started or recorded.
    pub fn permits_chain_repair(&self) -> bool {
        matches!(self, Self::Stable)
    }
}

/// Settle the managed node's identity, installing its marker if that has not happened
/// yet.
///
/// Called before the first `BitcoinD` of a managed-node start, and again on an explicit
/// repair — so a start that could not establish it does not poison later attempts, it
/// just declines to repair until one of them succeeds.
pub fn establish_node_identity(config: &BitcoindConfig) -> NodeIdentity {
    match &config.rpc_auth {
        coincubed::config::BitcoindRpcAuth::CookieFile(cookie_path) => {
            match ensure_node_instance_marker(cookie_path) {
                Ok(instance) => {
                    tracing::debug!("managed node identity: {instance}");
                    NodeIdentity::Stable
                }
                Err(e) => {
                    warn!(
                        "could not establish the managed node's identity ({e}); it will start \
                         and sync as usual, but chain repairs are declined until this succeeds"
                    );
                    NodeIdentity::Unstable
                }
            }
        }
        // No cookie file for a marker to sit beside, so none is expected and none will
        // ever appear. The endpoint-and-username identity such a node reports is already
        // settled — weaker than a marker, but it does not change under us, which is what
        // matters here. Repairs may proceed.
        coincubed::config::BitcoindRpcAuth::UserPass(..) => NodeIdentity::Stable,
    }
}

/// Installed under an advisory lock on a sibling file, because the whole
/// read-validate-replace-install sequence has to be one transaction and not just its
/// last step.
///
/// The atomic install alone is not enough once a *malformed* marker is in the way.
/// Two callers both read it, both decide to replace it, and the second one's
/// `remove_file` — decided on a read that is now stale — deletes the valid marker the
/// first has just installed, after which they disagree about the identity forever.
/// Holding the lock across the read means the file cannot change under a caller
/// between validating it and replacing it.
///
/// An OS advisory lock rather than a lock *file*, deliberately: the kernel drops it
/// when the holder's descriptor closes, including on a crash, so there is no stale lock
/// to break and no timeout to guess. The lock file itself is left in place — it carries
/// no state, and its existence is not what locks anything.
pub fn ensure_node_instance_marker(cookie_path: &Path) -> std::io::Result<String> {
    use fs4::fs_std::FileExt;

    let dir = cookie_path.parent().ok_or_else(|| {
        std::io::Error::other("the managed node's cookie path has no parent directory")
    })?;
    std::fs::create_dir_all(dir)?;

    let lock_path = dir.join(format!("{}.lock", coincubed::NODE_INSTANCE_FILE));
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)?;
    // Bounded rather than blocking. Real contention here is a handful of Vaults starting
    // in the same instant, all of which finish in microseconds — but this sits on the
    // startup path of every Vault, and a holder that is wedged rather than crashed would
    // otherwise wedge startup with it.
    //
    // Giving up is *not* the same as "no marker is expected here": the identity a
    // `BitcoinD` caches while a marker is still on its way will change once it lands. See
    // [`establish_node_identity`], which is what turns that distinction into a refusal to
    // repair rather than a repair recorded against an identity about to move.
    let (lock_attempts, lock_retry) = lock_acquisition_bound();
    let mut acquired = false;
    for attempt in 0..lock_attempts {
        if lock.try_lock_exclusive()? {
            acquired = true;
            break;
        }
        if attempt + 1 < lock_attempts {
            std::thread::sleep(lock_retry);
        }
    }
    if !acquired {
        return Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "another process is still establishing the managed node's identity",
        ));
    }
    let established = establish_node_instance(dir);
    // Explicit, so the unlock is not left to the order fields happen to drop in.
    let _ = FileExt::unlock(&lock);
    established
}

/// The locked part of [`ensure_node_instance_marker`]. Assumes exclusive access to
/// `dir`'s marker.
fn establish_node_instance(dir: &Path) -> std::io::Result<String> {
    use rand::Rng;

    /// Distinguishes concurrent stagers; only reachable by callers in different
    /// processes holding the lock in turn, but the name still has to be unique.
    static STAGING_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    let marker = dir.join(coincubed::NODE_INSTANCE_FILE);
    match std::fs::read_to_string(&marker) {
        Ok(existing) => {
            let existing = existing.trim().to_string();
            if coincubed::valid_node_instance(&existing) {
                return Ok(existing);
            }
            // Empty or truncated — a crash between creating the file and filling it, or
            // a marker from an older version of this code. Trusting it would hand out an
            // identity that differs from whatever a complete marker later says, which is
            // exactly the mismatch this function exists to prevent. Safe to replace only
            // because the lock means no one has installed a valid marker since the read.
            warn!("discarding a malformed managed-node identity at {marker:?}");
            match std::fs::remove_file(&marker) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }

    let instance: String = rand::thread_rng()
        .sample_iter(rand::distributions::Alphanumeric)
        .take(coincubed::NODE_INSTANCE_LEN)
        .map(char::from)
        .collect();
    let staged = dir.join(format!(
        "{}.{}.{}.tmp",
        coincubed::NODE_INSTANCE_FILE,
        std::process::id(),
        STAGING_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let install = (|| -> std::io::Result<()> {
        {
            use std::io::Write;
            let mut file = std::fs::File::create(&staged)?;
            file.write_all(instance.as_bytes())?;
            file.sync_all()?;
        }
        // Rename rather than link: under the lock there is nothing to lose a race
        // against, and it leaves no second name to clean up. The final name therefore
        // goes from absent to complete in one step — a concurrent *reader*, which takes
        // no lock, never sees a half-written marker.
        std::fs::rename(&staged, &marker)?;
        // The contents are durable but the directory entry naming them may not be, and
        // a lost entry is a marker that silently changes identity after a crash.
        sync_directory(dir)
    })();
    // Nothing to remove on the success path — the rename consumed it — but a failure
    // part-way must not leave staging files behind.
    if install.is_err() {
        let _ = std::fs::remove_file(&staged);
    }
    install.map(|()| instance)
}

/// Flush a directory entry, so a rename or link into it survives a crash.
///
/// Unix only: Windows cannot open a directory as a file without backup semantics, and
/// NTFS metadata journalling makes the operation durable there anyway. Mirrors the
/// managed-node state sidecar's own writes.
fn sync_directory(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    std::fs::File::open(dir)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
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

// --- Node-resource settings (prune target + mempool cap) -------------------
//
// The managed node exposes two resource knobs (see the node-resources UI in the
// installer and Vault settings). `prune` is per-network and already first-class;
// `maxmempool` is a standalone general-section key. Presets are a thin setter
// layer over the two source-of-truth values below (see [`NodeResources`]).

/// bitcoind's minimum legal `prune` target, in MiB. The managed node is *always*
/// pruned (invariant I1: no "keep everything" option), so this is the floor for
/// every prune choice — no UI path emits a smaller value or `prune=0`. Enforced
/// by input validation before a value is written.
pub const PRUNE_MIN: u32 = 550;

/// "Minimal" prune preset — bitcoind's floor, 550 MB of block data.
pub const PRUNE_MINIMAL_MB: u32 = 550;

/// "Compact" prune preset — 5 GB of block data.
pub const PRUNE_COMPACT_MB: u32 = 5_000;

/// Default prune target for new managed nodes — 15 GB of block data. Historical
/// default, kept here so every node-resource constant lives in one place.
pub const PRUNE_DEFAULT: u32 = 15_000;

/// bitcoind's minimum legal `maxmempool`, in MB. Below this bitcoind refuses to
/// start ("-maxmempool must be at least N MB"). The floor is
/// `ceil(limitdescendantsize_kvB * 1000 * 40 / 1_000_000)` = `ceil(101 * 40 /
/// 1000)` = 5 MB with the default descendant-size limit, which the pinned Knots
/// build inherits unchanged from Core. Enforced by input validation before we
/// emit the key.
pub const MAX_MEMPOOL_MB_MIN: u32 = 5;

/// "Small" mempool preset — a 100 MB cap.
pub const MAX_MEMPOOL_SMALL_MB: u32 = 100;

/// bitcoind's own default mempool cap, 300 MB. Represented as
/// `max_mempool_mb = None` (key omitted), so choosing "Default" restores
/// byte-identical output (invariant I2).
pub const MAX_MEMPOOL_DEFAULT_MB: u32 = 300;

/// Rough non-prunable footprint (GB) added on top of the prune target for the
/// estimated-total-disk line: chainstate (~12–15 GB, unprunable) plus block
/// index / undo / overhead (~1–2 GB). The prune choice only bounds *block* data,
/// so the honest total is always `prune + this` — the UI must not pretend the
/// chainstate floor away.
pub const CHAINSTATE_OVERHEAD_GB: u32 = 14;

/// Estimated total on-disk footprint, in GB, of a managed node keeping
/// `prune_mb` of block data: the prune target rounded to GB plus the unprunable
/// [`CHAINSTATE_OVERHEAD_GB`]. Backs the node-resources "estimated total disk"
/// line so the number the user picks reflects reality.
pub fn estimated_total_disk_gb(prune_mb: u32) -> u32 {
    (prune_mb as f64 / 1024.0).round() as u32 + CHAINSTATE_OVERHEAD_GB
}

/// A user's node-resource choices, applied onto an [`InternalBitcoindConfig`]:
/// the per-network prune target (MiB) and the global mempool cap
/// (`None` = bitcoind's 300 MB default, key omitted). Presets — Minimal /
/// Compact / Default, Small / Default, and one-click "Small computer" — are a
/// thin setter layer over these two fields, so a future "Miner" preset can be
/// added without reworking the surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeResources {
    pub prune_mb: u32,
    pub max_mempool_mb: Option<u32>,
}

impl NodeResources {
    /// The one-click "Small computer" preset: bitcoind's floor prune (550 MB)
    /// plus a 100 MB mempool cap. Deliberately does *not* trim `maxconnections`
    /// — the always-on ~1 GB/day `maxuploadtarget` is the real bandwidth guard.
    pub fn small_computer() -> Self {
        Self {
            prune_mb: PRUNE_MINIMAL_MB,
            max_mempool_mb: Some(MAX_MEMPOOL_SMALL_MB),
        }
    }

    /// The one-click "Regular computer" preset: the out-of-the-box profile —
    /// the managed-node default 15 GB prune target ([`PRUNE_DEFAULT`]) and
    /// bitcoind's own default 300 MB mempool (key omitted). The inverse of
    /// [`Self::small_computer`], and the middle of the machine-profile row a
    /// future "Miner" preset joins.
    pub fn regular_computer() -> Self {
        Self {
            prune_mb: PRUNE_DEFAULT,
            max_mempool_mb: None,
        }
    }
}

/// Validate and store a prune-target value in `field` (the text widget already
/// restricts input to digits): it must be non-empty and at least [`PRUNE_MIN`].
/// Shared by the installer and settings node-resource editors so their
/// validation never drifts.
pub fn set_prune_form_value(field: &mut form::Value<String>, value: String) {
    field.value = value;
    let ok = field
        .value
        .parse::<u32>()
        .map(|n| n >= PRUNE_MIN)
        .unwrap_or(false);
    field.valid = ok;
    field.warning = if ok { None } else { Some("Minimum is 550 MB.") };
}

/// Validate and store a mempool-cap value in `field`: blank = bitcoind's 300 MB
/// default (valid, emitted as `None`), otherwise at least [`MAX_MEMPOOL_MB_MIN`].
pub fn set_max_mempool_form_value(field: &mut form::Value<String>, value: String) {
    field.value = value;
    let ok = field.value.is_empty()
        || field
            .value
            .parse::<u32>()
            .map(|n| n >= MAX_MEMPOOL_MB_MIN)
            .unwrap_or(false);
    field.valid = ok;
    field.warning = if ok {
        None
    } else {
        Some("Minimum is 5 MB (or leave blank for the default).")
    };
}

/// Read validated [`NodeResources`] from two editor fields, re-validating both
/// first so a value pre-filled from disk (which never went through the edit
/// handlers) is still checked. Returns `None` — and leaves the offending field
/// flagged invalid — when either is out of range, so callers can refuse to write.
pub fn resources_from_forms(
    prune: &mut form::Value<String>,
    mempool: &mut form::Value<String>,
) -> Option<NodeResources> {
    set_prune_form_value(prune, prune.value.clone());
    set_max_mempool_form_value(mempool, mempool.value.clone());
    if !prune.valid || !mempool.valid {
        return None;
    }
    let prune_mb = prune.value.parse::<u32>().ok()?;
    let max_mempool_mb = if mempool.value.is_empty() {
        None
    } else {
        Some(mempool.value.parse::<u32>().ok()?)
    };
    Some(NodeResources {
        prune_mb,
        max_mempool_mb,
    })
}

/// Loopback host bitcoind uses to reach the co-located managed `tor` daemon's
/// control and SOCKS ports (`torcontrol`/`proxy`).
const TOR_LOOPBACK_HOST: &str = "127.0.0.1";

/// Represents the `bitcoin.conf` file to be used by internal bitcoind.
#[derive(Debug, Clone)]
pub struct InternalBitcoindConfig {
    pub networks: BTreeMap<Network, InternalBitcoindNetworkConfig>,
    /// Which managed node flavour this config is for.
    ///
    /// **Not persisted here.** bitcoind rejects unknown options, so the file has
    /// no room for a key of our own, and the one marker it used to be recovered
    /// from (`consensusrules=rdts`) is no longer written. A config parsed off
    /// disk therefore only reports `Knots` when it still carries that legacy
    /// line; the durable answer lives in the flavour ledger
    /// (`revalidate::ManagedNodeState::configured_flavor`) and, once a node is
    /// up, in its subversion. See [`configured_managed_flavor`].
    pub flavor: NodeFlavor,
    /// Legacy: a `consensusrules=rdts` line left in the file by a release that
    /// still asked the node to enforce BIP-110.
    ///
    /// Parsed, never written. It survives only so an existing datadir can be
    /// recognised as Knots' once, on the first start after the update, and have
    /// the line stripped — the pinned build does not enforce RDTS, and a key it
    /// may not even accept has no business staying in the file. Deleted once no
    /// datadir can still carry it.
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
    /// Mempool memory cap emitted as `maxmempool` (MB). `None` = bitcoind's own
    /// 300 MB default (key omitted). A **standalone** resource key — unlike the
    /// bandwidth caps above it is *not* gated on `inbound_tor`; it is emitted
    /// whenever set, and an untouched (`None`) config stays byte-identical.
    pub max_mempool_mb: Option<u32>,
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
            max_mempool_mb: None,
            tor_control_port: None,
            tor_socks_port: None,
        }
    }

    /// A config for the given managed-node flavour.
    ///
    /// Inbound-over-Tor stays off here: the config-layer default is all-off for
    /// backward compatibility. The product default (ON for new installs) is
    /// applied at node setup, where the user is shown the disclosure and a
    /// one-click opt-out (see [`Self::with_inbound_tor_defaults`]).
    pub fn for_flavor(flavor: NodeFlavor) -> Self {
        Self {
            networks: BTreeMap::new(),
            flavor,
            enforce_rdts: false,
            inbound_tor: false,
            outbound_via_tor: false,
            max_upload_target_mb_day: None,
            max_connections: None,
            max_mempool_mb: None,
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
        let mut max_mempool_mb = None;
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
                // The general (section-less) part of the file. We write the
                // global listen/proxy/bandwidth options when inbound-over-Tor is
                // on. Recover each preference from its persisted marker; anything
                // else is unexpected.
                for (key, value) in prop.iter() {
                    match key {
                        // Read-only legacy: written by releases that asked Knots
                        // to enforce BIP-110. Parsed so an existing datadir still
                        // loads (and so the line can be recognised and dropped on
                        // the next write), never emitted again.
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
                        // Standalone resource key: parsed back whether or not
                        // inbound-over-Tor is on.
                        "maxmempool" => {
                            max_mempool_mb = Some(value.parse::<u32>().map_err(|e| {
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
        // A legacy `consensusrules=rdts` still identifies the file as a Knots
        // node's, and nothing else in it can. Absent the line the answer is
        // simply not in this file — callers resolve it from the flavour ledger
        // instead (see [`configured_managed_flavor`]), so the `Core` here is a
        // placeholder, not a finding.
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
            max_mempool_mb,
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

        // No `consensusrules` line: we ship no build that enforces BIP-110, the
        // key only ever recorded consent, and the pinned build may not accept it
        // at all. Because the file is rebuilt from this struct rather than
        // edited, every rewrite also *strips* a legacy line an older release
        // left behind — which is the point. `self.enforce_rdts` is read-only
        // legacy state and is deliberately not consulted here.

        // Inbound-over-Tor. All of these are global (non-network-scoped)
        // bitcoind options, so they belong in the section-less general part of
        // the file. Emitted only when the feature is on;
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
                general.set("torcontrol", format!("{TOR_LOOPBACK_HOST}:{control_port}"));
            }
            // Route outbound peer connections through Tor too, when requested and
            // the SOCKS port is known.
            if self.outbound_via_tor {
                if let Some(socks_port) = self.tor_socks_port {
                    general.set("proxy", format!("{TOR_LOOPBACK_HOST}:{socks_port}"));
                }
            }
        }

        // Mempool memory cap — a standalone resource key, emitted whenever set
        // (deliberately NOT gated on `inbound_tor`, unlike the bandwidth caps
        // above). It lives in the section-less general part alongside the Tor
        // keys. `None` means bitcoind's own 300 MB default, so the key is omitted
        // and an untouched config produces a byte-identical file (invariant I2).
        if let Some(mb) = self.max_mempool_mb {
            conf_ini
                .with_general_section()
                .set("maxmempool", mb.to_string());
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

/// The flavour the managed node is configured to run as, or `None` when nothing
/// on disk answers the question.
///
/// There is no flavour key in `bitcoin.conf` — bitcoind rejects options it does
/// not know — so the answer is assembled from what does persist, most
/// authoritative first:
///
/// 1. the flavour ledger's `configured_flavor`, written by whichever surface last
///    wrote the managed config;
/// 2. a legacy `consensusrules=rdts` line, the marker releases before the RDTS
///    sunset used (see [`migrate_legacy_rdts_conf`], which converts it to 1 and
///    removes it);
/// 3. the flavour the node was last *observed* running as, for a datadir whose
///    ledger predates `configured_flavor`.
///
/// A `None` is now cheap: with no `consensusrules` in the file, either binary can
/// open either datadir, so the fallback in [`select_managed_bitcoind_exe`] is free
/// to launch whatever is installed. That was not true while the config carried a
/// Knots-only key.
pub fn configured_managed_flavor(coincube_datadir: &CoincubeDirectory) -> Option<NodeFlavor> {
    let state = crate::node::revalidate::ManagedNodeState::load(coincube_datadir);
    if let Some(flavor) = state.configured_flavor {
        return Some(flavor);
    }
    let conf_path = internal_bitcoind_config_path(&internal_bitcoind_datadir(coincube_datadir));
    if InternalBitcoindConfig::from_file(&conf_path).is_ok_and(|conf| conf.enforce_rdts) {
        return Some(NodeFlavor::Knots);
    }
    state.last_run_flavor
}

/// Strip a legacy `consensusrules=rdts` line from the managed `bitcoin.conf`,
/// recording the flavour it stood for in the ledger first so nothing is lost.
///
/// Runs on the start path rather than only where the config is rewritten, because
/// the loader starts the managed node without going through a rewrite: a datadir
/// set up by a release that enforced RDTS would otherwise hand the key straight to
/// the pinned build. Whether that build ignores or rejects the key is exactly the
/// kind of thing not worth depending on.
///
/// Best-effort and idempotent — a file we cannot read or write is left alone and
/// retried on the next start.
fn migrate_legacy_rdts_conf(coincube_datadir: &CoincubeDirectory) {
    let conf_path = internal_bitcoind_config_path(&internal_bitcoind_datadir(coincube_datadir));
    let Ok(mut conf) = InternalBitcoindConfig::from_file(&conf_path) else {
        return;
    };
    if !conf.enforce_rdts {
        return;
    }
    info!(
        "managed bitcoin.conf still carries `consensusrules=rdts`; recording the node as \
         Bitcoin Knots and removing the line — no build we ship enforces BIP-110"
    );
    // Ledger first: losing the line before its meaning is recorded would leave the
    // datadir with no flavour at all.
    crate::node::revalidate::ManagedNodeState::record_configured(
        coincube_datadir,
        NodeFlavor::Knots,
    );
    conf.flavor = NodeFlavor::Knots;
    conf.enforce_rdts = false;
    if let Err(e) = conf.to_file(&conf_path) {
        warn!("could not strip `consensusrules` from the managed bitcoin.conf: {e}");
    }
}

/// Pick the managed `bitcoind` binary to launch for `configured_flavor`,
/// preferring that flavour's versions (newest first) and falling back to the
/// other flavour's only if none are installed. Returns the first existing
/// `bitcoin-<version>/bin/bitcoind[.exe]` under the managed directory, or `None`
/// when nothing is installed.
///
/// Only versions in [`CORE_VERSIONS`] / [`KNOTS_VERSIONS`] are candidates, so a
/// Knots build we no longer ship — the RDTS-enforcing `29.3.knots20260508` — is
/// never launched, however it got onto the disk.
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
    // The second arg is only a label for the watchonly-client URL; it need not
    // name a loaded wallet. `BitcoinD::new`'s connection check issues a
    // non-wallet `echo` RPC, which bitcoind serves regardless of the
    // `/wallet/<name>` in the URL (only real wallet RPCs require the wallet to
    // exist). So `Ok` here means "the node is reachable", matching the same
    // dummy-path pattern used by `maybe_start` and `wait_for_internal_bitcoind_shutdown`.
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
        // Settle the datadir's identity before the first connection to it, not after.
        // Every managed-node start comes through here — the loader with a config it read
        // off disk, the installer with one it just wrote, the settings switch — and each
        // goes on to build a `BitcoinD`, which reads the marker once at construction and
        // caches what it derives. Establishing it later means a repair can be recorded
        // against the endpoint-and-cookie-path identity and then stop matching the moment
        // the marker appears.
        //
        // Not fatal if it fails: the node still starts and syncs, which is what the user
        // is waiting for. What it does cost is chain reconciliation — every repair path
        // declines while the answer is provisional, and the next start tries again.
        let identity = establish_node_identity(&config);
        // Drop any `consensusrules=rdts` an earlier release persisted before the
        // node — of either flavour — is handed the file.
        migrate_legacy_rdts_conf(coincube_datadir);
        // Launch the binary the user asked for. Nothing in the conf forces our
        // hand any more (it no longer carries a Knots-only key), but the choice
        // is still theirs: a machine with both flavours installed must launch the
        // configured one rather than whichever is found first.
        let configured_flavor =
            configured_managed_flavor(coincube_datadir).unwrap_or(NodeFlavor::Core);
        let selected_exe = select_managed_bitcoind_exe(coincube_datadir, configured_flavor);

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
            let running_subversion = running.subversion();
            let running_flavor = running_subversion
                .as_deref()
                .map(NodeFlavor::from_subversion)
                .unwrap_or(configured_flavor);
            // A matching flavour is not enough on its own: an RDTS-enforcing Knots
            // build left over from before the sunset is still "Knots", and reusing
            // it would keep the node on the stalled BIP-110 fork indefinitely —
            // the auto-repair declines to drag an enforcing node anywhere. Replace
            // it, but only if there is something to replace it *with*; stopping the
            // only node on the machine to then find no binary would be worse than
            // running the wrong one, and the download path can supply the pinned
            // build on the next attempt.
            let running_enforces_rdts = running_subversion
                .as_deref()
                .is_some_and(build_enforces_rdts);
            let replaceable = running_enforces_rdts && selected_exe.is_some();
            if running_flavor == configured_flavor && !replaceable {
                if running_enforces_rdts {
                    warn!(
                        "Managed node is running an RDTS-enforcing build and no replacement \
                         binary is installed; reusing it. Its chain cannot be repaired until \
                         the pinned build is downloaded."
                    );
                }
                info!("Internal bitcoind is already running ({running_flavor:?})");
                // Reconcile here too: this vault may be attaching to a node another
                // vault swapped the flavour of, so this is a start path like any
                // other. `running_flavor` is read from the node's own subversion.
                crate::node::revalidate::reconcile_after_start(
                    coincube_datadir,
                    &running,
                    &config,
                    &identity,
                    network,
                    ObservedBuild {
                        flavor: running_flavor,
                        enforces_rdts: running_enforces_rdts,
                    },
                );
                return Ok(Bitcoind {
                    config,
                    lock: LockFile::create(coincube_datadir.bitcoind_directory(), network)
                        .map_err(|e| StartInternalBitcoindError::Lock(format!("{:?}", e)))?,
                });
            }
            if replaceable {
                info!(
                    "Managed node is running an RDTS-enforcing build ({}); stopping it so the \
                     pinned {KNOTS_VERSION} build can take over",
                    running_subversion.as_deref().unwrap_or("unknown"),
                );
            } else {
                info!(
                    "Managed node flavour switch {running_flavor:?} → {configured_flavor:?}; \
                     stopping the running node so the configured binary can take over"
                );
            }
            running.stop();
            wait_for_internal_bitcoind_shutdown(&config);
        }
        let bitcoind_exe_path =
            selected_exe.ok_or(StartInternalBitcoindError::ExecutableNotFound)?;
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
        // Build a fresh bitcoind command each spawn attempt (we may respawn if
        // the datadir lock isn't free yet — see the retry below).
        let spawn_bitcoind = || -> Result<std::process::Child, StartInternalBitcoindError> {
            let mut command = std::process::Command::new(&bitcoind_exe_path);
            command
                .args(&args)
                // FIXME: can we pipe stderr to our logging system somehow?
                .stdout(std::process::Stdio::null());

            // Detach the child so closing the app doesn't take the node down.
            crate::node::detach_spawned_process(&mut command);

            command
                .spawn()
                .map_err(|e| StartInternalBitcoindError::CommandError(e.to_string()))
        };

        // When we've just asked a previous managed node to stop, it keeps the
        // datadir lock until it finishes flushing on shutdown; a fresh bitcoind
        // then exits immediately with "Cannot obtain a lock on directory". Retry
        // the spawn a bounded number of times (~15s) to ride out that window.
        // A genuine start failure just exhausts the retries and surfaces the
        // error, a few seconds later than before.
        const MAX_LOCK_RETRIES: u32 = 30;
        let mut lock_retries = 0;

        let mut process = spawn_bitcoind()?;

        // We've started bitcoind in the background, however it may fail to start for whatever
        // reason. And we need its JSONRPC interface to be available to continue. Thus wait for
        // the interface to be created successfully, regularly checking it did not fail to start.
        let mut try_count = 0;
        loop {
            match process.try_wait() {
                Ok(None) => {}
                Err(e) => log::error!("Error while trying to wait for bitcoind: {}", e),
                Ok(Some(status)) => {
                    if lock_retries < MAX_LOCK_RETRIES {
                        lock_retries += 1;
                        log::warn!(
                            "bitcoind exited early ({status}); a stopping node likely still \
                             holds the datadir lock — retrying ({lock_retries}/{MAX_LOCK_RETRIES})"
                        );
                        thread::sleep(time::Duration::from_millis(500));
                        process = spawn_bitcoind()?;
                        try_count = 0;
                        continue;
                    }
                    log::error!("Bitcoind exited with status '{}'", status);
                    return Err(StartInternalBitcoindError::ProcessExited(status));
                }
            }
            match coincubed::BitcoinD::new(&config, "internal_bitcoind_start".to_string()) {
                Ok(started) => {
                    log::info!("Bitcoind seems to have successfully started.");
                    // Ask the node what it actually is rather than trusting
                    // `configured_flavor`: `select_managed_bitcoind_exe` falls back to
                    // the other flavour's binary when the preferred one isn't
                    // installed, so the two can legitimately disagree.
                    let observed = started
                        .subversion()
                        .map(|sv| ObservedBuild::from_subversion(&sv))
                        .unwrap_or_else(|| ObservedBuild::assumed(configured_flavor));
                    crate::node::revalidate::reconcile_after_start(
                        coincube_datadir,
                        &started,
                        &config,
                        &identity,
                        network,
                        observed,
                    );
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

    fn a_marker_datadir(name: &str) -> (std::path::PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "coincube-node-instance-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let cookie_path = dir.join("signet").join(".cookie");
        (dir, cookie_path)
    }

    // The marker is what turns "this endpoint with this cookie path" into "this node".
    // It has to be stable while the datadir lives — restarts, upgrades and flavour
    // switches all keep it — and gone once the datadir is, so a repair authorisation
    // from the old datadir stops matching its replacement.
    #[test]
    fn the_node_instance_marker_is_stable_but_dies_with_its_datadir() {
        let (dir, cookie_path) = a_marker_datadir("stable");
        let marker = cookie_path
            .parent()
            .unwrap()
            .join(coincubed::NODE_INSTANCE_FILE);

        // The installer's shape: the network directory does not exist yet, and the
        // marker has to be installable before anything connects.
        assert!(!cookie_path.parent().unwrap().exists());
        let first = ensure_node_instance_marker(&cookie_path).expect("stamped");
        assert!(coincubed::valid_node_instance(&first));
        assert_eq!(std::fs::read_to_string(&marker).unwrap(), first);

        // Stable: stamping again is a no-op, so every later start of the same datadir
        // sees the same identity.
        assert_eq!(
            ensure_node_instance_marker(&cookie_path).expect("idempotent"),
            first
        );

        // Nothing but the marker and its (stateless) lock file.
        assert_eq!(
            marker_artifacts(&cookie_path),
            vec![format!("{}.lock", coincubed::NODE_INSTANCE_FILE)],
            "staging files were not cleaned up"
        );

        // Gone with the datadir, and the replacement is a different node.
        let _ = std::fs::remove_dir_all(&dir);
        let second = ensure_node_instance_marker(&cookie_path).expect("re-stamped");
        assert_ne!(second, first);

        let _ = std::fs::remove_dir_all(&dir);
    }

    // An empty or truncated marker is what the pre-atomic version of this function
    // could leave behind, and what a crash between creating the file and filling it
    // would leave. Trusting it hands out an identity that differs from the one the
    // finished file would give, so a repair recorded against it stops matching for
    // good. It has to be replaced, not adopted.
    #[test]
    fn an_incomplete_marker_is_replaced_rather_than_trusted() {
        let (dir, cookie_path) = a_marker_datadir("incomplete");
        let network_dir = cookie_path.parent().unwrap().to_path_buf();
        let marker = network_dir.join(coincubed::NODE_INSTANCE_FILE);

        for malformed in ["", "   ", "short", &"x".repeat(64), "not-alnum!!!"] {
            std::fs::create_dir_all(&network_dir).unwrap();
            std::fs::write(&marker, malformed).unwrap();
            assert!(!coincubed::valid_node_instance(malformed.trim()));

            let recovered = ensure_node_instance_marker(&cookie_path).expect("recovered");
            assert!(
                coincubed::valid_node_instance(&recovered),
                "an incomplete marker ({:?}) must be replaced with a complete one",
                malformed
            );
            assert_eq!(std::fs::read_to_string(&marker).unwrap(), recovered);
            let _ = std::fs::remove_dir_all(&dir);
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Every marker-directory entry that is not the marker itself: staging files, lock
    /// files, anything left behind.
    fn marker_artifacts(cookie_path: &Path) -> Vec<String> {
        std::fs::read_dir(cookie_path.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name != coincubed::NODE_INSTANCE_FILE)
            .collect()
    }

    // The hard case, and the one the fresh-datadir test below cannot reach: a *malformed*
    // marker is already there when several callers arrive. Each of them decides to replace
    // it, and without the whole read-validate-replace-install sequence being one
    // transaction, the second one's delete — decided on a read that is now stale — removes
    // the valid marker the first has just installed, leaving them permanently disagreeing.
    #[test]
    fn concurrent_recovery_from_a_malformed_marker_agrees_on_one_identity() {
        let (dir, cookie_path) = a_marker_datadir("concurrent-malformed");
        let network_dir = cookie_path.parent().unwrap().to_path_buf();
        let marker = network_dir.join(coincubed::NODE_INSTANCE_FILE);

        // The state a crash between creating the marker and filling it leaves behind.
        std::fs::create_dir_all(&network_dir).unwrap();
        std::fs::write(&marker, "").unwrap();

        const CALLERS: usize = 8;
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(CALLERS));
        let observed: Vec<String> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..CALLERS)
                .map(|_| {
                    let barrier = barrier.clone();
                    let cookie_path = cookie_path.clone();
                    scope.spawn(move || {
                        // Every caller reads the malformed marker at as near the same
                        // instant as we can arrange, which is what makes the stale-read
                        // delete reachable.
                        barrier.wait();
                        ensure_node_instance_marker(&cookie_path).expect("recovered")
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });

        let winner = &observed[0];
        assert!(
            coincubed::valid_node_instance(winner),
            "recovery produced a malformed identity"
        );
        for instance in &observed {
            assert_eq!(
                instance, winner,
                "concurrent recovery from a malformed marker disagreed about the identity"
            );
        }
        // Exactly one identity won, and it is the one on disk — not a later caller's that
        // deleted it.
        assert_eq!(&std::fs::read_to_string(&marker).unwrap(), winner);

        // The lock is expected to remain (it holds no state); nothing else may.
        let artifacts = marker_artifacts(&cookie_path);
        let expected_lock = format!("{}.lock", coincubed::NODE_INSTANCE_FILE);
        assert_eq!(
            artifacts,
            vec![expected_lock],
            "recovery left staging artifacts behind"
        );

        // And the settled marker is now stable: a further caller adopts it rather than
        // replacing it.
        assert_eq!(
            &ensure_node_instance_marker(&cookie_path).expect("stable"),
            winner
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // The timeout path, which is the one the bounded lock introduced. A holder that is
    // slow but not wedged makes a second caller give up — and a caller that then carried
    // on would build a `BitcoinD` caching the endpoint-and-cookie-path identity, record a
    // repair against it, and watch that authorisation stop matching the moment the marker
    // finally landed. So giving up has to mean "no repair", not "repair under whatever
    // identity we have".
    #[test]
    fn a_timed_out_caller_gets_an_unstable_identity_and_recovers_on_retry() {
        use fs4::fs_std::FileExt;

        let (dir, cookie_path) = a_marker_datadir("timeout");
        let network_dir = cookie_path.parent().unwrap().to_path_buf();
        let marker = network_dir.join(coincubed::NODE_INSTANCE_FILE);
        std::fs::create_dir_all(&network_dir).unwrap();

        let config = coincubed::config::BitcoindConfig {
            rpc_auth: coincubed::config::BitcoindRpcAuth::CookieFile(cookie_path.clone()),
            addr: "127.0.0.1:8332".parse().unwrap(),
        };

        // A holder takes the lock and keeps it for longer than the acquisition bound.
        let lock_path = network_dir.join(format!("{}.lock", coincubed::NODE_INSTANCE_FILE));
        let held = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .unwrap();
        held.lock_exclusive().unwrap();

        // The second caller exhausts its attempts and gives up. Joining takes exactly as
        // long as the bound, so nothing here depends on a guessed sleep.
        let timed_out = {
            let config = config.clone();
            std::thread::spawn(move || establish_node_identity(&config))
                .join()
                .unwrap()
        };
        assert_eq!(
            timed_out,
            NodeIdentity::Unstable,
            "a timed-out caller must not report a settled identity"
        );
        assert!(!timed_out.permits_chain_repair());

        // ...and with that, no chain operation can be claimed, so nothing can issue
        // `invalidateblock` or `reconsiderblock`, record a rollback floor, or reconcile.
        // The identity is checked before the maintenance guard is even reached, so this
        // says nothing about whether some other test happens to hold it.
        let (state_dir, datadir) = {
            let d = dir.join("coincube");
            (d.clone(), crate::dir::CoincubeDirectory::new(d))
        };
        assert!(matches!(
            crate::node::revalidate::probe_chain_operation(&datadir, &timed_out),
            Err(crate::node::revalidate::ClaimRefused::UnstableIdentity)
        ));
        // Nor can the manual "Re-check chain" repair, which surfaces it to the user.
        let refusal = crate::node::revalidate::clear_failure_flags(
            &datadir,
            &config,
            &timed_out,
            crate::node::revalidate::RevalidationPlan::ClearFailureFlags {
                anchor_height: crate::node::revalidate::RDTS_ANCHOR_MAINNET,
            },
        )
        .expect_err("must refuse");
        assert!(
            refusal.contains("identity"),
            "the refusal should say why: {}",
            refusal
        );
        // Refused before anything was written down, so there is no half-recorded repair
        // for a later start to trip over.
        assert_eq!(
            crate::node::revalidate::ManagedNodeState::load(&datadir).sanctioned_rollback,
            None
        );
        let _ = std::fs::remove_dir_all(&state_dir);

        // The original holder now finishes and releases, which is the "late successful
        // installation" the timed-out caller has to pick up.
        let installed = establish_node_instance(&network_dir).expect("installed");
        let _ = FileExt::unlock(&held);

        // A later start — or an explicit repair — settles on the marker that landed, and
        // repairs are permitted again.
        let retried = establish_node_identity(&config);
        assert_eq!(retried, NodeIdentity::Stable);
        assert!(retried.permits_chain_repair());
        assert_eq!(std::fs::read_to_string(&marker).unwrap(), installed);
        // Deliberately not asserting that a claim now succeeds: that also depends on the
        // process-wide maintenance guard other tests take, and taking it here to look
        // would make *their* assertions flaky in return. `permits_chain_repair` above is
        // the identity-level property this test owns; that the gate then opens is
        // asserted under the serialising lock in
        // `revalidate::tests::an_unsettled_identity_permits_no_chain_operation`.

        let _ = std::fs::remove_dir_all(&dir);
    }

    // The same property as the test above, established deterministically rather than by
    // running threads at each other and hoping the bad interleaving shows up: hold the
    // lock, prove a caller cannot proceed while it is held, install an identity underneath
    // it, and require the caller to adopt that one once released. A caller that could
    // delete a marker installed after its own validation read would return a different
    // identity here, every time.
    #[test]
    fn a_caller_cannot_replace_a_marker_installed_while_it_waited() {
        use fs4::fs_std::FileExt;

        let (dir, cookie_path) = a_marker_datadir("locked-out");
        let network_dir = cookie_path.parent().unwrap().to_path_buf();
        let marker = network_dir.join(coincubed::NODE_INSTANCE_FILE);
        std::fs::create_dir_all(&network_dir).unwrap();
        // Malformed, so the waiting caller's plan is to replace it.
        std::fs::write(&marker, "partial").unwrap();

        let lock_path = network_dir.join(format!("{}.lock", coincubed::NODE_INSTANCE_FILE));
        let held = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .unwrap();
        held.lock_exclusive().unwrap();

        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let waiter = {
            let cookie_path = cookie_path.clone();
            std::thread::spawn(move || {
                let instance = ensure_node_instance_marker(&cookie_path).expect("stamped");
                let _ = done_tx.send(());
                instance
            })
        };

        // It must not get past the lock. If it did, it would be reading the malformed
        // marker right now and preparing to delete whatever replaces it. Comfortably
        // inside the acquisition bound, so it is still waiting rather than giving up.
        assert!(
            done_rx
                .recv_timeout(std::time::Duration::from_millis(40))
                .is_err(),
            "a caller proceeded while the marker lock was held"
        );

        // Install the winning identity under the lock we are holding.
        let winner = establish_node_instance(&network_dir).expect("installed");
        assert!(coincubed::valid_node_instance(&winner));
        let _ = FileExt::unlock(&held);

        assert_eq!(
            waiter.join().unwrap(),
            winner,
            "a caller replaced a marker that was installed while it waited"
        );
        assert_eq!(std::fs::read_to_string(&marker).unwrap(), winner);
        assert_eq!(
            marker_artifacts(&cookie_path),
            vec![format!("{}.lock", coincubed::NODE_INSTANCE_FILE)]
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // Several Vaults can attach to one fresh managed datadir in the same instant. Every
    // one of them must come away with the *same* complete identity: if one of them
    // records a repair against an identity another never sees, the authorisation stops
    // matching and the repaired chain is refused indefinitely.
    #[test]
    fn concurrent_stamping_agrees_on_one_complete_identity() {
        let (dir, cookie_path) = a_marker_datadir("concurrent");

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let observed: Vec<String> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    let barrier = barrier.clone();
                    let cookie_path = cookie_path.clone();
                    scope.spawn(move || {
                        barrier.wait();
                        ensure_node_instance_marker(&cookie_path).expect("stamped")
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });

        let first = &observed[0];
        assert!(coincubed::valid_node_instance(first));
        for instance in &observed {
            assert_eq!(
                instance, first,
                "racing callers disagreed about the node's identity"
            );
        }
        // And what is on disk is what they all reported — never an empty or partial
        // file that a reader could have picked up in between.
        let marker = cookie_path
            .parent()
            .unwrap()
            .join(coincubed::NODE_INSTANCE_FILE);
        assert_eq!(&std::fs::read_to_string(&marker).unwrap(), first);

        let _ = std::fs::remove_dir_all(&dir);
    }

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
            "https://bitcoinknots.org/files/29.x/29.3.knots20260507/\
             bitcoin-29.3.knots20260507-arm64-apple-darwin.tar.gz"
        );
        // Knots, x86_64 Linux.
        assert_eq!(
            NodeFlavor::Knots.asset_url(KNOTS_VERSION, NodeOs::Linux, NodeArch::X86_64),
            "https://bitcoinknots.org/files/29.x/29.3.knots20260507/\
             bitcoin-29.3.knots20260507-x86_64-linux-gnu.tar.gz"
        );
        // Knots, Windows — the `-pgpverifiable` suffix Core lacks, confirmed
        // against the live 20260507 directory listing rather than carried over
        // from the 20260508 release.
        assert_eq!(
            NodeFlavor::Knots.asset_url(KNOTS_VERSION, NodeOs::Windows, NodeArch::X86_64),
            "https://bitcoinknots.org/files/29.x/29.3.knots20260507/\
             bitcoin-29.3.knots20260507-win64-pgpverifiable.zip"
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
            NodeFlavor::from_version("29.3.knots20260507"),
            NodeFlavor::Knots
        );
        assert_eq!(NodeFlavor::from_version("29.0"), NodeFlavor::Core);
    }

    // The pin is the last non-enforcing Knots release, and the enforcement
    // predicate agrees with it. If these two ever disagree, the managed node
    // enforces a stalled fork and the repair path refuses to pull it back off.
    #[test]
    fn the_pinned_knots_build_does_not_enforce_rdts() {
        assert_eq!(KNOTS_VERSION, "29.3.knots20260507");
        assert!(!build_enforces_rdts(&format!(
            "/Satoshi:29.3.0({})/",
            KNOTS_VERSION.rsplit('.').next().unwrap()
        )));
        // No enforcing build is offered for installation or reuse, so an
        // already-installed one never satisfies the Knots flavour.
        assert!(!KNOTS_VERSIONS.contains(&"29.3.knots20260508"));
    }

    // Enforcement is read off the build tag, not the flavour: the two Knots
    // builds either side of the RDTS release answer differently.
    #[test]
    fn rdts_enforcement_is_read_from_the_subversion() {
        assert!(!build_enforces_rdts("/Satoshi:29.3.0(knots20260507)/"));
        assert!(build_enforces_rdts("/Satoshi:29.3.0(knots20260508)/"));
        // Later builds keep enforcing.
        assert!(build_enforces_rdts("/Satoshi:29.4.0(knots20260601)/"));
        // Core never does, whatever the version.
        assert!(!build_enforces_rdts("/Satoshi:29.0.0/"));
        // A Knots build we cannot date is assumed enforcing: the cost of being
        // wrong the other way is a repair that loops on every start.
        assert!(build_enforces_rdts("/Satoshi:29.3.0(knots-custom)/"));
        // Case is not load-bearing.
        assert!(build_enforces_rdts("/Satoshi:29.3.0(KNOTS20260508)/"));
    }

    // `consensusrules` is never written again — not for either flavour, and not
    // even for a config parsed from a file that still carries it. Parsing it back
    // is retained only so such a file is still recognised as a Knots node's.
    #[test]
    fn consensusrules_is_read_but_never_written() {
        let net = InternalBitcoindNetworkConfig {
            rpc_port: 12345,
            p2p_port: 12346,
            prune: 15000,
            rpc_auth: None,
        };

        for flavor in [NodeFlavor::Core, NodeFlavor::Knots] {
            let mut conf = InternalBitcoindConfig::for_flavor(flavor);
            assert!(!conf.enforce_rdts, "{:?} must not opt into RDTS", flavor);
            conf.networks.insert(Network::Bitcoin, net.clone());
            assert!(
                conf.to_ini()
                    .general_section()
                    .get("consensusrules")
                    .is_none(),
                "{:?} emitted consensusrules",
                flavor
            );
        }

        // A legacy file still parses, and still identifies itself as Knots'.
        let legacy = "consensusrules=rdts\n[main]\nrpcport=12345\nport=12346\nprune=15000\n";
        let parsed = InternalBitcoindConfig::from_ini(
            &ini::Ini::load_from_str(legacy).expect("legacy conf parses"),
        )
        .expect("legacy conf loads");
        assert!(parsed.enforce_rdts);
        assert_eq!(parsed.flavor, NodeFlavor::Knots);

        // …and rewriting it drops the line, because the file is rebuilt from the
        // struct rather than edited. That is what keeps the key away from a build
        // that may not accept it.
        assert!(parsed
            .to_ini()
            .general_section()
            .get("consensusrules")
            .is_none());
    }

    // Inbound-over-Tor global options are emitted only when enabled, and every
    // preference round-trips through `to_ini`/`from_ini`.
    #[test]
    fn tor_inbound_emission() {
        let net = InternalBitcoindNetworkConfig {
            rpc_port: 12345,
            p2p_port: 12346,
            prune: 15000,
            rpc_auth: None,
        };

        // Off by default: a Knots config emits none of the Tor keys, leaving the
        // general section empty.
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
        let mut on =
            InternalBitcoindConfig::for_flavor(NodeFlavor::Knots).with_inbound_tor_defaults();
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
        // Nothing in the file marks the flavour any more, so a Knots config that
        // round-trips comes back reporting the placeholder — the flavour ledger is
        // what carries it (see `configured_managed_flavor`).
        assert!(!parsed.enforce_rdts);

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

    // `maxmempool` is emitted only when set, is NOT gated on inbound-over-Tor (a
    // standalone resource key), works on Core as well as Knots, and round-trips
    // through `to_ini`/`from_ini`. Untouched (`None`) stays byte-identical (I2).
    #[test]
    fn max_mempool_emission() {
        let net = InternalBitcoindNetworkConfig {
            rpc_port: 12345,
            p2p_port: 12346,
            prune: PRUNE_MINIMAL_MB,
            rpc_auth: None,
        };

        // Untouched (None) on a plain Core config: no `maxmempool`, and the
        // general section stays empty — byte-identical to today's output.
        let mut off = InternalBitcoindConfig::for_flavor(NodeFlavor::Core);
        off.networks.insert(Network::Bitcoin, net.clone());
        assert_eq!(off.max_mempool_mb, None);
        let off_ini = off.to_ini();
        assert!(off_ini.general_section().get("maxmempool").is_none());
        assert!(off_ini.general_section().is_empty());

        // Set on a Core config with inbound-over-Tor OFF: still emitted (proves
        // it is standalone, not Tor-gated, and not flavour-gated).
        let mut on = InternalBitcoindConfig::for_flavor(NodeFlavor::Core);
        on.max_mempool_mb = Some(MAX_MEMPOOL_SMALL_MB);
        on.networks.insert(Network::Bitcoin, net.clone());
        assert!(!on.inbound_tor);
        let on_ini = on.to_ini();
        assert_eq!(on_ini.general_section().get("maxmempool"), Some("100"));

        // Round-trip preserves `Some`.
        assert_eq!(
            InternalBitcoindConfig::from_ini(&on_ini)
                .expect("parse maxmempool conf")
                .max_mempool_mb,
            Some(100)
        );

        // Round-trip preserves `None` (the "Default 300 MB" choice).
        assert_eq!(
            InternalBitcoindConfig::from_ini(&off_ini)
                .expect("parse default conf")
                .max_mempool_mb,
            None
        );

        // Coexists with the inbound-over-Tor keys and RDTS: every preference
        // round-trips together.
        let mut both =
            InternalBitcoindConfig::for_flavor(NodeFlavor::Knots).with_inbound_tor_defaults();
        both.max_mempool_mb = Some(300);
        both.tor_control_port = Some(9151);
        both.tor_socks_port = Some(9150);
        both.networks.insert(Network::Bitcoin, net);
        let both_ini = both.to_ini();
        assert_eq!(both_ini.general_section().get("maxmempool"), Some("300"));
        assert!(general_marker_inbound(&both_ini));
        let parsed = InternalBitcoindConfig::from_ini(&both_ini).expect("parse combined conf");
        assert_eq!(parsed.max_mempool_mb, Some(300));
        assert!(parsed.inbound_tor);
        assert_eq!(parsed.max_upload_target_mb_day, Some(1000));
    }

    // The one-click "Small computer" preset: bitcoind's floor prune plus a
    // 100 MB mempool cap, and it leaves `maxconnections` alone (Decision 3.6).
    #[test]
    fn small_computer_preset_values() {
        let r = NodeResources::small_computer();
        assert_eq!(r.prune_mb, PRUNE_MIN);
        assert_eq!(r.prune_mb, 550);
        assert_eq!(r.max_mempool_mb, Some(100));
        // The estimated total is honest about the unprunable chainstate floor:
        // 550 MB of block data rounds to ~1 GB, plus the ~14 GB overhead.
        assert_eq!(
            estimated_total_disk_gb(PRUNE_MINIMAL_MB),
            1 + CHAINSTATE_OVERHEAD_GB
        );
        assert_eq!(
            estimated_total_disk_gb(PRUNE_DEFAULT),
            15 + CHAINSTATE_OVERHEAD_GB
        );

        // "Regular computer" is the out-of-the-box default profile: 15 GB prune
        // and the key-omitted (300 MB) mempool default — the inverse of Small.
        let reg = NodeResources::regular_computer();
        assert_eq!(reg.prune_mb, PRUNE_DEFAULT);
        assert_eq!(reg.max_mempool_mb, None);
    }

    // When both flavours are installed, the launched binary must match the
    // configured flavour — a machine that kept Core around after a Knots setup
    // would otherwise launch the wrong one.
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

    // An RDTS-enforcing Knots build already on disk must not satisfy the Knots
    // flavour, or the update would keep running the binary that stranded the node
    // instead of downloading the pinned one. Both "is it installed?" checks — the
    // launcher's and the installer's — key on the pinned version list, so a
    // 20260508 directory is simply not a candidate.
    #[test]
    fn an_installed_enforcing_build_does_not_satisfy_knots() {
        use std::fs;

        let base = std::env::temp_dir().join(format!(
            "coincube-enforcing-bin-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&base);
        let datadir = CoincubeDirectory::new(base.clone());

        // The stranded dev machine: the enforcing build, and nothing else.
        const ENFORCING: &str = "29.3.knots20260508";
        let stale = internal_bitcoind_exe_path(&datadir, ENFORCING);
        fs::create_dir_all(stale.parent().unwrap()).unwrap();
        fs::write(&stale, b"fake enforcing bitcoind").unwrap();

        // Not launchable: nothing is installed as far as the launcher is
        // concerned, for either flavour.
        assert_eq!(
            select_managed_bitcoind_exe(&datadir, NodeFlavor::Knots),
            None
        );
        assert_eq!(
            select_managed_bitcoind_exe(&datadir, NodeFlavor::Core),
            None
        );
        // And not "already installed" for the installer / settings setup, which
        // both test this exact path before deciding to download.
        assert!(!internal_bitcoind_exe_path(&datadir, NodeFlavor::Knots.version()).exists());

        // Install the pinned build alongside it: now Knots resolves, and to the
        // pinned build rather than the newer-looking one.
        let pinned = internal_bitcoind_exe_path(&datadir, KNOTS_VERSION);
        fs::create_dir_all(pinned.parent().unwrap()).unwrap();
        fs::write(&pinned, b"fake bitcoind").unwrap();
        assert_eq!(
            select_managed_bitcoind_exe(&datadir, NodeFlavor::Knots),
            Some(pinned)
        );

        let _ = fs::remove_dir_all(&base);
    }

    // The flavour survives the loss of its only marker in `bitcoin.conf`: a
    // datadir written by a release that enforced RDTS is recognised as Knots',
    // recorded in the ledger, and the legacy line is stripped from the file.
    #[test]
    fn a_legacy_rdts_conf_is_migrated_to_the_flavour_ledger() {
        use std::fs;

        let base =
            std::env::temp_dir().join(format!("coincube-rdts-migration-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let datadir = CoincubeDirectory::new(base.clone());
        let config_path = internal_bitcoind_config_path(&internal_bitcoind_datadir(&datadir));
        fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        fs::write(
            &config_path,
            "consensusrules=rdts\n[main]\nrpcport=12345\nport=12346\nprune=15000\n",
        )
        .unwrap();

        // Before: the legacy line is the only thing saying "Knots".
        assert_eq!(configured_managed_flavor(&datadir), Some(NodeFlavor::Knots));

        migrate_legacy_rdts_conf(&datadir);

        // After: the line is gone from the file the node will be handed…
        let migrated = InternalBitcoindConfig::from_file(&config_path).expect("conf still loads");
        assert!(!migrated.enforce_rdts);
        assert!(!fs::read_to_string(&config_path)
            .unwrap()
            .contains("consensusrules"));
        // …the ports it carried are untouched…
        assert_eq!(
            migrated.networks.get(&Network::Bitcoin).map(|n| n.rpc_port),
            Some(12345)
        );
        // …and the flavour it stood for survives in the ledger.
        assert_eq!(
            crate::node::revalidate::ManagedNodeState::load(&datadir).configured_flavor,
            Some(NodeFlavor::Knots)
        );
        assert_eq!(configured_managed_flavor(&datadir), Some(NodeFlavor::Knots));

        // Idempotent: running it again on the migrated datadir changes nothing.
        migrate_legacy_rdts_conf(&datadir);
        assert_eq!(configured_managed_flavor(&datadir), Some(NodeFlavor::Knots));

        let _ = fs::remove_dir_all(&base);
    }
}
