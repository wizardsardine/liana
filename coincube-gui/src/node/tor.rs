//! Lifecycle management for the managed Tor daemon that gives the COINCUBE
//! node opt-in **inbound** connectivity (a v3 onion service) without any
//! port-forwarding, and optionally routes its outbound peer traffic through Tor.
//!
//! Mirrors the managed-bitcoind process pattern in [`super::bitcoind`]: a
//! per-platform binary (the Tor Expert Bundle, fetched + signature-verified in
//! `installer::step::node::bitcoind`) is run headless with a minimal `torrc`,
//! its lifecycle tied to bitcoind's.
//!
//! ## Fail-safe
//!
//! Inbound-over-Tor is an *enhancement*, never a dependency. Every Tor problem
//! (binary missing, bootstrap timeout, crash) degrades to "inbound unavailable"
//! and the node runs exactly as it does today — outbound-only, no `listen`.
//! The user preference is stored in a sidecar ([`InboundTorPreference`]) so a
//! transient Tor failure never silently disables the feature; the next launch
//! retries. `bitcoin.conf` only ever carries the inbound knobs when Tor is
//! actually up this run (see [`prepare_inbound_tor`]).
//!
//! ## Duress
//!
//! The Tor data directory (and bitcoind's onion-service key) is identifying
//! material and is obliterated by the duress wipe — see
//! [`duress_identifying_targets`] and its use in `gui::tab`.

use std::io::{self, BufRead};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use coincube_core::miniscript::bitcoin::Network;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::dir::CoincubeDirectory;
#[cfg(test)]
use crate::node::bitcoind::internal_bitcoind_config_path;
use crate::node::bitcoind::{
    internal_bitcoind_datadir, internal_bitcoind_directory, internal_tor_directory,
    internal_tor_exe_path, internal_tor_geoip_dir, tor_download_url, tor_supported_on_host,
    InternalBitcoindConfig, MAX_CONNECTIONS_DEFAULT, MAX_UPLOAD_TARGET_MB_DAY_DEFAULT, TOR_VERSION,
};

/// How long to wait for Tor to reach "Bootstrapped 100%" before giving up and
/// falling back to outbound-only. Generous — a cold Tor bootstrap on a slow link
/// can take a while — but bounded so startup can never hang on Tor.
const BOOTSTRAP_TIMEOUT: Duration = Duration::from_secs(180);

/// Loopback host for Tor's SOCKS/control ports and bitcoind's `torcontrol`/
/// `proxy`.
const TOR_HOST: &str = "127.0.0.1";

/// The managed Tor daemon's data directory: holds Tor state and the control
/// auth cookie. Co-located under the managed bitcoind directory so a single
/// duress wipe target covers it. Kept separate from the versioned binary
/// install dir (`tor-<version>`) so it persists across Tor version bumps.
pub fn internal_tor_datadir(coincube_datadir: &CoincubeDirectory) -> PathBuf {
    internal_bitcoind_directory(coincube_datadir).join("tor-data")
}

/// Path of the generated `torrc` (rewritten on every start).
pub fn internal_tor_torrc_path(coincube_datadir: &CoincubeDirectory) -> PathBuf {
    internal_tor_datadir(coincube_datadir).join("torrc")
}

/// Path Tor logs to (scanned for the bootstrap line); truncated each start.
pub fn internal_tor_log_path(coincube_datadir: &CoincubeDirectory) -> PathBuf {
    internal_tor_datadir(coincube_datadir).join("tor.log")
}

/// Local ports the managed Tor listens on. Allocated by us (like bitcoind's
/// RPC/P2P ports) so bitcoind can be pointed at them immediately, with no
/// control-port round-trip to discover an `auto` port.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TorPorts {
    /// Control port — bitcoind's `torcontrol`, used to create the onion service.
    pub control: u16,
    /// SOCKS port — bitcoind's `proxy` when outbound-via-Tor is on.
    pub socks: u16,
}

/// A running managed Tor daemon. Cheap to clone (shares one child process); the
/// process is spawned detached, so dropping every clone does not kill it —
/// [`Tor::stop`] (via [`stop_managed_tor`]) does.
#[derive(Clone)]
pub struct Tor {
    ports: TorPorts,
    process: Arc<Mutex<std::process::Child>>,
}

/// Errors starting the managed Tor daemon. All are handled fail-safe by
/// [`prepare_inbound_tor`] (log + run outbound-only), never surfaced as a
/// wallet error.
#[derive(Debug)]
pub enum StartTorError {
    /// The managed `tor` binary is not installed for this host.
    ExecutableNotFound,
    /// Tor isn't published for this platform (Linux/Windows aarch64).
    UnsupportedPlatform,
    /// Could not allocate distinct local ports for Tor.
    Port(String),
    /// I/O error preparing the data dir / torrc or spawning the process.
    Io(String),
    /// Tor started but did not reach "Bootstrapped 100%" within the timeout.
    BootstrapTimeout,
    /// The Tor process exited before bootstrapping.
    ProcessExited,
}

impl std::fmt::Display for StartTorError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::ExecutableNotFound => write!(f, "managed tor binary not found"),
            Self::UnsupportedPlatform => write!(f, "tor is not available for this platform"),
            Self::Port(e) => write!(f, "could not allocate tor ports: {e}"),
            Self::Io(e) => write!(f, "tor i/o error: {e}"),
            Self::BootstrapTimeout => write!(f, "tor did not bootstrap within the timeout"),
            Self::ProcessExited => write!(f, "tor process exited before bootstrapping"),
        }
    }
}

impl Tor {
    /// The local ports Tor is listening on.
    pub fn ports(&self) -> TorPorts {
        self.ports
    }

    /// Start the managed Tor daemon for `version` and block until it has
    /// bootstrapped (or the timeout elapses). On success the SOCKS + control
    /// ports are live and bitcoind can be pointed at them.
    ///
    /// `reserved` is every port a managed-node conf under this datadir records
    /// (see [`crate::node::bitcoind::InternalBitcoindConfig::ports_in_use`]),
    /// so Tor's fresh ports never land on a node's RPC/P2P port that merely
    /// happens to be unbound right now.
    pub fn start(
        coincube_datadir: &CoincubeDirectory,
        version: &str,
        reserved: &[u16],
    ) -> Result<Self, StartTorError> {
        if !tor_supported_on_host() {
            return Err(StartTorError::UnsupportedPlatform);
        }
        let exe = internal_tor_exe_path(coincube_datadir, version);
        if !exe.exists() {
            return Err(StartTorError::ExecutableNotFound);
        }

        let datadir = internal_tor_datadir(coincube_datadir);
        // Tor refuses a group/other-accessible DataDirectory; `create_directory`
        // makes it 0700 on unix.
        crate::dir::create_directory(&datadir).map_err(|e| StartTorError::Io(e.to_string()))?;

        let ports = allocate_ports(reserved)?;
        let geoip_dir = internal_tor_geoip_dir(coincube_datadir, version);
        let torrc_path = internal_tor_torrc_path(coincube_datadir);
        let log_path = internal_tor_log_path(coincube_datadir);
        // Start each run from a clean log so we only ever match *this* run's
        // "Bootstrapped 100%".
        let _ = std::fs::remove_file(&log_path);
        std::fs::write(&torrc_path, torrc_contents(&datadir, &geoip_dir, ports))
            .map_err(|e| StartTorError::Io(e.to_string()))?;

        // We capture tor's log by pointing its `Log notice stdout` at a file we
        // open here, rather than tor's `Log ... file <path>` directive: tor's
        // Log-file parser mishandles a path containing spaces (e.g. the macOS
        // `Application Support` datadir), failing with ENOENT and refusing to
        // start. Opening the file ourselves sidesteps that entirely.
        let log_file =
            std::fs::File::create(&log_path).map_err(|e| StartTorError::Io(e.to_string()))?;
        let mut command = std::process::Command::new(&exe);
        command
            .arg("-f")
            .arg(&torrc_path)
            .stdout(std::process::Stdio::from(log_file))
            .stderr(std::process::Stdio::null());

        // Detach so closing the app doesn't SIGHUP tor; we stop it via `stop`.
        crate::node::detach_spawned_process(&mut command);

        let child = command
            .spawn()
            .map_err(|e| StartTorError::Io(e.to_string()))?;
        let tor = Tor {
            ports,
            process: Arc::new(Mutex::new(child)),
        };

        match tor.wait_for_bootstrap(&log_path) {
            Ok(()) => {
                info!(
                    "managed tor bootstrapped (socks 127.0.0.1:{}, control 127.0.0.1:{})",
                    ports.socks, ports.control
                );
                Ok(tor)
            }
            Err(e) => {
                // Don't leak a half-started tor on the failure path.
                tor.stop();
                Err(e)
            }
        }
    }

