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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread;
use std::time;

use tracing::{info, warn};

use crate::chain::ChainIdExt;
use crate::dir::{BitcoindDirectory, CoincubeDirectory};

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
    /// Bitcoin Knots for the **Bitcoin Blake2b** chain (the BLAKE2b
    /// proof-of-work hardfork), fetched from bitcoinknots.org — the
    /// `29.x.knots20260508` line, which *is* the enforcing build the Bitcoin
    /// flavours deliberately avoid. It serves only a
    /// [`NodeChainFamily::BitcoinBlake2b`] node, from its own directory, and is
    /// never resolved on a Bitcoin chain (see [`NodeFlavor::check_chain`]).
    /// Dormant in this build: the provider, its paths and ports are defined
    /// and tested, but no download or start path reaches it while
    /// `ChainId::runtime_support` is `Dormant`.
    KnotsBlake2b,
}

/// Which chain a managed node serves — the axis that decides *which directory,
/// ports, binary and repair rules* apply. Two families, because Bitcoin Blake2b
/// shares Bitcoin's encoding but not its chain, node or datadir.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeChainFamily {
    /// Bitcoin mainnet and its test networks: `bitcoind/`, Core or Knots.
    Bitcoin,
    /// Bitcoin Blake2b mainnet/testnet4: `bitcoind-blake2b/`, KnotsBlake2b only.
    BitcoinBlake2b,
}

impl NodeChainFamily {
    /// Every family, so a per-family invariant can be checked across all of them.
    pub const ALL: [NodeChainFamily; 2] =
        [NodeChainFamily::Bitcoin, NodeChainFamily::BitcoinBlake2b];

    pub fn from_chain(chain: crate::chain::ChainId) -> Self {
        if chain.is_blake2b() {
            NodeChainFamily::BitcoinBlake2b
        } else {
            NodeChainFamily::Bitcoin
        }
    }

    /// The managed-node root under the COINCUBE datadir: binaries, the node
    /// datadir (`bitcoin.conf`, cookie, chainstate), the lock directory and the
    /// managed-node state ledger all live below it. Distinct per family so the
    /// two chains never share a byte of node state.
    pub fn root_dir_name(self) -> &'static str {
        match self {
            NodeChainFamily::Bitcoin => "bitcoind",
            NodeChainFamily::BitcoinBlake2b => "bitcoind-blake2b",
        }
    }

    /// Flavours that may serve this family. The Bitcoin family keeps its two
    /// choices; Bitcoin Blake2b has exactly one, and it is offered nowhere else.
    pub fn allowed_flavors(self) -> &'static [NodeFlavor] {
        match self {
            NodeChainFamily::Bitcoin => &[NodeFlavor::Core, NodeFlavor::Knots],
            NodeChainFamily::BitcoinBlake2b => &[NodeFlavor::KnotsBlake2b],
        }
    }
}

/// A provider asked to serve a chain it does not belong to. Raised *before*
/// any download, write or spawn — it is the check every chain-aware entry
/// point runs first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderChainMismatch {
    pub flavor: NodeFlavor,
    pub chain: crate::chain::ChainId,
}

impl std::fmt::Display for ProviderChainMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} cannot serve {}: it is a {} node provider",
            self.flavor.display_name(),
            self.chain.label(),
            match self.flavor.chain_family() {
                NodeChainFamily::Bitcoin => "Bitcoin",
                NodeChainFamily::BitcoinBlake2b => "Bitcoin Blake2b",
            }
        )
    }
}

impl std::error::Error for ProviderChainMismatch {}

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
/// builds follows a dead chain, so they are not offered.
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

/// Managed Bitcoin Knots versions for the **Bitcoin Blake2b** chain, descending.
///
/// `29.5.1.knots20260508` is the current release of the line that carries the
/// BLAKE2b hardfork (mainnet `Blake2bHeight` 961,640, first shipped in
/// `29.4.1.knots20260508`) — the very build family the Bitcoin pin above stays
/// *below*.
///
/// Only the current release is listed, so an installed `29.4.1` binary no
/// longer satisfies the flavour and is never started again. Opening a Blake2b
/// Cube re-provisions the pinned release when it is missing
/// (`installer::step::node::bitcoind::ensure_pinned_fork_binary`, from the
/// loader), a still-running older build is replaced rather than reused
/// ([`is_pinned_blake2b_build`]), and a pruned datadir that cannot be
/// revalidated is reported as
/// [`StartInternalBitcoindError::PrunedRevalidationRequired`] with a resync
/// offered. Refusing the old build is required, not cosmetic: `29.5.1` changes consensus on both scheduled networks — a
/// rolling ~180-day coinbase maturity from mainnet height 979,919 (with the
/// historical long maturity at 973,440–979,919), and on testnet4 a longer
/// coinbase maturity window plus an RDTS expiry moved from 2026-10-13 to
/// 2027-04-01 (`src/kernel/chainparams.cpp` at the tag) — so an older binary
/// would follow a different chain. Same publisher, same URL scheme and same signed
/// `SHA256SUMS` manifest (verified against [`KNOTS_SIGNING_KEY_ASC`]); the
/// filenames and the manifest's signing identity for this release are pinned
/// by the installer test
/// `blake2b_release_manifest_verifies_under_the_pinned_knots_key` against a
/// vendored copy of the upstream manifest. Deliberately disjoint from
/// [`KNOTS_VERSIONS`]: a version string resolves to exactly one flavour.
pub const KNOTS_BLAKE2B_VERSIONS: [&str; 1] = ["29.5.1.knots20260508"];

/// Current managed Knots version for a Bitcoin Blake2b node.
pub const KNOTS_BLAKE2B_VERSION: &str = KNOTS_BLAKE2B_VERSIONS[0];

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
            NodeFlavor::KnotsBlake2b => KNOTS_BLAKE2B_VERSION,
        }
    }

    /// All known managed versions for this flavour, newest first. Used to find
    /// an already-installed binary on disk.
    pub fn versions(self) -> &'static [&'static str] {
        match self {
            NodeFlavor::Core => &CORE_VERSIONS,
            NodeFlavor::Knots => &KNOTS_VERSIONS,
            NodeFlavor::KnotsBlake2b => &KNOTS_BLAKE2B_VERSIONS,
        }
    }

    /// The chain family this flavour serves.
    pub fn chain_family(self) -> NodeChainFamily {
        match self {
            NodeFlavor::Core | NodeFlavor::Knots => NodeChainFamily::Bitcoin,
            NodeFlavor::KnotsBlake2b => NodeChainFamily::BitcoinBlake2b,
        }
    }

    /// Whether this flavour may serve `chain`.
    pub fn supports_chain(self, chain: crate::chain::ChainId) -> bool {
        self.chain_family() == NodeChainFamily::from_chain(chain)
    }

    /// Refuse a provider/chain pairing that does not belong together. Every
    /// chain-aware download, write and start path calls this first, so a
    /// Bitcoin Blake2b binary is never resolved, fetched or spawned for a
    /// Bitcoin chain and vice versa.
    pub fn check_chain(self, chain: crate::chain::ChainId) -> Result<(), ProviderChainMismatch> {
        if self.supports_chain(chain) {
            Ok(())
        } else {
            Err(ProviderChainMismatch {
                flavor: self,
                chain,
            })
        }
    }

    /// Infer the flavour from a managed-binary directory `version` string.
    /// Bitcoin Blake2b versions are matched exactly against
    /// [`KNOTS_BLAKE2B_VERSIONS`] first, so `29.5.1.knots20260508` can never be
    /// mistaken for the Bitcoin Knots flavour by the substring rule below.
    /// Knots version strings embed `knots`; Core's never do.
    pub fn from_version(version: &str) -> Self {
        if KNOTS_BLAKE2B_VERSIONS.contains(&version) {
            NodeFlavor::KnotsBlake2b
        } else if version.contains("knots") {
            NodeFlavor::Knots
        } else {
            NodeFlavor::Core
        }
    }

    /// Infer the flavour from a running node's `getnetworkinfo.subversion`
    /// (e.g. `/Satoshi:29.3.0/Knots:20260507/`). Knots embeds `Knots`; Core
    /// never does. Used to decide whether a reachable managed node already
    /// matches the configured flavour or must be replaced.
    ///
    /// Classifies a running **Bitcoin-chain** node only, and deliberately knows
    /// just the Bitcoin flavours: a Knots build observed on the Bitcoin chain
    /// is `Knots` whatever its build date (an enforcing one is then replaced
    /// by the pinned build — see `maybe_start`). A Bitcoin Blake2b node is
    /// never classified from a subversion; its identity comes from the chain
    /// it was started for, and its provider from [`NodeChainFamily`].
    pub fn from_subversion(subversion: &str) -> Self {
        if subversion.to_lowercase().contains("knots") {
            NodeFlavor::Knots
        } else {
            NodeFlavor::Core
        }
    }

    /// Command-line arguments a managed `bitcoind` of this flavour is spawned
    /// with, after `-chain`/`-datadir`. Decided by the binary actually being
    /// launched, not the configured flavour: `select_managed_bitcoind_exe` can
    /// fall back to the other Bitcoin flavour's binary.
    ///
    /// Knots launches a `tor` of its own whenever it cannot reach its Tor
    /// control port (`-torexecute`, default `tor`), and tries again on every
    /// reconnect backoff. With inbound Tor on, the managed node loses its
    /// control port whenever the managed Tor stops under it, and a failed exec
    /// in Knots v29.3's subprocess code closes a descriptor twice (#394,
    /// #605). COINCUBE owns the Tor the node talks to (`node::tor`), so the
    /// launch is switched off: `-torexecute=0` clears the command
    /// (`TorControlThread`, `src/torcontrol.cpp:828-833` at
    /// `v29.3.knots20260507`; the same handling at `src/torcontrol.cpp:863-868`
    /// in `v29.5.1.knots20260508`), and
    /// the option is accepted even by a build without the Tor subprocess
    /// (registered as a hidden argument, `src/init.cpp:620-624`).
    ///
    /// Core has no such option, and an unknown command-line parameter is fatal
    /// there (`Invalid parameter`, `src/common/args.cpp:234-239` at `v29.0`),
    /// so Core gets none.
    pub fn managed_spawn_args(self) -> &'static [&'static str] {
        match self {
            NodeFlavor::Core => &[],
            NodeFlavor::Knots | NodeFlavor::KnotsBlake2b => &["-torexecute=0"],
        }
    }

    /// Human-readable name for UI copy and logs.
    pub fn display_name(self) -> &'static str {
        match self {
            NodeFlavor::Core => "Bitcoin Core",
            NodeFlavor::Knots => "Bitcoin Knots",
            NodeFlavor::KnotsBlake2b => "Bitcoin Knots (Bitcoin Blake2b)",
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
                NodeFlavor::Knots | NodeFlavor::KnotsBlake2b => {
                    format!("bitcoin-{version}-win64-pgpverifiable.zip")
                }
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
            NodeFlavor::Knots | NodeFlavor::KnotsBlake2b => {
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
            NodeFlavor::Knots | NodeFlavor::KnotsBlake2b => {
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

/// The `YYYYMMDD` build date out of a Knots subversion, or `None` for Core and
/// for a Knots build that doesn't carry one where we can find it.
///
/// The separator between `Knots` and the date is skipped rather than assumed:
/// `Knots:20260508`, `knots20260508` and `Knots-20260508` all mean the same
/// thing, and only the date does any work.
fn knots_build_date(subversion: &str) -> Option<u32> {
    let subversion = subversion.to_lowercase();
    let (_, after) = subversion.split_once("knots")?;
    let build: String = after
        .trim_start_matches([':', '-', '_', ' ', '(', '/'])
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    build.parse().ok()
}

/// The client a subversion identifies, or `None` when it is not one we recognise.
///
/// Unlike [`NodeFlavor::from_subversion`], which answers `Core` for anything that
/// is not Knots, this one refuses to guess. That default is right where the
/// question is "which of our two managed binaries is this" — it is asked about a
/// node we downloaded ourselves — but wrong for naming a node to the user: an
/// *external* backend can be any client that speaks the RPC, and calling an
/// unrecognised one "Bitcoin Core" states something we do not know.
///
/// Recognition is by the client segment: Knots advertises `Knots:<date>`
/// alongside Core's `Satoshi:<version>`, and Core advertises `Satoshi` alone.
pub fn recognized_flavor(subversion: &str) -> Option<NodeFlavor> {
    if subversion.to_lowercase().contains("knots") {
        return Some(NodeFlavor::Knots);
    }
    subversion
        .trim_matches('/')
        .split('/')
        .next()?
        .split_once(':')
        .filter(|(client, _)| client.eq_ignore_ascii_case("Satoshi"))
        .map(|_| NodeFlavor::Core)
}

/// How to name the build a node is running, from its `getnetworkinfo.subversion`.
///
/// `/Satoshi:29.3.0/Knots:20260507/` → `29.3.0 (build 20260507)`, and
/// `/Satoshi:29.0.0/` → `29.0.0`. The flavour is deliberately left out: every
/// surface that shows this already names Core or Knots beside it, and repeating
/// it reads as a stutter.
///
/// `None` when the subversion is not the `/Client:version/…/` shape at all —
/// better to show nothing than to present a string we did not understand as a
/// version.
pub fn node_version_label(subversion: &str) -> Option<String> {
    let version = subversion
        .trim_matches('/')
        .split('/')
        .next()?
        .split_once(':')
        .map(|(_, version)| version.trim())
        .filter(|version| !version.is_empty())?;
    Some(match knots_build_date(subversion) {
        Some(build) => format!("{version} (build {build})"),
        None => version.to_string(),
    })
}

/// A `major.minor[.patch]` version padded to three numeric components, so
/// the pin's `29.3` and the subversion's `29.3.0` compare equal. `None` when a
/// component is not a number.
fn normalized_client_version(version: &str) -> Option<[u32; 3]> {
    let mut parts = [0u32; 3];
    let mut count = 0;
    for (i, part) in version.split('.').enumerate() {
        if i >= 3 {
            return None;
        }
        parts[i] = part.parse().ok()?;
        count += 1;
    }
    (count >= 2).then_some(parts)
}

/// What a running Bitcoin Blake2b node's `getnetworkinfo.subversion` says
/// about its build, relative to [`KNOTS_BLAKE2B_VERSION`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blake2bBuild {
    /// The pinned build: same client version and Knots build date.
    Pinned,
    /// A positively identified other build (another version or date, or a
    /// client without a Knots date).
    Other,
    /// Nothing identifiable: no answer, an empty string, or not the
    /// `/Satoshi:<version>…/` shape.
    Unreadable,
}

/// Classify a running node's subversion against the pinned Blake2b build,
/// e.g. `/Satoshi:29.5.1/Knots:20260508/`.
///
/// A Bitcoin Blake2b node's flavour cannot be read from its subversion (the
/// Bitcoin Knots line reports the same `Knots:` shape), but its *build* can,
/// and a pin bump is a consensus change: an older build still running on the
/// managed RPC port follows a different chain and must not be reused.
///
/// The build date is read from the explicit `Knots:<date>` field. Knots puts
/// user-agent comments inside the client segment's parentheses, before that
/// field, so a parenthesised `knots<date>` is used only when there is no
/// explicit field.
pub fn classify_blake2b_build(subversion: Option<&str>) -> Blake2bBuild {
    let Some(subversion) = subversion else {
        return Blake2bBuild::Unreadable;
    };
    let Some((pin_version, pin_build)) = KNOTS_BLAKE2B_VERSION.split_once(".knots") else {
        return Blake2bBuild::Unreadable;
    };
    let mut segments = subversion.trim_matches('/').split('/');
    let Some((client, version_and_comments)) =
        segments.next().and_then(|client| client.split_once(':'))
    else {
        return Blake2bBuild::Unreadable;
    };
    if !client.eq_ignore_ascii_case("Satoshi") {
        return Blake2bBuild::Unreadable;
    }
    let (version, comments) = match version_and_comments.split_once('(') {
        Some((version, comments)) => (version.trim(), Some(comments)),
        None => (version_and_comments.trim(), None),
    };
    let Some(running_version) = normalized_client_version(version) else {
        return Blake2bBuild::Unreadable;
    };
    let explicit_build = segments.find_map(|segment| {
        let (field, date) = segment.split_once(':')?;
        field
            .eq_ignore_ascii_case("Knots")
            .then(|| date.trim().parse::<u32>().ok())?
    });
    let build = explicit_build.or_else(|| comments.and_then(knots_build_date));
    if normalized_client_version(pin_version) == Some(running_version)
        && build.is_some()
        && pin_build.parse::<u32>().ok() == build
    {
        Blake2bBuild::Pinned
    } else {
        Blake2bBuild::Other
    }
}

/// Whether a running node's subversion is the pinned Blake2b build
/// ([`classify_blake2b_build`]).
pub fn is_pinned_blake2b_build(subversion: &str) -> bool {
    classify_blake2b_build(Some(subversion)) == Blake2bBuild::Pinned
}

/// How long identifying a running Blake2b node's build may take in total,
/// while the family lifecycle lock is held.
const BUILD_PROBE_BUDGET: time::Duration = time::Duration::from_secs(5);

/// `getnetworkinfo.subversion` over a client with a short timeout, so a node
/// that answers `echo` but stalls this call cannot hold the lifecycle lock
/// for the regular client's long RPC timeout. `None` on any failure.
fn probe_subversion(config: &BitcoindConfig, timeout: time::Duration) -> Option<String> {
    use jsonrpc::{client::Client, simple_http::SimpleHttpTransport};
    let builder = match &config.rpc_auth {
        coincubed::config::BitcoindRpcAuth::CookieFile(path) => {
            SimpleHttpTransport::builder().cookie_auth(std::fs::read_to_string(path).ok()?)
        }
        coincubed::config::BitcoindRpcAuth::UserPass(user, pass) => {
            SimpleHttpTransport::builder().auth(user, Some(pass))
        }
    };
    let client = Client::with_transport(
        builder
            .url(&format!("http://{}", config.addr))
            .ok()?
            .timeout(timeout)
            .build(),
    );
    let response = client
        .send_request(client.build_request("getnetworkinfo", None))
        .ok()?;
    let info: serde_json::Value = response.result().ok()?;
    info.get("subversion")?.as_str().map(String::from)
}

/// Identify a running Blake2b node's build within [`BUILD_PROBE_BUDGET`],
/// retrying while it stays unreadable.
fn identify_blake2b_build(config: &BitcoindConfig) -> (Blake2bBuild, Option<String>) {
    let deadline = time::Instant::now() + BUILD_PROBE_BUDGET;
    loop {
        let remaining = deadline.saturating_duration_since(time::Instant::now());
        let subversion = probe_subversion(config, remaining.min(time::Duration::from_secs(2)));
        let build = classify_blake2b_build(subversion.as_deref());
        if build != Blake2bBuild::Unreadable
            || time::Instant::now() + time::Duration::from_millis(500) >= deadline
        {
            return (build, subversion);
        }
        thread::sleep(time::Duration::from_millis(500));
    }
}

/// What a running managed node actually *is*, as opposed to what it was
/// configured to be: the flavour read off the node's own subversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObservedBuild {
    pub flavor: NodeFlavor,
}

impl ObservedBuild {
    /// Read the flavour off a node's `getnetworkinfo.subversion`.
    pub fn from_subversion(subversion: &str) -> Self {
        Self {
            flavor: NodeFlavor::from_subversion(subversion),
        }
    }

    /// Fall back to the configured flavour when the node would not tell us what it
    /// is.
    pub fn assumed(flavor: NodeFlavor) -> Self {
        Self { flavor }
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
    internal_bitcoind_directory_for(coincube_datadir, NodeChainFamily::Bitcoin)
}

/// Managed-node root for a chain family (see
/// [`CoincubeDirectory::bitcoind_directory_for`]). The Bitcoin form above is
/// this with [`NodeChainFamily::Bitcoin`], so legacy paths are byte-identical.
pub fn internal_bitcoind_directory_for(
    coincube_datadir: &CoincubeDirectory,
    family: NodeChainFamily,
) -> PathBuf {
    coincube_datadir
        .bitcoind_directory_for(family)
        .path()
        .to_path_buf()
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
    internal_bitcoind_datadir_for(coincube_datadir, NodeChainFamily::Bitcoin)
}

/// The node datadir (`bitcoin.conf`, `.cookie`, chainstate) for a chain family:
/// `<root>/<family root>/datadir`. A Bitcoin Blake2b node's chainstate,
/// cookie and config therefore never sit next to Bitcoin's.
pub fn internal_bitcoind_datadir_for(
    coincube_datadir: &CoincubeDirectory,
    family: NodeChainFamily,
) -> PathBuf {
    let mut datadir = internal_bitcoind_directory_for(coincube_datadir, family);
    datadir.push("datadir");
    datadir
}

/// Internal bitcoind executable path.
pub fn internal_bitcoind_exe_path(
    coincube_datadir: &CoincubeDirectory,
    bitcoind_version: &str,
) -> PathBuf {
    internal_bitcoind_exe_path_for(coincube_datadir, NodeChainFamily::Bitcoin, bitcoind_version)
}

/// Managed `bitcoind` executable for `bitcoind_version` under a chain family's
/// root. Binaries are looked up only under their own family's root, so a
/// Bitcoin Blake2b build installed under `bitcoind-blake2b/` is invisible to a
/// Bitcoin chain and vice versa.
pub fn internal_bitcoind_exe_path_for(
    coincube_datadir: &CoincubeDirectory,
    family: NodeChainFamily,
    bitcoind_version: &str,
) -> PathBuf {
    internal_bitcoind_directory_for(coincube_datadir, family)
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

/// Why a new managed-node network section could not be given ports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortAllocationError {
    /// A managed `bitcoin.conf` exists but could not be read, so the ports it
    /// holds are unknown. Allocation refuses rather than guess: a stopped node
    /// whose config we cannot read is exactly the one live socket probing would
    /// not notice.
    UnreadableConfig { path: PathBuf, error: String },
    /// The candidate source (the OS, in production) failed.
    Source(String),
    /// Every candidate offered was reserved, a bitcoind default, or a duplicate.
    Exhausted,
}

impl fmt::Display for PortAllocationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnreadableConfig { path, error } => write!(
                f,
                "cannot allocate node ports: the managed node config at {} could not be read ({})",
                path.display(),
                error
            ),
            Self::Source(e) => write!(f, "Could not get available port: {}.", e),
            Self::Exhausted => write!(f, "Could not get distinct ports. Please try again."),
        }
    }
}

impl std::error::Error for PortAllocationError {}

/// How many candidates [`allocate_managed_ports`] will consider before giving up.
const PORT_ALLOCATION_ATTEMPTS: usize = 20;

/// Every port a new managed-node network section must be allocated around:
/// the other network sections of `existing` (the conf being edited, already
/// parsed by the caller), plus every section of every *other* chain family's
/// `bitcoin.conf` under this COINCUBE datadir. That is what keeps a Bitcoin
/// node and a Bitcoin Blake2b node — each with a mainnet and a testnet4
/// section — from ever sharing an RPC or P2P port. Upstream is no help here:
/// the Blake2b build keeps Bitcoin's default ports, which is also why every
/// managed node runs on OS-allocated ports rather than defaults.
///
/// Fail-closed: a family with no conf reserves nothing, but one whose conf
/// exists and cannot be parsed is an error, because its ports are then unknown
/// and it may simply be stopped. (The caller has already refused on its own
/// family's conf being unreadable, for the same reason.)
///
/// This is a bounded policy, not a lock: two allocations running at the same
/// moment can still both pass this check before either persists. Serialising
/// allocate-and-persist across concurrent setups is a prerequisite for
/// activating a second family, recorded in `docs/BTCB2_MANAGED_NODE.md`.
pub fn reserved_managed_ports(
    coincube_datadir: &CoincubeDirectory,
    family: NodeChainFamily,
    existing: &InternalBitcoindConfig,
) -> Result<Vec<u16>, PortAllocationError> {
    let mut reserved = existing.ports_in_use();
    for other in NodeChainFamily::ALL.iter().copied() {
        if other == family {
            continue;
        }
        let conf_path =
            internal_bitcoind_config_path(&internal_bitcoind_datadir_for(coincube_datadir, other));
        match InternalBitcoindConfig::from_file(&conf_path) {
            Ok(conf) => reserved.extend(conf.ports_in_use()),
            Err(InternalBitcoindConfigError::FileNotFound) => {}
            Err(e) => {
                return Err(PortAllocationError::UnreadableConfig {
                    path: conf_path,
                    error: e.to_string(),
                })
            }
        }
    }
    Ok(reserved)
}

/// Pick two distinct ports (RPC, P2P) for a new managed-node network section.
///
/// Candidates come from `next_candidate` — the OS in production
/// (`installer::step::node::bitcoind::get_available_port`), a fixed sequence in
/// tests — and one is rejected if it is in `reserved` (see
/// [`reserved_managed_ports`]), is a bitcoind default port, or repeats the port
/// already chosen. Gives up after [`PORT_ALLOCATION_ATTEMPTS`] candidates so a
/// source that keeps offering the same port cannot spin forever. The single
/// policy behind both the installer and the settings node-setup paths.
pub fn allocate_managed_ports<E: fmt::Display>(
    reserved: &[u16],
    mut next_candidate: impl FnMut() -> Result<u16, E>,
) -> Result<(u16, u16), PortAllocationError> {
    let mut chosen: Vec<u16> = Vec::with_capacity(2);
    for _ in 0..PORT_ALLOCATION_ATTEMPTS {
        let port = next_candidate().map_err(|e| PortAllocationError::Source(e.to_string()))?;
        if reserved.contains(&port)
            || crate::installer::step::node::bitcoind::BITCOIND_DEFAULT_PORTS.contains(&port)
            || chosen.contains(&port)
        {
            continue;
        }
        chosen.push(port);
        if chosen.len() == 2 {
            return Ok((chosen[0], chosen[1]));
        }
    }
    Err(PortAllocationError::Exhausted)
}

