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
use crate::node::bitcoind::{
    internal_bitcoind_config_path, internal_bitcoind_datadir, internal_bitcoind_directory,
    internal_tor_directory, internal_tor_exe_path, internal_tor_geoip_dir, tor_download_url,
    tor_supported_on_host, InternalBitcoindConfig, MAX_CONNECTIONS_DEFAULT,
    MAX_UPLOAD_TARGET_MB_DAY_DEFAULT, TOR_VERSION,
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
    pub fn start(
        coincube_datadir: &CoincubeDirectory,
        version: &str,
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

        let ports = allocate_ports()?;
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
fn allocate_ports() -> Result<TorPorts, StartTorError> {
    use crate::installer::step::node::bitcoind::get_available_port;
    let control = get_available_port().map_err(|e| StartTorError::Port(e.to_string()))?;
    let socks = get_available_port().map_err(|e| StartTorError::Port(e.to_string()))?;
    if control == socks {
        return Err(StartTorError::Port("ports collided".to_string()));
    }
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

/// Stop and deregister the managed Tor, if any. Idempotent; call from every app
/// shutdown path alongside stopping the managed bitcoind.
pub fn stop_managed_tor() {
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

/// Start Tor (if the user wants inbound and it's available) and rewrite
/// `bitcoin.conf` so bitcoind emits the right inbound knobs for **this** run.
/// Call this immediately before starting the managed bitcoind.
///
/// Fail-safe and infallible by design: any problem (preference off, wrong
/// network, platform unsupported, binary missing, bootstrap timeout, I/O error)
/// leaves `bitcoin.conf` with **no** inbound knobs, so bitcoind runs
/// outbound-only exactly as today. Returns `true` iff Tor is up and bitcoind
/// will host an onion service. The user's preference sidecar is never modified
/// here, so a transient failure is retried next launch.
///
/// Inbound-over-Tor is **mainnet-only**: the managed enforcing node exists to
/// defend mainnet, and onion reachability has no value on test networks, so we
/// run outbound-only on anything but [`Network::Bitcoin`].
pub fn prepare_inbound_tor(coincube_datadir: &CoincubeDirectory, network: Network) -> bool {
    // Any prior managed tor from this process is replaced.
    stop_managed_tor();

    let config_path = internal_bitcoind_config_path(&internal_bitcoind_datadir(coincube_datadir));
    let Ok(base) = InternalBitcoindConfig::from_file(&config_path) else {
        // No managed-node config on disk → nothing to do (external backend).
        return false;
    };

    // Always start from a clean inbound state; whatever the file happened to
    // carry (stale ports from a previous run) is derived fresh below.
    let mut conf = base;
    let write_outbound_only = |mut conf: InternalBitcoindConfig| {
        conf.inbound_tor = false;
        conf.outbound_via_tor = false;
        conf.max_upload_target_mb_day = None;
        conf.max_connections = None;
        conf.tor_control_port = None;
        conf.tor_socks_port = None;
        if let Err(e) = conf.to_file(&config_path) {
            warn!("failed to write outbound-only bitcoin.conf: {e}");
        }
    };

    // Mainnet-only (see the doc comment above).
    if network != Network::Bitcoin {
        write_outbound_only(conf);
        return false;
    }

    let pref = InboundTorPreference::load(coincube_datadir);
    if !pref.enabled {
        write_outbound_only(conf);
        return false;
    }
    if !tor_supported_on_host() {
        info!("inbound-over-tor requested but unavailable on this platform; running outbound-only");
        write_outbound_only(conf);
        return false;
    }

    match Tor::start(coincube_datadir, TOR_VERSION) {
        Ok(tor) => {
            let ports = tor.ports();
            conf.inbound_tor = true;
            conf.outbound_via_tor = pref.outbound_via_tor;
            conf.max_upload_target_mb_day = pref.max_upload_target_mb_day;
            conf.max_connections = pref.max_connections;
            conf.tor_control_port = Some(ports.control);
            conf.tor_socks_port = Some(ports.socks);
            if let Err(e) = conf.to_file(&config_path) {
                // Couldn't persist the inbound config → don't start bitcoind
                // against a stale conf; tear tor down and run outbound-only.
                warn!("failed to write inbound bitcoin.conf ({e}); running outbound-only");
                tor.stop();
                write_outbound_only(conf);
                return false;
            }
            register_managed_tor(tor);
            info!("inbound-over-tor enabled; bitcoind will host a v3 onion service");
            true
        }
        Err(e) => {
            warn!("managed tor did not start ({e}); inbound unavailable, running outbound-only");
            write_outbound_only(conf);
            false
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

    fn temp_datadir(tag: &str) -> CoincubeDirectory {
        let seq = SEQ.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!(
            "coincube-tor-test-{}-{seq}-{tag}",
            std::process::id(),
        ));
        let _ = std::fs::remove_dir_all(&base);
        CoincubeDirectory::new(base)
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
        InboundTorPreference::default_enabled()
            .save(&datadir)
            .unwrap();

        // No tor binary on disk → prepare must fail-safe to outbound-only and
        // leave no inbound knobs in bitcoin.conf.
        let enabled = prepare_inbound_tor(&datadir, Network::Bitcoin);
        assert!(
            !enabled,
            "must not report inbound enabled without a tor binary"
        );
        let reloaded = InternalBitcoindConfig::from_file(&config_path).unwrap();
        assert!(!reloaded.inbound_tor, "no listen/listenonion emitted");
        assert!(reloaded.tor_control_port.is_none());
        // The base config is preserved.
        assert_eq!(reloaded.networks.len(), 1);
        // The user's preference is NOT clobbered by the failure — retried next launch.
        assert!(InboundTorPreference::load(&datadir).enabled);

        let _ = std::fs::remove_dir_all(datadir.path());
    }

    #[test]
    fn prepare_is_mainnet_only() {
        use crate::node::bitcoind::{InternalBitcoindConfig, InternalBitcoindNetworkConfig};

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

        let enabled = prepare_inbound_tor(&datadir, Network::Signet);
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
}