    /// Poll the Tor log for "Bootstrapped 100%", bailing out if the process
    /// dies or the timeout elapses.
    fn wait_for_bootstrap(&self, log_path: &Path) -> Result<(), StartTorError> {
        let deadline = Instant::now() + BOOTSTRAP_TIMEOUT;
        loop {
            if self.has_exited() {
                return Err(StartTorError::ProcessExited);
            }
            if log_reports_bootstrapped(log_path) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(StartTorError::BootstrapTimeout);
            }
            thread::sleep(Duration::from_millis(500));
        }
    }

    /// Whether the child process has already exited.
    fn has_exited(&self) -> bool {
        self.process
            .lock()
            .ok()
            .and_then(|mut c| c.try_wait().ok())
            .map(|status| status.is_some())
            .unwrap_or(false)
    }

    /// Stop the managed Tor daemon. Idempotent — safe to call from every clone;
    /// the first kill wins and later calls no-op.
    pub fn stop(&self) {
        if let Ok(mut child) = self.process.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
        info!("stopped managed tor");
    }
}

/// Allocate two distinct local ports for Tor's control + SOCKS listeners.
/// Two distinct local ports for Tor's control and SOCKS listeners, from the
/// same bounded policy the node ports use: OS bind-and-release candidates,
/// none of them a bitcoind default or a port in `reserved`.
///
/// Not race-free, and not claimed to be: each candidate is bound and released
/// before Tor is spawned, so between that release and Tor's own bind another
/// process — a third party, or a COINCUBE allocation that has not yet seen
/// these ports persisted — can take it. A collision fails Tor's bootstrap
/// (or bitcoind's bind) loudly rather than silently, and the ports are only
/// persisted into `bitcoin.conf` once Tor is actually up.
fn allocate_ports(reserved: &[u16]) -> Result<TorPorts, StartTorError> {
    use crate::installer::step::node::bitcoind::get_available_port;
    use crate::node::bitcoind::allocate_managed_ports;
    let (control, socks) = allocate_managed_ports(reserved, get_available_port)
        .map_err(|e| StartTorError::Port(e.to_string()))?;
    Ok(TorPorts { control, socks })
}

/// The minimal `torrc` for a headless onion-service host + SOCKS proxy. Paths
/// are quoted so spaces (common on Windows/macOS) are handled on every
/// platform. GeoIP lines are emitted only when the bundle's databases are
/// present (their absence is a warning in tor, not an error).
///
/// Logging uses `Log notice stdout` (not `Log ... file`): tor's Log-file parser
/// mishandles a path containing spaces (the macOS `Application Support` datadir),
/// so [`Tor::start`] captures stdout into the log file itself instead.
fn torrc_contents(datadir: &Path, geoip_dir: &Path, ports: TorPorts) -> String {
    let mut torrc = String::new();
    torrc.push_str(&format!("DataDirectory {}\n", quote_path(datadir)));
    torrc.push_str(&format!("SocksPort {TOR_HOST}:{}\n", ports.socks));
    torrc.push_str(&format!("ControlPort {TOR_HOST}:{}\n", ports.control));
    // Cookie auth: bitcoind reads the cookie path tor advertises over
    // PROTOCOLINFO, so no shared secret has to be configured out of band.
    torrc.push_str("CookieAuthentication 1\n");
    // We host an onion service and route client traffic; never act as a relay.
    torrc.push_str("ClientOnly 1\n");
    let geoip = geoip_dir.join("geoip");
    let geoip6 = geoip_dir.join("geoip6");
    if geoip.exists() {
        torrc.push_str(&format!("GeoIPFile {}\n", quote_path(&geoip)));
    }
    if geoip6.exists() {
        torrc.push_str(&format!("GeoIPv6File {}\n", quote_path(&geoip6)));
    }
    torrc.push_str("Log notice stdout\n");
    torrc
}

/// Quote a path for a `torrc` value (double quotes; backslashes/quotes escaped).
fn quote_path(path: &Path) -> String {
    let s = path
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    format!("\"{s}\"")
}

/// Whether a single Tor log line reports a completed bootstrap.
fn line_reports_bootstrapped(line: &str) -> bool {
    line.contains("Bootstrapped 100%")
}

/// Whether the Tor log file (if it exists yet) contains the bootstrap-complete
/// line.
fn log_reports_bootstrapped(log_path: &Path) -> bool {
    let Ok(file) = std::fs::File::open(log_path) else {
        return false;
    };
    io::BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .any(|line| line_reports_bootstrapped(&line))
}

// -------------------------------------------------------------------------
// User preference (sidecar)
// -------------------------------------------------------------------------

/// The user's inbound-over-Tor preference, persisted as a small JSON sidecar
/// beside the managed node. Kept out of `bitcoin.conf` (which bitcoind would
/// reject unknown keys in, and which is rewritten each run to reflect the
/// *actual* runtime state) so a transient Tor failure never clobbers the user's
/// choice: the runtime path can strip the inbound knobs from `bitcoin.conf`
/// while this file keeps saying "the user wants inbound", and the next launch
/// retries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboundTorPreference {
    /// Master switch: run Tor and make the node reachable as an onion service.
    pub enabled: bool,
    /// Also route outbound peer connections through Tor (Decision 2: default on
    /// once inbound is on — nearly free while Tor runs).
    pub outbound_via_tor: bool,
    /// Daily upload cap in MiB (`maxuploadtarget`). `None` = unlimited.
    pub max_upload_target_mb_day: Option<u32>,
    /// Total connection cap (`maxconnections`). `None` = bitcoind default.
    pub max_connections: Option<u16>,
}

impl Default for InboundTorPreference {
    /// The config-layer default is **off** (backward compatible: a node with no
    /// sidecar runs outbound-only, unchanged). The product default of ON for
    /// new installs is applied explicitly at node setup via
    /// [`Self::default_enabled`].
    fn default() -> Self {
        Self {
            enabled: false,
            outbound_via_tor: true,
            max_upload_target_mb_day: Some(MAX_UPLOAD_TARGET_MB_DAY_DEFAULT),
            max_connections: Some(MAX_CONNECTIONS_DEFAULT),
        }
    }
}

impl InboundTorPreference {
    /// The product default applied to a freshly set-up managed node: inbound ON
    /// with the always-on ~1 GB/day cap and outbound-via-Tor. Disclosed at node
    /// setup with a one-click opt-out (see the settings/installer UI).
    pub fn default_enabled() -> Self {
        Self {
            enabled: true,
            ..Self::default()
        }
    }

    /// Sidecar path: `<datadir>/bitcoind/inbound_tor.json`.
    pub fn path(coincube_datadir: &CoincubeDirectory) -> PathBuf {
        internal_bitcoind_directory(coincube_datadir).join("inbound_tor.json")
    }