/// Give the managed node's datadir an identity, unless it already has one.
///
/// Written beside the cookie file, i.e. inside the node's own network datadir, and
/// read back by `coincubed` as part of [`coincubed::BackendId`]. That placement is the
/// whole point: anything scoped to one specific node must not carry over to its
/// replacement, and the things that would otherwise identify it — the RPC port, the
/// cookie path — both outlive the datadir being deleted and recreated beneath them.
/// This marker goes with the datadir, so the replacement gets a fresh identity and
/// stale records stop matching. (It was introduced for the chain repair's rollback
/// authorisation, deleted in RDTS sunset PR 4; the identity it provides is now used
/// by the daemon's `BackendId` and by the flavour-ledger gate.)
///
/// Generated once and never rewritten, so it is stable across restarts, node upgrades
/// and flavour switches — all of which leave the datadir in place.
///
/// Installed atomically, and that matters more than it looks. Creating the final name
/// and *then* writing into it leaves a window in which the marker exists but is empty:
/// a second caller sees it, reports success, and connects — so anything recorded
/// against an identity derived from an empty marker stops matching every later reader,
/// which derives a different one from the finished file. So the contents are staged
/// under a private name, flushed, and
/// linked into place in one step. A reader sees either no marker or a complete one.
///
/// Returns the identity now in force, which may be another caller's if it got there
/// first. An incomplete marker — an empty file from a crash, or one left by an earlier
/// version of this function — is discarded and replaced rather than trusted forever.
/// Retry policy for the marker lock, as (maximum attempts, delay between them).
///
/// Production makes at most 40 attempts, with 50 ms sleeps between failed
/// attempts and no sleep after the last one. This is an attempt bound, not a
/// wall-clock deadline: exhausting it costs all lock calls plus 39 actual
/// sleeps (1.95 s of requested sleep). Scheduling and lock-call costs make the
/// elapsed time platform- and load-dependent; it may exceed two seconds.
/// Test durations quoted below are likewise nominal, not deadlines.
///
/// Under test the default is *generous* (500 attempts 10 ms apart, nominally ~5 s): a
/// waiter whose holder is merely slow — a loaded CI runner flushing a staged marker to a
/// shared disk can take most of a second — must wait it out and adopt the installed
/// identity, not give up with `WouldBlock`. That is margin, not immunity: a test whose
/// holder must finish inside the waiter's bound still depends on it, and says so. A test
/// that wants the *timeout* path sets a short bound on the contender's own thread with
/// [`with_quick_marker_lock_bound`], so the doomed join costs milliseconds instead of the
/// default — nothing in the suite waits the default out. The production bound is
/// unchanged.
fn lock_acquisition_bound() -> (u32, std::time::Duration) {
    #[cfg(not(test))]
    {
        (40, std::time::Duration::from_millis(50))
    }
    #[cfg(test)]
    {
        marker_lock_bound_override().unwrap_or((500, std::time::Duration::from_millis(10)))
    }
}

#[cfg(test)]
thread_local! {
    /// A per-thread override of the default test bound (see
    /// [`lock_acquisition_bound`]). Thread-local on purpose: a contender spawned by a
    /// test keeps the generous default unless *it* asks for the short bound, so a test
    /// thread's override never leaks into the waiter it is racing. Separate from the
    /// managed-conf lock's override — the two locks are independent and so are their
    /// test bounds.
    static MARKER_LOCK_BOUND_OVERRIDE: std::cell::Cell<Option<(u32, std::time::Duration)>> =
        const { std::cell::Cell::new(None) };
}

#[cfg(test)]
fn marker_lock_bound_override() -> Option<(u32, std::time::Duration)> {
    MARKER_LOCK_BOUND_OVERRIDE.with(|b| b.get())
}

/// The short bound [`with_quick_marker_lock_bound`] installs: 3 × 5 ms.
#[cfg(test)]
const QUICK_MARKER_LOCK_BOUND: (u32, std::time::Duration) =
    (3, std::time::Duration::from_millis(5));

/// Run `body` with *this thread's* marker-lock acquisition bound set short
/// ([`QUICK_MARKER_LOCK_BOUND`]), for a contender that is meant to time out. Scoped:
/// the previous value is captured and restored when `body` returns — normally or by
/// unwinding — so the override nests and a panicking body cannot leave the short bound
/// behind on a reused thread. A contender that runs on a spawned thread must call this
/// *inside* that thread; setting it on the parent does nothing for the child. Test-only.
#[cfg(test)]
pub(crate) fn with_quick_marker_lock_bound<T>(body: impl FnOnce() -> T) -> T {
    struct Restore(Option<(u32, std::time::Duration)>);
    impl Drop for Restore {
        fn drop(&mut self) {
            MARKER_LOCK_BOUND_OVERRIDE.with(|b| b.set(self.0));
        }
    }
    let prior = MARKER_LOCK_BOUND_OVERRIDE.with(|b| b.replace(Some(QUICK_MARKER_LOCK_BOUND)));
    let _restore = Restore(prior);
    body()
}

/// Whether the managed node's durable identity can be relied on yet.
///
/// A `BitcoinD` reads the datadir's instance marker once, at construction, and caches
/// the identity it derives. That is fine when the marker is there, and fine when none is
/// expected — but not when one is expected and merely *late*: everything built in the
/// meantime reports the endpoint-and-cookie-path identity, and everything built after the
/// marker lands reports a different one. Anything recorded in that window names an
/// identity that no later client agrees with — which is the failure the marker was
/// introduced to prevent, reached by another route.
///
/// So the answer to a failed marker install is not "carry on with the weaker identity".
/// It is to let the node start and sync, and to refuse the identity-gated writes until
/// the identity is settled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeIdentity {
    /// Settled: either the marker is installed and validated, or none is expected here,
    /// so what a `BitcoinD` reports now is what it will keep reporting.
    Stable,
    /// A marker is expected and is not there. Any identity derived right now is
    /// provisional, so nothing identity-gated may be recorded against it.
    Unstable,
}

impl NodeIdentity {
    /// Whether identity-gated records — today, the flavour ledger — may be
    /// written. Keeps the name it had while the chain repair was the gated write.
    pub fn permits_chain_repair(&self) -> bool {
        matches!(self, Self::Stable)
    }
}

/// Settle the managed node's identity, installing its marker if that has not happened
/// yet.
///
/// Called before the first `BitcoinD` of a managed-node start — so a start that could
/// not establish it does not poison later attempts, it just declines the identity-gated
/// writes until one of them succeeds.
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
                         and sync as usual, but its flavour is not recorded until this succeeds"
                    );
                    NodeIdentity::Unstable
                }
            }
        }
        // No cookie file for a marker to sit beside, so none is expected and none will
        // ever appear. The endpoint-and-username identity such a node reports is already
        // settled — weaker than a marker, but it does not change under us, which is what
        // matters here. Identity-gated writes may proceed.
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
/// every storage-size choice. The history controller uses `prune=1` for
/// managed deletion; no UI path emits `prune=0`.
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
    /// no room for a key of our own. A config parsed off disk always reports the
    /// `Core` placeholder; the durable answer lives in the flavour ledger
    /// (`revalidate::ManagedNodeState::configured_flavor`) and, once a node is
    /// up, in its subversion. See [`configured_managed_flavor`].
    pub flavor: NodeFlavor,
    /// Opt-in inbound connectivity over Tor. When true, [`Self::to_ini`] emits
    /// `listen=1`, `listenonion=1`, `discover=0` (and `torcontrol` once
    /// [`Self::tor_control_port`] is known), so bitcoind advertises itself as a
    /// v3 onion service and accepts inbound peers. The persisted marker is
    /// `listenonion=1`. Off by default — absent keys parse back to all-off —
    /// and off is written as `listenonion=0`, so neither bitcoind flavour
    /// falls back to its own onion-listening default (#598).
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
    /// Optional operator-configured OP_RETURN relay cap. New managed nodes
    /// use the daemon's default; an existing value still round-trips without
    /// changing unrelated settings.
    pub data_carrier_size: Option<u32>,
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
            inbound_tor: false,
            outbound_via_tor: false,
            max_upload_target_mb_day: None,
            max_connections: None,
            max_mempool_mb: None,
            data_carrier_size: None,
            tor_control_port: None,
            tor_socks_port: None,
        }
    }

    /// Every port this config records: RPC and P2P across all its network
    /// sections, plus the managed Tor control and SOCKS ports when inbound is
    /// configured (`torcontrol`/`proxy`). The Tor ports are re-derived on each
    /// start and are held by the running Tor, but a stopped one still owns
    /// them on paper — reserving them keeps the other family's allocation off
    /// them for as long as this file names them.
    pub fn ports_in_use(&self) -> Vec<u16> {
        self.networks
            .values()
            .flat_map(|n| vec![n.rpc_port, n.p2p_port])
            .chain(self.tor_control_port)
            .chain(self.tor_socks_port)
            .collect()
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
            inbound_tor: false,
            outbound_via_tor: false,
            max_upload_target_mb_day: None,
            max_connections: None,
            max_mempool_mb: None,
            data_carrier_size: None,
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
        let mut inbound_tor = false;
        let mut outbound_via_tor = false;
        let mut max_upload_target_mb_day = None;
        let mut max_connections = None;
        let mut max_mempool_mb = None;
        let mut data_carrier_size = None;
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
                        // Standalone relay-policy key (claim step 1's OP_RETURN
                        // marker); parsed back whether or not it is ours.
                        "datacarriersize" => {
                            data_carrier_size = Some(value.parse::<u32>().map_err(|e| {
                                InternalBitcoindConfigError::CouldNotParseValue(e.to_string())
                            })?);
                        }
                        // The legacy line: v1.0.1-rc1 and the other builds
                        // between 2026-06-09 and 2026-08-10 wrote
                        // `consensusrules=rdts` into every managed Knots node's
                        // file, and this build neither reads nor writes the
                        // key. Accepted and dropped: nothing is recorded from
                        // it (no field, no ledger write, no flavour
                        // inference), and `to_ini` never emits it, so the
                        // next rewrite from the parsed form — the Tor
                        // preparation on the loader and settings routes,
                        // `maybe_start`'s pre-spawn rewrite on every route —
                        // takes it off the disk before a node reads the file.
                        "consensusrules" => {}
                        _ => {
                            return Err(InternalBitcoindConfigError::UnexpectedSection(format!(
                                "Unexpected key in general section: {key}"
                            )));
                        }
                    }
                }
            }
        }
        // Nothing in the file records the flavour: callers resolve it from the
        // flavour ledger instead (see [`configured_managed_flavor`]), so the
        // `Core` here is a placeholder, not a finding.
        Ok(Self {
            networks,
            flavor: NodeFlavor::Core,
            inbound_tor,
            outbound_via_tor,
            max_upload_target_mb_day,
            max_connections,
            max_mempool_mb,
            data_carrier_size,
            tor_control_port,
            tor_socks_port,
        })
    }

    /// Read the managed `bitcoin.conf` at `path`.
    ///
    /// Only a file that is genuinely absent (`ErrorKind::NotFound`, including a
    /// missing parent directory) is [`InternalBitcoindConfigError::FileNotFound`].
    /// Any other failure to stat or read it — a parent directory this process
    /// cannot traverse, an unreadable file — is
    /// [`InternalBitcoindConfigError::ReadingFile`], so a caller that treats
    /// absence as "nothing configured" (a fresh install; the other chain
    /// family reserving no ports) never mistakes a file it merely cannot see
    /// for one that does not exist. `Path::exists` would have collapsed the two.
    pub fn from_file(path: &PathBuf) -> Result<Self, InternalBitcoindConfigError> {
        if let Err(e) = std::fs::metadata(path) {
            return Err(if e.kind() == std::io::ErrorKind::NotFound {
                InternalBitcoindConfigError::FileNotFound
            } else {
                InternalBitcoindConfigError::ReadingFile(e.to_string())
            });
        }
        let conf_ini = ini::Ini::load_from_file(path)
            .map_err(|e| InternalBitcoindConfigError::ReadingFile(e.to_string()))?;

        Self::from_ini(&conf_ini)
    }

    pub fn to_ini(&self) -> ini::Ini {
        let mut conf_ini = ini::Ini::new();

        // No `consensusrules` line: the key only ever recorded consent to a
        // deployment no shipped build enforces, and the pinned build may not
        // accept it at all. The file is rebuilt from this struct rather than
        // edited, so nothing an older release wrote survives a rewrite.

        self.write_inbound_tor_options(&mut conf_ini);

        // Mempool memory cap — a standalone resource key, emitted whenever set
        // (deliberately NOT gated on `inbound_tor`, unlike the Tor bandwidth
        // caps). It lives in the section-less general part alongside the Tor
        // keys. `None` means bitcoind's own 300 MB default, so the key is omitted
        // and an untouched config produces a byte-identical file (invariant I2).
        if let Some(mb) = self.max_mempool_mb {
            conf_ini
                .with_general_section()
                .set("maxmempool", mb.to_string());
        }

        // OP_RETURN relay cap — the same shape as `maxmempool`: a standalone
        // general-section key, emitted whenever set, `None` omitted so an
        // untouched config (and the Bitcoin Blake2b family's, which never sets
        // it) produces a byte-identical file.
        if let Some(bytes) = self.data_carrier_size {
            conf_ini
                .with_general_section()
                .set("datacarriersize", bytes.to_string());
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

    fn write_inbound_tor_options(&self, conf_ini: &mut ini::Ini) {
        // Inbound-over-Tor. All of these are global (non-network-scoped)
        // bitcoind options, so they belong in the section-less general part of
        // the file. Emitted only when the feature is on; when off, the only
        // Tor line is `listenonion=0` (see the `else` arm).
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
        } else {
            // Inbound off must say so. bitcoind's `-listenonion` defaults to on
            // whenever it listens, which it does by default, so a file silent
            // on the key still starts the torcontrol thread. On Knots that
            // thread, failing to reach a control port, launches `tor` itself
            // (`-torexecute`, default `tor`), and on a machine with no `tor`
            // the failed exec closes a pipe descriptor twice — the double
            // close behind #394's lost block file (#598). With a `tor` it
            // would instead publish an onion service the user never enabled.
            // `listenonion=0` is also valid, and equally meant, on Core.
            // It parses back as inbound off.
            conf_ini.with_general_section().set("listenonion", "0");
        }
    }

    /// Replace the file at `path` with this config, atomically: a reader sees
    /// the previous complete file or the new complete file, never a torn one
    /// (see [`crate::node::managed_conf::write_conf_atomically`]).
    ///
    /// Not serialised against other writers on its own — the locked
    /// [`crate::node::managed_conf::update_managed_conf`] is the path every
    /// production writer takes; this remains for that path's use and for tests.
    /// An `Err` means the file still holds its previous bytes. A replacement
    /// whose directory entry could not be confirmed durable is logged and
    /// reported as `Ok`: the new contents are in place.
    pub fn to_file(&self, path: &Path) -> Result<(), InternalBitcoindConfigError> {
        use crate::node::managed_conf::{write_conf_atomically, ConfWriteError};
        info!("Writing to file {}", path.to_string_lossy());
        let mut bytes = Vec::new();
        self.to_ini()
            .write_to(&mut bytes)
            .map_err(|e| InternalBitcoindConfigError::WritingFile(e.to_string()))?;
        match write_conf_atomically(path, &bytes) {
            Ok(()) => Ok(()),
            Err(ConfWriteError::ReplacedNotDurable(e)) => {
                warn!(
                    "{} was replaced but its directory entry was not confirmed durable: {}",
                    path.display(),
                    e
                );
                Ok(())
            }
            Err(ConfWriteError::NotReplaced(e)) => {
                Err(InternalBitcoindConfigError::WritingFile(e.to_string()))
            }
        }
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
    /// The chain has no runtime in this build ([`crate::chain::RuntimeSupport::Dormant`]).
    ChainUnavailable(&'static str),
    /// The managed node's configuration could not be brought to a safe state
    /// for this start — the datadir-wide conf lock was busy or the conf could
    /// not be read or replaced — so no node was started from what is on disk.
    /// Retryable.
    ConfigUnavailable(String),
    /// The configured managed-node provider cannot serve the chain being started.
    ProviderChainMismatch(ProviderChainMismatch),
    /// The Bitcoin Blake2b node exited because the active chain has to be
    /// revalidated under the current build's consensus rules and the block
    /// data that needs has been pruned (first start after a pin bump such as
    /// `29.4.1` → `29.5.1`). Recoverable only by deleting the chain data and
    /// resyncing: [`resync_managed_fork_chain_data`].
    PrunedRevalidationRequired(crate::chain::ChainId),
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
            Self::ChainUnavailable(reason) => write!(f, "{}", reason),
            Self::ConfigUnavailable(e) => write!(
                f,
                "The managed node's configuration could not be updated, so the node was not \
                 started: {}",
                e
            ),
            Self::ProviderChainMismatch(e) => write!(f, "{}", e),
            Self::PrunedRevalidationRequired(chain) => write!(
                f,
                "The {} node was updated to a release with new consensus rules and must \
                 revalidate its chain, but the block data it needs has been pruned. Delete \
                 the node's chain data and resync to continue; your wallet is not affected.",
                chain.label()
            ),
        }
    }
}
#[derive(Debug, Clone)]
pub struct Bitcoind {
    pub config: BitcoindConfig,
    _owner: Arc<BitcoindOwner>,
}

#[derive(Debug)]
struct BitcoindOwner {
    config: BitcoindConfig,
    lock: Option<LockFile>,
}

static OWNER_CLEANUP: OnceLock<(Mutex<usize>, Condvar)> = OnceLock::new();

fn owner_cleanup() -> &'static (Mutex<usize>, Condvar) {
    OWNER_CLEANUP.get_or_init(|| (Mutex::new(0), Condvar::new()))
}

/// Wait for final-owner releases after the UI has closed, up to `timeout`.
/// A lifecycle lock may be held by another process indefinitely. If the wait
/// expires, process exit releases our OS locks and the next attach reconciles
/// our stale owner files before deciding whether the node may be stopped.
pub fn wait_for_managed_node_cleanup(timeout: time::Duration) -> bool {
    let (count, changed) = owner_cleanup();
    let mut pending = count.lock().unwrap_or_else(|e| e.into_inner());
    let deadline = time::Instant::now() + timeout;
    while *pending != 0 {
        let remaining = deadline.saturating_duration_since(time::Instant::now());
        if remaining.is_zero() {
            return false;
        }
        pending = changed
            .wait_timeout(pending, remaining)
            .unwrap_or_else(|e| e.into_inner())
            .0;
    }
    true
}

/// Serializes the whole managed-node handoff, including Tor preparation and
/// the last owner's shutdown, across threads and GUI processes. The conf lock
/// protects individual writes; it cannot protect the interval between a Tor
/// replacement and the node acquiring its owner lock.
pub struct ManagedNodeLifecycle {
    file: std::fs::File,
    directory: BitcoindDirectory,
}

impl ManagedNodeLifecycle {
    pub fn acquire(
        root: &CoincubeDirectory,
        family: NodeChainFamily,
    ) -> Result<Self, StartInternalBitcoindError> {
        Self::acquire_directory(root.bitcoind_directory_for(family))
    }

    fn acquire_directory(directory: BitcoindDirectory) -> Result<Self, StartInternalBitcoindError> {
        use fs4::fs_std::FileExt;
        std::fs::create_dir_all(directory.path())
            .map_err(|e| StartInternalBitcoindError::Lock(e.to_string()))?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(directory.path().join("lifecycle.lock"))
            .map_err(|e| StartInternalBitcoindError::Lock(e.to_string()))?;
        file.lock_exclusive()
            .map_err(|e| StartInternalBitcoindError::Lock(e.to_string()))?;
        let lifecycle = Self { file, directory };
        lifecycle.reconcile_stale_owners().map_err(|e| {
            StartInternalBitcoindError::Lock(format!(
                "Could not reconcile managed-node owners: {e}"
            ))
        })?;
        Ok(lifecycle)
    }