    /// Load the preference, or the (disabled) default when the sidecar is
    /// absent or unreadable — always fail-safe toward "off".
    pub fn load(coincube_datadir: &CoincubeDirectory) -> Self {
        let path = Self::path(coincube_datadir);
        match std::fs::read_to_string(&path) {
            Ok(contents) => serde_json::from_str(&contents).unwrap_or_else(|e| {
                warn!("unreadable inbound-tor preference at {path:?} ({e}); treating as off");
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    /// Persist the preference to the sidecar.
    pub fn save(&self, coincube_datadir: &CoincubeDirectory) -> io::Result<()> {
        let path = Self::path(coincube_datadir);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        std::fs::write(path, json)
    }
}

// -------------------------------------------------------------------------
// Process-wide managed-tor registry
// -------------------------------------------------------------------------

/// The one managed Tor for the (single, process-wide) managed node. A global
/// registry — mirroring `dir::ACTIVE_DATADIR` — lets shutdown stop Tor without
/// threading a handle through every struct that owns the node.
static MANAGED_TOR: OnceLock<Mutex<Option<Tor>>> = OnceLock::new();
static MANAGED_TOR_ROOT: OnceLock<Mutex<Option<std::path::PathBuf>>> = OnceLock::new();

fn registered_root() -> &'static Mutex<Option<std::path::PathBuf>> {
    MANAGED_TOR_ROOT.get_or_init(|| Mutex::new(None))
}

fn registry() -> &'static Mutex<Option<Tor>> {
    MANAGED_TOR.get_or_init(|| Mutex::new(None))
}

/// Register the running managed Tor, replacing (and stopping) any previous one.
fn register_managed_tor(tor: Tor) {
    if let Ok(mut slot) = registry().lock() {
        if let Some(previous) = slot.replace(tor) {
            previous.stop();
        }
    }
}

fn register_managed_tor_for(root: &CoincubeDirectory, tor: Tor) {
    if let Ok(mut owner) = registered_root().lock() {
        register_managed_tor(tor);
        *owner = Some(root.path().to_path_buf());
    }
}

fn foreign_managed_tor(root: &CoincubeDirectory) -> bool {
    registered_root().lock().is_ok_and(|owner| {
        owner.as_ref().is_some_and(|path| path != root.path()) && managed_tor_ports().is_some()
    })
}

pub fn stop_managed_tor_for(root: &CoincubeDirectory) {
    if let Ok(mut owner) = registered_root().lock() {
        if owner.as_ref().is_some_and(|path| path == root.path()) {
            if let Ok(mut slot) = registry().lock() {
                if let Some(tor) = slot.take() {
                    tor.stop();
                }
                *owner = None;
            }
        }
    }
}

/// Serialise tests that touch the process-global managed-Tor registry.
///
/// Every test that can register, stop or observe the managed Tor — directly
/// or through `prepare_inbound_tor` / the loader and settings start helpers
/// that call it — takes this guard, so a test that registers a disposable
/// process cannot have it stopped by another test's `stop_managed_tor()`
/// running in parallel. Poison-tolerant: a panicking holder must not take
/// every later registry test down with it.
///
/// Returned as an opaque token rather than the bare `MutexGuard`: the async
/// loader tests hold it across `.await`, which is fine on their current-thread
/// runtime (nothing else can run on that thread meanwhile) but is exactly the
/// shape clippy's `await_holding_lock` warns about for multi-threaded ones.
#[cfg(test)]
pub(crate) fn registry_test_guard() -> RegistryTestGuard {
    static GUARD: Mutex<()> = Mutex::new(());
    RegistryTestGuard(
        GUARD
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    )
}

/// The hold returned by [`registry_test_guard`]; released on drop.
#[cfg(test)]
pub(crate) struct RegistryTestGuard(#[allow(dead_code)] std::sync::MutexGuard<'static, ()>);

/// Stop and deregister the process-global managed Tor, if any. Normal node
/// shutdown uses [`stop_managed_tor_for`] under the lifecycle lock so a Cube
/// cannot stop a Tor belonging to another datadir.
pub fn stop_managed_tor() {
    if let Ok(mut owner) = registered_root().lock() {
        *owner = None;
    }
    if let Ok(mut slot) = registry().lock() {
        if let Some(tor) = slot.take() {
            tor.stop();
        }
    }
}

/// Whether bitcoind has an onion-service key on disk — i.e. it has, at some
/// point, connected to tor's control port and created a v3 onion service, and
/// will re-publish that same onion on the next start with `listenonion`. Note
/// the key file **persists** even after inbound-over-Tor is turned off, so
/// callers must AND this with a running tor (see the settings status) to mean
/// "currently reachable". bitcoind stores the key in its network data dir.
pub fn onion_key_exists(coincube_datadir: &CoincubeDirectory, network: Network) -> bool {
    let mut path = internal_bitcoind_datadir(coincube_datadir);
    if let Some(netdir) = crate::node::bitcoind::bitcoind_network_dir(&network) {
        path.push(netdir);
    }
    // v3 is what bitcoind creates today; the legacy name is checked as a
    // fallback for completeness.
    path.join("onion_v3_private_key").exists() || path.join("onion_private_key").exists()
}

/// The managed Tor's live ports, if one is running — for the settings status
/// card (T4) and diagnostics.
pub fn managed_tor_ports() -> Option<TorPorts> {
    registry().lock().ok().and_then(|slot| {
        slot.as_ref().and_then(|tor| {
            if tor.has_exited() {
                None
            } else {
                Some(tor.ports())
            }
        })
    })
}

// -------------------------------------------------------------------------
// Tie-together: prepare inbound-over-Tor before starting bitcoind
// -------------------------------------------------------------------------

/// Why the managed conf could not be brought to a safe inbound/outbound state
/// for this start. Every variant means the same thing to a caller: **do not
/// start bitcoind from what is on disk** — it may still carry a previous run's
/// `listenonion`/`torcontrol`/`proxy` lines pointing at a Tor that is not
/// there. Retryable.
#[derive(Debug)]
pub struct PrepareInboundTorError(pub crate::node::managed_conf::ManagedConfError);

impl std::fmt::Display for PrepareInboundTorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for PrepareInboundTorError {}

/// The one thing `prepare_inbound_tor` writes: the inbound-over-Tor fields of
/// the conf. Everything else in the file — network sections, resources, a
/// section another setup added a moment ago — is taken from the fresh read
/// under the lock, never from an earlier snapshot.
#[derive(Debug, Clone, Copy)]
enum InboundFields {
    /// No inbound knobs at all: outbound-only, Tor not in use.
    OutboundOnly,
    /// Inbound on, with the running Tor's ports.
    Inbound {
        outbound_via_tor: bool,
        max_upload_target_mb_day: Option<u32>,
        max_connections: Option<u16>,
        ports: TorPorts,
    },
}

impl InboundFields {
    fn apply(self, conf: &mut InternalBitcoindConfig) {
        match self {
            Self::OutboundOnly => {
                conf.inbound_tor = false;
                conf.outbound_via_tor = false;
                conf.max_upload_target_mb_day = None;
                conf.max_connections = None;
                conf.tor_control_port = None;
                conf.tor_socks_port = None;
            }
            Self::Inbound {
                outbound_via_tor,
                max_upload_target_mb_day,
                max_connections,
                ports,
            } => {
                conf.inbound_tor = true;
                conf.outbound_via_tor = outbound_via_tor;
                conf.max_upload_target_mb_day = max_upload_target_mb_day;
                conf.max_connections = max_connections;
                conf.tor_control_port = Some(ports.control);
                conf.tor_socks_port = Some(ports.socks);
            }
        }
    }
}

/// Persist `fields` onto a fresh read of the managed conf, under the
/// datadir-wide lock. `Ok(true)` if there was a conf to write; `Ok(false)` if
/// there is no managed-node config at all (external backend; nothing to do).
fn persist_inbound_fields(
    coincube_datadir: &CoincubeDirectory,
    fields: InboundFields,
) -> Result<bool, PrepareInboundTorError> {
    use crate::node::bitcoind::NodeChainFamily;
    use crate::node::managed_conf::update_managed_conf;
    update_managed_conf(coincube_datadir, NodeChainFamily::Bitcoin, |txn| {
        let Some(mut conf) = txn.conf.clone() else {
            return Ok((false, None));
        };
        fields.apply(&mut conf);
        Ok((true, Some(conf)))
    })
    .map(|outcome| outcome.logged("writing the managed bitcoin.conf's inbound-over-tor state"))
    .map_err(PrepareInboundTorError)
}

/// Start Tor (if the user wants inbound and it's available) and rewrite
/// `bitcoin.conf` so bitcoind emits the right inbound knobs for **this** run.
/// Call this immediately before starting the managed bitcoind.
///
/// Fail-safe on every Tor problem (preference off, wrong network, platform
/// unsupported, binary missing, bootstrap timeout): `bitcoin.conf` is left
/// with **no** inbound knobs, so bitcoind runs outbound-only exactly as today,
/// and `Ok(false)` is returned. `Ok(true)` iff Tor is up and bitcoind will host
/// an onion service. The user's preference sidecar is never modified here, so a
/// transient failure is retried next launch.
///
/// **Not** fail-safe on a conf problem, deliberately: if the file cannot be
/// read, locked (another setup is writing it) or replaced, the previous run's
/// inbound lines may still be in it, and starting bitcoind against them would
/// point it at a Tor that is not running. That is `Err`, and every caller
/// turns it into a retryable start refusal rather than starting anyway. On
/// that refusal the process-global managed Tor is left exactly as it was: it
/// is stopped only once the outbound-only reset has actually been persisted,
/// so a sibling session's node in this process never loses the Tor its conf
/// still names because *this* start could not get the lock.
///
/// Lock discipline: Tor's bootstrap (seconds) runs *outside* the conf lock;
/// the lock is held only for a fresh read, the merge of the inbound fields
/// this function owns, and the atomic replace — so a network section another
/// setup persisted meanwhile is never erased, and no one waits on Tor.
///
/// Inbound-over-Tor is **mainnet-only**: the managed node exists to serve
/// mainnet, and onion reachability has no value on test networks, so we run
/// outbound-only on anything but [`Network::Bitcoin`].
/// Prepare the supported managed Tor lifecycle using explicit chain identity.
/// BTCB2 has a separate node directory but no separate Tor registry/bundle yet.
/// Fork nodes clear their own stale inbound fields and run outbound-only,
/// without touching the Bitcoin Tor registry. Connect-only startup skips this.
pub fn prepare_inbound_tor_for_chain(
    coincube_datadir: &CoincubeDirectory,
    chain: crate::chain::ChainId,
) -> Result<bool, PrepareInboundTorError> {
    if chain.is_blake2b() {
        // Fork nodes run outbound-only in this release. Clear their own stale
        // inbound fields without provisioning, replacing or stopping Bitcoin Tor.
        return crate::node::managed_conf::update_managed_conf(
            coincube_datadir,
            crate::node::bitcoind::NodeChainFamily::BitcoinBlake2b,
            |txn| {
                let conf = txn.conf.clone().map(|mut conf| {
                    InboundFields::OutboundOnly.apply(&mut conf);
                    conf
                });
                Ok((false, conf))
            },
        )
        .map(|outcome| outcome.logged("preparing the outbound-only Blake2b node"))
        .map_err(PrepareInboundTorError);
    }
    prepare_inbound_tor(coincube_datadir, chain.bitcoin_network())
}

/// Prepare Tor only when this start is the first owner of the managed node.
/// An attached Cube must keep the existing node's Tor ports and process alive.
pub fn prepare_inbound_tor_for_chain_locked(
    coincube_datadir: &CoincubeDirectory,
    chain: crate::chain::ChainId,
    lifecycle: &crate::node::bitcoind::ManagedNodeLifecycle,
) -> Result<bool, PrepareInboundTorError> {
    if lifecycle.has_owners() {
        return Ok(false);
    }
    prepare_inbound_tor_for_chain(coincube_datadir, chain)
}

pub fn prepare_inbound_tor(
    coincube_datadir: &CoincubeDirectory,
    network: Network,
) -> Result<bool, PrepareInboundTorError> {
    use crate::node::bitcoind::NodeChainFamily;
    use crate::node::managed_conf::update_managed_conf;

    // Always start from a clean inbound state, whatever the file carried from
    // a previous run; the inbound lines are re-derived below only once Tor is
    // actually up. This first pass also tells us whether there is a managed
    // conf at all, and reads (under the lock) every port the datadir's confs
    // record, so Tor's own ports are chosen around them.
    //
    // The process-global Tor from a previous start is stopped only *after*
    // this reset has succeeded (below). A refusal here — the lock is busy, the
    // conf unreadable or not replaceable — returns with the file untouched,
    // and it must also leave that Tor untouched: a sibling session in this
    // process may still be running a node whose conf points at it, and
    // stopping it on a refusal would cut that node's SOCKS/control path while
    // the file keeps naming a dead process. Nothing here needs the old Tor
    // gone yet: the reservation below reads confs, not the registry, and the
    // old Tor's ports are cleared from this conf by the reset (they are
    // re-derived from a fresh start), so its still-running listeners only
    // matter to the bind-and-release probe, which simply skips them.
    let reserved = update_managed_conf(coincube_datadir, NodeChainFamily::Bitcoin, |txn| {
        let Some(mut conf) = txn.conf.clone() else {
            return Ok((None, None));
        };
        InboundFields::OutboundOnly.apply(&mut conf);
        let reserved = crate::node::bitcoind::reserved_managed_ports(
            coincube_datadir,
            NodeChainFamily::Bitcoin,
            &conf,
        )?;
        Ok((Some(reserved), Some(conf)))
    })
    .map(|outcome| outcome.logged("resetting the managed bitcoin.conf to outbound-only"))
    .map_err(PrepareInboundTorError)?;

    // The conf is durably outbound-only (or absent): any prior managed tor
    // from this process is now safe to replace, and is — on every path from
    // here on, including "no conf" and "Tor stays off", exactly as before.
    stop_managed_tor_for(coincube_datadir);

    let Some(reserved) = reserved else {
        // No managed-node config on disk → nothing to do (external backend).
        return Ok(false);
    };

    // One process-global Tor slot cannot be rebound to a second datadir while
    // the first datadir still has active owners. The second node starts with
    // its freshly persisted outbound-only config instead.
    if foreign_managed_tor(coincube_datadir) {
        return Ok(false);
    }

    // Mainnet-only (see the doc comment above).
    if network != Network::Bitcoin {
        return Ok(false);
    }
    let pref = InboundTorPreference::load(coincube_datadir);
    if !pref.enabled {
        return Ok(false);
    }
    if !tor_supported_on_host() {
        info!("inbound-over-tor requested but unavailable on this platform; running outbound-only");
        return Ok(false);
    }

    // The slow part, with no lock held.
    let tor = match Tor::start(coincube_datadir, TOR_VERSION, &reserved) {
        Ok(tor) => tor,
        Err(e) => {
            warn!("managed tor did not start ({e}); inbound unavailable, running outbound-only");
            // The conf is already outbound-only from the first pass.
            return Ok(false);
        }
    };
    let fields = InboundFields::Inbound {
        outbound_via_tor: pref.outbound_via_tor,
        max_upload_target_mb_day: pref.max_upload_target_mb_day,
        max_connections: pref.max_connections,
        ports: tor.ports(),
    };
    match persist_inbound_fields(coincube_datadir, fields) {
        Ok(true) => {
            register_managed_tor_for(coincube_datadir, tor);
            info!("inbound-over-tor enabled; bitcoind will host a v3 onion service");
            Ok(true)
        }
        Ok(false) => {
            // The conf disappeared between the two passes; nothing to point
            // bitcoind at, so no reason to keep Tor either.
            tor.stop();
            Ok(false)
        }
        Err(e) => {
            // Couldn't persist the inbound config: the file is still the
            // outbound-only one from the first pass, but we cannot know that
            // for sure without the lock, and a stale-inbound start is the one
            // outcome to rule out — refuse, and tear tor down.
            warn!("failed to write inbound bitcoin.conf ({e}); refusing to start from it");
            tor.stop();
            Err(e)
        }
    }
}

// -------------------------------------------------------------------------
// Binary install
// -------------------------------------------------------------------------

/// Whether the managed Tor binary for the pinned version is installed.
pub fn tor_installed(coincube_datadir: &CoincubeDirectory) -> bool {
    internal_tor_exe_path(coincube_datadir, TOR_VERSION).exists()
}

/// Download, signature-verify, and install the managed Tor Expert Bundle for
/// the pinned version. No-op (Ok) if it's already installed. Reuses the same
/// signed-manifest verification path as the managed bitcoind (see
/// `installer::step::node::bitcoind`), so an unverifiable download is refused.
pub async fn download_and_install_tor(coincube_datadir: CoincubeDirectory) -> Result<(), String> {
    use crate::installer::step::node::bitcoind::{install_tor, DownloadVerification};

    if tor_installed(&coincube_datadir) {
        return Ok(());
    }
    if !tor_supported_on_host() {
        return Err("Tor is not available for this platform".to_string());
    }
    let url =
        tor_download_url().ok_or_else(|| "no Tor download URL for this platform".to_string())?;

    info!("downloading managed Tor {TOR_VERSION} from {url}");
    let bytes = crate::download::fetch_bytes(&url)
        .await
        .map_err(|e| format!("Tor download failed: {e}"))?;
    let manifest = crate::download::fetch_tor_release_manifest(TOR_VERSION)
        .await
        .map_err(|e| format!("Tor manifest fetch failed: {e}"))?;
    let verification = DownloadVerification::for_tor(Some(manifest))
        .ok_or_else(|| "could not build Tor download verification".to_string())?;

    let install_dir = internal_tor_directory(&coincube_datadir, TOR_VERSION);
    std::fs::create_dir_all(&install_dir)
        .map_err(|e| format!("could not create Tor install dir: {e}"))?;
    install_tor(&install_dir, &bytes, &verification)
        .map_err(|e| format!("Tor install failed: {e}"))?;
    info!("installed managed Tor {TOR_VERSION}");
    Ok(())
}

/// If the user wants inbound-over-Tor and it's available for this platform but
/// the binary isn't installed yet, install it now (best-effort). Called before
/// the runtime [`prepare_inbound_tor`] so a default-ON node self-provisions Tor
/// on first launch. Never errors — a failed install just means inbound is
/// unavailable this run (fail-safe), retried next launch.
pub async fn ensure_tor_installed_if_wanted(coincube_datadir: &CoincubeDirectory) {
    let pref = InboundTorPreference::load(coincube_datadir);
    if !pref.enabled || !tor_supported_on_host() || tor_installed(coincube_datadir) {
        return;
    }
    if let Err(e) = download_and_install_tor(coincube_datadir.clone()).await {
        warn!("could not auto-install managed Tor ({e}); inbound unavailable this run");
    }
}

// -------------------------------------------------------------------------
// Duress wipe
// -------------------------------------------------------------------------

/// Identifying material the duress wipe must obliterate: the managed Tor data
/// directory (Tor state) and bitcoind's onion-service private key(s). These
/// live under `<root>/bitcoind`, which the wipe otherwise preserves (the
/// blockchain is expensive to re-sync and not sensitive) — so they must be
/// listed explicitly. Returns only paths that currently exist.
///
/// bitcoind stores its ephemeral onion key as `onion_v3_private_key` (or the
/// legacy `onion_private_key`) in its network data dir, so we look in the
/// managed datadir root and one level of network subdirectories.
pub fn duress_identifying_targets(coincube_datadir_root: &Path) -> Vec<PathBuf> {
    // Derive paths through the same helpers the rest of the module uses, so the
    // wipe stays in lock-step with where Tor state and the onion keys actually
    // live — hardcoding the layout here would silently miss the fingerprint if
    // the directory scheme ever changed.
    let coincube_datadir = CoincubeDirectory::new(coincube_datadir_root.to_path_buf());
    let mut targets = Vec::new();

    // The whole managed Tor data directory (state + control cookie).
    let tor_data = internal_tor_datadir(&coincube_datadir);
    if tor_data.exists() {
        targets.push(tor_data);
    }

    // bitcoind's onion-service key(s), in the datadir root and per-network dirs.
    const ONION_KEY_FILES: &[&str] = &["onion_v3_private_key", "onion_private_key"];
    let datadir = internal_bitcoind_datadir(&coincube_datadir);
    let mut search_dirs = vec![datadir.clone()];
    if let Ok(entries) = std::fs::read_dir(&datadir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                search_dirs.push(path);
            }
        }
    }
    for dir in search_dirs {
        for name in ONION_KEY_FILES {
            let key = dir.join(name);
            if key.exists() {
                targets.push(key);
            }
        }
    }

    targets
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    #[cfg(unix)]
    #[test]
    fn attached_cube_preserves_tor_until_last_owner_closes() {
        let _registry = registry_test_guard();
        let root = temp_datadir("concurrent-tor-owners");
        let child = std::process::Command::new("sleep")
            .arg("60")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let process = Arc::new(Mutex::new(child));
        let fixture = DisposableManagedChild(Tor {
            ports: TorPorts {
                control: 9051,
                socks: 9050,
            },
            process: process.clone(),
        });
        register_managed_tor_for(&root, fixture.0.clone());
        let config = coincubed::config::BitcoindConfig {
            addr: "127.0.0.1:1".parse().unwrap(),
            rpc_auth: coincubed::config::BitcoindRpcAuth::CookieFile(
                root.path().join("bitcoind/.cookie"),
            ),
        };
        let first =
            crate::node::bitcoind::Bitcoind::for_test(config.clone(), &root, Network::Bitcoin);
        let lifecycle = crate::node::bitcoind::ManagedNodeLifecycle::acquire(
            &root,
            crate::node::bitcoind::NodeChainFamily::Bitcoin,
        )
        .unwrap();
        assert!(!prepare_inbound_tor_for_chain_locked(
            &root,
            crate::chain::ChainId::Bitcoin,
            &lifecycle,
        )
        .unwrap());
        let second = crate::node::bitcoind::Bitcoind::for_test(config, &root, Network::Bitcoin);
        drop(lifecycle);
        first.stop();
        assert!(process.lock().unwrap().try_wait().unwrap().is_none());
        second.stop();
        assert!(crate::node::bitcoind::wait_for_managed_node_cleanup(
            Duration::from_secs(120)
        ));
        assert!(process.lock().unwrap().try_wait().unwrap().is_some());
        std::fs::remove_dir_all(root.path()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn failed_start_guard_cleans_up_ownerless_tor() {
        let _registry = registry_test_guard();
        let root = temp_datadir("failed-start-tor");
        let lifecycle = crate::node::bitcoind::ManagedNodeLifecycle::acquire(
            &root,
            crate::node::bitcoind::NodeChainFamily::Bitcoin,
        )
        .unwrap();
        let child = std::process::Command::new("sleep")
            .arg("60")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let process = Arc::new(Mutex::new(child));
        let fixture = DisposableManagedChild(Tor {
            ports: TorPorts {
                control: 9051,
                socks: 9050,
            },
            process: process.clone(),
        });
        register_managed_tor_for(&root, fixture.0.clone());
        drop(lifecycle);
        assert!(process.lock().unwrap().try_wait().unwrap().is_some());
        std::fs::remove_dir_all(root.path()).unwrap();
    }

    /// Stops only this disposable child, even if setup or an assertion unwinds.
    /// The registry test lock is declared before this guard, so it remains held
    /// until the child has been deregistered and reaped.
    #[cfg(unix)]
    struct DisposableManagedChild(Tor);

    #[cfg(unix)]
    impl Drop for DisposableManagedChild {
        fn drop(&mut self) {
            let registered = registry()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_ref()
                .is_some_and(|owner| Arc::ptr_eq(&owner.process, &self.0.process));
            if registered {
                let owner = registry()
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .take();
                if let Some(owner) = owner {
                    owner.stop();
                }
            } else {
                // Also covers a panic between spawn and registration.
                self.0.stop();
            }
        }
    }

    fn temp_datadir(tag: &str) -> CoincubeDirectory {
        let seq = SEQ.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!(
            "coincube-tor-test-{}-{seq}-{tag}",
            std::process::id(),
        ));
        let _ = std::fs::remove_dir_all(&base);
        CoincubeDirectory::new(base)
    }

    #[cfg(unix)]
    #[test]
    fn app_and_loader_without_node_ownership_leave_global_tor_alone() {
        let _registry = registry_test_guard();
        let _session = crate::app::session::test_guard();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _enter = runtime.enter();
        let root = temp_datadir("lifecycle-owner");
        std::fs::create_dir_all(root.path()).unwrap();
        let child = std::process::Command::new("sleep")
            .arg("60")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("disposable managed-process fixture");
        let process = Arc::new(Mutex::new(child));
        let ports = TorPorts {
            control: 9051,
            socks: 9050,
        };
        let cleanup = DisposableManagedChild(Tor {
            ports,
            process: process.clone(),
        });
        register_managed_tor(cleanup.0.clone());
        let alive = || process.lock().unwrap().try_wait().unwrap().is_none();
        let (mut app, _) = crate::app::shell_tests::bitcoin_app(root.path());
        app.stop();
        assert!(alive(), "locking an App cannot stop process-global Tor");
        let (mut loader, startup) = crate::loader::Loader::new(
            root.clone(),
            crate::app::Config::new(false),
            Network::Bitcoin,
            None,
            None,
            None,
            app.cube_settings().clone(),
            None,
            None,
        );
        drop(startup);
        loader.stop();
        assert!(alive(), "closing a Loader cannot stop process-global Tor");
        let (mut gui, startup) =
            crate::gui::GUI::new(crate::gui::Config::new(root.clone(), None), None);
        drop(startup);
        drop(gui.update(crate::gui::Message::CtrlC));
        assert!(alive(), "window shutdown cannot stop another owner's Tor");
        stop_managed_tor();
        assert!(!alive());
        drop(gui);
        std::fs::remove_dir_all(root.path()).ok();
    }

    #[cfg(unix)]
    #[test]
    fn disposable_managed_child_is_reaped_on_unwind() {
        let _registry = registry_test_guard();
        let child = std::process::Command::new("sleep")
            .arg("60")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let process = Arc::new(Mutex::new(child));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe({
            let process = process.clone();
            move || {
                let cleanup = DisposableManagedChild(Tor {
                    ports: TorPorts {
                        control: 9051,
                        socks: 9050,
                    },
                    process,
                });
                register_managed_tor(cleanup.0.clone());
                assert!(managed_tor_ports().is_some());
                panic!("simulated setup failure after registration");
            }
        }));
        assert!(result.is_err());
        assert_eq!(managed_tor_ports(), None);
        assert!(process.lock().unwrap().try_wait().unwrap().is_some());
    }

    #[test]
    fn fork_tor_preparation_leaves_bitcoin_files_and_malformed_fork_files_untouched() {
        use crate::{chain::ChainId, node::bitcoind::NodeChainFamily};
        let _guard = registry_test_guard();
        for chain in [ChainId::BitcoinBlake2b, ChainId::BitcoinBlake2bTestnet4] {
            let root = temp_datadir("fork-refusal");
            assert!(!prepare_inbound_tor_for_chain(&root, chain).unwrap());
            assert!(!crate::node::bitcoind::internal_bitcoind_datadir_for(
                &root,
                NodeChainFamily::Bitcoin
            )
            .exists());
            let mut paths = Vec::new();
            for family in [NodeChainFamily::Bitcoin, NodeChainFamily::BitcoinBlake2b] {
                let dir = crate::node::bitcoind::internal_bitcoind_datadir_for(&root, family);
                std::fs::create_dir_all(&dir).unwrap();
                let path = dir.join("bitcoin.conf");
                std::fs::write(&path, b"sentinel: do not parse or rewrite").unwrap();
                paths.push(path);
            }
            assert!(prepare_inbound_tor_for_chain(&root, chain).is_err());
            for path in paths {
                assert_eq!(
                    std::fs::read(path).unwrap(),
                    b"sentinel: do not parse or rewrite"
                );
            }
            std::fs::remove_dir_all(root.path()).unwrap();
        }
    }

    #[test]
    fn bootstrap_line_detection() {
        assert!(line_reports_bootstrapped(
            "May 01 00:00:00.000 [notice] Bootstrapped 100% (done): Done"
        ));
        assert!(!line_reports_bootstrapped(
            "May 01 00:00:00.000 [notice] Bootstrapped 95% (circuit_create): Establishing"
        ));
        assert!(!line_reports_bootstrapped(
            "[notice] Opening Socks listener"
        ));
    }

    #[test]
    fn torrc_has_required_directives() {
        let datadir = Path::new("/tmp/coincube/tor-data");
        let geoip = Path::new("/tmp/coincube/tor-15/data");
        let torrc = torrc_contents(
            datadir,
            geoip,
            TorPorts {
                control: 9151,
                socks: 9150,
            },
        );
        assert!(torrc.contains("SocksPort 127.0.0.1:9150"));
        assert!(torrc.contains("ControlPort 127.0.0.1:9151"));
        assert!(torrc.contains("CookieAuthentication 1"));
        assert!(torrc.contains("ClientOnly 1"));
        assert!(torrc.contains("DataDirectory \"/tmp/coincube/tor-data\""));
        // GeoIP lines are omitted when the databases don't exist on disk.
        assert!(!torrc.contains("GeoIPFile"));
        // Logging goes to stdout (captured by us) — never `Log ... file`, which
        // tor's parser mishandles for paths containing spaces.
        assert!(torrc.contains("Log notice stdout"));
        assert!(!torrc.contains("Log notice file"));
    }

    #[test]
    fn preference_defaults_and_roundtrip() {
        // Config-layer default is OFF (backward compatible); product default ON.
        assert!(!InboundTorPreference::default().enabled);
        assert!(InboundTorPreference::default_enabled().enabled);
        assert!(InboundTorPreference::default_enabled().outbound_via_tor);

        let datadir = temp_datadir("pref");
        // Absent sidecar → disabled default.
        assert!(!InboundTorPreference::load(&datadir).enabled);

        // Round-trip through the sidecar.
        let pref = InboundTorPreference {
            enabled: true,
            outbound_via_tor: false,
            max_upload_target_mb_day: None,
            max_connections: Some(42),
        };
        pref.save(&datadir).expect("save preference");
        assert_eq!(InboundTorPreference::load(&datadir), pref);

        let _ = std::fs::remove_dir_all(datadir.path());
    }

    #[test]
    fn prepare_is_failsafe_without_tor_binary() {
        use crate::node::bitcoind::{InternalBitcoindConfig, InternalBitcoindNetworkConfig};

        let _registry = registry_test_guard();
        let datadir = temp_datadir("failsafe");
        // A managed-node config exists (Knots) but no tor binary is
        // installed, and the preference asks for inbound.
        let config_path = internal_bitcoind_config_path(&internal_bitcoind_datadir(&datadir));
        std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        let mut conf = InternalBitcoindConfig::for_flavor(crate::node::bitcoind::NodeFlavor::Knots);
        conf.networks.insert(
            Network::Bitcoin,
            InternalBitcoindNetworkConfig {
                rpc_port: 12345,
                p2p_port: 12346,
                prune: 15000,
                rpc_auth: None,
            },
        );
        conf.to_file(&config_path).unwrap();
        // The file also carries the legacy `consensusrules` line an older
        // release wrote: the outbound-only reset parses past it, and its
        // rewrite is what takes the line off the disk on this route.
        let written = std::fs::read_to_string(&config_path).unwrap();
        std::fs::write(&config_path, format!("consensusrules=rdts\n{written}")).unwrap();
        InboundTorPreference::default_enabled()
            .save(&datadir)
            .unwrap();

        // No tor binary on disk → prepare must fail-safe to outbound-only and
        // leave no inbound knobs in bitcoin.conf.
        let enabled = prepare_inbound_tor(&datadir, Network::Bitcoin).unwrap();
        assert!(
            !enabled,
            "must not report inbound enabled without a tor binary"
        );
        let reloaded = InternalBitcoindConfig::from_file(&config_path).unwrap();
        assert!(!reloaded.inbound_tor, "no listen/listenonion emitted");
        assert!(reloaded.tor_control_port.is_none());
        // The base config is preserved, and the legacy line is not carried
        // back to disk.
        assert_eq!(reloaded.networks.len(), 1);
        assert!(
            !std::fs::read_to_string(&config_path)
                .unwrap()
                .contains("consensusrules"),
            "the reset must not carry the legacy line back to disk"
        );
        // The user's preference is NOT clobbered by the failure — retried next launch.
        assert!(InboundTorPreference::load(&datadir).enabled);

        let _ = std::fs::remove_dir_all(datadir.path());
    }

    #[test]
    fn prepare_is_mainnet_only() {
        use crate::node::bitcoind::{InternalBitcoindConfig, InternalBitcoindNetworkConfig};

        let _registry = registry_test_guard();
        let datadir = temp_datadir("mainnet-only");
        let config_path = internal_bitcoind_config_path(&internal_bitcoind_datadir(&datadir));
        std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        let mut conf = InternalBitcoindConfig::for_flavor(crate::node::bitcoind::NodeFlavor::Knots);
        conf.networks.insert(
            Network::Signet,
            InternalBitcoindNetworkConfig {
                rpc_port: 12345,
                p2p_port: 12346,
                prune: 15000,
                rpc_auth: None,
            },
        );
        conf.to_file(&config_path).unwrap();
        // Preference enabled, but we're on a non-mainnet network.
        InboundTorPreference::default_enabled()
            .save(&datadir)
            .unwrap();

        let enabled = prepare_inbound_tor(&datadir, Network::Signet).unwrap();
        assert!(!enabled, "inbound-over-tor must be mainnet-only");
        let reloaded = InternalBitcoindConfig::from_file(&config_path).unwrap();
        assert!(!reloaded.inbound_tor, "no inbound knobs off-mainnet");
        assert!(reloaded.tor_control_port.is_none());
        // Base config preserved; preference untouched (still enabled for mainnet).
        assert_eq!(reloaded.networks.len(), 1);
        assert!(InboundTorPreference::load(&datadir).enabled);

        let _ = std::fs::remove_dir_all(datadir.path());
    }

    #[test]
    fn duress_targets_tor_data_and_onion_keys_not_blockchain() {
        let root = temp_datadir("duress");
        let root = root.path();
        let bitcoind = root.join("bitcoind");

        // Tor state + onion keys (identifying) — must be targeted.
        std::fs::create_dir_all(bitcoind.join("tor-data")).unwrap();
        std::fs::write(bitcoind.join("tor-data").join("state"), b"x").unwrap();
        std::fs::create_dir_all(bitcoind.join("datadir")).unwrap();
        std::fs::write(bitcoind.join("datadir").join("onion_v3_private_key"), b"k").unwrap();
        std::fs::create_dir_all(bitcoind.join("datadir").join("signet")).unwrap();
        std::fs::write(
            bitcoind
                .join("datadir")
                .join("signet")
                .join("onion_v3_private_key"),
            b"k",
        )
        .unwrap();
        // Blockchain (expensive, non-sensitive) — must NOT be targeted.
        std::fs::create_dir_all(bitcoind.join("datadir").join("blocks")).unwrap();
        std::fs::write(
            bitcoind.join("datadir").join("blocks").join("blk0.dat"),
            b"b",
        )
        .unwrap();

        let targets = duress_identifying_targets(root);
        assert!(
            targets.contains(&bitcoind.join("tor-data")),
            "tor data wiped"
        );
        assert!(
            targets.contains(&bitcoind.join("datadir").join("onion_v3_private_key")),
            "mainnet onion key wiped"
        );
        assert!(
            targets.contains(
                &bitcoind
                    .join("datadir")
                    .join("signet")
                    .join("onion_v3_private_key")
            ),
            "per-network onion key wiped"
        );
        assert!(
            !targets.iter().any(|t| t.ends_with("blk0.dat")),
            "blockchain preserved"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    // The outbound-only reset is a locked read-modify-write on a *fresh*
    // read: a network section another setup persists while this start waits
    // for the lock survives it. The other writer holds the lock, adds a
    // section and releases; `prepare_inbound_tor` waits (bounded), then
    // strips the inbound knobs from what is on disk *now*.
    #[test]
    fn prepare_strips_inbound_on_a_fresh_read_and_keeps_a_section_added_meanwhile() {
        use crate::node::bitcoind::{InternalBitcoindConfig, InternalBitcoindNetworkConfig};
        use crate::node::managed_conf::ManagedConfLock;

        let _registry = registry_test_guard();
        let datadir = temp_datadir("fresh-read");
        let config_path = internal_bitcoind_config_path(&internal_bitcoind_datadir(&datadir));
        std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        let section = |rpc_port: u16, p2p_port: u16| InternalBitcoindNetworkConfig {
            rpc_port,
            p2p_port,
            prune: 15000,
            rpc_auth: None,
        };
        let mut conf = InternalBitcoindConfig::for_flavor(crate::node::bitcoind::NodeFlavor::Knots);
        conf.networks.insert(Network::Signet, section(12345, 12346));
        conf.inbound_tor = true;
        conf.outbound_via_tor = true;
        conf.tor_control_port = Some(9051);
        conf.tor_socks_port = Some(9050);
        conf.to_file(&config_path).unwrap();

        // Handshake, not wall-clock: the writer takes the lock and waits to
        // be told to persist; while it holds, this thread proves contention
        // (a quick bounded acquire is `Busy`); then the writer persists its
        // section and releases; only then does the reset run — on a fresh
        // read that must include what the writer left behind.
        let (locked_tx, locked_rx) = std::sync::mpsc::channel::<()>();
        let (persist_tx, persist_rx) = std::sync::mpsc::channel::<()>();
        let writer = {
            let datadir = datadir.clone();
            let config_path = config_path.clone();
            std::thread::spawn(move || {
                let held = ManagedConfLock::acquire(&datadir).unwrap();
                locked_tx.send(()).unwrap();
                persist_rx.recv().unwrap();
                let mut conf = InternalBitcoindConfig::from_file(&config_path).unwrap();
                conf.networks
                    .insert(Network::Testnet4, section(22222, 22223));
                conf.to_file(&config_path).unwrap();
                drop(held);
            })
        };
        locked_rx.recv().unwrap();
        assert!(
            matches!(
                ManagedConfLock::acquire_with_bound(
                    &datadir,
                    2,
                    std::time::Duration::from_millis(5)
                ),
                Err(crate::node::managed_conf::ManagedConfLockError::Busy { .. })
            ),
            "the writer must be holding the lock at this point"
        );
        persist_tx.send(()).unwrap();
        writer.join().unwrap();
        // Signet: outbound-only by policy, so this is purely the reset write.
        let enabled = prepare_inbound_tor(&datadir, Network::Signet).unwrap();
        assert!(!enabled);
        let reloaded = InternalBitcoindConfig::from_file(&config_path).unwrap();
        assert!(!reloaded.inbound_tor);
        assert!(reloaded.tor_control_port.is_none());
        assert!(reloaded.tor_socks_port.is_none());
        assert_eq!(
            reloaded.networks.get(&Network::Signet),
            Some(&section(12345, 12346))
        );
        assert_eq!(
            reloaded.networks.get(&Network::Testnet4),
            Some(&section(22222, 22223)),
            "the section persisted while we waited must survive the reset"
        );
        let _ = std::fs::remove_dir_all(datadir.path());
    }

    // A conf that cannot be brought to a known state — the lock is busy for
    // the whole bounded wait — is a refusal, not an outbound-only success:
    // the file still carries the previous run's inbound lines, and the
    // caller must not start bitcoind from it.
    #[test]
    fn prepare_refuses_on_a_busy_lock_and_leaves_the_conf_untouched() {
        use crate::node::bitcoind::{InternalBitcoindConfig, InternalBitcoindNetworkConfig};
        use crate::node::managed_conf::ManagedConfLock;

        let _registry = registry_test_guard();
        let datadir = temp_datadir("busy");
        let config_path = internal_bitcoind_config_path(&internal_bitcoind_datadir(&datadir));
        std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        let mut conf = InternalBitcoindConfig::for_flavor(crate::node::bitcoind::NodeFlavor::Knots);
        conf.networks.insert(
            Network::Bitcoin,
            InternalBitcoindNetworkConfig {
                rpc_port: 12345,
                p2p_port: 12346,
                prune: 15000,
                rpc_auth: None,
            },
        );
        conf.inbound_tor = true;
        conf.tor_control_port = Some(9051);
        conf.tor_socks_port = Some(9050);
        conf.to_file(&config_path).unwrap();
        let before = std::fs::read(&config_path).unwrap();
        assert!(String::from_utf8_lossy(&before).contains("torcontrol"));

        let held = ManagedConfLock::acquire(&datadir).unwrap();
        let result = crate::node::managed_conf::with_quick_lock_bound(|| {
            prepare_inbound_tor(&datadir, Network::Bitcoin)
        });
        drop(held);
        match result {
            Err(PrepareInboundTorError(crate::node::managed_conf::ManagedConfError::Lock(
                crate::node::managed_conf::ManagedConfLockError::Busy { .. },
            ))) => {}
            other => panic!("expected a Busy refusal, got {:?}", other.map(|_| ())),
        }
        // Byte-identical: the stale inbound lines are still there, which is
        // exactly why the caller must not start from this file.
        assert_eq!(std::fs::read(&config_path).unwrap(), before);
        assert!(managed_tor_ports().is_none(), "no Tor was started");
        let _ = std::fs::remove_dir_all(datadir.path());
    }

    // No managed conf at all is not an error and writes nothing: an external
    // backend has nothing for this function to do.
    #[test]
    fn prepare_is_a_no_op_without_a_managed_conf() {
        let _registry = registry_test_guard();
        let datadir = temp_datadir("no-conf");
        std::fs::create_dir_all(datadir.path()).unwrap();
        InboundTorPreference::default_enabled()
            .save(&datadir)
            .unwrap();
        assert!(!prepare_inbound_tor(&datadir, Network::Bitcoin).unwrap());
        assert!(!internal_bitcoind_config_path(&internal_bitcoind_datadir(&datadir)).exists());
        let _ = std::fs::remove_dir_all(datadir.path());
    }

    // A refused reset must not take the process-global managed Tor down with
    // it: a sibling session's node in this process may still be using it,
    // and the conf — deliberately untouched on refusal — still names it. The
    // stop happens only once the outbound-only reset is durably in place,
    // after which the success and "no conf" paths stop it exactly as before.
    // The "Tor" here is a disposable child process registered as the managed
    // one; no real Tor or node.
    #[cfg(unix)]
    #[test]
    fn a_refused_reset_leaves_the_registered_tor_running_and_success_still_stops_it() {
        use crate::node::bitcoind::{InternalBitcoindConfig, InternalBitcoindNetworkConfig};
        use crate::node::managed_conf::{with_quick_lock_bound, ManagedConfLock};

        let _registry = registry_test_guard();
        let datadir = temp_datadir("tor-survives-refusal");
        let config_path = internal_bitcoind_config_path(&internal_bitcoind_datadir(&datadir));
        std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        let mut conf = InternalBitcoindConfig::for_flavor(crate::node::bitcoind::NodeFlavor::Knots);
        conf.networks.insert(
            Network::Bitcoin,
            InternalBitcoindNetworkConfig {
                rpc_port: 12345,
                p2p_port: 12346,
                prune: 15000,
                rpc_auth: None,
            },
        );
        conf.inbound_tor = true;
        conf.outbound_via_tor = true;
        conf.tor_control_port = Some(9051);
        conf.tor_socks_port = Some(9050);
        conf.to_file(&config_path).unwrap();
        let before = std::fs::read(&config_path).unwrap();
        InboundTorPreference::default_enabled()
            .save(&datadir)
            .unwrap();

        // A disposable process standing in for a running managed Tor, and a
        // handle on it that outlives the registry entry so its fate can be
        // observed either way.
        let register_fake =
            |root: &CoincubeDirectory, ports: TorPorts| -> Arc<Mutex<std::process::Child>> {
                let child = std::process::Command::new("sleep")
                    .arg("60")
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .expect("spawn a disposable child");
                let process = Arc::new(Mutex::new(child));
                register_managed_tor_for(
                    root,
                    Tor {
                        ports,
                        process: process.clone(),
                    },
                );
                process
            };
        let alive = |process: &Arc<Mutex<std::process::Child>>| -> bool {
            process.lock().unwrap().try_wait().unwrap().is_none()
        };
        let ports = TorPorts {
            control: 9051,
            socks: 9050,
        };
        let process = register_fake(&datadir, ports);
        assert_eq!(managed_tor_ports(), Some(ports));
        assert!(alive(&process));
        // Preparing a Blake2b companion must also leave this registered
        // Bitcoin Tor child alive, while clearing only the fork's config.
        let fork_dir = crate::node::bitcoind::internal_bitcoind_datadir_for(
            &datadir,
            crate::node::bitcoind::NodeChainFamily::BitcoinBlake2b,
        );
        std::fs::create_dir_all(&fork_dir).unwrap();
        let fork_path = internal_bitcoind_config_path(&fork_dir);
        conf.to_file(&fork_path).unwrap();
        assert!(
            !prepare_inbound_tor_for_chain(&datadir, crate::chain::ChainId::BitcoinBlake2b)
                .unwrap()
        );
        assert!(alive(&process));
        assert_eq!(managed_tor_ports(), Some(ports));
        assert_eq!(std::fs::read(&config_path).unwrap(), before);
        let fork = InternalBitcoindConfig::from_file(&fork_path).unwrap();
        assert!(!fork.inbound_tor);
        assert!(fork.tor_control_port.is_none());
        assert_eq!(fork.networks.len(), conf.networks.len());

        // Refusal: the lock is busy for the whole (quick) bounded wait.
        let held = ManagedConfLock::acquire(&datadir).unwrap();
        let result = with_quick_lock_bound(|| prepare_inbound_tor(&datadir, Network::Bitcoin));
        drop(held);
        assert!(
            matches!(
                result,
                Err(PrepareInboundTorError(
                    crate::node::managed_conf::ManagedConfError::Lock(
                        crate::node::managed_conf::ManagedConfLockError::Busy { .. },
                    )
                ))
            ),
            "expected a Busy refusal"
        );
        // The registered Tor is untouched — still registered, still running —
        // and so is the conf that names it.
        assert_eq!(
            managed_tor_ports(),
            Some(ports),
            "the refusal deregistered Tor"
        );
        assert!(
            alive(&process),
            "the refusal stopped the registered process"
        );
        assert_eq!(std::fs::read(&config_path).unwrap(), before);

        // Success path (lock free; no tor binary, so Tor stays off): the reset
        // is persisted first, then the previous Tor is stopped and
        // deregistered, as it always was.
        let enabled = prepare_inbound_tor(&datadir, Network::Bitcoin).unwrap();
        assert!(!enabled);
        assert_eq!(
            managed_tor_ports(),
            None,
            "success must replace the previous Tor"
        );
        assert!(!alive(&process), "success must stop the previous process");
        let reloaded = InternalBitcoindConfig::from_file(&config_path).unwrap();
        assert!(!reloaded.inbound_tor);
        assert!(reloaded.tor_control_port.is_none());

        // "No conf" path: also stops the previous Tor, as before.
        let bare = temp_datadir("tor-no-conf");
        std::fs::create_dir_all(bare.path()).unwrap();
        let process = register_fake(&bare, ports);
        assert!(!prepare_inbound_tor(&bare, Network::Bitcoin).unwrap());
        assert_eq!(managed_tor_ports(), None);
        assert!(!alive(&process));

        let _ = std::fs::remove_dir_all(datadir.path());
        let _ = std::fs::remove_dir_all(bare.path());
    }
}