    /// Remove only owner files proved stale while the datadir lifecycle lock
    /// excludes every new-version attach/stop. Unknown files and uncertain
    /// liveness remain owners, so a read failure cannot stop a live node.
    fn reconcile_stale_owners(&self) -> std::io::Result<()> {
        let locks = self.directory.path().join(LOCK_DIRECTORY_NAME);
        let networks = match std::fs::read_dir(&locks) {
            Ok(networks) => networks,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        for network in networks {
            let network = network?;
            Self::reconcile_network_owners(network)?;
        }
        if std::fs::read_dir(&locks)?.next().is_none() {
            std::fs::remove_dir(locks)?;
        }
        Ok(())
    }

    fn reconcile_network_owners(network: std::fs::DirEntry) -> std::io::Result<()> {
        use fs4::fs_std::FileExt;
        for owner in std::fs::read_dir(network.path())? {
            let owner = owner?;
            let name = owner.file_name();
            let Some(name) = name.to_str() else { continue };
            let Some((pid, suffix)) = parse_owner_name(name) else {
                continue;
            };
            let stale = if let Some(uuid) = suffix.strip_prefix("v2-") {
                if uuid::Uuid::parse_str(uuid).is_err() {
                    continue;
                }
                let file = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(owner.path())?;
                let stale = file.try_lock_exclusive()?;
                drop(file);
                stale
            } else if suffix.parse::<u64>().is_ok() || uuid::Uuid::parse_str(suffix).is_ok() {
                !legacy_process_alive(pid)?
            } else {
                false
            };
            if stale {
                std::fs::remove_file(owner.path())?;
            }
        }
        if std::fs::read_dir(network.path())?.next().is_none() {
            std::fs::remove_dir(network.path())?;
        }
        Ok(())
    }

    /// Includes other processes' owners. Called only while the lifecycle lock
    /// is held, so attach and last-owner release cannot cross this observation.
    pub fn try_has_owners(&self) -> std::io::Result<bool> {
        let locks = self.directory.path().join(LOCK_DIRECTORY_NAME);
        let networks = match std::fs::read_dir(locks) {
            Ok(networks) => networks,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e),
        };
        for network in networks {
            let network = network?;
            let mut owners = std::fs::read_dir(network.path())?;
            if let Some(owner) = owners.next() {
                owner?;
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// A settings change may proceed with its own Cube's owner, but never with
    /// another live Cube attached. The caller's path is only an identity hint:
    /// it must match an owner file observed under this lifecycle lock.
    pub fn ensure_exclusive_settings_owner(
        &self,
        initiating_owner: Option<&Path>,
    ) -> Result<(), String> {
        let locks = self.directory.path().join(LOCK_DIRECTORY_NAME);
        let networks = match std::fs::read_dir(locks) {
            Ok(networks) => networks,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(format!("Could not enumerate managed-node owners: {e}")),
        };
        for network in networks {
            let network =
                network.map_err(|e| format!("Could not enumerate managed-node owners: {e}"))?;
            for owner in std::fs::read_dir(network.path())
                .map_err(|e| format!("Could not enumerate managed-node owners: {e}"))?
            {
                let owner =
                    owner.map_err(|e| format!("Could not enumerate managed-node owners: {e}"))?;
                if initiating_owner != Some(owner.path().as_path()) {
                    return Err("Another Cube owns the managed node. Close it before changing node settings here.".into());
                }
            }
        }
        Ok(())
    }

    /// A failed enumeration is treated as an active owner for cleanup.
    pub fn has_owners(&self) -> bool {
        self.try_has_owners().unwrap_or(true)
    }

    pub fn try_has_foreign_owners(&self) -> std::io::Result<bool> {
        let locks = self.directory.path().join(LOCK_DIRECTORY_NAME);
        let networks = match std::fs::read_dir(locks) {
            Ok(networks) => networks,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e),
        };
        let own_prefix = format!("{}-", std::process::id());
        for network in networks {
            for owner in std::fs::read_dir(network?.path())? {
                let owner = owner?;
                if !owner.file_name().to_string_lossy().starts_with(&own_prefix) {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    pub fn ensure_foreign_attach_safe(
        &self,
        root: &CoincubeDirectory,
        family: NodeChainFamily,
    ) -> Result<(), StartInternalBitcoindError> {
        if family != NodeChainFamily::Bitcoin
            || !self.try_has_foreign_owners().map_err(|e| {
                StartInternalBitcoindError::Lock(format!(
                    "Could not enumerate managed-node owners: {e}"
                ))
            })?
        {
            return Ok(());
        }
        let config_path = internal_bitcoind_config_path(&internal_bitcoind_datadir(root));
        let conf = InternalBitcoindConfig::from_file(&config_path).map_err(|e| {
            StartInternalBitcoindError::ConfigUnavailable(format!(
                "Another process owns this node and its Tor configuration could not be verified: {e}"
            ))
        })?;
        if conf.inbound_tor
            || conf.outbound_via_tor
            || conf.tor_control_port.is_some()
            || conf.tor_socks_port.is_some()
        {
            return Err(StartInternalBitcoindError::ConfigUnavailable(
                "Another app process owns this node's managed Tor. Close that process before opening this Cube here."
                    .into(),
            ));
        }
        Ok(())
    }
}

impl Drop for ManagedNodeLifecycle {
    fn drop(&mut self) {
        if self
            .directory
            .path()
            .ends_with(NodeChainFamily::Bitcoin.root_dir_name())
            && !self.has_owners()
        {
            if let Some(root) = self.directory.path().parent() {
                crate::node::tor::stop_managed_tor_for(&CoincubeDirectory::new(root.to_path_buf()));
            }
        }
        let _ = self.file.unlock();
    }
}

fn parse_owner_name(name: &str) -> Option<(i32, &str)> {
    let name = name.strip_suffix(".lock")?;
    let (pid, suffix) = name.split_once('-')?;
    let pid: i32 = pid.parse().ok()?;
    (pid > 0 && !suffix.is_empty()).then_some((pid, suffix))
}

#[cfg(unix)]
fn legacy_process_alive(pid: i32) -> std::io::Result<bool> {
    // kill(pid, 0) sends no signal. EPERM proves the process exists; only
    // ESRCH proves absence. All other failures preserve its owner file.
    if unsafe { libc::kill(pid, 0) } == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::ESRCH) => Ok(false),
        Some(libc::EPERM) => Ok(true),
        _ => Err(error),
    }
}

#[cfg(windows)]
fn legacy_process_alive(pid: i32) -> std::io::Result<bool> {
    use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows::Win32::System::Threading::{
        OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
    };
    let handle = match unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid as u32) } {
        Ok(handle) => handle,
        Err(error) if error.code().0 as u32 == 0x8007_0057 => return Ok(false),
        Err(error) if error.code().0 as u32 == 0x8007_0005 => return Ok(true),
        Err(error) => return Err(std::io::Error::other(error)),
    };
    let status = unsafe { WaitForSingleObject(handle, 0) };
    let _ = unsafe { CloseHandle(handle) };
    if status == WAIT_OBJECT_0 {
        Ok(false)
    } else if status == WAIT_TIMEOUT {
        Ok(true)
    } else {
        Err(std::io::Error::other(
            "Could not check legacy owner process",
        ))
    }
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
/// 2. the flavour the node was last *observed* running as, for a datadir whose
///    ledger predates `configured_flavor`.
///
/// A `None` is now cheap: with no `consensusrules` in the file, either binary can
/// open either datadir, so the fallback in [`select_managed_bitcoind_exe`] is free
/// to launch whatever is installed. That was not true while the config carried a
/// Knots-only key.
pub fn configured_managed_flavor(coincube_datadir: &CoincubeDirectory) -> Option<NodeFlavor> {
    configured_managed_flavor_for(coincube_datadir, NodeChainFamily::Bitcoin)
}

pub fn configured_managed_flavor_for(
    coincube_datadir: &CoincubeDirectory,
    family: NodeChainFamily,
) -> Option<NodeFlavor> {
    let state =
        crate::node::revalidate::ManagedNodeState::try_load_for(coincube_datadir, family).ok()?;
    if let Some(flavor) = state.configured_flavor {
        return Some(flavor);
    }
    state.last_run_flavor
}

/// Pick the managed `bitcoind` binary to launch for `configured_flavor`,
/// preferring that flavour's versions (newest first) and falling back to the
/// other flavour's only if none are installed. Returns the first existing
/// `bitcoin-<version>/bin/bitcoind[.exe]` under the managed directory together
/// with the flavour of that binary (from its version), or `None` when nothing
/// is installed. The flavour returned is the one the spawn arguments follow
/// ([`NodeFlavor::managed_spawn_args`]), since it differs from
/// `configured_flavor` on a fallback.
///
/// Only versions in [`CORE_VERSIONS`] / [`KNOTS_VERSIONS`] are candidates, so a
/// Knots build we no longer ship — `29.3.knots20260508`, which enforces a stalled
/// fork — is never launched, however it got onto the disk.
fn select_managed_bitcoind_exe(
    coincube_datadir: &CoincubeDirectory,
    configured_flavor: NodeFlavor,
) -> Option<(PathBuf, NodeFlavor)> {
    // A Bitcoin flavour may fall back to the other Bitcoin flavour's binary
    // (same chain, same rules); the Bitcoin Blake2b flavour has no fallback and
    // is searched only under its own family root, so no Bitcoin chain ever
    // resolves a Blake2b binary and no Blake2b chain resolves a Bitcoin one.
    let (primary, secondary): (&[&str], &[&str]) = match configured_flavor {
        NodeFlavor::Knots => (&KNOTS_VERSIONS, &CORE_VERSIONS),
        NodeFlavor::Core => (&CORE_VERSIONS, &KNOTS_VERSIONS),
        NodeFlavor::KnotsBlake2b => (&KNOTS_BLAKE2B_VERSIONS, &[]),
    };
    let family = configured_flavor.chain_family();
    primary
        .iter()
        .chain(secondary.iter())
        .map(|v| {
            (
                internal_bitcoind_exe_path_for(coincube_datadir, family, v),
                NodeFlavor::from_version(v),
            )
        })
        .find(|(path, _)| path.exists())
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
    pub fn owner_lock_path(&self) -> Option<&Path> {
        self._owner.lock.as_ref().map(|lock| lock.path.as_path())
    }

    fn owned(config: BitcoindConfig, lock: LockFile) -> Self {
        Self {
            config: config.clone(),
            _owner: Arc::new(BitcoindOwner {
                config,
                lock: Some(lock),
            }),
        }
    }

    /// A handle with no node behind it, for a test that has to hand a caller an
    /// existing managed-node handle without running one. Takes the same lock
    /// file a started node would, so the lock bookkeeping stays honest, and
    /// starts no process. Test-only.
    #[cfg(test)]
    pub(crate) fn for_test(
        config: BitcoindConfig,
        coincube_datadir: &CoincubeDirectory,
        network: Network,
    ) -> Self {
        let lock = LockFile::create(coincube_datadir.bitcoind_directory(), network)
            .expect("synthetic lock file");
        lock.activate();
        Self::owned(config, lock)
    }

    #[cfg(test)]
    pub(crate) fn local_fork_for_test(
        config: BitcoindConfig,
        root: &CoincubeDirectory,
        network: Network,
    ) -> Self {
        let lock = LockFile::create(
            BitcoindDirectory::new(internal_bitcoind_datadir_for(
                root,
                NodeChainFamily::BitcoinBlake2b,
            )),
            network,
        )
        .expect("synthetic local node owner");
        lock.activate();
        Self::owned(config, lock)
    }

    /// Start the managed node for a chain, by identity.
    ///
    /// Select a managed family by exact identity. Fork nodes use their own
    /// binary, configuration, cookie and lock; generic wallet runtime admission
    /// remains separate from starting a syncing companion.
    pub fn maybe_start_for_chain(
        chain: crate::chain::ChainId,
        config: BitcoindConfig,
        coincube_datadir: &CoincubeDirectory,
    ) -> Result<Self, StartInternalBitcoindError> {
        // Generic startup has no authenticated Claim capability. Refuse a
        // dormant chain before the lifecycle lock creates its family root.
        if let crate::chain::RuntimeSupport::Dormant { reason } = chain.runtime_support() {
            return Err(StartInternalBitcoindError::ChainUnavailable(reason));
        }
        Self::preflight_for_chain(chain, coincube_datadir)?;
        let lifecycle =
            ManagedNodeLifecycle::acquire(coincube_datadir, NodeChainFamily::from_chain(chain))?;
        Self::maybe_start_for_chain_locked(chain, config, coincube_datadir, &lifecycle)
    }

    pub fn maybe_start_for_chain_locked(
        chain: crate::chain::ChainId,
        config: BitcoindConfig,
        coincube_datadir: &CoincubeDirectory,
        _lifecycle: &ManagedNodeLifecycle,
    ) -> Result<Self, StartInternalBitcoindError> {
        Self::maybe_start_for_chain_locked_with_restart_owner(
            chain,
            config,
            coincube_datadir,
            _lifecycle,
            None,
        )
    }

    fn maybe_start_for_chain_locked_with_restart_owner(
        chain: crate::chain::ChainId,
        config: BitcoindConfig,
        coincube_datadir: &CoincubeDirectory,
        lifecycle: &ManagedNodeLifecycle,
        restart_owner: Option<&Path>,
    ) -> Result<Self, StartInternalBitcoindError> {
        if let Some(owner) = restart_owner {
            lifecycle
                .ensure_exclusive_settings_owner(Some(owner))
                .map_err(StartInternalBitcoindError::ConfigUnavailable)?;
        }
        let existing_owners = lifecycle.try_has_owners().map_err(|e| {
            StartInternalBitcoindError::Lock(format!(
                "Could not enumerate managed-node owners: {e}"
            ))
        })?;
        lifecycle
            .ensure_foreign_attach_safe(coincube_datadir, NodeChainFamily::from_chain(chain))?;
        let foreign_owners = lifecycle.try_has_foreign_owners().map_err(|e| {
            StartInternalBitcoindError::Lock(format!(
                "Could not enumerate managed-node owners: {e}"
            ))
        })?;
        Self::preflight_for_chain(chain, coincube_datadir)?;
        Self::maybe_start_on_chain(
            chain,
            config,
            coincube_datadir,
            existing_owners,
            foreign_owners,
            restart_owner.is_some(),
        )
    }

    /// Check that the configured provider serves this chain without a side
    /// effect. Generic startup also checks runtime support at its entry point;
    /// the separately admitted Claim path calls this under its lifecycle lock.
    ///
    /// For callers that have work of their own to do before the start —
    /// the loader provisions Tor, rewrites the managed conf for inbound and
    /// may stop a running node first — so the refusal lands before any of it,
    /// not only at the start's own boundary (which checks again).
    pub fn preflight_for_chain(
        chain: crate::chain::ChainId,
        coincube_datadir: &CoincubeDirectory,
    ) -> Result<NodeFlavor, StartInternalBitcoindError> {
        let family = NodeChainFamily::from_chain(chain);
        let default = if chain.is_blake2b() {
            NodeFlavor::KnotsBlake2b
        } else {
            NodeFlavor::Core
        };
        let configured = configured_managed_flavor_for(coincube_datadir, family).unwrap_or(default);
        configured
            .check_chain(chain)
            .map_err(StartInternalBitcoindError::ProviderChainMismatch)?;
        Ok(configured)
    }

    /// Start internal bitcoind for the given network.
    ///
    /// This is the Bitcoin-family start path: the datadir, ledger and lock it
    /// touches are all under the `bitcoind/` root. The configured provider is
    /// checked against the chain first, so a ledger naming a provider of the
    /// other family fails here — before the identity marker is written, the
    /// conf migrated or a process spawned — instead of resolving a binary that
    /// would open a Bitcoin datadir under different consensus rules.
    pub fn maybe_start(
        network: bitcoin::Network,
        config: BitcoindConfig,
        coincube_datadir: &CoincubeDirectory,
    ) -> Result<Self, StartInternalBitcoindError> {
        Self::maybe_start_for_chain(network.into(), config, coincube_datadir)
    }

    pub fn maybe_start_locked(
        network: bitcoin::Network,
        config: BitcoindConfig,
        coincube_datadir: &CoincubeDirectory,
        lifecycle: &ManagedNodeLifecycle,
    ) -> Result<Self, StartInternalBitcoindError> {
        Self::maybe_start_for_chain_locked(network.into(), config, coincube_datadir, lifecycle)
    }

    /// The settings restart already stopped the node while holding the
    /// lifecycle lock. Its own Cube still holds an owner file until the new
    /// handle is adopted, so only that authenticated owner may pass the
    /// otherwise correct unreachable-owner refusal.
    pub fn maybe_restart_locked(
        network: bitcoin::Network,
        config: BitcoindConfig,
        coincube_datadir: &CoincubeDirectory,
        lifecycle: &ManagedNodeLifecycle,
        initiating_owner: Option<&Path>,
    ) -> Result<Self, StartInternalBitcoindError> {
        Self::maybe_start_for_chain_locked_with_restart_owner(
            network.into(),
            config,
            coincube_datadir,
            lifecycle,
            initiating_owner,
        )
    }

    fn maybe_start_on_chain(
        chain: crate::chain::ChainId,
        config: BitcoindConfig,
        coincube_datadir: &CoincubeDirectory,
        existing_owners: bool,
        foreign_owners: bool,
        authorized_restart: bool,
    ) -> Result<Self, StartInternalBitcoindError> {
        let network = chain.bitcoin_network();
        let family = NodeChainFamily::from_chain(chain);
        let configured_flavor = Self::preflight_for_chain(chain, coincube_datadir)?;
        if chain.is_blake2b() {
            verify_managed_fork_rpc_config(&config, coincube_datadir, family, network)?;
        }
        configured_flavor
            .check_chain(chain)
            .map_err(StartInternalBitcoindError::ProviderChainMismatch)?;
        let bitcoind_datadir = internal_bitcoind_datadir_for(coincube_datadir, family);
        // Claim ownership before probing or spawning. Once a process is live,
        // a failure to create this lock would otherwise leave an ownerless
        // node and an ownerless Tor. Errors below drop the provisional lock.
        let owner_lock = LockFile::create(coincube_datadir.bitcoind_directory_for(family), network)
            .map_err(|e| StartInternalBitcoindError::Lock(e.to_string()))?;
        // Settle the datadir's identity before the first connection to it, not after.
        // Every managed-node start comes through here — the loader with a config it read
        // off disk, the installer with one it just wrote, the settings switch — and each
        // goes on to build a `BitcoinD`, which reads the marker once at construction and
        // caches what it derives. Establishing it later means anything recorded against
        // the endpoint-and-cookie-path identity stops matching the moment the marker
        // appears.
        //
        // Not fatal if it fails: the node still starts and syncs, which is what the user
        // is waiting for. What it does cost is the flavour-ledger record — declined while
        // the answer is provisional, and written on the next start instead.
        let identity = establish_node_identity(&config);
        // Launch the binary the user asked for. Nothing in the conf forces our
        // hand (it carries no Knots-only key; those go on the command line,
        // per binary — see `NodeFlavor::managed_spawn_args`), but the choice is
        // still theirs: a machine with both flavours installed must launch the
        // configured one rather than whichever is found first.
        let selected_exe = select_managed_bitcoind_exe(coincube_datadir, configured_flavor);

        // Is a managed node already running on this RPC endpoint? Its flavour
        // is read from its own subversion; `configured_flavor` is assumed when
        // it does not say. A Bitcoin Blake2b node's flavour is not readable
        // that way, but its build is: anything other than the pinned build (an
        // orphaned node from before a pin bump, or one whose subversion cannot
        // be read) is not reused, since a pin bump changes consensus.
        let running = coincubed::BitcoinD::new(&config, "internal_bitcoind_start".to_string())
            .ok()
            .map(|running| {
                if chain.is_blake2b() {
                    // Replaced only on a build positively read as another
                    // one; identifying it is bounded, since the lifecycle
                    // lock is held.
                    let (build, subversion) = identify_blake2b_build(&config);
                    return (running, configured_flavor, build, subversion);
                }
                let subversion = running.subversion();
                let running_flavor = subversion
                    .as_deref()
                    .map(NodeFlavor::from_subversion)
                    .unwrap_or(configured_flavor);
                let build = if running_flavor == configured_flavor {
                    Blake2bBuild::Pinned
                } else {
                    Blake2bBuild::Other
                };
                (running, running_flavor, build, subversion)
            });
        // For Bitcoin, `Pinned` above just means "the configured flavour".
        // Blake2b: a node whose build cannot be read is neither reused (it may
        // be a stale build) nor stopped (it may be a healthy shared node that
        // is just busy); the start is refused and can be retried.
        let running = match running {
            Some((_, _, Blake2bBuild::Unreadable, subversion)) => {
                warn!(
                    "Could not identify the running Bitcoin Blake2b node's build ({:?}); \
                     leaving it running",
                    subversion
                );
                return Err(StartInternalBitcoindError::BitcoinDError(
                    "Could not read which build the running Bitcoin Blake2b node is; it was \
                     left running. Try again in a moment."
                        .into(),
                ));
            }
            Some((running, running_flavor, build, subversion)) => {
                let reusable = build == Blake2bBuild::Pinned;
                if chain.is_blake2b() && !reusable {
                    warn!(
                        "A Bitcoin Blake2b node that is not the pinned {KNOTS_BLAKE2B_VERSION} \
                         build is running on the managed RPC port ({}); it will be replaced",
                        subversion.as_deref().unwrap_or_default()
                    );
                }
                Some((running, running_flavor, reusable))
            }
            None => None,
        };
        verify_managed_replacement(
            running.as_ref().map(|(_, _, reusable)| *reusable),
            existing_owners,
            foreign_owners,
            authorized_restart,
        )?;
        if let Some((running_node, running_flavor, reusable)) = &running {
            if chain.is_blake2b() && *reusable {
                running_node
                    .check_local_fork_chain(chain, false)
                    .map_err(|e| StartInternalBitcoindError::BitcoinDError(e.to_string()))?;
            }
            // The managed node is shared by every Vault, so flavour is global.
            // If the running node already matches the configured flavour (and,
            // for Blake2b, the pinned build), reuse it: it has read its file
            // already, so no conf rewrite is needed.
            if *reusable {
                info!("Internal bitcoind is already running ({running_flavor:?})");
                // Reconcile here too: this vault may be attaching to a node another
                // vault swapped the flavour of, so this is a start path like any
                // other. `running_flavor` is read from the node's own subversion.
                if !chain.is_blake2b() {
                    crate::node::revalidate::reconcile_after_start(
                        coincube_datadir,
                        &identity,
                        crate::chain::ChainId::from(network),
                        ObservedBuild {
                            flavor: *running_flavor,
                        },
                    );
                }
                owner_lock.activate();
                return Ok(Bitcoind::owned(config, owner_lock));
            }
        }
        // Rewrite the conf from its parsed form before the binary reads it.
        // `from_ini` accepts and drops the legacy `consensusrules` line older
        // releases wrote, and `to_ini` never emits it, so an identity edit
        // under the conf lock is what takes it off the disk. The loader and
        // the settings restart already rewrite the file this way through
        // `tor::prepare_inbound_tor` before they get here; the loader's
        // pending-node route does not, and this is the only rewrite it gets.
        // A file last written by `to_ini` comes back byte-identical. A
        // refusal — lock busy, unreadable, not replaceable — is the same
        // `ConfigUnavailable` the Tor preparation returns for the same
        // reasons: no node is spawned on a file this build could not settle.
        // It runs before the first irreversible step below — stopping a
        // mismatched running node — so a refusal leaves that node running
        // rather than stopped with no replacement started.
        let explicit_wallet_loading =
            rewrite_config_for_managed_spawn(chain, coincube_datadir, family, network)?;
        // Resolve the replacement binary before stopping anything: with no
        // binary to launch, a running node is left as it is rather than
        // stopped with nothing started in its place.
        let (bitcoind_exe_path, exe_flavor) =
            selected_exe.ok_or(StartInternalBitcoindError::ExecutableNotFound)?;
        if let Some((running, running_flavor, _)) = running {
            // A global flavour switch — e.g. Core is up for existing Vaults and
            // the user just picked Knots — or, for Bitcoin Blake2b, a build
            // other than the pinned one: stop it so we can relaunch the
            // configured binary on the same datadir/port; every Vault then
            // reconnects to the new node on the same RPC port.
            info!(
                "Replacing the running managed node ({running_flavor:?}) with the configured \
                 {configured_flavor:?} binary"
            );
            running.stop();
            wait_for_internal_bitcoind_shutdown(&config);
        }
        info!(
            "Found bitcoind executable at '{}' ({exe_flavor:?}).",
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

        // The flavour-specific arguments follow the binary being launched,
        // which is not the configured flavour on a fallback: Core refuses an
        // option it does not know, so a Knots-only one must never reach it.
        let mut args = vec![
            format!("-chain={}", network.to_core_arg()),
            format!("-datadir={}", datadir_path_str),
        ];
        if chain.is_blake2b() {
            args.extend([
                "-listen=0".into(),
                "-listenonion=0".into(),
                "-discover=0".into(),
            ]);
        }
        if explicit_wallet_loading {
            args.push("-nowallet".into());
        }
        args.extend(
            exe_flavor
                .managed_spawn_args()
                .iter()
                .map(|a| a.to_string()),
        );
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

        // Only what this start appends to the node's log is read when it exits
        // early, so an old failure further up the file is never mistaken for
        // this one.
        let debug_log = managed_node_debug_log_path(&bitcoind_datadir, &network);
        let log_offset = std::fs::metadata(&debug_log).map(|m| m.len()).unwrap_or(0);
        let diagnose_early_exit = || {
            (chain.is_blake2b() && pruned_revalidation_logged_since(&debug_log, log_offset))
                .then_some(StartInternalBitcoindError::PrunedRevalidationRequired(
                    chain,
                ))
        };
        let (mut process, started) =
            wait_for_managed_rpc(spawn_bitcoind, &config, diagnose_early_exit)?;
        verify_started_managed_node(
            &started,
            &mut process,
            chain,
            configured_flavor,
            coincube_datadir,
            &identity,
        )?;
        owner_lock.activate();
        Ok(Self::owned(config, owner_lock))
    }

    /// Stop (internal) bitcoind.
    pub fn stop(self) {
        // Clones are references to one Cube's ownership token. Closing one
        // reference must not release the token while another still needs it.
        // BitcoindOwner::drop schedules release after the final Arc goes away.
    }
}

fn verify_managed_fork_rpc_config(
    config: &BitcoindConfig,
    coincube_datadir: &CoincubeDirectory,
    family: NodeChainFamily,
    network: Network,
) -> Result<(), StartInternalBitcoindError> {
    let dir = internal_bitcoind_datadir_for(coincube_datadir, family);
    let conf = InternalBitcoindConfig::from_file(&internal_bitcoind_config_path(&dir))
        .map_err(|e| StartInternalBitcoindError::ConfigUnavailable(e.to_string()))?;
    let Some(section) = conf.networks.get(&network) else {
        return Err(StartInternalBitcoindError::ConfigUnavailable(
            "Missing managed fork network configuration".into(),
        ));
    };
    if config.addr
        != crate::installer::step::node::bitcoind::internal_bitcoind_address(section.rpc_port)
        || config.rpc_auth
            != coincubed::config::BitcoindRpcAuth::CookieFile(internal_bitcoind_cookie_path(
                &dir, &network,
            ))
    {
        return Err(StartInternalBitcoindError::ConfigUnavailable(
            "Local fork RPC configuration differs from the managed node".into(),
        ));
    }
    Ok(())
}

/// `running` is `None` when no node answers on the managed RPC port, otherwise
/// whether the running node can be reused as is (configured flavour and, for
/// Bitcoin Blake2b, the pinned build).
fn verify_managed_replacement(
    running: Option<bool>,
    existing_owners: bool,
    foreign_owners: bool,
    authorized_restart: bool,
) -> Result<(), StartInternalBitcoindError> {
    if existing_owners && running.is_none() && !authorized_restart {
        return Err(StartInternalBitcoindError::ConfigUnavailable(
            "The managed node has active Cube owners but is unreachable. Close those Cubes before starting a replacement.".into(),
        ));
    }
    if foreign_owners && running != Some(true) {
        return Err(StartInternalBitcoindError::ConfigUnavailable(
            "Another app process owns this managed node; it cannot be restarted or replaced here. Close that process first.".into(),
        ));
    }
    Ok(())
}

fn rewrite_config_for_managed_spawn(
    chain: crate::chain::ChainId,
    coincube_datadir: &CoincubeDirectory,
    family: NodeChainFamily,
    network: Network,
) -> Result<bool, StartInternalBitcoindError> {
    let history_lease = if chain.is_blake2b() {
        None
    } else {
        Some(
            crate::node::history::HistoryLease::acquire(&crate::node::retention::directory(
                coincube_datadir,
                network,
            ))
            .map_err(StartInternalBitcoindError::ConfigUnavailable)?,
        )
    };
    let rolling = if chain.is_blake2b() {
        None
    } else {
        crate::node::retention::load(coincube_datadir, network)
            .map_err(StartInternalBitcoindError::ConfigUnavailable)?
    };
    let explicit_wallet_loading = rolling.is_some()
        || (!chain.is_blake2b()
            && crate::node::retention::requires_wallet_bootstrap(coincube_datadir, network)
                .map_err(StartInternalBitcoindError::ConfigUnavailable)?);
    crate::node::managed_conf::update_managed_conf(coincube_datadir, family, |txn| {
        let mut conf = txn.conf.clone();
        if rolling.is_some() {
            if let Some(section) = conf
                .as_mut()
                .and_then(|conf| conf.networks.get_mut(&network))
            {
                section.prune = 1;
            }
        }
        Ok(((), conf))
    })
    .map(|outcome| outcome.logged("rewriting the managed bitcoin.conf before the spawn"))
    .map_err(|e| StartInternalBitcoindError::ConfigUnavailable(e.to_string()))?;
    drop(history_lease);
    Ok(explicit_wallet_loading)
}

/// The node's `debug.log` for `network` under a managed node datadir: the
/// datadir root on mainnet, its network subdirectory otherwise.
pub fn managed_node_debug_log_path(bitcoind_datadir: &Path, network: &Network) -> PathBuf {
    let mut path = bitcoind_datadir.to_path_buf();
    if let Some(dir) = bitcoind_network_dir(network) {
        path.push(dir);
    }
    path.join("debug.log")
}

/// What Knots logs (and exits on) when the active chain must be revalidated
/// for the running build's consensus rules but the blocks it needs are pruned.
const PRUNED_REVALIDATION_MESSAGE: &str = "required block data has been pruned";

/// Whether `debug_log` gained the pruned-revalidation failure after byte
/// `offset`. A missing or unreadable log is `false`: the caller then reports
/// the plain exit it already has.
fn pruned_revalidation_logged_since(debug_log: &Path, offset: u64) -> bool {
    use std::io::{Read as _, Seek as _};
    let Ok(mut file) = std::fs::File::open(debug_log) else {
        return false;
    };
    // A log shorter than the offset was shrunk at startup (Knots keeps the
    // last ~10 MB of a log over ~11 MB) or truncated, so the offset means
    // nothing: only what follows the last startup banner is this launch's.
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let shrunk = len < offset;
    let start = if shrunk { 0 } else { offset };
    if file.seek(std::io::SeekFrom::Start(start)).is_err() {
        return false;
    }
    let mut appended = Vec::new();
    if file.read_to_end(&mut appended).is_err() {
        return false;
    }
    let appended = String::from_utf8_lossy(&appended);
    let this_launch = if shrunk {
        match STARTUP_BANNERS
            .iter()
            .filter_map(|banner| appended.rfind(banner))
            .max()
        {
            Some(at) => &appended[at..],
            // No banner to anchor on: do not guess from older lines.
            None => return false,
        }
    } else {
        &appended[..]
    };
    this_launch.contains(PRUNED_REVALIDATION_MESSAGE)
}

/// The first line a starting node logs (`LogPrintf("%s version %s\n", …)`).
const STARTUP_BANNERS: [&str; 2] = ["Bitcoin Knots version", "Bitcoin Core version"];

/// The chain-data directories a Bitcoin Blake2b resync deletes for `network`:
/// `blocks/` and `chainstate/` directly under the node datadir on mainnet
/// (Knots keeps mainnet in the datadir root), under `testnet4/` (etc.)
/// otherwise. Nothing else under the datadir — `bitcoin.conf`, the cookie,
/// wallets, `debug.log` — is touched.
pub fn managed_fork_chain_data_dirs(root: &CoincubeDirectory, network: &Network) -> [PathBuf; 2] {
    let mut base = internal_bitcoind_datadir_for(root, NodeChainFamily::BitcoinBlake2b);
    if let Some(dir) = bitcoind_network_dir(network) {
        base.push(dir);
    }
    [base.join("blocks"), base.join("chainstate")]
}

/// The lock a running `bitcoind` holds on `<network datadir>/.lock`, taken the
/// same way the node takes it (a POSIX `fcntl` write lock on unix,
/// `LockFileEx` on Windows), so holding it proves no node is running on that
/// datadir and keeps one from starting until it is dropped.
struct NodeDatadirLock {
    _file: std::fs::File,
}

impl NodeDatadirLock {
    fn try_acquire(net_dir: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(net_dir)
            .map_err(|e| format!("Could not open '{}': {e}", net_dir.display()))?;
        let path = net_dir.join(".lock");
        let file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(|e| format!("Could not open '{}': {e}", path.display()))?;
        if !Self::try_lock(&file)
            .map_err(|e| format!("Could not lock '{}': {e}", path.display()))?
        {
            return Err(
                "The Bitcoin Blake2b node is still running (its data directory is locked). \
                 Stop it before resyncing."
                    .into(),
            );
        }
        Ok(Self { _file: file })
    }

    #[cfg(unix)]
    fn try_lock(file: &std::fs::File) -> std::io::Result<bool> {
        use std::os::unix::io::AsRawFd;
        // SAFETY: a zeroed `flock` is a valid value; the fields set below
        // describe a whole-file write lock, and `fd` is open for writing for
        // the duration of the call.
        let mut lock: libc::flock = unsafe { std::mem::zeroed() };
        lock.l_type = libc::F_WRLCK as _;
        lock.l_whence = libc::SEEK_SET as _;
        lock.l_start = 0;
        lock.l_len = 0;
        if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETLK, &lock) } == 0 {
            return Ok(true);
        }
        let e = std::io::Error::last_os_error();
        match e.raw_os_error() {
            Some(code) if code == libc::EACCES || code == libc::EAGAIN => Ok(false),
            _ => Err(e),
        }
    }

    #[cfg(not(unix))]
    fn try_lock(file: &std::fs::File) -> std::io::Result<bool> {
        use fs4::fs_std::FileExt;
        match file.try_lock_exclusive() {
            Ok(true) => Ok(true),
            Ok(false) => Ok(false),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(false),
            Err(e) => Err(e),
        }
    }
}

/// Delete the Bitcoin Blake2b node's chain data for `chain` so the next start
/// resyncs from scratch: the recovery for
/// [`StartInternalBitcoindError::PrunedRevalidationRequired`].
///
/// Runs under the family's lifecycle lock and refuses while any Cube owns the
/// node, while the node's datadir lock is held, while anything accepts
/// connections on the managed RPC port recorded for the network, or when that network has no
/// managed configuration, so the data of a running node is never pulled from
/// under it.
/// Refuses any chain other than a Bitcoin Blake2b one: the Bitcoin node's
/// datadir is never touched here.
pub fn resync_managed_fork_chain_data(
    root: &CoincubeDirectory,
    chain: crate::chain::ChainId,
) -> Result<(), String> {
    if !chain.is_blake2b() {
        return Err("Only the Bitcoin Blake2b managed node can be resynced here.".into());
    }
    let family = NodeChainFamily::BitcoinBlake2b;
    let network = chain.bitcoin_network();
    let lifecycle = ManagedNodeLifecycle::acquire(root, family).map_err(|e| e.to_string())?;
    if lifecycle
        .try_has_owners()
        .map_err(|e| format!("Could not enumerate managed-node owners: {e}"))?
    {
        return Err(
            "Another Cube is using the Bitcoin Blake2b node. Close it before resyncing.".into(),
        );
    }
    let datadir = internal_bitcoind_datadir_for(root, family);
    let conf = InternalBitcoindConfig::from_file(&internal_bitcoind_config_path(&datadir))
        .map_err(|e| format!("Could not read the Bitcoin Blake2b node configuration: {e}"))?;
    let section = conf.networks.get(&network).ok_or_else(|| {
        format!(
            "The Bitcoin Blake2b node has no {} configuration; nothing was deleted.",
            chain.label()
        )
    })?;
    let rpc_addr =
        crate::installer::step::node::bitcoind::internal_bitcoind_address(section.rpc_port);
    // The node's own datadir lock is the authority on whether it is running
    // (an orphaned node whose owner file was reconciled away, or one too busy
    // to answer RPC, still holds it). It stays held through the deletion, so
    // a node cannot start on the datadir while it is being emptied. Anything
    // accepting connections on the recorded RPC port is a second refusal; a
    // plain TCP connect needs no cookie, so a missing cookie never makes an
    // occupied port look idle.
    let mut net_dir = datadir.clone();
    if let Some(dir) = bitcoind_network_dir(&network) {
        net_dir.push(dir);
    }
    let _datadir_lock = NodeDatadirLock::try_acquire(&net_dir)?;
    if std::net::TcpStream::connect_timeout(&rpc_addr, time::Duration::from_secs(1)).is_ok() {
        return Err(
            "Something is still answering on the Bitcoin Blake2b node's RPC port. Stop the node \
             before resyncing."
                .into(),
        );
    }
    for dir in managed_fork_chain_data_dirs(root, &chain.bitcoin_network()) {
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => info!("Deleted '{}' for a resync", dir.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("Could not delete '{}': {e}", dir.display())),
        }
    }
    drop(lifecycle);
    Ok(())
}

fn wait_for_managed_rpc(
    spawn_bitcoind: impl Fn() -> Result<std::process::Child, StartInternalBitcoindError>,
    config: &BitcoindConfig,
    diagnose_early_exit: impl Fn() -> Option<StartInternalBitcoindError>,
) -> Result<(std::process::Child, coincubed::BitcoinD), StartInternalBitcoindError> {
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
                // An exit the node explained in its log is final: respawning
                // would only repeat it.
                if let Some(diagnosed) = diagnose_early_exit() {
                    log::error!("Bitcoind exited with status '{status}': {diagnosed}");
                    return Err(diagnosed);
                }
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
        match coincubed::BitcoinD::new(config, "internal_bitcoind_start".to_string()) {
            Ok(started) => {
                log::info!("Bitcoind seems to have successfully started.");
                return Ok((process, started));
            }
            Err(coincubed::BitcoindError::CookieFile(_)) => {
                // This is only raised if we're using cookie authentication.
                // Assume cookie file has not been created yet and try again.
            }
            Err(e) => {
                check_managed_rpc_start_error(e, try_count, &mut process)?;
            }
        }
        try_count += 1;
        log::info!("Waiting for bitcoind to start.");
        thread::sleep(time::Duration::from_millis(500));
    }
}

fn verify_started_managed_node(
    started: &coincubed::BitcoinD,
    process: &mut std::process::Child,
    chain: crate::chain::ChainId,
    configured_flavor: NodeFlavor,
    coincube_datadir: &CoincubeDirectory,
    identity: &NodeIdentity,
) -> Result<(), StartInternalBitcoindError> {
    // Ask the node what it actually is rather than trusting
    // `configured_flavor`: `select_managed_bitcoind_exe` falls back to
    // the other flavour's binary when the preferred one isn't
    // installed, so the two can legitimately disagree.
    let observed = started
        .subversion()
        .map(|sv| ObservedBuild::from_subversion(&sv))
        .unwrap_or_else(|| ObservedBuild::assumed(configured_flavor));
    if chain.is_blake2b() {
        // We launched the pinned binary; a node that answers with another
        // build is not ours (something else holds the port) and is refused.
        // An unreadable subversion is accepted here: the binary was ours.
        let wrong_build = started
            .subversion()
            .filter(|sv| !is_pinned_blake2b_build(sv))
            .map(|sv| {
                format!(
                    "The started Bitcoin Blake2b node reports '{sv}', not the pinned \
                     {KNOTS_BLAKE2B_VERSION} build"
                )
            });
        let refusal = wrong_build.or_else(|| {
            started
                .check_local_fork_chain(chain, false)
                .err()
                .map(|e| e.to_string())
        });
        if let Some(e) = refusal {
            // We own this freshly spawned process. A wrong-chain
            // binary must not be orphaned on a refused start.
            let _ = process.kill();
            let _ = process.wait();
            return Err(StartInternalBitcoindError::BitcoinDError(e));
        }
    } else {
        crate::node::revalidate::reconcile_after_start(coincube_datadir, identity, chain, observed);
    }
    Ok(())
}

fn check_managed_rpc_start_error(
    e: coincubed::BitcoindError,
    try_count: i32,
    process: &mut std::process::Child,
) -> Result<(), StartInternalBitcoindError> {
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
    Ok(())
}

impl BitcoindOwner {
    fn release(config: BitcoindConfig, mut lock: LockFile) {
        let lifecycle = match ManagedNodeLifecycle::acquire_directory(lock.directory.clone()) {
            Ok(lifecycle) => lifecycle,
            Err(e) => {
                tracing::error!("Failed to acquire managed-node lifecycle lock: {e}");
                return;
            }
        };
        match lock.delete() {
            Err(e) => {
                tracing::error!("Failed to release bitcoind lock: {}", e);
            }
            Ok(false) => {
                info!("Other processes are using internal bitcoind. Process lock has been deleted");
            }
            Ok(true) => {
                match coincubed::BitcoinD::new(&config, "internal_bitcoind_stop".to_string()) {
                    Ok(bitcoind) => {
                        info!("Stopping internal bitcoind...");
                        bitcoind.stop();
                        wait_for_internal_bitcoind_shutdown(&config);
                        info!("Stopped coincube managed bitcoind");
                    }
                    Err(e) => {
                        warn!("Could not create interface to internal bitcoind: '{}'.", e);
                    }
                }
            }
        }
        drop(lifecycle);
    }
}

impl Drop for BitcoindOwner {
    fn drop(&mut self) {
        // A canceled spawn_blocking receiver drops its completed result. Keep
        // that from leaving a running node and Tor without an owner. Arc's
        // inner Drop runs exactly once after all handle clones are gone. Move
        // the owner lock into the worker so it stays live until cleanup, without
        // waiting for the cross-process lifecycle lock on the UI thread.
        let Some(lock) = self.lock.take() else { return };
        let config = self.config.clone();
        let (count, _) = owner_cleanup();
        *count.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        thread::spawn(move || {
            struct Completion;
            impl Drop for Completion {
                fn drop(&mut self) {
                    let (count, changed) = owner_cleanup();
                    *count.lock().unwrap_or_else(|e| e.into_inner()) -= 1;
                    changed.notify_all();
                }
            }
            let _completion = Completion;
            Self::release(config, lock);
        });
    }
}

const LOCK_DIRECTORY_NAME: &str = "locks";

#[derive(Debug)]
struct LockFile {
    path: PathBuf,
    directory: BitcoindDirectory,
    network: Network,
    _file: Option<std::fs::File>,
    released: AtomicBool,
    provisional: AtomicBool,
}

impl LockFile {
    fn create(
        directory: BitcoindDirectory,
        network: Network,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        use fs4::fs_std::FileExt;
        let mut path = directory.clone().path().to_path_buf();
        path.push(LOCK_DIRECTORY_NAME);
        path.push(network.to_string());
        std::fs::create_dir_all(&path)?;

        path.push(format!(
            "{}-v2-{}.lock",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));

        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        if let Err(e) = file.lock_exclusive() {
            drop(file);
            let _ = std::fs::remove_file(&path);
            return Err(Box::new(e));
        }
        Ok(Self {
            path,
            directory,
            network,
            _file: Some(file),
            released: AtomicBool::new(false),
            provisional: AtomicBool::new(true),
        })
    }

    fn activate(&self) {
        self.provisional.store(false, Ordering::Release);
    }

    // returns true if the lock directory is removed because empty.
    fn delete(&mut self) -> Result<bool, Box<dyn std::error::Error>> {
        if self.released.load(Ordering::Acquire) {
            return Ok(false);
        }
        // On Windows, deleting an open locked file is deferred until its handle
        // closes. Close it while the lifecycle guard still excludes new owners,
        // before checking whether this was the final owner.
        drop(self._file.take());
        std::fs::remove_file(&self.path)?;
        self.released.store(true, Ordering::Release);
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

impl Drop for LockFile {
    fn drop(&mut self) {
        if self.provisional.load(Ordering::Acquire) && !self.released.load(Ordering::Acquire) {
            // A start failed after claiming ownership but before returning a
            // Bitcoind handle. Its surrounding lifecycle guard still holds
            // the OS lock and will stop ownerless Tor after this file is gone.
            let _ = self.delete();
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

    #[test]
    fn same_process_owners_are_unique_and_released_once() {
        let root = CoincubeDirectory::new(
            std::env::temp_dir().join(format!("managed-owners-{}", uuid::Uuid::new_v4())),
        );
        let directory = root.bitcoind_directory();
        let mut first = LockFile::create(directory.clone(), Network::Bitcoin).unwrap();
        let mut second = LockFile::create(directory.clone(), Network::Bitcoin).unwrap();
        assert_ne!(
            first.path, second.path,
            "same-second starts need distinct owners"
        );
        assert!(
            !first.delete().unwrap(),
            "the second Cube still owns the node"
        );
        assert!(second.path.exists());
        assert!(!first.delete().unwrap(), "a stale owner releases only once");
        assert!(
            second.path.exists(),
            "an old owner cannot stop its replacement"
        );
        assert!(second.delete().unwrap());
        assert!(!directory.path().join(LOCK_DIRECTORY_NAME).exists());
        std::fs::remove_dir_all(root.path()).unwrap();
    }

    #[test]
    fn final_owner_delete_removes_lock_directory_before_struct_drop() {
        let root = CoincubeDirectory::new(
            std::env::temp_dir().join(format!("final-owner-delete-{}", uuid::Uuid::new_v4())),
        );
        let directory = root.bitcoind_directory();
        let mut owner = LockFile::create(directory.clone(), Network::Bitcoin).unwrap();
        owner.activate();
        let locks = directory.path().join("locks/bitcoin");
        assert!(locks.exists());
        assert!(owner.delete().unwrap());
        assert!(!owner.path.exists());
        assert!(!locks.exists(), "final owner removes its lock directory");
        assert!(!owner.delete().unwrap(), "release only happens once");
        drop(owner);
        std::fs::remove_dir_all(root.path()).unwrap();
    }

    #[test]
    fn failed_start_drops_provisional_owner_before_handoff() {
        let root = CoincubeDirectory::new(
            std::env::temp_dir().join(format!("provisional-owner-{}", uuid::Uuid::new_v4())),
        );
        let directory = root.bitcoind_directory();
        let provisional = LockFile::create(directory.clone(), Network::Bitcoin).unwrap();
        assert!(provisional.path.exists());
        drop(provisional);
        assert!(!directory.path().join("locks").exists());
        std::fs::remove_dir_all(root.path()).unwrap();
    }

    #[test]
    fn abandoned_start_result_releases_its_owner() {
        let root = CoincubeDirectory::new(
            std::env::temp_dir().join(format!("abandoned-owner-{}", uuid::Uuid::new_v4())),
        );
        let config = BitcoindConfig {
            addr: "127.0.0.1:1".parse().unwrap(),
            rpc_auth: coincubed::config::BitcoindRpcAuth::CookieFile(
                root.path().join("bitcoind/.cookie"),
            ),
        };
        let node = Bitcoind::for_test(config, &root, Network::Bitcoin);
        assert!(root
            .bitcoind_directory()
            .path()
            .join("locks/bitcoin")
            .exists());
        drop(node);
        assert!(wait_for_managed_node_cleanup(time::Duration::from_secs(
            120
        )));
        assert!(!root.bitcoind_directory().path().join("locks").exists());
        std::fs::remove_dir_all(root.path()).unwrap();
    }

    #[test]
    fn stopping_a_clone_keeps_the_cube_owner_until_last_reference() {
        let root = CoincubeDirectory::new(
            std::env::temp_dir().join(format!("cloned-owner-{}", uuid::Uuid::new_v4())),
        );
        let config = BitcoindConfig {
            addr: "127.0.0.1:1".parse().unwrap(),
            rpc_auth: coincubed::config::BitcoindRpcAuth::CookieFile(
                root.path().join("bitcoind/.cookie"),
            ),
        };
        let owner = Bitcoind::for_test(config, &root, Network::Bitcoin);
        let clone = owner.clone();
        let locks = root.bitcoind_directory().path().join("locks/bitcoin");
        clone.stop();
        assert_eq!(std::fs::read_dir(&locks).unwrap().count(), 1);
        owner.stop();
        assert!(wait_for_managed_node_cleanup(time::Duration::from_secs(
            120
        )));
        assert!(!locks.exists());
        std::fs::remove_dir_all(root.path()).unwrap();
    }

    #[test]
    fn concurrent_clone_drops_release_one_owner() {
        let root = CoincubeDirectory::new(
            std::env::temp_dir().join(format!("concurrent-drops-{}", uuid::Uuid::new_v4())),
        );
        let config = BitcoindConfig {
            addr: "127.0.0.1:1".parse().unwrap(),
            rpc_auth: coincubed::config::BitcoindRpcAuth::CookieFile(
                root.path().join("bitcoind/.cookie"),
            ),
        };
        let owner = Bitcoind::for_test(config, &root, Network::Bitcoin);
        let a = owner.clone();
        let b = owner.clone();
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let t1 = {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                a.stop();
            })
        };
        let t2 = {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                b.stop();
            })
        };
        drop(owner);
        let locks = root.bitcoind_directory().path().join("locks/bitcoin");
        assert_eq!(std::fs::read_dir(&locks).unwrap().count(), 1);
        barrier.wait();
        t1.join().unwrap();
        t2.join().unwrap();
        assert!(wait_for_managed_node_cleanup(time::Duration::from_secs(
            120
        )));
        assert!(!locks.exists(), "the last Arc drop must release the owner");
        std::fs::remove_dir_all(root.path()).unwrap();
    }

    #[test]
    fn final_owner_drop_does_not_wait_for_contended_lifecycle_lock() {
        let root = CoincubeDirectory::new(
            std::env::temp_dir().join(format!("contended-drop-{}", uuid::Uuid::new_v4())),
        );
        let config = BitcoindConfig {
            addr: "127.0.0.1:1".parse().unwrap(),
            rpc_auth: coincubed::config::BitcoindRpcAuth::CookieFile(
                root.path().join("bitcoind/.cookie"),
            ),
        };
        let node = Bitcoind::for_test(config, &root, Network::Bitcoin);
        let owner_path = node._owner.lock.as_ref().unwrap().path.clone();
        let lifecycle = ManagedNodeLifecycle::acquire(&root, NodeChainFamily::Bitcoin).unwrap();
        let (sent, received) = std::sync::mpsc::channel();
        let dropping = std::thread::spawn(move || {
            drop(node);
            sent.send(()).unwrap();
        });
        let dropped_without_waiting = received.recv_timeout(time::Duration::from_secs(2));
        assert!(
            owner_path.exists(),
            "owner must survive until cleanup acquires the lock"
        );
        assert!(!wait_for_managed_node_cleanup(time::Duration::from_millis(
            10
        )));
        drop(lifecycle);
        dropping.join().unwrap();
        assert!(
            dropped_without_waiting.is_ok(),
            "last owner drop blocked on lifecycle cleanup"
        );
        assert!(wait_for_managed_node_cleanup(time::Duration::from_secs(
            120
        )));
        assert!(!owner_path.exists());
        std::fs::remove_dir_all(root.path()).unwrap();
    }

    #[test]
    fn crashed_v2_owner_is_reconciled_before_attach() {
        let root = CoincubeDirectory::new(
            std::env::temp_dir().join(format!("stale-v2-owner-{}", uuid::Uuid::new_v4())),
        );
        let owners = root.bitcoind_directory().path().join("locks/bitcoin");
        std::fs::create_dir_all(&owners).unwrap();
        std::fs::write(
            owners.join(format!(
                "{}-v2-{}.lock",
                std::process::id(),
                uuid::Uuid::new_v4()
            )),
            b"",
        )
        .unwrap();
        let lifecycle = ManagedNodeLifecycle::acquire(&root, NodeChainFamily::Bitcoin).unwrap();
        assert!(!lifecycle.has_owners());
        drop(lifecycle);
        std::fs::remove_dir_all(root.path()).unwrap();
    }

    #[test]
    fn live_locked_v2_owner_survives_reconciliation() {
        let root = CoincubeDirectory::new(
            std::env::temp_dir().join(format!("live-v2-owner-{}", uuid::Uuid::new_v4())),
        );
        let mut owner = LockFile::create(root.bitcoind_directory(), Network::Bitcoin).unwrap();
        owner.activate();
        let path = owner.path.clone();
        let lifecycle = ManagedNodeLifecycle::acquire(&root, NodeChainFamily::Bitcoin).unwrap();
        assert!(lifecycle.has_owners());
        assert!(path.exists(), "a live locked owner must never be deleted");
        drop(lifecycle);
        owner.delete().unwrap();
        std::fs::remove_dir_all(root.path()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn legacy_dead_pid_is_pruned_but_live_pid_is_preserved() {
        let root = CoincubeDirectory::new(
            std::env::temp_dir().join(format!("legacy-owners-{}", uuid::Uuid::new_v4())),
        );
        let owners = root.bitcoind_directory().path().join("locks/bitcoin");
        std::fs::create_dir_all(&owners).unwrap();
        let mut child = std::process::Command::new("sleep")
            .arg("60")
            .spawn()
            .unwrap();
        let path = owners.join(format!("{}-123.lock", child.id()));
        std::fs::write(&path, b"").unwrap();
        let lifecycle = ManagedNodeLifecycle::acquire(&root, NodeChainFamily::Bitcoin).unwrap();
        assert!(path.exists());
        drop(lifecycle);
        child.kill().unwrap();
        child.wait().unwrap();
        let lifecycle = ManagedNodeLifecycle::acquire(&root, NodeChainFamily::Bitcoin).unwrap();
        assert!(!path.exists());
        assert!(!lifecycle.has_owners());
        drop(lifecycle);
        std::fs::remove_dir_all(root.path()).unwrap();
    }

    #[test]
    fn unreadable_owner_directory_never_looks_empty() {
        let root = CoincubeDirectory::new(
            std::env::temp_dir().join(format!("unreadable-owners-{}", uuid::Uuid::new_v4())),
        );
        let lifecycle = ManagedNodeLifecycle::acquire(&root, NodeChainFamily::Bitcoin).unwrap();
        std::fs::write(
            root.bitcoind_directory().path().join("locks"),
            b"not a directory",
        )
        .unwrap();
        assert!(lifecycle.try_has_owners().is_err());
        assert!(lifecycle.has_owners());
        drop(lifecycle);
        std::fs::remove_dir_all(root.path()).unwrap();
    }

    #[test]
    fn foreign_process_attach_requires_outbound_only_config() {
        let root = CoincubeDirectory::new(
            std::env::temp_dir().join(format!("foreign-owner-{}", uuid::Uuid::new_v4())),
        );
        let lifecycle = ManagedNodeLifecycle::acquire(&root, NodeChainFamily::Bitcoin).unwrap();
        let locks = root.bitcoind_directory().path().join("locks/bitcoin");
        std::fs::create_dir_all(&locks).unwrap();
        std::fs::write(locks.join("other-process.lock"), b"").unwrap();
        let config_path = internal_bitcoind_config_path(&internal_bitcoind_datadir(&root));
        let mut conf = InternalBitcoindConfig::new();
        conf.to_file(&config_path).unwrap();
        assert!(lifecycle
            .ensure_foreign_attach_safe(&root, NodeChainFamily::Bitcoin)
            .is_ok());
        conf.inbound_tor = true;
        conf.tor_control_port = Some(9051);
        conf.tor_socks_port = Some(9050);
        conf.to_file(&config_path).unwrap();
        assert!(lifecycle
            .ensure_foreign_attach_safe(&root, NodeChainFamily::Bitcoin)
            .is_err());
        drop(lifecycle);
        std::fs::remove_dir_all(root.path()).unwrap();
    }

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
    // on would build a `BitcoinD` caching the endpoint-and-cookie-path identity and act
    // on it, only to watch it stop matching the moment the marker finally landed. So
    // giving up has to mean "unstable", not "settled on whatever identity we have".
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
        // long as the bound, so nothing here depends on a guessed sleep — and the bound is
        // the *short* one, set inside the contender's own thread (an override on this
        // thread would not reach it), so the doomed join costs milliseconds rather than
        // the generous default a successful waiter gets.
        let timed_out = {
            let config = config.clone();
            std::thread::spawn(move || {
                with_quick_marker_lock_bound(|| establish_node_identity(&config))
            })
            .join()
            .unwrap()
        };
        assert_eq!(
            timed_out,
            NodeIdentity::Unstable,
            "a timed-out caller must not report a settled identity"
        );
        assert!(!timed_out.permits_chain_repair());

        // The original holder now finishes and releases, which is the "late successful
        // installation" the timed-out caller has to pick up.
        let installed = establish_node_instance(&network_dir).expect("installed");
        let _ = FileExt::unlock(&held);

        // A later start settles on the marker that landed, and the identity-gated
        // write is permitted again.
        let retried = establish_node_identity(&config);
        assert_eq!(retried, NodeIdentity::Stable);
        assert!(retried.permits_chain_repair());
        assert_eq!(std::fs::read_to_string(&marker).unwrap(), installed);
        // `permits_chain_repair` is the identity-level property this test owns; what
        // that gate protects — the flavour ledger advancing only under a settled
        // identity — is asserted in
        // `revalidate::tests::an_unsettled_identity_leaves_the_ledger_alone`.

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
        // marker right now and preparing to delete whatever replaces it. The waiter is
        // on the generous default bound (500 attempts, nominally ~5 s, longer where
        // `sleep` overshoots), so this 40 ms wait plus the install below leave it still
        // waiting rather than giving up — a margin, not a guarantee: this test still
        // requires the holder's critical section (this wait, the install's
        // stage/flush/link, the unlock) to finish inside the waiter's bound, which the
        // old 30-attempt bound did not on a loaded runner.
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

    // The success scenario the nightly failure's shape calls for, with the holder's delay
    // under the test's control instead of the runner's disk: a holder keeps the lock for
    // 600 ms while it installs — well past the old 30-attempt bound's *nominal* 300 ms
    // and far under the default's nominal 5 s — and a waiter on the default bound must
    // wait it out and adopt the installed identity, never give up with `WouldBlock`.
    //
    // What this does and does not demonstrate: the bound is an attempt count, so the
    // old bound's real wall clock is 30 × (one `try_lock_exclusive` + one 10 ms `sleep`)
    // — about 300 ms where `sleep` tracks nominal, and several times that where it
    // overshoots (the same macOS host has measured it at ~0.3 s and at ~2.8 s in
    // different sessions). Against the old bound this test therefore fails where the old
    // bound really costs less than 600 ms and passes where it costs more; it does not
    // distinguish the old bound on every run or platform, and it does not reproduce the
    // runner's own I/O timing. The invariant it asserts — a default waiter outlasts a
    // 600 ms holder — is the one the nightly broke, and it holds under the default bound
    // wherever it has been run.
    #[test]
    fn a_default_waiter_outlasts_a_holder_slower_than_the_old_bound() {
        use fs4::fs_std::FileExt;

        let (dir, cookie_path) = a_marker_datadir("slow-holder");
        let network_dir = cookie_path.parent().unwrap().to_path_buf();
        let marker = network_dir.join(coincubed::NODE_INSTANCE_FILE);
        std::fs::create_dir_all(&network_dir).unwrap();
        std::fs::write(&marker, "partial").unwrap();

        let lock_path = network_dir.join(format!("{}.lock", coincubed::NODE_INSTANCE_FILE));
        let held = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .unwrap();
        held.lock_exclusive().unwrap();

        let waiter = {
            let cookie_path = cookie_path.clone();
            std::thread::spawn(move || ensure_node_instance_marker(&cookie_path))
        };

        // Longer than the old bound's nominal 300 ms, far under the default's nominal 5 s.
        std::thread::sleep(std::time::Duration::from_millis(600));
        let winner = establish_node_instance(&network_dir).expect("installed");
        let _ = FileExt::unlock(&held);

        let adopted = waiter
            .join()
            .unwrap()
            .expect("a default-bound waiter must outlast a slow holder, not time out");
        assert_eq!(adopted, winner);
        assert_eq!(std::fs::read_to_string(&marker).unwrap(), winner);

        let _ = std::fs::remove_dir_all(&dir);
    }

    // The override is per thread and scoped: it does not reach a spawned thread, it
    // nests by restoring the previous value, and a panicking body does not leave the
    // short bound behind.
    #[test]
    fn the_quick_marker_bound_is_thread_scoped_nests_and_survives_unwinding() {
        let default = (500, std::time::Duration::from_millis(10));
        assert_eq!(lock_acquisition_bound(), default);

        with_quick_marker_lock_bound(|| {
            assert_eq!(lock_acquisition_bound(), QUICK_MARKER_LOCK_BOUND);
            // A thread spawned from inside the override keeps the default.
            let seen_by_child = std::thread::spawn(lock_acquisition_bound).join().unwrap();
            assert_eq!(
                seen_by_child, default,
                "the override leaked into a spawned thread"
            );
            // Nesting restores the value that was in force, not `None`.
            with_quick_marker_lock_bound(|| {
                assert_eq!(lock_acquisition_bound(), QUICK_MARKER_LOCK_BOUND);
            });
            assert_eq!(lock_acquisition_bound(), QUICK_MARKER_LOCK_BOUND);
        });
        assert_eq!(lock_acquisition_bound(), default);

        // A body that panics unwinds through the guard, and the bound is restored.
        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            with_quick_marker_lock_bound(|| {
                assert_eq!(lock_acquisition_bound(), QUICK_MARKER_LOCK_BOUND);
                panic!("body panicked while the short bound was in force");
            })
        }));
        assert!(unwound.is_err());
        assert_eq!(
            lock_acquisition_bound(),
            default,
            "a panicking body left the short bound set on this thread"
        );
    }

    // The timeout contender's bound really is the short one *inside its own thread*: a
    // doomed contender that set the override on its parent instead would wait the
    // generous default (500 attempts, nominally ~5 s and never less) out. Bounded well
    // under that, so a wrong placement fails.
    #[test]
    fn a_quick_bound_contender_gives_up_promptly_while_a_default_waiter_does_not() {
        use fs4::fs_std::FileExt;

        let (dir, cookie_path) = a_marker_datadir("quick-vs-default");
        let network_dir = cookie_path.parent().unwrap().to_path_buf();
        std::fs::create_dir_all(&network_dir).unwrap();
        let lock_path = network_dir.join(format!("{}.lock", coincubed::NODE_INSTANCE_FILE));
        let held = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .unwrap();
        held.lock_exclusive().unwrap();

        // Short bound, set inside the contender: gives up in milliseconds.
        let begin = std::time::Instant::now();
        let quick = {
            let cookie_path = cookie_path.clone();
            std::thread::spawn(move || {
                with_quick_marker_lock_bound(|| ensure_node_instance_marker(&cookie_path))
            })
            .join()
            .unwrap()
        };
        let quick_elapsed = begin.elapsed();
        assert_eq!(
            quick
                .expect_err("held lock must time the quick contender out")
                .kind(),
            std::io::ErrorKind::WouldBlock
        );
        assert!(
            quick_elapsed < std::time::Duration::from_secs(2),
            "the quick contender took {:?}: the override did not apply inside its thread",
            quick_elapsed
        );

        // Default bound: still waiting well past the quick contender's give-up point,
        // and it adopts what is installed once the holder releases.
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let waiter = {
            let cookie_path = cookie_path.clone();
            std::thread::spawn(move || {
                let r = ensure_node_instance_marker(&cookie_path);
                let _ = done_tx.send(());
                r
            })
        };
        assert!(
            done_rx
                .recv_timeout(quick_elapsed + std::time::Duration::from_millis(200))
                .is_err(),
            "a default-bound waiter gave up as fast as a quick one"
        );
        let winner = establish_node_instance(&network_dir).expect("installed");
        let _ = FileExt::unlock(&held);
        assert_eq!(waiter.join().unwrap().expect("adopted"), winner);

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
        // 2 network sections plus the general section. Inbound-over-Tor is
        // off, which is written down (#598) and is all that section holds.
        assert_eq!(conf_ini.len(), 3);
        assert_eq!(conf_ini.general_section().get("listenonion"), Some("0"));
        assert_eq!(conf_ini.general_section().len(), 1);
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
                // Inbound-over-Tor off, written down (#598).
                assert_eq!(prop.len(), 1);
                assert_eq!(prop.get("listenonion"), Some("0"));
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

    // The pin is the last Knots release that does not enforce the stalled fork.
    // If it ever moves onto `20260508` or later, the managed node follows a dead
    // chain and nothing in this tree pulls it back off.
    #[test]
    fn the_knots_pin_stays_below_the_enforcing_release() {
        assert_eq!(KNOTS_VERSION, "29.3.knots20260507");
        // No enforcing build is offered for installation or reuse, so an
        // already-installed one never satisfies the Knots flavour.
        assert!(!KNOTS_VERSIONS.contains(&"29.3.knots20260508"));
    }

    // The build date is read off whatever Knots puts between its name and the
    // date, and only off a Knots subversion. The two verbatim `Knots:` strings are
    // what the real binaries either side of the 20260508 release report, captured
    // from `getnetworkinfo`; an earlier parser assumed `(knots20260508)` and could
    // read neither.
    #[test]
    fn the_knots_build_date_is_read_whatever_separates_it() {
        assert_eq!(
            knots_build_date("/Satoshi:29.3.0/Knots:20260507/"),
            Some(20_260_507)
        );
        assert_eq!(
            knots_build_date("/Satoshi:29.3.0/Knots:20260508/"),
            Some(20_260_508)
        );
        assert_eq!(
            knots_build_date("/Satoshi:29.4.0/Knots:20260601/"),
            Some(20_260_601)
        );
        // Other ways the date could be attached.
        assert_eq!(
            knots_build_date("/Satoshi:29.3.0(knots20260507)/"),
            Some(20_260_507)
        );
        assert_eq!(
            knots_build_date("/Satoshi:29.3.0/Knots-20260507/"),
            Some(20_260_507)
        );
        // Case is not load-bearing.
        assert_eq!(
            knots_build_date("/SATOSHI:29.3.0/KNOTS:20260508/"),
            Some(20_260_508)
        );
        // Core carries no date, and a Knots build without a readable one is `None`.
        assert_eq!(knots_build_date("/Satoshi:29.0.0/"), None);
        assert_eq!(knots_build_date("/Satoshi:29.3.0/Knots:custom/"), None);
    }

    // Naming a node to the user requires recognising its client, which is a
    // stricter question than `from_subversion`'s "which of our two binaries is
    // this". An external backend can be any client that speaks the RPC.
    #[test]
    fn an_unrecognised_client_is_not_named_bitcoin_core() {
        assert_eq!(
            recognized_flavor("/Satoshi:29.3.0/Knots:20260507/"),
            Some(NodeFlavor::Knots)
        );
        assert_eq!(
            recognized_flavor("/Satoshi:29.0.0/"),
            Some(NodeFlavor::Core)
        );

        // The cases that matter: another implementation, or nothing readable at
        // all. `from_subversion` answers Core for every one of these — correctly,
        // for picking between our own binaries; not for telling the user what
        // they are running.
        for unknown in [
            "/btcd:0.24.2/",
            "/bcoin:2.2.0/",
            "/libbitcoin:3.0/",
            "",
            "/Satoshi/",
        ] {
            assert_eq!(recognized_flavor(unknown), None, "{unknown:?}");
            assert_eq!(NodeFlavor::from_subversion(unknown), NodeFlavor::Core);
        }
    }

    // The version shown beside the node card's flavour switcher, from the same
    // real subversions.
    #[test]
    fn node_version_labels_name_the_running_build() {
        assert_eq!(
            node_version_label("/Satoshi:29.3.0/Knots:20260507/").as_deref(),
            Some("29.3.0 (build 20260507)")
        );
        // Core carries no build date, so the label is just the version.
        assert_eq!(
            node_version_label("/Satoshi:29.0.0/").as_deref(),
            Some("29.0.0")
        );
        // An external node running something else still gets its version read.
        assert_eq!(
            node_version_label("/Satoshi:28.1.0/").as_deref(),
            Some("28.1.0")
        );
        // Nothing we recognise as a version: show nothing rather than echoing an
        // unparsed string back at the user as if it were one.
        assert_eq!(node_version_label("/Satoshi/"), None);
        assert_eq!(node_version_label(""), None);
        assert_eq!(node_version_label("/Satoshi:/"), None);
    }

    // `consensusrules` is never written — not for either flavour — and a file
    // that still carries one parses as if the line were absent. The read-compat
    // that inferred a flavour from it and the start-path migration that wrote
    // the ledger from it went with RDTS sunset PR 4; what remains is tolerance:
    // v1.0.1-rc1 wrote the line into every managed Knots node's file, and such
    // a datadir must still start. Nothing is recorded from the line, and the
    // parsed form re-serialises without it, which is what the pre-spawn
    // rewrite in `maybe_start` relies on to strip it.
    #[test]
    fn consensusrules_is_never_written_and_is_dropped_on_read() {
        let net = InternalBitcoindNetworkConfig {
            rpc_port: 12345,
            p2p_port: 12346,
            prune: 15000,
            rpc_auth: None,
        };

        for flavor in [NodeFlavor::Core, NodeFlavor::Knots] {
            let mut conf = InternalBitcoindConfig::for_flavor(flavor);
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

        // A legacy file parses to the same config as the same file without the
        // line: the key is dropped, not recorded, and the canonical form the
        // next rewrite persists no longer carries it.
        let legacy = "consensusrules=rdts\n[main]\nrpcport=12345\nport=12346\nprune=15000\n";
        let stripped = "[main]\nrpcport=12345\nport=12346\nprune=15000\n";
        let canonical = |text: &str| {
            let conf = InternalBitcoindConfig::from_ini(
                &ini::Ini::load_from_str(text).expect("conf parses as ini"),
            )
            .expect("the conf parses");
            let mut bytes = Vec::new();
            conf.to_ini().write_to(&mut bytes).unwrap();
            String::from_utf8(bytes).unwrap()
        };
        let from_legacy = canonical(legacy);
        assert_eq!(from_legacy, canonical(stripped));
        assert!(!from_legacy.contains("consensusrules"), "{}", from_legacy);

        // The tolerance is for that one key: a near miss is still refused
        // like any other unknown general-section key, with the key named.
        let unknown = "consensusrule=1\n[main]\nrpcport=12345\nport=12346\nprune=15000\n";
        match InternalBitcoindConfig::from_ini(
            &ini::Ini::load_from_str(unknown).expect("conf parses as ini"),
        ) {
            Err(InternalBitcoindConfigError::UnexpectedSection(msg)) => {
                assert!(msg.contains("consensusrule"), "{}", msg)
            }
            other => panic!("expected the unknown key to be refused, got {:?}", other),
        }
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

        // Off by default: a Knots config says `listenonion=0` and emits none
        // of the other Tor keys, and that parses back as off.
        let mut off = InternalBitcoindConfig::for_flavor(NodeFlavor::Knots);
        off.networks.insert(Network::Bitcoin, net.clone());
        assert!(!off.inbound_tor);
        let off_ini = off.to_ini();
        assert_eq!(off_ini.general_section().get("listenonion"), Some("0"));
        assert_eq!(off_ini.general_section().len(), 1);
        assert!(
            !InternalBitcoindConfig::from_ini(&off_ini)
                .expect("parse inbound-off conf")
                .inbound_tor
        );
        for key in [
            "listen",
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
        assert_eq!(general.get_all("listenonion").count(), 1);
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
        // Nothing in the file marks the flavour, so a Knots config that
        // round-trips comes back reporting the placeholder — the flavour ledger is
        // what carries it (see `configured_managed_flavor`).
        assert_eq!(parsed.flavor, NodeFlavor::Core);

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
        // general section holds only the inbound-off `listenonion=0`.
        let mut off = InternalBitcoindConfig::for_flavor(NodeFlavor::Core);
        off.networks.insert(Network::Bitcoin, net.clone());
        assert_eq!(off.max_mempool_mb, None);
        let off_ini = off.to_ini();
        assert!(off_ini.general_section().get("maxmempool").is_none());
        assert_eq!(off_ini.general_section().len(), 1);

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

        // Coexists with the inbound-over-Tor keys: every preference
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
            Some((
                internal_bitcoind_exe_path(&datadir, KNOTS_VERSION),
                NodeFlavor::Knots
            ))
        );
        // Core conf -> Core binary.
        assert_eq!(
            select_managed_bitcoind_exe(&datadir, NodeFlavor::Core),
            Some((
                internal_bitcoind_exe_path(&datadir, CORE_VERSION),
                NodeFlavor::Core
            ))
        );

        // Fallback: with only Knots installed, a Core conf still finds the
        // Knots binary rather than failing to locate any executable — and
        // reports it as Knots, which is what its spawn arguments follow.
        let core_install = internal_bitcoind_exe_path(&datadir, CORE_VERSION)
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        fs::remove_dir_all(&core_install).unwrap();
        assert_eq!(
            select_managed_bitcoind_exe(&datadir, NodeFlavor::Core),
            Some((
                internal_bitcoind_exe_path(&datadir, KNOTS_VERSION),
                NodeFlavor::Knots
            ))
        );

        let _ = fs::remove_dir_all(&base);
    }

    // A Knots build that enforces the stalled fork, already on disk, must not
    // satisfy the Knots flavour, or the update would keep running the binary that
    // stranded the node
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
            Some((pinned, NodeFlavor::Knots))
        );

        let _ = fs::remove_dir_all(&base);
    }

    // `datacarriersize` is emitted only when set, round-trips, and an untouched
    // (`None`) config stays byte-identical — the same contract as `maxmempool`.
    #[test]
    fn data_carrier_size_emission() {
        let net = InternalBitcoindNetworkConfig {
            rpc_port: 12345,
            p2p_port: 12346,
            prune: PRUNE_MINIMAL_MB,
            rpc_auth: None,
        };
        let mut off = InternalBitcoindConfig::for_flavor(NodeFlavor::Core);
        off.networks.insert(Network::Bitcoin, net.clone());
        assert_eq!(off.data_carrier_size, None);
        let off_ini = off.to_ini();
        assert!(off_ini.general_section().get("datacarriersize").is_none());
        assert_eq!(off_ini.general_section().len(), 1, "only `listenonion=0`");

        let mut on = InternalBitcoindConfig::for_flavor(NodeFlavor::Knots);
        on.data_carrier_size = Some(100);
        on.networks.insert(Network::Bitcoin, net);
        let on_ini = on.to_ini();
        assert_eq!(on_ini.general_section().get("datacarriersize"), Some("100"));
        let parsed = InternalBitcoindConfig::from_ini(&on_ini).expect("parse datacarriersize conf");
        assert_eq!(parsed.data_carrier_size, Some(100));
        // Nothing else rides along with the key.
        assert_eq!(parsed.max_mempool_mb, None);
        assert!(!parsed.inbound_tor);
        assert_eq!(parsed.networks.len(), 1);
    }

    // --- Bitcoin Blake2b provider (dormant) ---------------------------------

    /// A fresh COINCUBE datadir under the OS temp dir, unique per test thread.
    fn a_temp_coincube_datadir(name: &str) -> (std::path::PathBuf, CoincubeDirectory) {
        let base = std::env::temp_dir().join(format!(
            "coincube-btcb2-provider-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).expect("temp datadir");
        (base.clone(), CoincubeDirectory::new(base))
    }

    // The pinned Bitcoin Blake2b release resolves to the exact filenames
    // published for every platform COINCUBE builds for, under the Knots URL
    // scheme, and is verified against the same signed manifest as Knots. The
    // filenames were checked against the published `SHA256SUMS` for this
    // release (vendored in the installer tests) and each URL answered a ranged
    // GET at the time of writing.
    #[test]
    fn blake2b_provider_release_matrix() {
        assert_eq!(KNOTS_BLAKE2B_VERSION, "29.5.1.knots20260508");
        let base = "https://bitcoinknots.org/files/29.x/29.5.1.knots20260508";
        let expected = [
            (
                NodeOs::MacOs,
                NodeArch::Aarch64,
                "bitcoin-29.5.1.knots20260508-arm64-apple-darwin.tar.gz",
            ),
            (
                NodeOs::MacOs,
                NodeArch::X86_64,
                "bitcoin-29.5.1.knots20260508-x86_64-apple-darwin.tar.gz",
            ),
            (
                NodeOs::Linux,
                NodeArch::X86_64,
                "bitcoin-29.5.1.knots20260508-x86_64-linux-gnu.tar.gz",
            ),
            (
                NodeOs::Linux,
                NodeArch::Aarch64,
                "bitcoin-29.5.1.knots20260508-aarch64-linux-gnu.tar.gz",
            ),
            (
                NodeOs::Windows,
                NodeArch::X86_64,
                "bitcoin-29.5.1.knots20260508-win64-pgpverifiable.zip",
            ),
        ];
        for (os, arch, filename) in expected.iter() {
            assert_eq!(
                NodeFlavor::KnotsBlake2b.asset_filename(KNOTS_BLAKE2B_VERSION, *os, *arch),
                *filename
            );
            assert_eq!(
                NodeFlavor::KnotsBlake2b.asset_url(KNOTS_BLAKE2B_VERSION, *os, *arch),
                format!("{}/{}", base, filename)
            );
        }
        assert_eq!(
            NodeFlavor::KnotsBlake2b.manifest_urls(),
            Some((
                format!("{}/SHA256SUMS", base),
                format!("{}/SHA256SUMS.asc", base)
            ))
        );
        assert_eq!(
            NodeFlavor::KnotsBlake2b.display_name(),
            "Bitcoin Knots (Bitcoin Blake2b)"
        );
        // The pin is its own flavour, and the Bitcoin pins are untouched by it.
        assert_eq!(
            NodeFlavor::from_version(KNOTS_BLAKE2B_VERSION),
            NodeFlavor::KnotsBlake2b
        );
        assert_eq!(NodeFlavor::from_version(KNOTS_VERSION), NodeFlavor::Knots);
        assert_eq!(NodeFlavor::from_version(CORE_VERSION), NodeFlavor::Core);
        assert_eq!(KNOTS_VERSION, "29.3.knots20260507");
        assert_eq!(CORE_VERSION, "29.0");
        assert!(!KNOTS_VERSIONS.contains(&KNOTS_BLAKE2B_VERSION));
        assert!(!CORE_VERSIONS.contains(&KNOTS_BLAKE2B_VERSION));
        assert_eq!(NodeFlavor::KnotsBlake2b.versions(), &KNOTS_BLAKE2B_VERSIONS);
    }

    // The provider/chain pairing is refused in both directions, and each
    // family offers exactly its own providers.
    #[test]
    fn provider_chain_mismatch_is_refused_both_ways() {
        use crate::chain::ChainId;
        for chain in [
            ChainId::Bitcoin,
            ChainId::Testnet4,
            ChainId::Signet,
            ChainId::Regtest,
        ]
        .iter()
        .copied()
        {
            assert_eq!(NodeFlavor::Core.check_chain(chain), Ok(()));
            assert_eq!(NodeFlavor::Knots.check_chain(chain), Ok(()));
            assert_eq!(
                NodeFlavor::KnotsBlake2b.check_chain(chain),
                Err(ProviderChainMismatch {
                    flavor: NodeFlavor::KnotsBlake2b,
                    chain
                })
            );
            assert!(!NodeFlavor::KnotsBlake2b.supports_chain(chain));
        }
        for chain in [ChainId::BitcoinBlake2b, ChainId::BitcoinBlake2bTestnet4]
            .iter()
            .copied()
        {
            assert_eq!(NodeFlavor::KnotsBlake2b.check_chain(chain), Ok(()));
            for flavor in [NodeFlavor::Core, NodeFlavor::Knots].iter().copied() {
                assert_eq!(
                    flavor.check_chain(chain),
                    Err(ProviderChainMismatch { flavor, chain })
                );
                assert!(!flavor.supports_chain(chain));
            }
            assert_eq!(
                NodeChainFamily::from_chain(chain),
                NodeChainFamily::BitcoinBlake2b
            );
        }
        assert_eq!(
            NodeChainFamily::Bitcoin.allowed_flavors(),
            &[NodeFlavor::Core, NodeFlavor::Knots]
        );
        assert_eq!(
            NodeChainFamily::BitcoinBlake2b.allowed_flavors(),
            &[NodeFlavor::KnotsBlake2b]
        );
        // The refusal names both sides, so the message is actionable.
        let e = NodeFlavor::Knots
            .check_chain(ChainId::BitcoinBlake2b)
            .unwrap_err();
        assert_eq!(
            e.to_string(),
            "Bitcoin Knots cannot serve Bitcoin Blake2b: it is a Bitcoin node provider"
        );
        let e = NodeFlavor::KnotsBlake2b
            .check_chain(ChainId::Bitcoin)
            .unwrap_err();
        assert_eq!(
            e.to_string(),
            "Bitcoin Knots (Bitcoin Blake2b) cannot serve Bitcoin: \
             it is a Bitcoin Blake2b node provider"
        );
    }

    // Binary, datadir, config, cookie, ledger and lock locations are all keyed
    // by family, so a Bitcoin node and a Bitcoin Blake2b node — each on mainnet
    // and testnet4 — never share a path; and the Bitcoin family's paths are the
    // historical ones byte for byte.
    #[test]
    fn managed_node_paths_are_isolated_per_family_and_network() {
        use crate::node::revalidate::ManagedNodeState;
        let root = CoincubeDirectory::new(PathBuf::from("/tmp/coincube-test"));
        let mut seen: Vec<PathBuf> = Vec::new();
        let mut distinct = |p: PathBuf| {
            assert!(!seen.contains(&p), "path shared: {}", p.display());
            seen.push(p);
        };
        for family in NodeChainFamily::ALL.iter().copied() {
            let node_dir = internal_bitcoind_directory_for(&root, family);
            let datadir = internal_bitcoind_datadir_for(&root, family);
            assert_eq!(node_dir, root.path().join(family.root_dir_name()));
            assert_eq!(datadir, node_dir.join("datadir"));
            distinct(node_dir.clone());
            distinct(datadir.clone());
            distinct(internal_bitcoind_config_path(&datadir));
            distinct(ManagedNodeState::path_for(&root, family));
            // The lock directory is the same family root.
            assert_eq!(root.bitcoind_directory_for(family).path(), node_dir);
            for network in [Network::Bitcoin, Network::Testnet4].iter() {
                distinct(internal_bitcoind_cookie_path(&datadir, network));
            }
            for version in family
                .allowed_flavors()
                .iter()
                .flat_map(|f| f.versions().iter())
            {
                distinct(internal_bitcoind_exe_path_for(&root, family, version));
            }
        }
        // Legacy (family-less) helpers are the Bitcoin family's, unchanged.
        assert_eq!(
            internal_bitcoind_directory(&root),
            root.path().join("bitcoind")
        );
        assert_eq!(
            internal_bitcoind_datadir(&root),
            root.path().join("bitcoind").join("datadir")
        );
        assert_eq!(
            internal_bitcoind_exe_path(&root, KNOTS_VERSION),
            internal_bitcoind_exe_path_for(&root, NodeChainFamily::Bitcoin, KNOTS_VERSION)
        );
        assert_eq!(
            ManagedNodeState::path(&root),
            root.path().join("bitcoind").join("managed_node_state.json")
        );
        assert_eq!(
            internal_bitcoind_exe_path_for(
                &root,
                NodeChainFamily::BitcoinBlake2b,
                KNOTS_BLAKE2B_VERSION
            ),
            root.path()
                .join("bitcoind-blake2b")
                .join("bitcoin-29.5.1.knots20260508")
                .join("bin")
                .join(if cfg!(target_os = "windows") {
                    "bitcoind.exe"
                } else {
                    "bitcoind"
                })
        );
    }

    // A Bitcoin Blake2b config is an ordinary managed-node config: it never
    // emits `consensusrules`, and each family's file is separate from and
    // invisible to the other's. The ports here are hand-written stand-ins for
    // OS-allocated ones; what this test establishes is *file separation* and
    // that the reservation reader sees the other family's sections — not that
    // two allocations running at once cannot collide (see
    // `port_allocation_policy_is_bounded_and_fails_closed` and the doc).
    #[test]
    fn concurrent_bitcoin_and_blake2b_configs_keep_ports_and_files_apart() {
        let (base, root) = a_temp_coincube_datadir("ports");
        let section = |rpc_port: u16, p2p_port: u16| InternalBitcoindNetworkConfig {
            rpc_port,
            p2p_port,
            prune: PRUNE_DEFAULT,
            rpc_auth: None,
        };
        let fresh = InternalBitcoindConfig::new();
        // Nothing configured yet: nothing reserved either way.
        assert_eq!(
            reserved_managed_ports(&root, NodeChainFamily::Bitcoin, &fresh),
            Ok(vec![])
        );
        assert_eq!(
            reserved_managed_ports(&root, NodeChainFamily::BitcoinBlake2b, &fresh),
            Ok(vec![])
        );

        let mut bitcoin = InternalBitcoindConfig::for_flavor(NodeFlavor::Knots);
        bitcoin
            .networks
            .insert(Network::Bitcoin, section(43345, 42355));
        bitcoin
            .networks
            .insert(Network::Testnet4, section(34067, 45175));
        let bitcoin_conf = internal_bitcoind_config_path(&internal_bitcoind_datadir_for(
            &root,
            NodeChainFamily::Bitcoin,
        ));
        std::fs::create_dir_all(bitcoin_conf.parent().unwrap()).unwrap();
        bitcoin.to_file(&bitcoin_conf).unwrap();

        let mut blake2b = InternalBitcoindConfig::for_flavor(NodeFlavor::KnotsBlake2b);
        blake2b
            .networks
            .insert(Network::Bitcoin, section(51001, 51002));
        blake2b
            .networks
            .insert(Network::Testnet4, section(51003, 51004));
        let blake2b_conf = internal_bitcoind_config_path(&internal_bitcoind_datadir_for(
            &root,
            NodeChainFamily::BitcoinBlake2b,
        ));
        assert_ne!(bitcoin_conf, blake2b_conf);
        std::fs::create_dir_all(blake2b_conf.parent().unwrap()).unwrap();
        blake2b.to_file(&blake2b_conf).unwrap();

        // Neither file carries a consensus-rules key, whatever the flavour.
        for conf in [&bitcoin_conf, &blake2b_conf].iter() {
            let text = std::fs::read_to_string(conf).unwrap();
            assert!(
                !text.contains("consensusrules"),
                "{} must not emit consensusrules:\n{}",
                conf.display(),
                text
            );
        }
        assert!(blake2b
            .to_ini()
            .general_section()
            .get("consensusrules")
            .is_none());

        // Each family sees exactly the other's ports, on both networks (a
        // fresh conf of its own contributes nothing).
        let mut reserved_for_blake2b =
            reserved_managed_ports(&root, NodeChainFamily::BitcoinBlake2b, &fresh).unwrap();
        reserved_for_blake2b.sort_unstable();
        assert_eq!(reserved_for_blake2b, vec![34067, 42355, 43345, 45175]);
        let mut reserved_for_bitcoin =
            reserved_managed_ports(&root, NodeChainFamily::Bitcoin, &fresh).unwrap();
        reserved_for_bitcoin.sort_unstable();
        assert_eq!(reserved_for_bitcoin, vec![51001, 51002, 51003, 51004]);
        // The conf being edited reserves its own other sections too: adding a
        // third network to the Bitcoin file must avoid its mainnet and
        // testnet4 ports as well as everything Blake2b holds.
        let mut reserved_for_new_bitcoin_section =
            reserved_managed_ports(&root, NodeChainFamily::Bitcoin, &bitcoin).unwrap();
        reserved_for_new_bitcoin_section.sort_unstable();
        assert_eq!(
            reserved_for_new_bitcoin_section,
            vec![34067, 42355, 43345, 45175, 51001, 51002, 51003, 51004]
        );
        // And the two files' sets are disjoint, so all four (family, network)
        // endpoints are distinct.
        assert!(reserved_for_bitcoin
            .iter()
            .all(|p| !reserved_for_blake2b.contains(p)));

        // Writing one family's file did not touch the other's.
        let reread = InternalBitcoindConfig::from_file(&bitcoin_conf).unwrap();
        assert_eq!(reread.networks, bitcoin.networks);
        let reread = InternalBitcoindConfig::from_file(&blake2b_conf).unwrap();
        assert_eq!(reread.networks, blake2b.networks);

        let _ = std::fs::remove_dir_all(&base);
    }

    // The one allocation policy behind the installer and the settings path,
    // driven by a deterministic candidate source: a recorded-but-unbound
    // other-family port is refused, so are this file's other sections,
    // bitcoind defaults and repeats; the first two acceptable candidates are
    // taken in order; a source that never offers two acceptable ports is
    // bounded; and an unreadable other-family conf refuses allocation
    // outright rather than guessing. What it does *not* claim: that two
    // allocations racing each other cannot both pass — that synchronisation
    // is a pre-activation prerequisite, not part of this dormant slice.
    #[test]
    fn port_allocation_policy_is_bounded_and_fails_closed() {
        use std::cell::Cell;
        let source = |seq: &'static [u16]| {
            let i = Cell::new(0usize);
            move || -> Result<u16, String> {
                let n = i.get();
                i.set(n + 1);
                seq.get(n).copied().ok_or_else(|| "source dry".to_string())
            }
        };
        // Reserved (recorded elsewhere, not necessarily bound), a bitcoind
        // default, and a repeat are all skipped; the next two distinct
        // acceptable candidates win, in order.
        assert_eq!(
            allocate_managed_ports(
                &[43345, 51001],
                source(&[43345, 8332, 51001, 40001, 40001, 48333, 40002, 40003])
            ),
            Ok((40001, 40002))
        );
        // Nothing reserved: first two distinct non-default candidates.
        assert_eq!(
            allocate_managed_ports(&[], source(&[40010, 40010, 40011])),
            Ok((40010, 40011))
        );
        // A source that only ever repeats one port is bounded, not looped.
        let stuck = || -> Result<u16, String> { Ok(40020) };
        assert_eq!(
            allocate_managed_ports(&[], stuck),
            Err(PortAllocationError::Exhausted)
        );
        // A source failure is surfaced, not retried past the sequence.
        assert_eq!(
            allocate_managed_ports(&[], source(&[40030])),
            Err(PortAllocationError::Source("source dry".to_string()))
        );

        // Fail closed: an other-family conf that exists but cannot be parsed
        // makes the reservation set unknowable, so allocation is refused.
        let (base, root) = a_temp_coincube_datadir("ports-unreadable");
        let blake2b_conf = internal_bitcoind_config_path(&internal_bitcoind_datadir_for(
            &root,
            NodeChainFamily::BitcoinBlake2b,
        ));
        std::fs::create_dir_all(blake2b_conf.parent().unwrap()).unwrap();
        std::fs::write(&blake2b_conf, "[main]\nrpcport=notanumber\nport=1\n").unwrap();
        match reserved_managed_ports(
            &root,
            NodeChainFamily::Bitcoin,
            &InternalBitcoindConfig::new(),
        ) {
            Err(PortAllocationError::UnreadableConfig { path, .. }) => {
                assert_eq!(path, blake2b_conf)
            }
            other => panic!("expected UnreadableConfig, got {:?}", other),
        }
        // The Blake2b family, editing its own (unreadable-to-others) file, is
        // not blocked by it: only *other* families' confs are read here, and
        // its own is the caller's to refuse.
        assert_eq!(
            reserved_managed_ports(
                &root,
                NodeChainFamily::BitcoinBlake2b,
                &InternalBitcoindConfig::new()
            ),
            Ok(vec![])
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    // Binary resolution never crosses families: a Bitcoin provider ignores a
    // Blake2b binary even with nothing else installed, and the Blake2b provider
    // ignores every Bitcoin binary.
    #[test]
    fn binary_resolution_never_crosses_chain_families() {
        let (base, root) = a_temp_coincube_datadir("exe");
        let install = |family: NodeChainFamily, version: &str| {
            let path = internal_bitcoind_exe_path_for(&root, family, version);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, b"").unwrap();
            path
        };
        // Only the Blake2b binary on disk: no Bitcoin provider resolves it.
        let blake2b_exe = install(NodeChainFamily::BitcoinBlake2b, KNOTS_BLAKE2B_VERSION);
        assert_eq!(select_managed_bitcoind_exe(&root, NodeFlavor::Core), None);
        assert_eq!(select_managed_bitcoind_exe(&root, NodeFlavor::Knots), None);
        assert_eq!(
            select_managed_bitcoind_exe(&root, NodeFlavor::KnotsBlake2b),
            Some((blake2b_exe.clone(), NodeFlavor::KnotsBlake2b))
        );
        // A Bitcoin Knots binary: Bitcoin providers resolve it (Core by
        // fallback, as before); the Blake2b provider still does not.
        let knots_exe = install(NodeChainFamily::Bitcoin, KNOTS_VERSION);
        assert_eq!(
            select_managed_bitcoind_exe(&root, NodeFlavor::Knots),
            Some((knots_exe.clone(), NodeFlavor::Knots))
        );
        assert_eq!(
            select_managed_bitcoind_exe(&root, NodeFlavor::Core),
            Some((knots_exe, NodeFlavor::Knots))
        );
        assert_eq!(
            select_managed_bitcoind_exe(&root, NodeFlavor::KnotsBlake2b),
            Some((blake2b_exe.clone(), NodeFlavor::KnotsBlake2b))
        );
        // Remove the Blake2b binary: the Blake2b provider has no fallback.
        std::fs::remove_file(&blake2b_exe).unwrap();
        assert_eq!(
            select_managed_bitcoind_exe(&root, NodeFlavor::KnotsBlake2b),
            None
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    // The start path refuses a chain/provider mismatch before it writes the
    // identity marker, migrates the conf or resolves a binary — and the
    // chain-aware entry refuses a dormant chain before it even reads the ledger.
    #[test]
    fn start_refuses_provider_and_family_mismatch_before_touching_the_datadir() {
        use crate::chain::ChainId;
        use crate::node::revalidate::ManagedNodeState;
        let (base, root) = a_temp_coincube_datadir("start");
        let bitcoin_datadir = internal_bitcoind_datadir(&root);
        let cookie_path = internal_bitcoind_cookie_path(&bitcoin_datadir, &Network::Bitcoin);
        let config = BitcoindConfig {
            rpc_auth: coincubed::config::BitcoindRpcAuth::CookieFile(cookie_path.clone()),
            addr: std::net::SocketAddr::from(([127, 0, 0, 1], 1)),
        };
        let untouched = |root: &CoincubeDirectory| {
            assert!(
                !cookie_path.parent().unwrap().exists(),
                "the Bitcoin node datadir was created"
            );
            assert!(
                !internal_bitcoind_directory_for(root, NodeChainFamily::BitcoinBlake2b).exists(),
                "the Bitcoin Blake2b root was created"
            );
        };

        // A dormant chain: refused with the dormant reason, nothing created.
        for chain in [ChainId::BitcoinBlake2b, ChainId::BitcoinBlake2bTestnet4]
            .iter()
            .copied()
        {
            match Bitcoind::maybe_start_for_chain(chain, config.clone(), &root) {
                Err(StartInternalBitcoindError::ChainUnavailable(reason)) => {
                    assert_eq!(reason, crate::chain::BTCB2_DORMANT_REASON);
                }
                other => panic!("expected ChainUnavailable, got {:?}", other.map(|_| ())),
            }
            untouched(&root);
        }

        // A Bitcoin chain whose ledger names the Blake2b provider: refused as a
        // mismatch, before the identity marker's directory exists.
        let ledger = ManagedNodeState {
            configured_flavor: Some(NodeFlavor::KnotsBlake2b),
            ..Default::default()
        };
        ledger.save(&root).unwrap();
        assert_eq!(
            configured_managed_flavor(&root),
            Some(NodeFlavor::KnotsBlake2b)
        );
        let ledger_bytes = std::fs::read(ManagedNodeState::path(&root)).unwrap();
        match Bitcoind::maybe_start(Network::Bitcoin, config.clone(), &root) {
            Err(StartInternalBitcoindError::ProviderChainMismatch(e)) => assert_eq!(
                e,
                ProviderChainMismatch {
                    flavor: NodeFlavor::KnotsBlake2b,
                    chain: ChainId::Bitcoin
                }
            ),
            other => panic!(
                "expected ProviderChainMismatch, got {:?}",
                other.map(|_| ())
            ),
        }
        match Bitcoind::maybe_start_for_chain(ChainId::Bitcoin, config.clone(), &root) {
            Err(StartInternalBitcoindError::ProviderChainMismatch(_)) => {}
            other => panic!(
                "expected ProviderChainMismatch, got {:?}",
                other.map(|_| ())
            ),
        }
        untouched(&root);
        assert_eq!(
            std::fs::read(ManagedNodeState::path(&root)).unwrap(),
            ledger_bytes,
            "the ledger was rewritten by a refused start"
        );

        // The same start with a matching provider gets past the guard: it
        // establishes the identity and then fails on the (absent) binary, which
        // is the ordinary Bitcoin path this build ships.
        ManagedNodeState {
            configured_flavor: Some(NodeFlavor::Knots),
            ..Default::default()
        }
        .save(&root)
        .unwrap();
        match Bitcoind::maybe_start_for_chain(ChainId::Bitcoin, config, &root) {
            Err(StartInternalBitcoindError::ExecutableNotFound) => {}
            other => panic!("expected ExecutableNotFound, got {:?}", other.map(|_| ())),
        }
        assert!(cookie_path.parent().unwrap().exists());
        assert!(!internal_bitcoind_directory_for(&root, NodeChainFamily::BitcoinBlake2b).exists());

        let _ = std::fs::remove_dir_all(&base);
    }

    // A conf still carrying the legacy `consensusrules` line — what v1.0.1-rc1
    // wrote for every managed Knots node — reaches `maybe_start` and is
    // rewritten without the line before the binary is resolved, which is what
    // the one route with no Tor preparation ahead of it (the loader's pending
    // node) relies on. The spawn itself is refused past that point on the
    // absent binary, as the other start tests do. A busy conf lock refuses the
    // start in the Tor preparation's class (`ConfigUnavailable`) with the file
    // untouched; a file already in canonical form is rewritten byte-identically;
    // no conf is created where there was none.
    #[test]
    fn start_strips_a_legacy_consensusrules_line_before_the_spawn() {
        use crate::node::managed_conf::{with_quick_lock_bound, ManagedConfLock};
        use crate::node::revalidate::ManagedNodeState;
        let (base, root) = a_temp_coincube_datadir("strip");
        let bitcoin_datadir = internal_bitcoind_datadir(&root);
        let conf_path = internal_bitcoind_config_path(&bitcoin_datadir);
        let cookie_path = internal_bitcoind_cookie_path(&bitcoin_datadir, &Network::Bitcoin);
        let config = BitcoindConfig {
            rpc_auth: coincubed::config::BitcoindRpcAuth::CookieFile(cookie_path),
            addr: std::net::SocketAddr::from(([127, 0, 0, 1], 1)),
        };
        ManagedNodeState {
            configured_flavor: Some(NodeFlavor::Knots),
            ..Default::default()
        }
        .save(&root)
        .unwrap();
        let expect_absent_binary =
            |result: Result<Bitcoind, StartInternalBitcoindError>| match result {
                Err(StartInternalBitcoindError::ExecutableNotFound) => {}
                other => panic!("expected ExecutableNotFound, got {:?}", other.map(|_| ())),
            };

        // No conf: the start gets as far as the binary without inventing one.
        expect_absent_binary(Bitcoind::maybe_start(
            Network::Bitcoin,
            config.clone(),
            &root,
        ));
        assert!(!conf_path.exists(), "a start invented a conf");

        // The legacy file, as an older release left it.
        std::fs::create_dir_all(&bitcoin_datadir).unwrap();
        let legacy = "consensusrules=rdts\n[main]\nrpcport=12345\nport=12346\nprune=15000\n";
        std::fs::write(&conf_path, legacy).unwrap();

        // Busy lock: refused in the Tor preparation's class, file untouched.
        let held = ManagedConfLock::acquire(&root).unwrap();
        let refused = with_quick_lock_bound(|| {
            Bitcoind::maybe_start(Network::Bitcoin, config.clone(), &root)
        });
        drop(held);
        match refused {
            Err(StartInternalBitcoindError::ConfigUnavailable(e)) => {
                assert!(e.contains("another setup is updating"), "{}", e)
            }
            other => panic!("expected ConfigUnavailable, got {:?}", other.map(|_| ())),
        }
        assert_eq!(std::fs::read_to_string(&conf_path).unwrap(), legacy);

        // Lock free: the line is gone from disk before the binary is looked
        // for, and the rest of the file survives the rewrite.
        expect_absent_binary(Bitcoind::maybe_start(
            Network::Bitcoin,
            config.clone(),
            &root,
        ));
        let rewritten = std::fs::read_to_string(&conf_path).unwrap();
        assert!(!rewritten.contains("consensusrules"), "{}", rewritten);
        assert!(!rewritten.contains("datacarriersize"), "{}", rewritten);
        // Inbound-over-Tor is off here, and the rewrite now says so (#598).
        assert!(rewritten.contains("listenonion=0"), "{}", rewritten);
        let reloaded = InternalBitcoindConfig::from_file(&conf_path).unwrap();
        let main = &reloaded.networks[&Network::Bitcoin];
        assert_eq!(
            (main.rpc_port, main.p2p_port, main.prune),
            (12345, 12346, 15000)
        );

        // Canonical already: the next start's rewrite is byte-identical.
        expect_absent_binary(Bitcoind::maybe_start(Network::Bitcoin, config, &root));
        assert_eq!(std::fs::read_to_string(&conf_path).unwrap(), rewritten);

        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn sole_owner_restart_reaches_replacement_spawn_but_second_owner_cannot() {
        use std::io::{Read, Write};
        use std::os::unix::fs::PermissionsExt;

        let (base, root) = a_temp_coincube_datadir("exclusive-restart");
        std::fs::create_dir_all(internal_bitcoind_datadir(&root)).unwrap();
        let exe = internal_bitcoind_exe_path(&root, CORE_VERSION);
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        let spawned = exe.parent().unwrap().join("restart-spawned");
        std::fs::write(
            &exe,
            format!(
                "#!/bin/sh\nprintf 'spawned\\n' > '{}'\nexec sleep 30\n",
                spawned.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let config = BitcoindConfig {
            rpc_auth: coincubed::config::BitcoindRpcAuth::UserPass("user".into(), "pass".into()),
            addr: listener.local_addr().unwrap(),
        };
        let first = Bitcoind::for_test(config.clone(), &root, Network::Bitcoin);
        let other = Bitcoind::for_test(config.clone(), &root, Network::Bitcoin);
        let owner_path = first.owner_lock_path().unwrap().to_path_buf();
        let lifecycle = ManagedNodeLifecycle::acquire(&root, NodeChainFamily::Bitcoin).unwrap();
        let refused = Bitcoind::maybe_restart_locked(
            Network::Bitcoin,
            config.clone(),
            &root,
            &lifecycle,
            Some(&owner_path),
        );
        assert!(matches!(
            refused,
            Err(StartInternalBitcoindError::ConfigUnavailable(_))
        ));
        assert!(!spawned.exists());
        drop(lifecycle);
        drop(other);
        assert!(wait_for_managed_node_cleanup(time::Duration::from_secs(
            120
        )));

        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let server = {
            let done = done.clone();
            let spawned = spawned.clone();
            std::thread::spawn(move || {
                for (n, stream) in listener.incoming().enumerate() {
                    if done.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(mut stream) = stream else { break };
                    let _ = stream.set_read_timeout(Some(time::Duration::from_millis(200)));
                    let _ = stream.read(&mut [0u8; 4096]);
                    if n > 0 {
                        let deadline = time::Instant::now() + time::Duration::from_secs(20);
                        while !spawned.exists() && time::Instant::now() < deadline {
                            thread::sleep(time::Duration::from_millis(20));
                        }
                    }
                    let _ = stream.write_all(
                        b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                    let _ = stream.flush();
                }
            })
        };
        let lifecycle = ManagedNodeLifecycle::acquire(&root, NodeChainFamily::Bitcoin).unwrap();
        let result = Bitcoind::maybe_restart_locked(
            Network::Bitcoin,
            config.clone(),
            &root,
            &lifecycle,
            Some(&owner_path),
        );
        assert!(matches!(
            result,
            Err(StartInternalBitcoindError::BitcoinDError(_))
        ));
        assert!(spawned.exists(), "the replacement binary was never spawned");
        drop(lifecycle);
        done.store(true, Ordering::SeqCst);
        let _ = std::net::TcpStream::connect(config.addr);
        server.join().unwrap();
        drop(first);
        assert!(wait_for_managed_node_cleanup(time::Duration::from_secs(
            120
        )));
        let _ = std::fs::remove_dir_all(base);
    }

    // #605: a Knots binary is spawned with `-torexecute=0`, so losing its Tor
    // control port never makes it launch a `tor` of its own; a Core binary is
    // spawned without it, because Core refuses an unknown command-line option.
    // The argument follows the binary actually launched, not the configured
    // flavour, so both fallbacks are covered.
    //
    // Goes through `Bitcoind::maybe_start`'s own spawn: the managed binary is a
    // script that records its argv and then waits. The RPC endpoint is a
    // listener that answers every request with HTTP 403 — a non-transient
    // error — once that record exists, so the start kills the child and
    // returns instead of waiting for a node that will never come up.
    #[cfg(unix)]
    #[test]
    fn the_spawned_binary_gets_torexecute_0_exactly_when_it_is_knots() {
        use crate::node::revalidate::ManagedNodeState;
        use std::os::unix::fs::PermissionsExt;

        struct Case {
            configured: NodeFlavor,
            installed: &'static str,
            expect_torexecute: bool,
        }
        let cases = [
            Case {
                configured: NodeFlavor::Knots,
                installed: KNOTS_VERSION,
                expect_torexecute: true,
            },
            // Fallback: Knots configured, only Core installed.
            Case {
                configured: NodeFlavor::Knots,
                installed: CORE_VERSION,
                expect_torexecute: false,
            },
            Case {
                configured: NodeFlavor::Core,
                installed: CORE_VERSION,
                expect_torexecute: false,
            },
            // Fallback: Core configured, only Knots installed.
            Case {
                configured: NodeFlavor::Core,
                installed: KNOTS_VERSION,
                expect_torexecute: true,
            },
        ];

        for (i, case) in cases.iter().enumerate() {
            let (base, root) = a_temp_coincube_datadir(&format!("spawn-args-{}", i));
            std::fs::create_dir_all(internal_bitcoind_datadir(&root)).unwrap();
            ManagedNodeState {
                configured_flavor: Some(case.configured),
                ..Default::default()
            }
            .save(&root)
            .unwrap();

            let exe = internal_bitcoind_exe_path(&root, case.installed);
            let bin_dir = exe.parent().unwrap().to_path_buf();
            std::fs::create_dir_all(&bin_dir).unwrap();
            let argv_path = bin_dir.join("argv");
            let argv_tmp = bin_dir.join("argv.tmp");
            std::fs::write(
                &exe,
                format!(
                    "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}' && mv '{}' '{}'\nexec sleep 30\n",
                    argv_tmp.display(),
                    argv_tmp.display(),
                    argv_path.display()
                ),
            )
            .unwrap();
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();

            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let server = {
                let done = done.clone();
                let argv_path = argv_path.clone();
                std::thread::spawn(move || serve_spawn_args_probe(listener, &argv_path, &done))
            };
            let config = BitcoindConfig {
                rpc_auth: coincubed::config::BitcoindRpcAuth::UserPass(
                    "user".to_string(),
                    "pass".to_string(),
                ),
                addr,
            };

            match Bitcoind::maybe_start(Network::Bitcoin, config, &root) {
                Err(StartInternalBitcoindError::BitcoinDError(_)) => {}
                other => panic!(
                    "case {i}: expected BitcoinDError from the stand-in endpoint, got {:?}",
                    other.map(|_| ())
                ),
            }
            done.store(true, std::sync::atomic::Ordering::SeqCst);
            let _ = std::net::TcpStream::connect(addr);
            server.join().unwrap();

            let argv: Vec<String> = std::fs::read_to_string(&argv_path)
                .unwrap_or_else(|e| panic!("case {}: the binary was not spawned: {}", i, e))
                .lines()
                .map(str::to_string)
                .collect();
            assert!(
                argv.iter().any(|a| a == "-chain=main"),
                "case {}: {:?}",
                i,
                argv
            );
            assert!(
                argv.iter().any(|a| a.starts_with("-datadir=")),
                "case {}: {:?}",
                i,
                argv
            );
            let torexecute: Vec<&String> = argv
                .iter()
                .filter(|a| a.starts_with("-torexecute"))
                .collect();
            if case.expect_torexecute {
                assert_eq!(torexecute, ["-torexecute=0"], "case {}: {:?}", i, argv);
            } else {
                assert!(torexecute.is_empty(), "case {}: {:?}", i, argv);
            }
            let _ = std::fs::remove_dir_all(&base);
        }
    }

    #[cfg(unix)]
    fn serve_spawn_args_probe(
        listener: std::net::TcpListener,
        argv_path: &Path,
        done: &std::sync::atomic::AtomicBool,
    ) {
        use std::io::{Read, Write};

        for (n, stream) in listener.incoming().enumerate() {
            if done.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            let Ok(mut stream) = stream else { break };
            let _ = stream.set_read_timeout(Some(time::Duration::from_millis(200)));
            let _ = stream.read(&mut [0u8; 4096]);
            // The first request is the pre-spawn "is a node already
            // running?" check: answered at once. Every later one is
            // held until the spawned script has recorded its argv,
            // so the start cannot kill it first. Bounded, and a
            // second pre-spawn request would only cost its own 3 s
            // client timeout.
            let wait = if n == 0 { 0 } else { 20 };
            let deadline = time::Instant::now() + time::Duration::from_secs(wait);
            while !argv_path.exists() && time::Instant::now() < deadline {
                thread::sleep(time::Duration::from_millis(20));
            }
            let _ = stream.write_all(
                b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
            let _ = stream.flush();
        }
    }

    // #598: with `listenonion` unset, bitcoind listens and so defaults it on,
    // and Knots 29.3 then launches `tor` itself when no control port answers —
    // the subprocess path whose failed exec closes a descriptor twice (#394).
    // The pre-spawn rewrite every start takes must therefore leave the file
    // with exactly one `listenonion` line matching the user's choice: `0` for
    // a file that predates the key (inbound off), and the inbound-on file's
    // `1` kept as it was, with its companion keys, and no `0` beside it.
    #[test]
    fn start_writes_listenonion_for_both_inbound_states_before_the_spawn() {
        use crate::node::revalidate::ManagedNodeState;
        let (base, root) = a_temp_coincube_datadir("listenonion");
        let bitcoin_datadir = internal_bitcoind_datadir(&root);
        let conf_path = internal_bitcoind_config_path(&bitcoin_datadir);
        let cookie_path = internal_bitcoind_cookie_path(&bitcoin_datadir, &Network::Bitcoin);
        let config = BitcoindConfig {
            rpc_auth: coincubed::config::BitcoindRpcAuth::CookieFile(cookie_path),
            addr: std::net::SocketAddr::from(([127, 0, 0, 1], 1)),
        };
        ManagedNodeState {
            configured_flavor: Some(NodeFlavor::Knots),
            ..Default::default()
        }
        .save(&root)
        .unwrap();
        std::fs::create_dir_all(&bitcoin_datadir).unwrap();
        let start = || match Bitcoind::maybe_start(Network::Bitcoin, config.clone(), &root) {
            Err(StartInternalBitcoindError::ExecutableNotFound) => {}
            other => panic!("expected ExecutableNotFound, got {:?}", other.map(|_| ())),
        };
        let listenonion_lines = |text: &str| -> Vec<String> {
            text.lines()
                .filter(|l| l.trim_start().starts_with("listenonion"))
                .map(str::to_string)
                .collect()
        };

        // An existing operator-set relay cap survives unrelated Tor updates.
        std::fs::write(
            &conf_path,
            "maxmempool=300\ndatacarriersize=100\n[main]\nrpcport=12345\nport=12346\nprune=15000\n",
        )
        .unwrap();
        start();
        let off = std::fs::read_to_string(&conf_path).unwrap();
        assert_eq!(listenonion_lines(&off), vec!["listenonion=0"], "{}", off);
        assert!(off.contains("datacarriersize=100"), "{}", off);
        assert!(!off.contains("torcontrol"), "{}", off);
        assert!(
            !InternalBitcoindConfig::from_file(&conf_path)
                .unwrap()
                .inbound_tor
        );

        // Inbound on, as `prepare_inbound_tor` leaves it with Tor up.
        std::fs::write(
            &conf_path,
            "listen=1\nlistenonion=1\ndiscover=0\ntorcontrol=127.0.0.1:9151\n\
             datacarriersize=100\n[main]\nrpcport=12345\nport=12346\nprune=15000\n",
        )
        .unwrap();
        start();
        let on = std::fs::read_to_string(&conf_path).unwrap();
        assert_eq!(listenonion_lines(&on), vec!["listenonion=1"], "{}", on);
        assert!(on.contains("listen=1"), "{}", on);
        assert!(on.contains("discover=0"), "{}", on);
        assert!(on.contains("torcontrol=127.0.0.1:9151"), "{}", on);
        assert!(on.contains("datacarriersize=100"), "{}", on);
        let reloaded = InternalBitcoindConfig::from_file(&conf_path).unwrap();
        assert!(reloaded.inbound_tor);
        assert_eq!(reloaded.tor_control_port, Some(9151));

        let _ = std::fs::remove_dir_all(&base);
    }

    /// A stand-in for a managed node that is already up on the endpoint:
    /// answers `echo` on both the node and the wallet URL, `getnetworkinfo`
    /// with the given `subversion`, and `stop` — after which it closes its
    /// listener, so the shutdown wait sees the endpoint go away as it would
    /// for a real node. Records every method it was asked, in order. One
    /// request per connection, which is how the client talks to it.
    struct MockNode {
        addr: std::net::SocketAddr,
        methods: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    }

    impl MockNode {
        fn serve(subversion: &'static str) -> Self {
            use std::io::Write;
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let methods = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let seen = methods.clone();
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else { break };
                    let Some((buf, start)) = Self::read_request(&mut stream) else {
                        continue;
                    };
                    let req: serde_json::Value =
                        serde_json::from_slice(&buf[start..]).unwrap_or(serde_json::Value::Null);
                    let method = req
                        .get("method")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let id = req.get("id").cloned().unwrap_or(serde_json::Value::Null);
                    seen.lock().unwrap().push(method.clone());
                    let result = match method.as_str() {
                        "echo" => serde_json::json!([]),
                        "getnetworkinfo" => {
                            serde_json::json!({ "version": 290000, "subversion": subversion })
                        }
                        "stop" => serde_json::json!("stopping"),
                        _ => serde_json::Value::Null,
                    };
                    let body = serde_json::json!({ "result": result, "error": null, "id": id })
                        .to_string();
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                         Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.flush();
                    let _ = stream.shutdown(std::net::Shutdown::Write);
                    if method == "stop" {
                        break;
                    }
                }
                // The listener drops here: every later connection is refused,
                // which is what a stopped node looks like to the shutdown wait.
            });
            Self { addr, methods }
        }

        fn read_request(stream: &mut std::net::TcpStream) -> Option<(Vec<u8>, usize)> {
            use std::io::Read;

            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            let (body_start, body_len) = loop {
                let n = stream.read(&mut chunk).unwrap_or(0);
                if n == 0 {
                    break (None, 0);
                }
                buf.extend_from_slice(&chunk[..n]);
                if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&buf[..pos]).into_owned();
                    let len = head
                        .lines()
                        .filter_map(|l| l.split_once(':'))
                        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                        .and_then(|(_, v)| v.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    break (Some(pos + 4), len);
                }
            };
            let start = body_start?;
            while buf.len() < start + body_len {
                let n = stream.read(&mut chunk).unwrap_or(0);
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            Some((buf, start))
        }

        fn methods(&self) -> Vec<String> {
            self.methods.lock().unwrap().clone()
        }
    }

    // A flavour switch (the ledger names Knots, a Core node is up on the
    // endpoint) settles the conf before it tells the running node to stop: with
    // the conf lock held the start is refused as `ConfigUnavailable`, the node
    // has not been asked to stop, is still reachable, and the file is
    // byte-identical; with the lock free the file is rewritten and the start
    // fails on the absent binary as the other start tests do, without
    // stopping the running node it had nothing to replace with.
    #[test]
    fn a_flavour_switch_settles_the_conf_before_stopping_the_running_node() {
        use crate::node::managed_conf::{with_quick_lock_bound, ManagedConfLock};
        use crate::node::revalidate::ManagedNodeState;
        let (base, root) = a_temp_coincube_datadir("switch");
        let bitcoin_datadir = internal_bitcoind_datadir(&root);
        let conf_path = internal_bitcoind_config_path(&bitcoin_datadir);
        let cookie_path = internal_bitcoind_cookie_path(&bitcoin_datadir, &Network::Bitcoin);
        std::fs::create_dir_all(cookie_path.parent().unwrap()).unwrap();
        std::fs::create_dir_all(&bitcoin_datadir).unwrap();
        std::fs::write(&cookie_path, "__cookie__:not-a-secret").unwrap();
        let legacy = "consensusrules=rdts\n[main]\nrpcport=12345\nport=12346\nprune=15000\n";
        std::fs::write(&conf_path, legacy).unwrap();
        ManagedNodeState {
            configured_flavor: Some(NodeFlavor::Knots),
            ..Default::default()
        }
        .save(&root)
        .unwrap();
        let node = MockNode::serve("/Satoshi:29.0.0/");
        let config = BitcoindConfig {
            rpc_auth: coincubed::config::BitcoindRpcAuth::CookieFile(cookie_path),
            addr: node.addr,
        };

        // Lock held: refused before the running node is told to stop.
        let held = ManagedConfLock::acquire(&root).unwrap();
        let refused = with_quick_lock_bound(|| {
            Bitcoind::maybe_start(Network::Bitcoin, config.clone(), &root)
        });
        drop(held);
        match refused {
            Err(StartInternalBitcoindError::ConfigUnavailable(e)) => {
                assert!(e.contains("another setup is updating"), "{}", e)
            }
            other => panic!("expected ConfigUnavailable, got {:?}", other.map(|_| ())),
        }
        let methods = node.methods();
        assert!(
            !methods.iter().any(|m| m == "stop"),
            "the running node was stopped on a refused start: {:?}",
            methods
        );
        assert!(
            methods.iter().any(|m| m == "getnetworkinfo"),
            "{:?}",
            methods
        );
        assert_eq!(std::fs::read_to_string(&conf_path).unwrap(), legacy);
        // ... and it is still there to be reused or switched on the retry.
        assert!(
            coincubed::BitcoinD::new(&config, "liveness".to_string()).is_ok(),
            "the running node went away on a refused start"
        );

        // Lock free: the conf is settled, and with no binary to launch in its
        // place the running node is left up rather than stopped with nothing
        // started (the replacement is resolved before anything is stopped).
        match Bitcoind::maybe_start(Network::Bitcoin, config.clone(), &root) {
            Err(StartInternalBitcoindError::ExecutableNotFound) => {}
            other => panic!("expected ExecutableNotFound, got {:?}", other.map(|_| ())),
        }
        let methods = node.methods();
        assert_eq!(
            methods.iter().filter(|m| *m == "stop").count(),
            0,
            "{:?}",
            methods
        );
        assert!(
            coincubed::BitcoinD::new(&config, "liveness".to_string()).is_ok(),
            "the running node was stopped with no replacement to start"
        );
        let rewritten = std::fs::read_to_string(&conf_path).unwrap();
        assert!(!rewritten.contains("consensusrules"), "{}", rewritten);

        let _ = std::fs::remove_dir_all(&base);
    }

    /// Makes `dir` non-traversable for the duration and restores it on drop
    /// (including on a panic), so a failing assertion never leaves a fixture
    /// behind that `remove_dir_all` cannot delete.
    #[cfg(unix)]
    struct Untraversable(PathBuf);

    #[cfg(unix)]
    impl Untraversable {
        fn new(dir: &Path) -> Self {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o000)).unwrap();
            Self(dir.to_path_buf())
        }
    }

    #[cfg(unix)]
    impl Drop for Untraversable {
        fn drop(&mut self) {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
        }
    }

    // A conf that is genuinely absent is `FileNotFound`; one that exists but
    // cannot be seen is not. `Path::exists` answers "false" to both, which
    // would let the reservation reader skip a stopped node's ports because a
    // directory happened to be non-traversable. Both the reader and the
    // own-family callers key on the distinction.
    #[test]
    fn from_file_distinguishes_absence_from_unreadable_metadata() {
        let (base, root) = a_temp_coincube_datadir("metadata");
        // Genuinely absent: no file, and no parent directory either.
        let missing = root.path().join("nowhere").join("bitcoin.conf");
        assert_eq!(
            InternalBitcoindConfig::from_file(&missing).map(|_| ()),
            Err(InternalBitcoindConfigError::FileNotFound)
        );
        let fresh = InternalBitcoindConfig::new();
        assert_eq!(
            reserved_managed_ports(&root, NodeChainFamily::Bitcoin, &fresh),
            Ok(vec![])
        );

        #[cfg(unix)]
        {
            // An other-family conf under a directory this process may not
            // traverse.
            let blake2b_datadir =
                internal_bitcoind_datadir_for(&root, NodeChainFamily::BitcoinBlake2b);
            let blake2b_conf = internal_bitcoind_config_path(&blake2b_datadir);
            std::fs::create_dir_all(&blake2b_datadir).unwrap();
            let mut conf = InternalBitcoindConfig::for_flavor(NodeFlavor::KnotsBlake2b);
            conf.networks.insert(
                Network::Bitcoin,
                InternalBitcoindNetworkConfig {
                    rpc_port: 51001,
                    p2p_port: 51002,
                    prune: PRUNE_DEFAULT,
                    rpc_auth: None,
                },
            );
            conf.to_file(&blake2b_conf).unwrap();
            let original = std::fs::read(&blake2b_conf).unwrap();
            {
                let _guard = Untraversable::new(&blake2b_datadir);
                if std::fs::metadata(&blake2b_conf).is_ok() {
                    // Privileged process: permissions do not apply, so the
                    // precondition cannot be produced here.
                    eprintln!(
                        "skipping the permission fixture: metadata succeeds despite mode 000"
                    );
                } else {
                    // The precondition the reader must not misread.
                    assert!(!blake2b_conf.exists());
                    let err = InternalBitcoindConfig::from_file(&blake2b_conf).unwrap_err();
                    match &err {
                        InternalBitcoindConfigError::ReadingFile(e) => {
                            assert!(e.to_lowercase().contains("permission denied"), "{}", e)
                        }
                        other => panic!("expected ReadingFile, got {:?}", other),
                    }
                    // Not absence: the reader refuses rather than reserving nothing.
                    match reserved_managed_ports(&root, NodeChainFamily::Bitcoin, &fresh) {
                        Err(PortAllocationError::UnreadableConfig { path, error }) => {
                            assert_eq!(path, blake2b_conf);
                            assert!(error.to_lowercase().contains("permission denied"));
                        }
                        other => panic!("expected UnreadableConfig, got {:?}", other),
                    }
                }
            }
            // Permissions restored: the file is intact and readable again.
            assert_eq!(std::fs::read(&blake2b_conf).unwrap(), original);
            assert_eq!(
                reserved_managed_ports(&root, NodeChainFamily::Bitcoin, &fresh),
                Ok(vec![51001, 51002])
            );
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    // A conf's Tor control/SOCKS ports are reserved like its node ports, so
    // the other family's allocation never lands on them.
    #[test]
    fn recorded_tor_ports_are_reserved_too() {
        let (base, root) = a_temp_coincube_datadir("tor-ports");
        let mut bitcoin = InternalBitcoindConfig::for_flavor(NodeFlavor::Knots);
        bitcoin.networks.insert(
            Network::Bitcoin,
            InternalBitcoindNetworkConfig {
                rpc_port: 43345,
                p2p_port: 42355,
                prune: PRUNE_DEFAULT,
                rpc_auth: None,
            },
        );
        bitcoin.inbound_tor = true;
        // `proxy=` (the SOCKS port) is only written when outbound-via-Tor is
        // on; `torcontrol=` whenever inbound is. Both on, so both round-trip.
        bitcoin.outbound_via_tor = true;
        bitcoin.tor_control_port = Some(45001);
        bitcoin.tor_socks_port = Some(45002);
        let mut ports = bitcoin.ports_in_use();
        ports.sort_unstable();
        assert_eq!(ports, vec![42355, 43345, 45001, 45002]);
        let bitcoin_conf = internal_bitcoind_config_path(&internal_bitcoind_datadir_for(
            &root,
            NodeChainFamily::Bitcoin,
        ));
        std::fs::create_dir_all(bitcoin_conf.parent().unwrap()).unwrap();
        bitcoin.to_file(&bitcoin_conf).unwrap();
        // Round-trips through the file …
        let mut reserved = reserved_managed_ports(
            &root,
            NodeChainFamily::BitcoinBlake2b,
            &InternalBitcoindConfig::new(),
        )
        .unwrap();
        reserved.sort_unstable();
        assert_eq!(reserved, vec![42355, 43345, 45001, 45002]);
        // … and the allocator skips them.
        let mut seq = vec![45001u16, 45002, 46001, 46002].into_iter();
        assert_eq!(
            allocate_managed_ports(&reserved, || seq.next().ok_or("dry")),
            Ok((46001, 46002))
        );
        let _ = std::fs::remove_dir_all(&base);
    }
}

#[cfg(all(test, unix))]
mod local_fork_smoke {
    use super::*;
    #[test]
    #[ignore = "requires COINCUBE_BLAKE2B_SMOKE_EXE pointing to the verified pinned release"]
    fn blake2b_real_managed_node_starts_without_touching_bitcoin() {
        use std::os::unix::fs::PermissionsExt;
        let binary = std::env::var("COINCUBE_BLAKE2B_SMOKE_EXE").expect("verified release path");
        let base =
            std::env::temp_dir().join(format!("blake2b-managed-smoke-{}", std::process::id()));
        let root = CoincubeDirectory::new(base.clone());
        let family = NodeChainFamily::BitcoinBlake2b;
        let datadir = internal_bitcoind_datadir_for(&root, family);
        std::fs::create_dir_all(&datadir).unwrap();
        let bitcoin = internal_bitcoind_datadir(&root);
        std::fs::create_dir_all(&bitcoin).unwrap();
        std::fs::write(bitcoin.join("bitcoin.conf"), b"Bitcoin sentinel").unwrap();
        let mut conf = InternalBitcoindConfig::for_flavor(NodeFlavor::KnotsBlake2b);
        let rpc_port = crate::installer::step::node::bitcoind::get_available_port().unwrap();
        let p2p_port = crate::installer::step::node::bitcoind::get_available_port().unwrap();
        conf.networks.insert(
            Network::Testnet4,
            InternalBitcoindNetworkConfig {
                rpc_port,
                p2p_port,
                prune: 550,
                rpc_auth: None,
            },
        );
        conf.to_ini()
            .write_to_file(internal_bitcoind_config_path(&datadir))
            .unwrap();
        crate::node::revalidate::ManagedNodeState::record_configured(
            &root,
            NodeFlavor::KnotsBlake2b,
        );
        let exe = internal_bitcoind_exe_path_for(&root, family, NodeFlavor::KnotsBlake2b.version());
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        // Exercise the production spawn path while disabling peer discovery and
        // connections in this disposable test. Never use a live node datadir.
        assert!(!binary.contains('\''));
        std::fs::write(
            &exe,
            format!(
                "#!/bin/sh\nexec '{}' -connect=0 -dnsseed=0 \"$@\"\n",
                binary
            ),
        )
        .unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o700)).unwrap();
        let cfg = BitcoindConfig {
            addr: std::net::SocketAddr::from(([127, 0, 0, 1], rpc_port)),
            rpc_auth: coincubed::config::BitcoindRpcAuth::CookieFile(
                internal_bitcoind_cookie_path(&datadir, &Network::Testnet4),
            ),
        };
        crate::node::tor::prepare_inbound_tor_for_chain(
            &root,
            crate::chain::ChainId::BitcoinBlake2bTestnet4,
        )
        .unwrap();
        // This isolated smoke fixture exercises the authenticated local-fork
        // node route, whose caller has already admitted the chain.
        let lifecycle = ManagedNodeLifecycle::acquire(&root, family).unwrap();
        let node = Bitcoind::maybe_start_for_chain_locked(
            crate::chain::ChainId::BitcoinBlake2bTestnet4,
            cfg.clone(),
            &root,
            &lifecycle,
        )
        .unwrap();
        drop(lifecycle);
        let rpc = coincubed::BitcoinD::new(&cfg, "smoke".into()).unwrap();
        assert!(rpc
            .check_local_fork_chain(crate::chain::ChainId::BitcoinBlake2bTestnet4, false)
            .is_ok());
        assert!(rpc
            .check_local_fork_chain(crate::chain::ChainId::BitcoinBlake2bTestnet4, true)
            .is_err());
        assert_eq!(
            std::fs::read(bitcoin.join("bitcoin.conf")).unwrap(),
            b"Bitcoin sentinel"
        );
        assert!(!crate::node::revalidate::ManagedNodeState::path(&root).exists());
        node.stop();
        // The cookie disappears before chainstate is flushed. Wait for Knots'
        // final shutdown message before deleting any files from its datadir.
        let log = datadir
            .join(bitcoind_network_dir(&Network::Testnet4).unwrap())
            .join("debug.log");
        let mut stopped = false;
        for _ in 0..200 {
            if std::fs::read_to_string(&log)
                .unwrap_or_default()
                .contains("Shutdown: done")
            {
                stopped = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(stopped, "disposable node did not finish shutdown");
        assert!(!internal_bitcoind_cookie_path(&datadir, &Network::Testnet4).exists());
        std::fs::remove_dir_all(base).unwrap();
    }
}

/// What happens to an existing Bitcoin Blake2b install after a pin bump:
/// the running-build check, the pruned-revalidation diagnosis and the resync.
#[cfg(test)]
mod blake2b_pin_bump {
    use super::*;
    use crate::chain::ChainId;

    fn temp_root(name: &str) -> (PathBuf, CoincubeDirectory) {
        let base = std::env::temp_dir().join(format!(
            "coincube-btcb2-pin-bump-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        (base.clone(), CoincubeDirectory::new(base))
    }

    /// A managed Blake2b conf with a testnet4 section on `rpc_port`, and the
    /// cookie the managed RPC config authenticates with.
    fn write_fork_conf(root: &CoincubeDirectory, rpc_port: u16) -> BitcoindConfig {
        let datadir = internal_bitcoind_datadir_for(root, NodeChainFamily::BitcoinBlake2b);
        std::fs::create_dir_all(&datadir).unwrap();
        let mut conf = InternalBitcoindConfig::for_flavor(NodeFlavor::KnotsBlake2b);
        conf.networks.insert(
            Network::Testnet4,
            InternalBitcoindNetworkConfig {
                rpc_port,
                p2p_port: rpc_port.wrapping_add(1),
                prune: 550,
                rpc_auth: None,
            },
        );
        conf.to_ini()
            .write_to_file(internal_bitcoind_config_path(&datadir))
            .unwrap();
        let cookie = internal_bitcoind_cookie_path(&datadir, &Network::Testnet4);
        std::fs::create_dir_all(cookie.parent().unwrap()).unwrap();
        std::fs::write(&cookie, "__cookie__:test").unwrap();
        BitcoindConfig {
            addr: crate::installer::step::node::bitcoind::internal_bitcoind_address(rpc_port),
            rpc_auth: coincubed::config::BitcoindRpcAuth::CookieFile(cookie),
        }
    }

    #[test]
    fn only_the_pinned_build_is_recognised() {
        assert_eq!(KNOTS_BLAKE2B_VERSION, "29.5.1.knots20260508");
        assert!(is_pinned_blake2b_build("/Satoshi:29.5.1/Knots:20260508/"));
        // Case and a parenthesised suffix are not load-bearing.
        assert!(is_pinned_blake2b_build("/SATOSHI:29.5.1/KNOTS:20260508/"));
        assert!(is_pinned_blake2b_build("/Satoshi:29.5.1(knots20260508)/"));
        for other in [
            // The release the pin moved away from: same build date.
            "/Satoshi:29.4.1/Knots:20260508/",
            // Same version, another build.
            "/Satoshi:29.5.1/Knots:20260601/",
            "/Satoshi:29.5.10/Knots:20260508/",
            "/Satoshi:29.5/Knots:20260508/",
            // The Bitcoin Knots pin and Core.
            "/Satoshi:29.3.0/Knots:20260507/",
            "/Satoshi:29.5.1/",
            // Unreadable.
            "",
            "/btcd:29.5.1/Knots:20260508/",
            "/Satoshi:abc/Knots:20260508/",
        ] {
            assert!(!is_pinned_blake2b_build(other), "{:?}", other);
        }
        // The explicit `Knots:` field wins over a user-agent comment that
        // carries another build date inside the client segment.
        assert!(!is_pinned_blake2b_build(
            "/Satoshi:29.5.1(knots20260508)/Knots:20260601/"
        ));
        assert!(is_pinned_blake2b_build(
            "/Satoshi:29.5.1(knots20260601)/Knots:20260508/"
        ));
        // Three outcomes: nothing identifiable is not "another build".
        assert_eq!(classify_blake2b_build(None), Blake2bBuild::Unreadable);
        assert_eq!(classify_blake2b_build(Some("")), Blake2bBuild::Unreadable);
        assert_eq!(classify_blake2b_build(Some("/")), Blake2bBuild::Unreadable);
        assert_eq!(
            classify_blake2b_build(Some("/btcd:29.5.1/")),
            Blake2bBuild::Unreadable
        );
        assert_eq!(
            classify_blake2b_build(Some("/Satoshi:abc/Knots:20260508/")),
            Blake2bBuild::Unreadable
        );
        assert_eq!(
            classify_blake2b_build(Some("/Satoshi:29.4.1/Knots:20260508/")),
            Blake2bBuild::Other
        );
        assert_eq!(
            classify_blake2b_build(Some("/Satoshi:29.5.1/")),
            Blake2bBuild::Other
        );
        assert_eq!(
            classify_blake2b_build(Some("/Satoshi:29.5.1/Knots:20260508/")),
            Blake2bBuild::Pinned
        );
        // The padding that makes the Bitcoin pin's `29.3` match `29.3.0`.
        assert_eq!(normalized_client_version("29.3"), Some([29, 3, 0]));
        assert_eq!(normalized_client_version("29.5.1"), Some([29, 5, 1]));
        assert_eq!(normalized_client_version("29"), None);
        assert_eq!(normalized_client_version("29.5.1.2"), None);
    }

    #[test]
    fn a_running_node_that_cannot_be_reused_is_not_replaced_under_another_process() {
        // Another process owns the node: only a reusable node may be attached.
        assert!(verify_managed_replacement(Some(true), true, true, false).is_ok());
        assert!(matches!(
            verify_managed_replacement(Some(false), true, true, false),
            Err(StartInternalBitcoindError::ConfigUnavailable(_))
        ));
        // Same-process owners: a stale build is replaced, as a flavour
        // switch is.
        assert!(verify_managed_replacement(Some(false), true, false, false).is_ok());
        // Owners but nothing answering stays refused unless authorised.
        assert!(verify_managed_replacement(None, true, false, false).is_err());
        assert!(verify_managed_replacement(None, true, false, true).is_ok());
        assert!(verify_managed_replacement(None, false, false, false).is_ok());
    }

    #[test]
    fn the_pruned_revalidation_failure_is_read_only_from_this_start() {
        let (base, _root) = temp_root("log");
        let log = base.join("debug.log");
        // No log: nothing to diagnose.
        assert!(!pruned_revalidation_logged_since(&log, 0));
        let old = "2026-11-01T00:00:00Z Error: The active chain must be revalidated for \
                   current consensus rules, but required block data has been pruned\n";
        std::fs::write(&log, old).unwrap();
        let offset = std::fs::metadata(&log).unwrap().len();
        // An earlier start's failure is not this start's.
        assert!(!pruned_revalidation_logged_since(&log, offset));
        let mut file = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
        std::io::Write::write_all(&mut file, b"2026-11-22T00:00:00Z Shutdown: done\n").unwrap();
        assert!(!pruned_revalidation_logged_since(&log, offset));
        std::io::Write::write_all(&mut file, old.as_bytes()).unwrap();
        assert!(pruned_revalidation_logged_since(&log, offset));
        // A shrunk (shorter) log: only what follows the last startup banner
        // is this launch's.
        let banner = "2026-11-22T00:00:01Z Bitcoin Knots version v29.5.1.knots20260508\n";
        std::fs::write(&log, format!("{old}{banner}")).unwrap();
        assert!(!pruned_revalidation_logged_since(&log, offset * 4));
        std::fs::write(&log, format!("{old}{banner}{old}")).unwrap();
        assert!(pruned_revalidation_logged_since(&log, offset * 4));
        // No banner to anchor on: an older line is never taken for this one.
        std::fs::write(&log, old).unwrap();
        assert!(!pruned_revalidation_logged_since(&log, offset * 4));
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn the_chain_data_directories_follow_knots_layout() {
        let (base, root) = temp_root("dirs");
        let datadir = internal_bitcoind_datadir_for(&root, NodeChainFamily::BitcoinBlake2b);
        // Knots keeps mainnet in the datadir root.
        assert_eq!(
            managed_fork_chain_data_dirs(&root, &Network::Bitcoin),
            [datadir.join("blocks"), datadir.join("chainstate")]
        );
        assert_eq!(
            managed_fork_chain_data_dirs(&root, &Network::Testnet4),
            [
                datadir.join("testnet4").join("blocks"),
                datadir.join("testnet4").join("chainstate")
            ]
        );
        assert_eq!(
            managed_node_debug_log_path(&datadir, &Network::Bitcoin),
            datadir.join("debug.log")
        );
        assert_eq!(
            managed_node_debug_log_path(&datadir, &Network::Testnet4),
            datadir.join("testnet4").join("debug.log")
        );
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn a_resync_deletes_only_this_networks_blocks_and_chainstate() {
        let (base, root) = temp_root("resync");
        // A port nothing listens on: the node is stopped.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        write_fork_conf(&root, port);
        let datadir = internal_bitcoind_datadir_for(&root, NodeChainFamily::BitcoinBlake2b);
        let testnet4 = datadir.join("testnet4");
        let bitcoin_datadir = internal_bitcoind_datadir(&root);
        let keep = [
            datadir.join("blocks").join("blk00000.dat"),
            datadir.join("chainstate").join("CURRENT"),
            testnet4.join("wallets").join("w").join("wallet.dat"),
            testnet4.join("debug.log"),
            bitcoin_datadir
                .join("testnet4")
                .join("blocks")
                .join("blk00000.dat"),
        ];
        let delete = [
            testnet4.join("blocks").join("blk00000.dat"),
            testnet4.join("blocks").join("index").join("000001.ldb"),
            testnet4.join("chainstate").join("CURRENT"),
        ];
        for file in keep.iter().chain(delete.iter()) {
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, b"x").unwrap();
        }

        // Never the Bitcoin node's data.
        assert!(resync_managed_fork_chain_data(&root, ChainId::from(Network::Testnet4)).is_err());
        assert!(keep.iter().chain(delete.iter()).all(|f| f.exists()));

        resync_managed_fork_chain_data(&root, ChainId::BitcoinBlake2bTestnet4).unwrap();
        for file in &delete {
            assert!(!file.exists(), "{} survived", file.display());
        }
        assert!(!testnet4.join("blocks").exists());
        assert!(!testnet4.join("chainstate").exists());
        for file in &keep {
            assert!(file.exists(), "{} was deleted", file.display());
        }
        assert!(internal_bitcoind_config_path(&datadir).exists());
        assert!(internal_bitcoind_cookie_path(&datadir, &Network::Testnet4).exists());
        // Nothing left to delete is not an error.
        resync_managed_fork_chain_data(&root, ChainId::BitcoinBlake2bTestnet4).unwrap();
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn a_resync_is_refused_while_the_node_is_owned_or_unconfigured() {
        let (base, root) = temp_root("resync-refused");
        // No managed conf: nothing is known about the node, so nothing goes.
        let blocks = internal_bitcoind_datadir_for(&root, NodeChainFamily::BitcoinBlake2b)
            .join("testnet4")
            .join("blocks");
        std::fs::create_dir_all(&blocks).unwrap();
        assert!(resync_managed_fork_chain_data(&root, ChainId::BitcoinBlake2bTestnet4).is_err());
        assert!(blocks.exists());

        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let config = write_fork_conf(&root, port);
        // An owner exactly where a started node records one.
        let lock = LockFile::create(
            root.bitcoind_directory_for(NodeChainFamily::BitcoinBlake2b),
            Network::Testnet4,
        )
        .unwrap();
        lock.activate();
        let owner = Bitcoind::owned(config, lock);
        let refused = resync_managed_fork_chain_data(&root, ChainId::BitcoinBlake2bTestnet4)
            .expect_err("an owned node's data must not be deleted");
        assert!(refused.contains("Another Cube"), "{}", refused);
        assert!(blocks.exists());
        drop(owner);
        assert!(wait_for_managed_node_cleanup(time::Duration::from_secs(60)));
        let _ = std::fs::remove_dir_all(base);
    }

    // An exit the node explained is returned at once, not respawned through
    // the 30 lock retries.
    #[cfg(unix)]
    #[test]
    fn a_diagnosed_early_exit_is_not_respawned() {
        let spawns = std::cell::Cell::new(0u32);
        let config = BitcoindConfig {
            addr: std::net::SocketAddr::from(([127, 0, 0, 1], 1)),
            rpc_auth: coincubed::config::BitcoindRpcAuth::UserPass("u".into(), "p".into()),
        };
        let started = time::Instant::now();
        let result = wait_for_managed_rpc(
            || {
                spawns.set(spawns.get() + 1);
                std::process::Command::new("/bin/sh")
                    .args(["-c", "exit 1"])
                    .spawn()
                    .map_err(|e| StartInternalBitcoindError::CommandError(e.to_string()))
            },
            &config,
            || {
                Some(StartInternalBitcoindError::PrunedRevalidationRequired(
                    ChainId::BitcoinBlake2bTestnet4,
                ))
            },
        );
        assert!(matches!(
            result,
            Err(StartInternalBitcoindError::PrunedRevalidationRequired(
                ChainId::BitcoinBlake2bTestnet4
            ))
        ));
        assert_eq!(spawns.get(), 1);
        assert!(started.elapsed() < time::Duration::from_secs(10));
    }

    /// A minimal JSON-RPC node on `listener`: answers `echo`, reports
    /// `subversion` from `getnetworkinfo`, refuses every other method, and on
    /// `stop` records it and closes the port, as a stopping node does.
    fn fake_running_node(
        listener: std::net::TcpListener,
        subversion: Option<&'static str>,
        stopped: std::sync::Arc<std::sync::atomic::AtomicBool>,
        done: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> std::thread::JoinHandle<()> {
        use std::io::{Read, Write};
        use std::sync::atomic::Ordering;
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if done.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(mut stream) = stream else { break };
                let _ = stream.set_read_timeout(Some(time::Duration::from_secs(2)));
                let mut request = Vec::new();
                let mut buf = [0u8; 4096];
                let body = loop {
                    let Ok(n) = stream.read(&mut buf) else {
                        break None;
                    };
                    if n == 0 {
                        break None;
                    }
                    request.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&request).to_string();
                    let Some((head, body)) = text.split_once("\r\n\r\n") else {
                        continue;
                    };
                    let length = head
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                        })
                        .unwrap_or(0);
                    if body.len() >= length {
                        break Some(body[..length].to_string());
                    }
                };
                let Some(body) = body else { continue };
                let request: serde_json::Value =
                    serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
                let id = request
                    .get("id")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                let method = request.get("method").and_then(|m| m.as_str()).unwrap_or("");
                let (result, error) = match method {
                    "echo" => (serde_json::json!([]), serde_json::Value::Null),
                    "getnetworkinfo" if subversion.is_some() => (
                        serde_json::json!({ "subversion": subversion, "version": 290000 }),
                        serde_json::Value::Null,
                    ),
                    "stop" => (serde_json::json!("stopping"), serde_json::Value::Null),
                    _ => (
                        serde_json::Value::Null,
                        serde_json::json!({ "code": -32601, "message": "Method not found" }),
                    ),
                };
                let reply =
                    serde_json::json!({ "result": result, "error": error, "id": id }).to_string();
                let _ = stream.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                         Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                        reply.len(),
                        reply
                    )
                    .as_bytes(),
                );
                let _ = stream.flush();
                if method == "stop" {
                    stopped.store(true, Ordering::SeqCst);
                    break;
                }
            }
        })
    }

    /// The pinned binary, as a script that records each launch, logs the
    /// pruned-revalidation failure to the testnet4 `debug.log` and exits —
    /// what `29.5.1` does on a pruned `29.4.1` datadir.
    #[cfg(unix)]
    fn install_pruned_failing_binary(root: &CoincubeDirectory) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let family = NodeChainFamily::BitcoinBlake2b;
        let exe = internal_bitcoind_exe_path_for(root, family, KNOTS_BLAKE2B_VERSION);
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        let launches = exe.parent().unwrap().join("launches");
        let log = managed_node_debug_log_path(
            &internal_bitcoind_datadir_for(root, family),
            &Network::Testnet4,
        );
        std::fs::create_dir_all(log.parent().unwrap()).unwrap();
        std::fs::write(
            &exe,
            format!(
                "#!/bin/sh\nprintf 'x\\n' >> '{}'\nprintf 'Error: The active chain must be \
                 revalidated for current consensus rules, but required block data has been \
                 pruned\\n' >> '{}'\nexit 1\n",
                launches.display(),
                log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        launches
    }

    // Gap 2 and gap 3 end to end through the production start: a `29.4.1`
    // node still answering on the managed RPC port is stopped rather than
    // reused, the pinned binary is launched in its place, and its
    // pruned-revalidation exit is reported as such after a single launch.
    #[cfg(unix)]
    #[test]
    fn a_stale_blake2b_build_is_replaced_and_a_pruned_revalidation_is_reported() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        let (base, root) = temp_root("stale-build");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let config = write_fork_conf(&root, listener.local_addr().unwrap().port());
        let launches = install_pruned_failing_binary(&root);
        let stopped = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let server = fake_running_node(
            listener,
            Some("/Satoshi:29.4.1/Knots:20260508/"),
            stopped.clone(),
            done.clone(),
        );

        let chain = ChainId::BitcoinBlake2bTestnet4;
        let lifecycle =
            ManagedNodeLifecycle::acquire(&root, NodeChainFamily::BitcoinBlake2b).unwrap();
        let result = Bitcoind::maybe_start_for_chain_locked(chain, config, &root, &lifecycle);
        drop(lifecycle);
        done.store(true, Ordering::SeqCst);
        server.join().unwrap();

        assert!(
            matches!(
                result,
                Err(StartInternalBitcoindError::PrunedRevalidationRequired(c)) if c == chain
            ),
            "{:?}",
            result
        );
        assert!(stopped.load(Ordering::SeqCst), "the stale node was reused");
        assert_eq!(std::fs::read_to_string(&launches).unwrap(), "x\n");
        let _ = std::fs::remove_dir_all(base);
    }

    // The pinned build on the port is reused: nothing is stopped or launched
    // (the fake node then fails the fork-chain check, which only the reuse
    // path asks, so the start ends there).
    #[cfg(unix)]
    #[test]
    fn the_pinned_blake2b_build_is_reused_not_replaced() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        let (base, root) = temp_root("pinned-build");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let config = write_fork_conf(&root, addr.port());
        let launches = install_pruned_failing_binary(&root);
        let stopped = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let server = fake_running_node(
            listener,
            Some("/Satoshi:29.5.1/Knots:20260508/"),
            stopped.clone(),
            done.clone(),
        );

        let lifecycle =
            ManagedNodeLifecycle::acquire(&root, NodeChainFamily::BitcoinBlake2b).unwrap();
        let result = Bitcoind::maybe_start_for_chain_locked(
            ChainId::BitcoinBlake2bTestnet4,
            config,
            &root,
            &lifecycle,
        );
        drop(lifecycle);
        done.store(true, Ordering::SeqCst);
        let _ = std::net::TcpStream::connect(addr);
        server.join().unwrap();

        assert!(
            matches!(result, Err(StartInternalBitcoindError::BitcoinDError(_))),
            "{:?}",
            result
        );
        assert!(
            !stopped.load(Ordering::SeqCst),
            "the pinned node was stopped"
        );
        assert!(!launches.exists(), "a replacement was launched");
        let _ = std::fs::remove_dir_all(base);
    }

    // A node whose build cannot be read is neither reused nor stopped: the
    // start is refused and nothing is launched.
    #[cfg(unix)]
    #[test]
    fn a_blake2b_node_whose_build_cannot_be_read_is_left_alone() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        let (base, root) = temp_root("unreadable-build");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let config = write_fork_conf(&root, addr.port());
        let launches = install_pruned_failing_binary(&root);
        let stopped = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let server = fake_running_node(listener, None, stopped.clone(), done.clone());

        let lifecycle =
            ManagedNodeLifecycle::acquire(&root, NodeChainFamily::BitcoinBlake2b).unwrap();
        let result = Bitcoind::maybe_start_for_chain_locked(
            ChainId::BitcoinBlake2bTestnet4,
            config,
            &root,
            &lifecycle,
        );
        drop(lifecycle);
        done.store(true, Ordering::SeqCst);
        let _ = std::net::TcpStream::connect(addr);
        server.join().unwrap();

        assert!(
            matches!(&result, Err(StartInternalBitcoindError::BitcoinDError(e)) if e.contains("left running")),
            "{:?}",
            result
        );
        assert!(!stopped.load(Ordering::SeqCst));
        assert!(!launches.exists());
        let _ = std::fs::remove_dir_all(base);
    }

    // The node's own datadir lock, held by another process as a running (or
    // orphaned, RPC-unresponsive) node holds it, refuses the resync; so does a
    // network with no managed configuration.
    #[cfg(unix)]
    #[test]
    fn a_resync_is_refused_while_the_datadir_is_locked_or_the_network_unconfigured() {
        let (base, root) = temp_root("resync-datadir-lock");
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        write_fork_conf(&root, port);
        let datadir = internal_bitcoind_datadir_for(&root, NodeChainFamily::BitcoinBlake2b);

        // Mainnet has no section in this conf.
        let mainnet_blocks = datadir.join("blocks");
        std::fs::create_dir_all(&mainnet_blocks).unwrap();
        let refused = resync_managed_fork_chain_data(&root, ChainId::BitcoinBlake2b)
            .expect_err("an unconfigured network must not be resynced");
        assert!(refused.contains("no"), "{}", refused);
        assert!(mainnet_blocks.exists());

        let testnet4 = datadir.join("testnet4");
        let blocks = testnet4.join("blocks");
        std::fs::create_dir_all(&blocks).unwrap();
        let lock = testnet4.join(".lock");
        let ready = base.join("locked");
        // `fcntl` locks are per process, so the holder must be another one;
        // Python's `lockf` takes the same POSIX lock `bitcoind` does.
        let holder = std::process::Command::new("python3")
            .args([
                "-c",
                "import fcntl,sys,time\nf=open(sys.argv[1],'a+')\nfcntl.lockf(f,fcntl.LOCK_EX|fcntl.LOCK_NB)\nopen(sys.argv[2],'w').close()\ntime.sleep(30)",
            ])
            .arg(&lock)
            .arg(&ready)
            .spawn();
        let Ok(mut holder) = holder else {
            eprintln!("python3 unavailable; skipping the cross-process lock case");
            let _ = std::fs::remove_dir_all(base);
            return;
        };
        let deadline = time::Instant::now() + time::Duration::from_secs(10);
        while !ready.exists() && time::Instant::now() < deadline {
            thread::sleep(time::Duration::from_millis(20));
        }
        assert!(ready.exists(), "the lock holder never took the lock");
        let refused = resync_managed_fork_chain_data(&root, ChainId::BitcoinBlake2bTestnet4)
            .expect_err("a locked datadir must not be emptied");
        assert!(refused.contains("locked"), "{}", refused);
        assert!(blocks.exists());
        let _ = holder.kill();
        let _ = holder.wait();
        // Released: the resync proceeds.
        resync_managed_fork_chain_data(&root, ChainId::BitcoinBlake2bTestnet4).unwrap();
        assert!(!blocks.exists());
        let _ = std::fs::remove_dir_all(base);
    }

    // An empty subversion identifies nothing: the node is left running,
    // exactly as when the call fails, and identifying it is time-bounded.
    #[cfg(unix)]
    #[test]
    fn an_empty_subversion_is_unreadable_not_another_build() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        let (base, root) = temp_root("empty-build");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let config = write_fork_conf(&root, addr.port());
        let launches = install_pruned_failing_binary(&root);
        let stopped = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let server = fake_running_node(listener, Some(""), stopped.clone(), done.clone());

        let started = time::Instant::now();
        let lifecycle =
            ManagedNodeLifecycle::acquire(&root, NodeChainFamily::BitcoinBlake2b).unwrap();
        let result = Bitcoind::maybe_start_for_chain_locked(
            ChainId::BitcoinBlake2bTestnet4,
            config,
            &root,
            &lifecycle,
        );
        drop(lifecycle);
        done.store(true, Ordering::SeqCst);
        let _ = std::net::TcpStream::connect(addr);
        server.join().unwrap();

        assert!(
            matches!(&result, Err(StartInternalBitcoindError::BitcoinDError(e)) if e.contains("left running")),
            "{:?}",
            result
        );
        assert!(!stopped.load(Ordering::SeqCst));
        assert!(!launches.exists());
        assert!(
            started.elapsed() < BUILD_PROBE_BUDGET + time::Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
        let _ = std::fs::remove_dir_all(base);
    }

    // Anything accepting connections on the recorded RPC port refuses the
    // resync, cookie or not.
    #[test]
    fn a_resync_is_refused_while_the_rpc_port_is_occupied() {
        let (base, root) = temp_root("resync-port");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        write_fork_conf(&root, listener.local_addr().unwrap().port());
        let datadir = internal_bitcoind_datadir_for(&root, NodeChainFamily::BitcoinBlake2b);
        std::fs::remove_file(internal_bitcoind_cookie_path(&datadir, &Network::Testnet4)).unwrap();
        let blocks = datadir.join("testnet4").join("blocks");
        std::fs::create_dir_all(&blocks).unwrap();
        let refused = resync_managed_fork_chain_data(&root, ChainId::BitcoinBlake2bTestnet4)
            .expect_err("an occupied RPC port must refuse the resync");
        assert!(refused.contains("RPC port"), "{}", refused);
        assert!(blocks.exists());
        drop(listener);
        let _ = std::fs::remove_dir_all(base);
    }
}
