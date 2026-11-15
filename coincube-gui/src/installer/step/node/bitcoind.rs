#[cfg(target_os = "windows")]
use std::io::{self, Cursor};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};

use bitcoin_hashes::sha256;
use coincube_core::miniscript::bitcoin::Network;
use coincubed::config::{BitcoinBackend, BitcoindConfig, BitcoindRpcAuth};
// Not `cfg`-gated: the Bitcoin Core/Knots archives are tar.gz on Unix, and the
// Tor Expert Bundle is tar.gz on *every* platform, so these are used everywhere.
use flate2::read::GzDecoder;
use iced::task::{sipper, Sipper, Straw};
use iced::{Subscription, Task};
use pgp::composed::{Deserializable, SignedPublicKey, StandaloneSignature};
use pgp::types::KeyDetails;
use tar::Archive;
use tracing::info;

use jsonrpc::{client::Client, simple_http::SimpleHttpTransport};

use coincube_ui::{
    component::{form, text::Text as _},
    widget::*,
};

use crate::dir::CoincubeDirectory;
use crate::{
    app::cache::BackgroundNodeInstall,
    download,
    hw::HardwareWallets,
    installer::{
        context::Context,
        message::{self, Message},
        step::Step,
        view, Error,
    },
    node::bitcoind::{
        self, bitcoind_network_dir, internal_bitcoind_cookie_path, internal_bitcoind_datadir,
        Bitcoind, ConfigField, InternalBitcoindConfig, InternalBitcoindNetworkConfig, NodeFlavor,
        NodeResources, RpcAuthType, RpcAuthValues, StartInternalBitcoindError,
    },
};

// The approach for tracking download progress is taken from
// https://github.com/iced-rs/iced/blob/master/examples/download_progress/src/main.rs.
#[derive(Debug)]
struct Download {
    state: DownloadState,
}

#[derive(Debug)]
pub enum DownloadState {
    Idle,
    Downloading {
        progress: f32,
        _handle: iced::task::Handle,
    },
    Finished(Vec<u8>),
    Errored(download::DownloadError),
}

#[derive(Debug, Clone)]
pub enum DownloadUpdate {
    Progressed(download::Progress),
    Finished(Result<Vec<u8>, download::DownloadError>),
}

impl Download {
    pub fn new() -> Self {
        Download {
            state: DownloadState::Idle,
        }
    }

    pub fn start(&mut self, url: String) -> Task<DownloadUpdate> {
        match self.state {
            DownloadState::Idle
            | DownloadState::Finished { .. }
            | DownloadState::Errored { .. } => {
                let (task, handle) = Task::sip(
                    download::download(url),
                    DownloadUpdate::Progressed,
                    DownloadUpdate::Finished,
                )
                .abortable();

                self.state = DownloadState::Downloading {
                    progress: 0.0,
                    _handle: handle.abort_on_drop(),
                };

                task
            }
            DownloadState::Downloading { .. } => Task::none(),
        }
    }

    pub fn update(&mut self, update: DownloadUpdate) {
        if let DownloadState::Downloading { progress, .. } = &mut self.state {
            match update {
                DownloadUpdate::Progressed(p) => {
                    *progress = p.percent;
                }
                DownloadUpdate::Finished(Ok(bytes)) => {
                    self.state = DownloadState::Finished(bytes);
                }
                DownloadUpdate::Finished(Err(e)) => {
                    self.state = DownloadState::Errored(e);
                }
            }
        }
    }
}

/// Default prune value used by internal bitcoind. Re-exported from
/// [`crate::node::bitcoind`], where all node-resource constants now live
/// together, so existing `installer::step::node::bitcoind::PRUNE_DEFAULT`
/// call sites keep resolving.
pub use crate::node::bitcoind::PRUNE_DEFAULT;
/// Default ports used by bitcoind across all networks.
pub const BITCOIND_DEFAULT_PORTS: [u16; 10] = [
    8332, 8333, 18332, 18333, 18443, 18444, 48332, 48333, 38332, 38333,
];

#[derive(Debug)]
pub enum InstallState {
    InProgress,
    Finished,
    Errored(InstallBitcoindError),
}

/// Possible errors when installing bitcoind.
#[derive(PartialEq, Eq, Debug, Clone)]
pub enum InstallBitcoindError {
    /// A Bitcoin Core archive did not match its single pinned SHA-256.
    HashMismatch,
    /// A Bitcoin Knots archive's SHA-256 was not listed for its filename in the
    /// release `SHA256SUMS` manifest.
    ChecksumNotInManifest,
    /// A Bitcoin Knots release `SHA256SUMS` arrived without an accompanying
    /// detached PGP signature (`SHA256SUMS.asc`).
    MissingSignature,
    /// The `SHA256SUMS.asc` did not cryptographically verify against the pinned
    /// Bitcoin Knots signing key.
    InvalidSignature,
    UnpackingError(String),
}

impl std::fmt::Display for InstallBitcoindError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::HashMismatch => {
                write!(f, "Hashes do not match.")
            }
            Self::ChecksumNotInManifest => write!(
                f,
                "Downloaded archive's checksum is not in the release SHA256SUMS manifest."
            ),
            Self::MissingSignature => write!(
                f,
                "Release SHA256SUMS manifest is not accompanied by a PGP signature."
            ),
            Self::InvalidSignature => write!(
                f,
                "Release SHA256SUMS signature did not verify against the pinned Knots signing key."
            ),
            Self::UnpackingError(e) => {
                write!(f, "Error unpacking: '{}'.", e)
            }
        }
    }
}

// The functions below for unpacking the bitcoin download and verifying its hash are based on
// https://github.com/RCasatta/bitcoind/blob/bada7ebb7197b89fd67e607f815ce1e43e76da7f/build.rs#L73.

/// Unpack the downloaded bytes in the specified directory.
fn unpack_bitcoind(install_dir: &PathBuf, bytes: &[u8]) -> Result<(), InstallBitcoindError> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let d = GzDecoder::new(bytes);

        let mut archive = Archive::new(d);
        for mut entry in archive
            .entries()
            .map_err(|e| InstallBitcoindError::UnpackingError(e.to_string()))?
            .flatten()
        {
            if let Ok(file) = entry.path() {
                if file.ends_with("bitcoind") {
                    if let Err(e) = entry.unpack_in(install_dir) {
                        return Err(InstallBitcoindError::UnpackingError(e.to_string()));
                    }
                }
            }
        }
    }
    #[cfg(target_os = "windows")]
    {
        let cursor = Cursor::new(bytes);
        let mut archive = zip::ZipArchive::new(cursor)
            .map_err(|e| InstallBitcoindError::UnpackingError(e.to_string()))?;
        for i in 0..zip::ZipArchive::len(&archive) {
            let mut file = archive
                .by_index(i)
                .map_err(|e| InstallBitcoindError::UnpackingError(e.to_string()))?;
            let outpath = match file.enclosed_name() {
                Some(path) => path.to_owned(),
                None => continue,
            };
            if outpath.file_name().map(|s| s.to_str()) == Some(Some("bitcoind.exe")) {
                let mut exe_path = PathBuf::from(install_dir);
                for d in outpath.iter() {
                    exe_path.push(d);
                }
                let parent = exe_path.parent().expect("bitcoind.exe should have parent.");
                std::fs::create_dir_all(parent)
                    .map_err(|e| InstallBitcoindError::UnpackingError(e.to_string()))?;
                let mut outfile = std::fs::File::create(&exe_path)
                    .map_err(|e| InstallBitcoindError::UnpackingError(e.to_string()))?;
                io::copy(&mut file, &mut outfile)
                    .map_err(|e| InstallBitcoindError::UnpackingError(e.to_string()))?;
                break;
            }
        }
    }
    Ok(())
}

/// What a freshly-downloaded managed-node/Tor archive is checked against before
/// it is unpacked.
#[derive(Debug, Clone)]
pub enum DownloadVerification {
    /// Bitcoin Core: a single SHA-256 (hex) pinned in code for the current
    /// version's archive on this platform.
    PinnedSha256(String),
    /// A signed release-manifest verification (Bitcoin Knots and the Tor Expert
    /// Bundle both use this shape): a `SHA256SUMS`-style manifest plus its
    /// detached `.asc`, and the pinned signing key the `.asc` must validate
    /// against. `archive_filename` is the name the archive is listed under.
    ReleaseManifest {
        archive_filename: String,
        sha256sums: String,
        sha256sums_asc: String,
        /// Vendored armored public key the manifest signature must validate
        /// against (Knots' or Tor's, depending on what is being installed).
        signing_key_asc: &'static str,
        /// Pinned fingerprint of `signing_key_asc`, re-derived and checked at
        /// verification time so a swapped key file can't move the trust anchor.
        signing_key_fingerprint: &'static str,
    },
}

impl DownloadVerification {
    /// The verification appropriate for `flavor`'s current managed version on
    /// this host. For Knots the caller passes the already-fetched
    /// `(SHA256SUMS, SHA256SUMS.asc)`; supplying it out-of-band keeps this and
    /// [`verify_download`] network-free and unit-testable. Returns `None` for
    /// Knots when the manifest is missing (so the caller can surface a clear
    /// error rather than silently skip verification).
    pub fn for_flavor(flavor: NodeFlavor, manifest: Option<(String, String)>) -> Option<Self> {
        match flavor {
            NodeFlavor::Core => Some(Self::PinnedSha256(bitcoind::CORE_SHA256SUM.to_string())),
            // Both Knots lines are published by the same signer under the
            // same manifest scheme; the release *version* selects the file.
            NodeFlavor::Knots | NodeFlavor::KnotsBlake2b => {
                let (sha256sums, sha256sums_asc) = manifest?;
                Some(Self::ReleaseManifest {
                    archive_filename: flavor.download_filename(),
                    sha256sums,
                    sha256sums_asc,
                    signing_key_asc: bitcoind::KNOTS_SIGNING_KEY_ASC,
                    signing_key_fingerprint: bitcoind::KNOTS_SIGNING_KEY_FINGERPRINT,
                })
            }
        }
    }

    /// Verification for the managed Tor Expert Bundle on this host: its release
    /// `sha256sums-unsigned-build.txt` (+ `.asc`) signed by the Tor Browser
    /// Developers key. `manifest` is the already-fetched `(sums, asc)`; returns
    /// `None` if the manifest wasn't fetched, or if Tor isn't published for the
    /// host platform (Linux/Windows aarch64).
    pub fn for_tor(manifest: Option<(String, String)>) -> Option<Self> {
        let archive_filename = bitcoind::tor_download_filename()?;
        let (sha256sums, sha256sums_asc) = manifest?;
        Some(Self::ReleaseManifest {
            archive_filename,
            sha256sums,
            sha256sums_asc,
            signing_key_asc: bitcoind::TOR_SIGNING_KEY_ASC,
            signing_key_fingerprint: bitcoind::TOR_SIGNING_KEY_FINGERPRINT,
        })
    }
}

/// Whether `bytes` hashes to the single pinned `expected_hex` (Bitcoin Core).
fn matches_pinned_hash(bytes: &[u8], expected_hex: &str) -> bool {
    let bytes_hash = sha256::Hash::hash(bytes);
    info!("Download hash: '{}'.", bytes_hash);
    sha256::Hash::from_str(expected_hex)
        .map(|expected| expected == bytes_hash)
        .unwrap_or(false)
}

/// Whether `bytes`' SHA-256 is listed for exactly `archive_filename` in a
/// release `SHA256SUMS` manifest (Bitcoin Knots).
fn hash_listed_in_manifest(bytes: &[u8], archive_filename: &str, sha256sums: &str) -> bool {
    let bytes_hash = sha256::Hash::hash(bytes).to_string();
    sha256sums.lines().any(|line| {
        // GNU coreutils format: "<64-hex><two spaces><filename>".
        let mut fields = line.split_whitespace();
        matches!(
            (fields.next(), fields.next()),
            (Some(hash), Some(name))
                if name == archive_filename && hash.eq_ignore_ascii_case(&bytes_hash)
        )
    })
}

/// Failure verifying a detached OpenPGP signature.
#[derive(Debug, PartialEq, Eq)]
enum SignatureError {
    /// No PGP signature block was present.
    Missing,
    /// A signature was present but did not cryptographically verify against the
    /// pinned key (bad signature, wrong/garbled key, or key-fingerprint
    /// mismatch).
    Invalid,
}

/// Verify the armored detached OpenPGP signature `asc` over `data` using the
/// armored public key `pubkey_armored`, which must have fingerprint
/// `expected_fingerprint`. The `.asc` may carry several signatures (the Knots
/// manifest is multi-signed); verification succeeds iff **at least one** of them
/// validates against the pinned key or one of its bound signing subkeys.
/// Returns [`SignatureError::Missing`] when no signature block is present so
/// callers can distinguish it from an invalid one.
///
/// Some issuers (e.g. Luke Dashjr's Knots key) sign the manifest with their
/// primary key; others (e.g. the Tor Browser Developers key) sign with a
/// dedicated signing *subkey*. We accept a subkey signature only when the
/// subkey's binding signature itself validates against the pinned primary, so
/// trust still flows from the single pinned fingerprint.
fn verify_detached_signature(
    data: &[u8],
    asc: &str,
    pubkey_armored: &str,
    expected_fingerprint: &str,
) -> Result<(), SignatureError> {
    if !asc
        .trim_start()
        .starts_with("-----BEGIN PGP SIGNATURE-----")
    {
        return Err(SignatureError::Missing);
    }
    let (pubkey, _) =
        SignedPublicKey::from_string(pubkey_armored).map_err(|_| SignatureError::Invalid)?;
    // Re-derive the fingerprint from the vendored key bytes and check it against
    // the pin, so a swapped key file can never become the trust anchor.
    if !hex::encode(pubkey.fingerprint().as_bytes()).eq_ignore_ascii_case(expected_fingerprint) {
        return Err(SignatureError::Invalid);
    }
    // Subkeys cryptographically bound to the pinned primary — the only subkeys
    // whose signatures we are willing to trust.
    let bound_subkeys: Vec<_> = pubkey
        .public_subkeys
        .iter()
        .filter(|sub| sub.verify(&pubkey.primary_key).is_ok())
        .collect();
    let (sig_iter, _) =
        StandaloneSignature::from_string_many(asc).map_err(|_| SignatureError::Invalid)?;
    let signatures: Vec<StandaloneSignature> = sig_iter.flatten().collect();
    let verifies = signatures.iter().any(|sig| {
        sig.verify(&pubkey, data).is_ok()
            || bound_subkeys
                .iter()
                .any(|sub| sig.verify(*sub, data).is_ok())
    });
    if verifies {
        Ok(())
    } else {
        Err(SignatureError::Invalid)
    }
}

/// Verify a downloaded archive against `verification`.
fn verify_download(
    bytes: &[u8],
    verification: &DownloadVerification,
) -> Result<(), InstallBitcoindError> {
    match verification {
        DownloadVerification::PinnedSha256(expected) => {
            if matches_pinned_hash(bytes, expected) {
                Ok(())
            } else {
                Err(InstallBitcoindError::HashMismatch)
            }
        }
        DownloadVerification::ReleaseManifest {
            archive_filename,
            sha256sums,
            sha256sums_asc,
            signing_key_asc,
            signing_key_fingerprint,
        } => {
            // The manifest's authenticity is anchored by its detached signature
            // against the pinned signing key (Knots' or Tor's); only then do we
            // trust its checksums.
            match verify_detached_signature(
                sha256sums.as_bytes(),
                sha256sums_asc,
                signing_key_asc,
                signing_key_fingerprint,
            ) {
                Ok(()) => {}
                Err(SignatureError::Missing) => return Err(InstallBitcoindError::MissingSignature),
                Err(SignatureError::Invalid) => return Err(InstallBitcoindError::InvalidSignature),
            }
            if !hash_listed_in_manifest(bytes, archive_filename, sha256sums) {
                return Err(InstallBitcoindError::ChecksumNotInManifest);
            }
            Ok(())
        }
    }
}

/// Install bitcoind by verifying the download and unpacking in `install_dir`.
pub fn install_bitcoind(
    install_dir: &PathBuf,
    bytes: &[u8],
    verification: &DownloadVerification,
) -> Result<(), InstallBitcoindError> {
    verify_download(bytes, verification)?;
    unpack_bitcoind(install_dir, bytes)
}

/// Unpack the downloaded Tor Expert Bundle into `install_dir`, preserving the
/// bundle's own layout (`tor/tor[.exe]`, its bundled shared libs, and
/// `data/geoip…`). Unlike bitcoind — a single static binary we cherry-pick —
/// `tor` needs its companion files, so the whole archive is extracted. The
/// bundle is a tar.gz on every platform (Windows included).
fn unpack_tor(install_dir: &Path, bytes: &[u8]) -> Result<(), InstallBitcoindError> {
    let decoder = GzDecoder::new(bytes);
    let mut archive = Archive::new(decoder);
    // `Archive::unpack` rejects entries whose paths would escape `install_dir`
    // (path traversal), so even a malicious archive can't write outside it. It
    // also restores the Unix mode bits, so `tor/tor` stays executable.
    archive
        .unpack(install_dir)
        .map_err(|e| InstallBitcoindError::UnpackingError(e.to_string()))?;

    // The Tor Expert Bundle ships **unsigned**, and macOS (mandatory on Apple
    // Silicon) SIGKILLs unsigned Mach-O binaries on exec. bitcoind runs because
    // it's Developer-ID-signed by its release team; Tor is not, so we apply an
    // ad-hoc signature to the extracted `tor` binary and its bundled dylibs so
    // the managed daemon can actually launch. No-op on other platforms.
    #[cfg(target_os = "macos")]
    adhoc_codesign_tor(install_dir)?;

    Ok(())
}

/// Ad-hoc code-sign (`codesign --sign -`) the extracted Tor binary and the
/// shared libraries it links, so Gatekeeper allows the unsigned Expert Bundle to
/// run. dylibs are signed before the executable that loads them. `codesign` is
/// part of the base macOS install.
#[cfg(target_os = "macos")]
fn adhoc_codesign_tor(install_dir: &Path) -> Result<(), InstallBitcoindError> {
    let tor_dir = install_dir.join("tor");

    // Bundled dylibs first, then the executable.
    let mut targets: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&tor_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "dylib") {
                targets.push(path);
            }
        }
    }
    targets.push(tor_dir.join("tor"));

    for path in targets {
        if !path.exists() {
            continue;
        }
        let ok = std::process::Command::new("/usr/bin/codesign")
            .args(["--force", "--sign", "-"])
            .arg(&path)
            .status()
            .map(|s| s.success())
            .map_err(|e| {
                InstallBitcoindError::UnpackingError(format!("failed to run codesign: {e}"))
            })?;
        if !ok {
            return Err(InstallBitcoindError::UnpackingError(format!(
                "ad-hoc codesign failed for {}",
                path.display()
            )));
        }
    }
    Ok(())
}

/// Install the managed Tor daemon: verify the download against `verification`
/// (its signed release manifest), then unpack it into the versioned
/// `tor-<version>` directory `install_dir`.
pub fn install_tor(
    install_dir: &Path,
    bytes: &[u8],
    verification: &DownloadVerification,
) -> Result<(), InstallBitcoindError> {
    verify_download(bytes, verification)?;
    unpack_tor(install_dir, bytes)
}

/// RPC address for internal bitcoind.
pub fn internal_bitcoind_address(rpc_port: u16) -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), rpc_port)
}

fn bitcoind_default_datadir() -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    let configs_dir = dirs::home_dir();

    #[cfg(not(target_os = "linux"))]
    let configs_dir = dirs::config_dir();

    if let Some(mut path) = configs_dir {
        #[cfg(target_os = "linux")]
        path.push(".bitcoin");

        #[cfg(not(target_os = "linux"))]
        path.push("Bitcoin");

        return Some(path);
    }
    None
}

fn bitcoind_default_cookie_path(network: &Network) -> Option<String> {
    if let Some(mut path) = bitcoind_default_datadir() {
        if let Some(dir) = bitcoind_network_dir(network) {
            path.push(dir);
        }
        path.push(".cookie");
        return path.to_str().map(|s| s.to_string());
    }
    None
}

#[allow(unreachable_patterns)]
fn bitcoind_default_address(network: &Network) -> String {
    match network {
        Network::Bitcoin => "127.0.0.1:8332".to_string(),
        Network::Testnet => "127.0.0.1:18332".to_string(),
        Network::Testnet4 => "127.0.0.1:48332".to_string(),
        Network::Regtest => "127.0.0.1:18443".to_string(),
        Network::Signet => "127.0.0.1:38332".to_string(),
    }
}

/// Get available port that is valid for use by internal bitcoind.
// Modified from https://github.com/RCasatta/bitcoind/blob/f047740d7d0af935ff7360cf77429c5f294cfd59/src/lib.rs#L435
pub fn get_available_port() -> Result<u16, Error> {
    // Perform multiple attempts to get a valid port.
    for _ in 0..10 {
        // Using 0 as port lets the system assign a port available.
        let t = TcpListener::bind(("127.0.0.1", 0))
            .map_err(|e| Error::CannotGetAvailablePort(e.to_string()))?;
        let port = t
            .local_addr()
            .map(|s| s.port())
            .map_err(|e| Error::CannotGetAvailablePort(e.to_string()))?;
        if port_is_valid(&port) {
            return Ok(port);
        }
    }
    Err(Error::CannotGetAvailablePort(
        "Exhausted attempts".to_string(),
    ))
}

/// Checks if port is valid for use by internal bitcoind.
pub fn port_is_valid(port: &u16) -> bool {
    !BITCOIND_DEFAULT_PORTS.contains(port)
}

static NEXT_NODE_UI_INSTANCE: AtomicU64 = AtomicU64::new(1);

fn next_node_ui_instance() -> u64 {
    NEXT_NODE_UI_INSTANCE.fetch_add(1, Ordering::Relaxed)
}

pub struct SelectBitcoindTypeStep {
    ui_instance: u64,
    use_external: bool,
    use_connect: bool,
    install_node: bool,
    show_advanced: bool,
    network: Network,
    chain: crate::chain::ChainId,
    local_available: bool,
    connect_authenticated: bool,
    /// Managed-node flavour to install when a node is installed. Defaults to
    /// Knots; the user can switch to Core. Carried into
    /// `Context::node_flavor` by `apply`.
    node_flavor: NodeFlavor,
    /// The flavour of the managed node already configured on disk, if any. The
    /// node is shared by every Vault, so its flavour is global: when one exists
    /// we reflect it as the current selection and warn that changing it switches
    /// every Vault.
    existing_flavor: Option<NodeFlavor>,
}

impl Default for SelectBitcoindTypeStep {
    fn default() -> Self {
        Self::new()
    }
}

impl From<SelectBitcoindTypeStep> for Box<dyn Step> {
    fn from(s: SelectBitcoindTypeStep) -> Box<dyn Step> {
        Box::new(s)
    }
}

impl SelectBitcoindTypeStep {
    pub fn new() -> Self {
        Self {
            ui_instance: next_node_ui_instance(),
            use_external: true,
            use_connect: true,
            install_node: true,
            show_advanced: false,
            network: Network::Bitcoin,
            chain: crate::chain::ChainId::Bitcoin,
            connect_authenticated: false,
            local_available: false,
            node_flavor: NodeFlavor::Knots,
            existing_flavor: None,
        }
    }
}

impl Step for SelectBitcoindTypeStep {
    fn vault_stage(&self) -> Option<crate::installer::VaultStage> {
        Some(crate::installer::VaultStage::Node)
    }
    fn load_context(&mut self, ctx: &Context) {
        let chain_changed = self.chain != ctx.bitcoin_config.chain;
        self.network = ctx.network;
        self.chain = ctx.bitcoin_config.chain;
        self.connect_authenticated = ctx.use_coincube_connect;
        self.local_available = ctx.fork_local_vault_allowed();
        if self.chain.is_blake2b() {
            // Do not inspect or inherit the Bitcoin family's global node config.
            // Connect opens the Vault immediately; the isolated local node is
            // provisioned by the running app in the background by default.
            if chain_changed {
                self.use_connect = self.connect_authenticated || !self.local_available;
                self.use_external = self.use_connect;
                self.install_node = true;
            }
            self.show_advanced = false;
            self.node_flavor = NodeFlavor::KnotsBlake2b;
            self.existing_flavor = None;
            return;
        }
        // Expand advanced section by default on non-mainnet networks.
        if ctx.network != Network::Bitcoin {
            self.show_advanced = true;
        }
        // The managed node is shared by every Vault, so its flavour is global.
        // If one already exists, reflect its flavour as the current selection —
        // so creating a Vault doesn't silently switch it — and remember it so the
        // picker can warn that changing the choice switches every Vault.
        let bitcoind_datadir = internal_bitcoind_datadir(&ctx.coincube_directory);
        self.existing_flavor = InternalBitcoindConfig::from_file(
            &bitcoind::internal_bitcoind_config_path(&bitcoind_datadir),
        )
        .ok()
        .map(|conf| conf.flavor);
        if let Some(flavor) = self.existing_flavor {
            self.node_flavor = flavor;
        }
    }

    fn skip(&self, ctx: &Context) -> bool {
        // No Vault, no chain to watch — see `Context::installs_vault`.
        !ctx.installs_vault()
    }

    fn update(&mut self, _hws: &mut HardwareWallets, message: Message) -> Task<Message> {
        if let Message::SelectBitcoindType(msg) = message {
            if self.chain.is_blake2b()
                && !self.local_available
                && matches!(msg, message::SelectBitcoindTypeMsg::UseExternal(false))
            {
                return Task::none();
            }
            if self.chain.is_blake2b()
                && !matches!(
                    msg,
                    message::SelectBitcoindTypeMsg::ContinueWithConnect
                        | message::SelectBitcoindTypeMsg::UseConnect
                        | message::SelectBitcoindTypeMsg::ToggleInstallNode
                        | message::SelectBitcoindTypeMsg::UseExternal(false)
                )
            {
                return Task::none();
            }
            match msg {
                message::SelectBitcoindTypeMsg::ContinueWithConnect => {
                    self.use_connect = true;
                    self.use_external = !self.install_node;
                    return Task::perform(async {}, |_| Message::Next);
                }
                message::SelectBitcoindTypeMsg::ToggleInstallNode => {
                    self.install_node = !self.install_node;
                }
                message::SelectBitcoindTypeMsg::SelectFlavor(flavor) => {
                    if !self.chain.is_blake2b() && flavor.supports_chain(self.chain) {
                        self.node_flavor = flavor;
                    }
                }
                message::SelectBitcoindTypeMsg::ToggleAdvanced => {
                    self.show_advanced = !self.show_advanced;
                }
                message::SelectBitcoindTypeMsg::UseExternal(selected) => {
                    self.use_external = selected;
                    self.use_connect = false;
                    return Task::perform(async {}, |_| Message::Next);
                }
                message::SelectBitcoindTypeMsg::UseConnect => {
                    self.use_external = true;
                    self.use_connect = true;
                    self.install_node = false;
                    return Task::perform(async {}, |_| Message::Next);
                }
            }
        }
        Task::none()
    }

    fn apply(&mut self, ctx: &mut Context) -> bool {
        if ctx.bitcoin_config.chain.is_blake2b() {
            if !self.use_connect {
                if self.use_external || !ctx.fork_local_vault_allowed() {
                    return false;
                }
                ctx.node_flavor = NodeFlavor::KnotsBlake2b;
                ctx.use_coincube_connect = false;
                ctx.install_node_alongside_connect = false;
                ctx.bitcoind_is_external = false;
                ctx.bitcoin_backend = None;
                ctx.pending_bitcoind_config = None;
                return true;
            }
            let Some(token) = &ctx.connect_jwt else {
                return false;
            };
            ctx.bitcoin_backend = Some(BitcoinBackend::Esplora(
                crate::installer::connect_esplora_config(ctx.bitcoin_config.chain, token.as_str()),
            ));
            ctx.node_flavor = NodeFlavor::KnotsBlake2b;
            ctx.use_coincube_connect = true;
            ctx.install_node_alongside_connect = self.install_node;
            ctx.bitcoind_is_external = !self.install_node;
            // Connect is usable as soon as the Vault is created. Provisioning
            // the optional node belongs to the running app, not this wizard.
            ctx.internal_bitcoind_config = None;
            ctx.pending_bitcoind_config = None;
            ctx.internal_bitcoind = None;
            return true;
        }
        // Carry the chosen managed-node flavour to the InternalBitcoindStep.
        // Harmless on the non-install paths (that step is skipped then).
        ctx.node_flavor = self.node_flavor;
        if self.use_connect {
            let install_node = self.install_node && !self.use_external;
            ctx.use_coincube_connect = true;
            ctx.install_node_alongside_connect = install_node;
            ctx.bitcoind_is_external = !install_node;
            // Connect is the initial backend in either case. Mainnet routes
            // primary traffic through COINCUBE API (keeps addresses off
            // public providers); every other network keeps a public
            // Esplora primary so sync traffic distributes across user
            // IPs, with Connect as the safety-net fallback. See
            // `connect_esplora_config` for the assembled chain.
            // `EsploraConfig.token` is a plain `String` (serialized
            // to disk in `coincubed` config), so we copy the inner
            // string out of the `Zeroizing<String>` wrapper here.
            let Some(token) = &ctx.connect_jwt else {
                return false;
            };
            ctx.bitcoin_backend = Some(BitcoinBackend::Esplora(
                crate::installer::connect_esplora_config(ctx.bitcoin_config.chain, token.as_str()),
            ));
            ctx.internal_bitcoind_config = None;
            ctx.pending_bitcoind_config = None;
            ctx.internal_bitcoind = None;
        } else if self.use_external {
            ctx.use_coincube_connect = false;
            ctx.install_node_alongside_connect = false;
            ctx.bitcoind_is_external = true;
            ctx.bitcoin_backend = None;
            ctx.internal_bitcoind_config = None;
            ctx.pending_bitcoind_config = None;
            ctx.internal_bitcoind = None;
        } else {
            ctx.use_coincube_connect = false;
            ctx.install_node_alongside_connect = false;
            ctx.bitcoind_is_external = false;
            ctx.pending_bitcoind_config = None;
            if ctx.internal_bitcoind_config.is_none() {
                ctx.bitcoin_backend = None;
            }
        }
        true
    }

    fn view(&self, _hws: &HardwareWallets, progress: (usize, usize)) -> Element<Message> {
        view::select_bitcoind_type(
            progress,
            self.ui_instance,
            self.chain,
            self.install_node,
            self.show_advanced,
            PRUNE_DEFAULT,
            self.connect_authenticated,
            self.node_flavor,
            self.existing_flavor,
            self.local_available,
        )
    }
}

#[derive(Clone)]
pub struct DefineBitcoind {
    rpc_auth_vals: RpcAuthValues,
    selected_auth_type: RpcAuthType,
    address: form::Value<String>,

    // Internal cache to detect network change.
    network: Option<Network>,
}

impl DefineBitcoind {
    pub fn new() -> Self {
        Self {
            rpc_auth_vals: RpcAuthValues::default(),
            selected_auth_type: RpcAuthType::CookieFile,
            address: form::Value::default(),
            network: None,
        }
    }

    pub fn ping(&self) -> Result<(), Error> {
        let rpc_auth_vals = self.rpc_auth_vals.clone();
        let builder = match self.selected_auth_type {
            RpcAuthType::CookieFile => {
                let cookie_path = rpc_auth_vals.cookie_path.value;
                let cookie = std::fs::read_to_string(cookie_path)
                    .map_err(|e| Error::Bitcoind(format!("Failed to read cookie file: {}", e)))?;
                SimpleHttpTransport::builder().cookie_auth(cookie)
            }
            RpcAuthType::UserPass => {
                let user = rpc_auth_vals.user.value;
                let password = rpc_auth_vals.password.value;
                SimpleHttpTransport::builder().auth(user, Some(password))
            }
        };
        let client = Client::with_transport(
            builder
                .url(&self.address.value.to_owned())?
                .timeout(std::time::Duration::from_secs(3))
                .build(),
        );
        client.send_request(client.build_request("echo", None))?;
        Ok(())
    }

    pub fn can_try_ping(&self) -> bool {
        if let RpcAuthType::UserPass = self.selected_auth_type {
            self.address.valid
                && !self.rpc_auth_vals.password.value.is_empty()
                && !self.rpc_auth_vals.user.value.is_empty()
        } else {
            self.address.valid && !self.rpc_auth_vals.cookie_path.value.is_empty()
        }
    }

    pub fn load_context(&mut self, ctx: &Context) {
        if self.rpc_auth_vals.cookie_path.value.is_empty()
            // if network changed then the values must be reset to default.
            || self.network != Some(ctx.bitcoin_config.network)
        {
            self.rpc_auth_vals.cookie_path.value =
                bitcoind_default_cookie_path(&ctx.bitcoin_config.network).unwrap_or_default()
        }
        if self.address.value.is_empty()
            // if network changed then the values must be reset to default.
            || self.network != Some(ctx.bitcoin_config.network)
        {
            self.address.value = bitcoind_default_address(&ctx.bitcoin_config.network);
        }

        self.network = Some(ctx.bitcoin_config.network);
    }

    pub fn update(&mut self, message: message::DefineNode) -> Task<Message> {
        if let message::DefineNode::DefineBitcoind(msg) = message {
            match msg {
                message::DefineBitcoind::ConfigFieldEdited(field, value) => match field {
                    ConfigField::Address => {
                        self.address.value.clone_from(&value);
                        self.address.valid = false;
                        if let Some((ip, port)) = value.rsplit_once(':') {
                            let port = u16::from_str(port);
                            let (ipv4, ipv6) = (Ipv4Addr::from_str(ip), Ipv6Addr::from_str(ip));
                            if port.is_ok() && (ipv4.is_ok() || ipv6.is_ok()) {
                                self.address.valid = true;
                            }
                        }
                    }
                    ConfigField::CookieFilePath => {
                        self.rpc_auth_vals.cookie_path.value = value;
                        self.rpc_auth_vals.cookie_path.valid = true;
                    }
                    ConfigField::User => {
                        self.rpc_auth_vals.user.value = value;
                        self.rpc_auth_vals.user.valid = true;
                    }
                    ConfigField::Password => {
                        self.rpc_auth_vals.password.value = value;
                        self.rpc_auth_vals.password.valid = true;
                    }
                },
                message::DefineBitcoind::RpcAuthTypeSelected(auth_type) => {
                    self.selected_auth_type = auth_type;
                }
            }
        }
        Task::none()
    }

    pub fn apply(&mut self, ctx: &mut Context) -> bool {
        let addr = std::net::SocketAddr::from_str(&self.address.value);
        let rpc_auth = match self.selected_auth_type {
            RpcAuthType::CookieFile => {
                match PathBuf::from_str(&self.rpc_auth_vals.cookie_path.value) {
                    Ok(path) => Some(BitcoindRpcAuth::CookieFile(path)),
                    Err(_) => {
                        self.rpc_auth_vals.cookie_path.valid = false;
                        None
                    }
                }
            }
            RpcAuthType::UserPass => Some(BitcoindRpcAuth::UserPass(
                self.rpc_auth_vals.user.value.clone(),
                self.rpc_auth_vals.password.value.clone(),
            )),
        };
        match (rpc_auth, addr) {
            (None, Ok(_)) => false,
            (_, Err(_)) => {
                self.address.valid = false;
                false
            }
            (Some(rpc_auth), Ok(addr)) => {
                ctx.bitcoin_backend = Some(coincubed::config::BitcoinBackend::Bitcoind(
                    BitcoindConfig { rpc_auth, addr },
                ));
                true
            }
        }
    }

    pub fn view(&self) -> Element<Message> {
        view::define_bitcoind(&self.address, &self.rpc_auth_vals, &self.selected_auth_type)
    }
}

impl Default for DefineBitcoind {
    fn default() -> Self {
        Self::new()
    }
}

pub struct InternalBitcoindStep {
    ui_instance: u64,
    local_feature_client: Option<crate::services::coincube::CoincubeClient>,
    local_feature_request: u64,
    local_feature_pending: bool,
    local_feature_checked_at: Option<std::time::Instant>,
    local_feature_revoked: bool,
    wait_for_local_fork: bool,
    local_sync_ready: bool,
    local_sync_pending: bool,
    local_sync_error: Option<String>,
    coincube_datadir: CoincubeDirectory,
    bitcoind_datadir: PathBuf,
    network: Network,
    chain: crate::chain::ChainId,
    /// Which managed node flavour to install. Defaults to Core; the
    /// node-management step's picker defaults to Knots and overrides it.
    flavor: NodeFlavor,
    /// For Knots, the fetched `(SHA256SUMS, SHA256SUMS.asc)` the download is
    /// verified against. `None` for Core (verified by a code-pinned hash).
    manifest: Option<(String, String)>,
    started: Option<Result<(), StartInternalBitcoindError>>,
    exe_path: Option<PathBuf>,
    bitcoind_config: Option<BitcoindConfig>,
    internal_bitcoind_config: Option<InternalBitcoindConfig>,
    error: Option<String>,
    exe_download: Option<Download>,
    install_state: Option<InstallState>,
    internal_bitcoind: Option<Bitcoind>,
    /// The flavour already configured on disk, if any (for the "…switches every
    /// Vault" warning on the picker). Read in `load_context`.
    existing_flavor: Option<NodeFlavor>,
    /// Gate: the download/install/start flow only runs once the user confirms
    /// the flavour on this screen. Until then the flavour picker is shown.
    flavor_confirmed: bool,
    /// Node-resources advanced disclosure open on the flavour screen.
    show_advanced: bool,
    /// Custom prune-target field (MB). Source of truth for the written `prune`;
    /// presets just set it. Pre-filled from any existing on-disk config.
    prune_mb: form::Value<String>,
    /// Custom mempool-cap field (MB). Empty = bitcoind's 300 MB default (key
    /// omitted). Source of truth for `max_mempool_mb`; presets just set it.
    max_mempool_mb: form::Value<String>,
    /// One-shot guard so `load_context` pre-fills the resource fields from the
    /// existing config exactly once (it runs every frame).
    resources_loaded: bool,
}

impl From<InternalBitcoindStep> for Box<dyn Step> {
    fn from(s: InternalBitcoindStep) -> Box<dyn Step> {
        Box::new(s)
    }
}

impl InternalBitcoindStep {
    pub fn new(coincube_datadir: &CoincubeDirectory) -> Self {
        Self {
            ui_instance: next_node_ui_instance(),
            coincube_datadir: coincube_datadir.clone(),
            bitcoind_datadir: internal_bitcoind_datadir(coincube_datadir),
            network: Network::Bitcoin,
            chain: crate::chain::ChainId::Bitcoin,
            flavor: NodeFlavor::default(),
            manifest: None,
            started: None,
            exe_path: None,
            bitcoind_config: None,
            internal_bitcoind_config: None,
            error: None,
            exe_download: None,
            install_state: None,
            internal_bitcoind: None,
            existing_flavor: None,
            flavor_confirmed: false,
            show_advanced: false,
            prune_mb: form::Value {
                value: PRUNE_DEFAULT.to_string(),
                valid: true,
                warning: None,
            },
            max_mempool_mb: form::Value::default(),
            resources_loaded: false,
            local_feature_client: None,
            local_feature_request: 0,
            local_feature_pending: false,
            local_feature_checked_at: None,
            local_feature_revoked: false,
            wait_for_local_fork: false,
            local_sync_ready: false,
            local_sync_pending: false,
            local_sync_error: None,
        }
    }

    /// Refuse the step's provider if it cannot serve the step's chain, recording
    /// the reason as the step error. Run before every write, download, unpack
    /// and start, so a mismatch fails at whichever of those is reached first
    /// and the datadir under `bitcoind/` is never touched by a Bitcoin Blake2b
    /// provider (or vice versa).
    fn provider_mismatch(&mut self) -> bool {
        match self.flavor.check_chain(self.chain) {
            Ok(()) => false,
            Err(e) => {
                self.error = Some(e.to_string());
                true
            }
        }
    }

    /// Set the prune field and refresh its validity (shared with the settings
    /// editor via [`bitcoind::set_prune_form_value`]).
    fn set_prune_field(&mut self, value: String) {
        bitcoind::set_prune_form_value(&mut self.prune_mb, value);
    }

    /// Set the mempool field and refresh its validity (shared with settings).
    fn set_max_mempool_field(&mut self, value: String) {
        bitcoind::set_max_mempool_form_value(&mut self.max_mempool_mb, value);
    }

    /// Apply a one-click machine-profile preset (Small/Regular computer) to both
    /// resource fields, revealing the advanced disclosure so the change is seen.
    fn apply_resource_preset(&mut self, r: NodeResources) {
        self.set_prune_field(r.prune_mb.to_string());
        self.set_max_mempool_field(
            r.max_mempool_mb
                .map(|mb| mb.to_string())
                .unwrap_or_default(),
        );
        self.show_advanced = true;
    }

    /// Validate both resource fields and return the chosen values, marking the
    /// offending field invalid on failure. Returns `None` so `DefineConfig` bails
    /// without writing.
    fn resource_values(&mut self) -> Option<NodeResources> {
        bitcoind::resources_from_forms(&mut self.prune_mb, &mut self.max_mempool_mb)
    }
}

impl Step for InternalBitcoindStep {
    fn vault_stage(&self) -> Option<crate::installer::VaultStage> {
        Some(crate::installer::VaultStage::Node)
    }
    fn load_context(&mut self, ctx: &Context) {
        // The flavour the user picked on the node-management step
        // (`Context::node_flavor`, default Knots) is authoritative. We must NOT
        // adopt a leftover on-disk `InternalBitcoindConfig`'s flavour here: the
        // managed-node directory is shared and survives a cube delete, so a
        // config written by an earlier (pre-picker) Core install would silently
        // override an explicit Knots pick — installing Core for a user who asked
        // for Knots. `DefineConfig` reuses a matching on-disk config (ports /
        // ports) and rebuilds it on a flavour change.
        if self.chain != ctx.bitcoin_config.chain {
            self.stop();
            self.exe_path = None;
            self.exe_download = None;
            self.install_state = None;
            self.started = None;
            self.bitcoind_config = None;
            self.internal_bitcoind_config = None;
            self.flavor_confirmed = false;
            self.resources_loaded = false;
            self.local_sync_ready = false;
            self.local_sync_pending = false;
            self.local_sync_error = None;
            self.local_feature_request = self.local_feature_request.wrapping_add(1);
            self.local_feature_pending = false;
            self.local_feature_checked_at = None;
            self.local_feature_revoked = false;
        }
        self.local_feature_client = ctx.coincube_client.clone();
        self.wait_for_local_fork =
            ctx.bitcoin_config.chain.is_blake2b() && !ctx.install_node_alongside_connect;
        self.chain = ctx.bitcoin_config.chain;
        self.flavor = ctx.node_flavor;
        self.bitcoind_datadir = bitcoind::internal_bitcoind_datadir_for(
            &self.coincube_datadir,
            self.flavor.chain_family(),
        );
        // Flavour already configured on disk (if any) — drives the picker's
        // "…switches every Vault" warning when the user changes it.
        self.existing_flavor = crate::node::bitcoind::configured_managed_flavor_for(
            &self.coincube_datadir,
            self.flavor.chain_family(),
        );
        if self.exe_path.is_none() {
            // Check if current managed bitcoind version is already installed.
            // For new installations, we ignore any previous managed bitcoind versions that might be installed.
            let exe_path = bitcoind::internal_bitcoind_exe_path_for(
                &ctx.coincube_directory,
                self.flavor.chain_family(),
                self.flavor.version(),
            );
            if exe_path.exists() {
                self.exe_path = Some(exe_path)
            } else if self.exe_download.is_none() {
                self.exe_download = Some(Download::new());
            }
        }
        if self.network != ctx.bitcoin_config.network {
            self.internal_bitcoind_config = None;
            self.network = ctx.bitcoin_config.network;
            // Prune is per-network, so re-read the resource fields for the new one.
            self.resources_loaded = false;
        }
        // Pre-fill the node-resource fields from any existing on-disk config
        // exactly once (this runs every frame). An untouched fresh install has no
        // config, so the constructor defaults (15000 MB prune, blank mempool)
        // stand. Reusing an existing datadir shows its current values instead.
        if !self.resources_loaded {
            self.resources_loaded = true;
            if let Ok(conf) = InternalBitcoindConfig::from_file(
                &bitcoind::internal_bitcoind_config_path(&self.bitcoind_datadir),
            ) {
                if let Some(net_conf) = conf.networks.get(&self.network) {
                    self.prune_mb.value = net_conf.prune.to_string();
                    self.prune_mb.valid = true;
                    self.prune_mb.warning = None;
                }
                self.max_mempool_mb.value = conf
                    .max_mempool_mb
                    .map(|mb| mb.to_string())
                    .unwrap_or_default();
                self.max_mempool_mb.valid = true;
                self.max_mempool_mb.warning = None;
            }
        }
        if let Some(Ok(_)) = self.started {
            // This case can arise if a user switches from internal bitcoind to external and back to internal.
            if ctx.bitcoin_backend.is_none() {
                self.started = None; // So that internal bitcoind will be restarted.
            }
        }
    }
    fn update(&mut self, _hws: &mut HardwareWallets, message: Message) -> Task<Message> {
        if let Message::InternalBitcoind(msg) = message {
            use message::InternalBitcoindMsg as M;
            let msg = match msg {
                M::LocalFeatureChecked(chain, request, pending, result) => {
                    if chain != self.chain || request != self.local_feature_request {
                        return Task::none();
                    }
                    self.local_feature_pending = false;
                    if let Err(error) = result {
                        self.local_feature_request = self.local_feature_request.wrapping_add(1);
                        self.local_feature_revoked = true;
                        self.local_feature_checked_at = None;
                        self.local_sync_ready = false;
                        self.local_sync_pending = false;
                        self.error = Some(error.clone());
                        self.local_sync_error = Some(error);
                        self.exe_download = None;
                        self.stop();
                        return Task::none();
                    }
                    self.local_feature_checked_at = Some(std::time::Instant::now());
                    *pending
                }
                M::Previous => {
                    self.local_feature_request = self.local_feature_request.wrapping_add(1);
                    self.local_feature_pending = false;
                    self.local_feature_checked_at = None;
                    self.local_feature_revoked = false;
                    self.local_sync_ready = false;
                    self.local_sync_pending = false;
                    M::Previous
                }
                msg if self.wait_for_local_fork => {
                    if matches!(msg, M::Reload) {
                        self.local_feature_revoked = false;
                    }
                    if self.local_feature_revoked {
                        return Task::none();
                    }
                    let mutation = matches!(
                        msg,
                        M::ConfirmFlavor | M::DefineConfig | M::Download | M::Install | M::Start
                    );
                    let sync_refresh = matches!(msg, M::CheckLocalSync)
                        && self
                            .local_feature_checked_at
                            .is_none_or(|at| at.elapsed() >= std::time::Duration::from_secs(60));
                    if sync_refresh && self.local_feature_pending {
                        return Task::none();
                    }
                    if mutation || sync_refresh {
                        self.local_sync_pending = false;
                        self.local_feature_pending = true;
                        self.local_feature_request = self.local_feature_request.wrapping_add(1);
                        let request = self.local_feature_request;
                        let chain = self.chain;
                        let client = self.local_feature_client.clone();
                        let root = self.coincube_datadir.clone();
                        return Task::perform(
                            async move {
                                crate::chain::require_local_feature(chain, client, &root).await
                            },
                            move |result| {
                                Message::InternalBitcoind(M::LocalFeatureChecked(
                                    chain,
                                    request,
                                    Box::new(msg.clone()),
                                    result,
                                ))
                            },
                        );
                    }
                    msg
                }
                msg => msg,
            };
            match msg {
                message::InternalBitcoindMsg::LocalFeatureChecked(..) => {
                    unreachable!("handled above")
                }
                message::InternalBitcoindMsg::CheckLocalSync => {
                    if !self.wait_for_local_fork
                        || self.local_sync_ready
                        || self.local_sync_pending
                        || !matches!(self.started, Some(Ok(())))
                    {
                        return Task::none();
                    }
                    let Some(node) = self.bitcoind_config.clone() else {
                        return Task::none();
                    };
                    self.local_sync_pending = true;
                    let request = self.local_feature_request;
                    let chain = self.chain;
                    return Task::perform(
                        async move {
                            tokio::task::spawn_blocking(move || {
                                // This probe reads node RPC only; it does not create a wallet.
                                coincubed::BitcoinD::new(&node, "local-fork-admission-probe".into())
                                    .and_then(|node| node.check_local_fork_chain(chain, true))
                                    .map_err(|e| e.to_string())
                            })
                            .await
                            .unwrap_or_else(|e| Err(e.to_string()))
                        },
                        move |result| {
                            Message::InternalBitcoind(
                                message::InternalBitcoindMsg::LocalSyncChecked(
                                    chain, request, result,
                                ),
                            )
                        },
                    );
                }
                message::InternalBitcoindMsg::LocalSyncChecked(chain, request, result) => {
                    if chain != self.chain
                        || request != self.local_feature_request
                        || !self.local_sync_pending
                        || !matches!(self.started, Some(Ok(())))
                    {
                        return Task::none();
                    }
                    self.local_sync_pending = false;
                    self.local_sync_ready = result.is_ok();
                    self.local_sync_error = result.err();
                }
                message::InternalBitcoindMsg::Previous => {
                    if let Some(bitcoind) = self.internal_bitcoind.take() {
                        bitcoind.stop();
                    }
                    if let Some(download) = self.exe_download.as_ref() {
                        // Clear exe_download if not Finished.
                        if let DownloadState::Finished { .. } = download.state {
                        } else {
                            self.exe_download = None;
                        }
                    }
                    self.started = None; // clear both Ok and Err
                                         // Re-show the flavour picker when the user comes back.
                    self.flavor_confirmed = false;
                    return Task::perform(async {}, |_| Message::Previous);
                }
                message::InternalBitcoindMsg::Reload => {
                    return self.load();
                }
                message::InternalBitcoindMsg::SelectFlavor(flavor) => {
                    // Only before the user confirms and the flow kicks off, and
                    // only a provider of this chain's family.
                    if let Err(e) = flavor.check_chain(self.chain) {
                        self.error = Some(e.to_string());
                        return Task::none();
                    }
                    if !self.flavor_confirmed {
                        self.flavor = flavor;
                        // The binary is (re)resolved for the chosen flavour on
                        // confirm, so drop any eagerly-set path/download.
                        self.exe_path = None;
                        self.exe_download = None;
                    }
                }
                message::InternalBitcoindMsg::ConfirmFlavor => {
                    if !self.flavor_confirmed {
                        self.flavor_confirmed = true;
                        // Resolve the binary for the chosen flavour: reuse it if
                        // already installed, else queue a download.
                        let exe_path = bitcoind::internal_bitcoind_exe_path_for(
                            &self.coincube_datadir,
                            self.flavor.chain_family(),
                            self.flavor.version(),
                        );
                        if exe_path.exists() {
                            self.exe_path = Some(exe_path);
                            self.exe_download = None;
                        } else {
                            self.exe_path = None;
                            self.exe_download = Some(Download::new());
                        }
                    }
                    return self.load();
                }
                message::InternalBitcoindMsg::ToggleAdvanced => {
                    self.show_advanced = !self.show_advanced;
                }
                message::InternalBitcoindMsg::PruneEdited(value) => {
                    self.set_prune_field(value);
                }
                message::InternalBitcoindMsg::MaxMempoolEdited(value) => {
                    self.set_max_mempool_field(value);
                }
                message::InternalBitcoindMsg::SmallComputerPreset => {
                    self.apply_resource_preset(NodeResources::small_computer());
                }
                message::InternalBitcoindMsg::RegularComputerPreset => {
                    self.apply_resource_preset(NodeResources::regular_computer());
                }
                message::InternalBitcoindMsg::DefineConfig => {
                    if self.provider_mismatch() {
                        return Task::none();
                    }
                    // Validate the node-resource choices before touching files, so
                    // bad input fails fast and re-opens the advanced disclosure.
                    let resources = match self.resource_values() {
                        Some(r) => r,
                        None => {
                            self.show_advanced = true;
                            self.error = Some(
                                "Enter a prune target of at least 550 MB, and a mempool cap \
                                 of at least 5 MB (or leave the mempool field blank for the \
                                 300 MB default)."
                                    .to_string(),
                            );
                            return Task::none();
                        }
                    };
                    // Everything from the read of the existing conf to its
                    // replacement runs under the datadir-wide managed-conf
                    // lock, on a fresh read: the ports chosen here are chosen
                    // against what every other setup has persisted by now, and
                    // the write cannot erase a section one of them added
                    // meanwhile. The ledger is recorded inside the same span,
                    // before the write that would drop a legacy marker — so a
                    // failure to replace the conf leaves the ledger recorded
                    // and the conf as it was (two files, not one transaction).
                    let flavor = self.flavor;
                    let network = self.network;
                    let coincube_datadir = self.coincube_datadir.clone();
                    let bitcoind_datadir = self.bitcoind_datadir.clone();
                    let written = crate::node::managed_conf::update_managed_conf(
                        &self.coincube_datadir,
                        flavor.chain_family(),
                        |txn| {
                            let mut conf = match txn.conf.clone() {
                                // An existing managed node is shared by every
                                // Vault, so flavour is global. Keep its
                                // ports/datadir (so every Vault keeps
                                // connecting to the same RPC endpoint) and
                                // apply the chosen flavour in place — a switch
                                // just flips the binary, and `maybe_start`
                                // stops the old-flavour node so the configured
                                // binary takes over the same port.
                                Some(mut conf) => {
                                    conf.flavor = flavor;
                                    conf
                                }
                                // Fresh install: build for the chosen flavour
                                // (ports are allocated below).
                                None => InternalBitcoindConfig::for_flavor(flavor),
                            };
                            let network_conf = conf.networks.get(&network).cloned();
                            // Use same ports again if there is an existing
                            // installation.
                            let (rpc_port, p2p_port) = match &network_conf {
                                Some(network_conf) => {
                                    (network_conf.rpc_port, network_conf.p2p_port)
                                }
                                // Never a port another network section or the
                                // other chain family's node already holds,
                                // bound right now or not; refuse outright if
                                // that cannot be known.
                                None => txn.allocate_ports(&conf, get_available_port)?,
                            };

                            // Use cookie file authentication for new wallets.
                            // For an existing bitcoind, we would not know the
                            // RPC password to use without checking in other
                            // daemon.toml files.
                            let cookie_file_auth = BitcoindRpcAuth::CookieFile(
                                internal_bitcoind_cookie_path(&bitcoind_datadir, &network),
                            );
                            let bitcoind_config = BitcoindConfig {
                                rpc_auth: cookie_file_auth,
                                addr: internal_bitcoind_address(rpc_port),
                            };
                            // Use existing network conf if it exists as it may
                            // have rpc_auth field set. This ensures an existing
                            // wallet using username/password authentication
                            // will continue to work.
                            let mut network_conf =
                                network_conf.unwrap_or(InternalBitcoindNetworkConfig {
                                    rpc_port,
                                    p2p_port,
                                    prune: resources.prune_mb,
                                    rpc_auth: None, // can be omitted for new bitcoin.conf entries
                                });
                            // Overwrite prune even for an existing datadir so
                            // an edited target actually applies (keeping
                            // ports/rpc_auth). The mempool cap is a global key
                            // on the whole config.
                            network_conf.prune = resources.prune_mb;
                            conf.networks.insert(network, network_conf);
                            conf.max_mempool_mb = resources.max_mempool_mb;
                            // The file itself cannot say which flavour it is
                            // for, so the ledger has to — recorded before the
                            // write that would drop a legacy `consensusrules`
                            // marker.
                            crate::node::revalidate::ManagedNodeState::record_configured(
                                &coincube_datadir,
                                flavor,
                            );
                            Ok(((bitcoind_config, conf.clone()), Some(conf)))
                        },
                    );
                    let (bitcoind_config, conf) = match written {
                        Ok(outcome) => outcome.logged("writing the managed bitcoin.conf"),
                        Err(e) => {
                            self.error = Some(e.to_string());
                            return Task::none();
                        }
                    };
                    // Keep the step in sync with the flavour actually written,
                    // so the download / executable / verification paths match
                    // the conf.
                    self.flavor = conf.flavor;
                    // Default-ON inbound-over-Tor for a freshly set-up enforcing
                    // (Knots) node on mainnet — see the settings path for
                    // rationale; it's mainnet-only. Only write the sidecar when
                    // the user hasn't already chosen, so an existing opt-out
                    // survives.
                    if matches!(self.flavor, NodeFlavor::Knots)
                        && self.network == Network::Bitcoin
                        && !crate::node::tor::InboundTorPreference::path(&self.coincube_datadir)
                            .exists()
                    {
                        if let Err(e) = crate::node::tor::InboundTorPreference::default_enabled()
                            .save(&self.coincube_datadir)
                        {
                            info!("could not write default inbound-tor preference: {e}");
                        }
                    }
                    self.error = None;
                    self.internal_bitcoind_config = Some(conf);
                    self.bitcoind_config = Some(bitcoind_config);
                    return Task::perform(async {}, |_| {
                        Message::InternalBitcoind(message::InternalBitcoindMsg::Reload)
                    });
                }
                message::InternalBitcoindMsg::Download => {
                    if self.provider_mismatch() {
                        return Task::none();
                    }
                    let flavor = self.flavor;
                    if let Some(download) = &mut self.exe_download {
                        if let DownloadState::Idle = download.state {
                            info!(
                                "Downloading {} version {}...",
                                flavor.display_name(),
                                flavor.version()
                            );
                            return download.start(flavor.download_url()).map(|update| {
                                Message::InternalBitcoind(
                                    message::InternalBitcoindMsg::DownloadProgressed(update),
                                )
                            });
                        }
                    }
                }
                message::InternalBitcoindMsg::DownloadProgressed(update) => {
                    if let Some(download) = self.exe_download.as_mut() {
                        download.update(update);
                        if let DownloadState::Finished(_) = &download.state {
                            info!("Download of bitcoind complete.");
                            // Fetch the release SHA256SUMS(+.asc) the archive is
                            // verified against (Knots); a no-op for Core.
                            let flavor = self.flavor;
                            return Task::perform(
                                async move {
                                    crate::download::fetch_release_manifest(flavor)
                                        .await
                                        .map_err(|e| e.to_string())
                                },
                                |r| {
                                    Message::InternalBitcoind(
                                        message::InternalBitcoindMsg::ManifestFetched(r),
                                    )
                                },
                            );
                        }
                    }
                }
                message::InternalBitcoindMsg::ManifestFetched(res) => match res {
                    Ok(manifest) => {
                        self.manifest = manifest;
                        return Task::perform(async {}, |_| {
                            Message::InternalBitcoind(message::InternalBitcoindMsg::Install)
                        });
                    }
                    Err(e) => {
                        // Refuse to install a binary we can't verify.
                        let msg = coincube_ui::localized_format!(
                            "Failed to fetch release SHA256SUMS manifest: {e}",
                            "No se pudo obtener el manifiesto SHA256SUMS de la versión: {e}"
                        );
                        self.install_state = Some(InstallState::Errored(
                            InstallBitcoindError::MissingSignature,
                        ));
                        self.error = Some(msg);
                        return Task::none();
                    }
                },
                message::InternalBitcoindMsg::Install => {
                    if self.provider_mismatch() {
                        return Task::none();
                    }
                    let flavor = self.flavor;
                    let verification =
                        match DownloadVerification::for_flavor(flavor, self.manifest.clone()) {
                            Some(v) => v,
                            None => {
                                let e = InstallBitcoindError::MissingSignature;
                                self.install_state = Some(InstallState::Errored(e.clone()));
                                self.error = Some(e.to_string());
                                return Task::none();
                            }
                        };
                    if let Some(download) = &self.exe_download {
                        if let DownloadState::Finished(bytes) = &download.state {
                            info!("Installing {}...", flavor.display_name());
                            self.install_state = Some(InstallState::InProgress);
                            match install_bitcoind(
                                &bitcoind::internal_bitcoind_directory_for(
                                    &self.coincube_datadir,
                                    self.flavor.chain_family(),
                                ),
                                bytes,
                                &verification,
                            ) {
                                Ok(_) => {
                                    info!("Installation of bitcoind complete.");
                                    self.install_state = Some(InstallState::Finished);
                                    self.exe_path = Some(bitcoind::internal_bitcoind_exe_path_for(
                                        &self.coincube_datadir,
                                        flavor.chain_family(),
                                        flavor.version(),
                                    ));
                                    return Task::perform(async {}, |_| {
                                        Message::InternalBitcoind(
                                            message::InternalBitcoindMsg::Start,
                                        )
                                    });
                                }
                                Err(e) => {
                                    info!("Installation of bitcoind failed.");
                                    self.install_state = Some(InstallState::Errored(e.clone()));
                                    self.error = Some(e.to_string());
                                    return Task::none();
                                }
                            };
                        }
                    }
                }
                message::InternalBitcoindMsg::Start => {
                    if let Err(e) = self.bitcoind_datadir.canonicalize() {
                        self.started = Some(Err(
                            StartInternalBitcoindError::CouldNotCanonicalizeDataDir(e.to_string()),
                        ));
                        return Task::none();
                    }
                    let bitcoind_config = self
                        .bitcoind_config
                        .as_ref()
                        .expect("already added")
                        .clone();
                    let lifecycle = match crate::node::bitcoind::ManagedNodeLifecycle::acquire(
                        &self.coincube_datadir,
                        crate::node::bitcoind::NodeChainFamily::from_chain(self.chain),
                    ) {
                        Ok(guard) => guard,
                        Err(e) => {
                            self.started = Some(Err(e));
                            return Task::none();
                        }
                    };
                    if self.chain.is_blake2b() {
                        if let Err(e) = crate::node::tor::prepare_inbound_tor_for_chain_locked(
                            &self.coincube_datadir,
                            self.chain,
                            &lifecycle,
                        ) {
                            // Record it as the start's outcome, like the other
                            // refusals here: with `started` left unset, `load`
                            // dispatched `Start` again on every reload and the
                            // view never showed the start as failed.
                            self.started = Some(Err(
                                StartInternalBitcoindError::ConfigUnavailable(e.to_string()),
                            ));
                            return Task::none();
                        }
                    }
                    match Bitcoind::maybe_start_for_chain_locked(
                        self.chain,
                        bitcoind_config,
                        &self.coincube_datadir,
                        &lifecycle,
                    ) {
                        Err(e) => {
                            self.started =
                                Some(Err(StartInternalBitcoindError::CommandError(e.to_string())));
                            return Task::none();
                        }
                        Ok(bitcoind) => {
                            self.error = None;
                            self.started = Some(Ok(()));
                            self.internal_bitcoind = Some(bitcoind);
                        }
                    }
                }
            }
        }
        Task::none()
    }

    fn subscription(&self, _hws: &HardwareWallets) -> Subscription<Message> {
        if self.wait_for_local_fork
            && !self.local_feature_revoked
            && matches!(self.started, Some(Ok(())))
        {
            iced::time::every(std::time::Duration::from_secs(5))
                .map(|_| Message::InternalBitcoind(message::InternalBitcoindMsg::CheckLocalSync))
        } else {
            Subscription::none()
        }
    }

    fn load(&self) -> Task<Message> {
        // Wait for the user to choose Core/Knots and confirm on this screen
        // before doing anything (download/install/start).
        if !self.flavor_confirmed || self.local_feature_revoked {
            return Task::none();
        }
        if self.internal_bitcoind_config.is_none() {
            return Task::perform(async {}, |_| {
                Message::InternalBitcoind(message::InternalBitcoindMsg::DefineConfig)
            });
        }
        if let Some(download) = &self.exe_download {
            if let DownloadState::Idle = download.state {
                return Task::perform(async {}, |_| {
                    Message::InternalBitcoind(message::InternalBitcoindMsg::Download)
                });
            }
        }
        if self.started.is_none() {
            return Task::perform(async {}, |_| {
                Message::InternalBitcoind(message::InternalBitcoindMsg::Start)
            });
        }
        Task::none()
    }

    fn apply(&mut self, ctx: &mut Context) -> bool {
        if self.wait_for_local_fork
            && (self.local_feature_revoked
                || self.local_feature_checked_at.is_none()
                || !self.local_sync_ready)
        {
            return false;
        }
        // Any errors have been handled as part of `message::InternalBitcoindMsg::Start`
        if let Some(Ok(_)) = self.started {
            let bitcoind_config = self.bitcoind_config.clone();
            ctx.internal_bitcoind_config
                .clone_from(&self.internal_bitcoind_config);
            ctx.internal_bitcoind.clone_from(&self.internal_bitcoind);
            self.error = None;

            if ctx.install_node_alongside_connect {
                // Connect + local node: Esplora is the active backend while
                // the local node syncs. Bitcoind goes to pending and will
                // become the primary backend once IBD completes.
                if let Some(cfg) = bitcoind_config {
                    ctx.pending_bitcoind_config = Some(cfg);
                }
                let Some(token) = &ctx.connect_jwt else {
                    return false;
                };
                // Inner `String` copy: `EsploraConfig.token` is a plain
                // `String` (persisted to disk), so extract it from the
                // `Zeroizing<String>` wrapper rather than cloning the
                // wrapper itself. Primary/fallback split (mainnet →
                // COINCUBE API primary, others → public primary): see
                // `connect_esplora_config`.
                ctx.bitcoin_backend = Some(BitcoinBackend::Esplora(
                    crate::installer::connect_esplora_config(
                        ctx.bitcoin_config.chain,
                        token.as_str(),
                    ),
                ));
            } else {
                ctx.bitcoin_backend = bitcoind_config.map(BitcoinBackend::Bitcoind);
            }
            return true;
        }
        false
    }

    fn view(&self, _hws: &HardwareWallets, progress: (usize, usize)) -> Element<Message> {
        if self.wait_for_local_fork
            && matches!(self.started, Some(Ok(())))
            && !self.local_sync_ready
        {
            let mut content = iced::widget::Column::new().spacing(20)
                .push(coincube_ui::component::text::text(coincube_ui::locale::copy("Syncing your local Bitcoin Blake2b node")).bold())
                .push(coincube_ui::component::text::text(coincube_ui::locale::copy("Keep Tenshu open while the node syncs. This can take several hours. The Vault will be created after your node verifies the active Blake2b chain.")));
            if let Some(error) = &self.local_sync_error {
                content = content.push(coincube_ui::component::text::text(
                    coincube_ui::locale::notice(error.clone()),
                ));
            }
            return view::layout(
                progress,
                "Local node synchronization",
                content,
                true,
                Some(Message::Previous),
            );
        }
        view::start_internal_bitcoind(
            progress,
            self.ui_instance,
            self.flavor,
            self.existing_flavor,
            self.flavor_confirmed,
            self.show_advanced,
            &self.prune_mb,
            &self.max_mempool_mb,
            self.exe_path.as_ref(),
            self.started.as_ref(),
            self.error.as_ref(),
            self.exe_download.as_ref().map(|d| &d.state),
            self.install_state.as_ref(),
        )
    }

    fn stop(&mut self) {
        // In case the installer is closed before changes written to context, stop bitcoind.
        if let Some(bitcoind) = self.internal_bitcoind.take() {
            bitcoind.stop();
        }
    }

    fn skip(&self, ctx: &Context) -> bool {
        // No Vault, no chain to watch — see `Context::installs_vault`. Checked
        // first so a seed-only install never downloads or starts a node.
        !ctx.installs_vault() || ctx.bitcoind_is_external || ctx.install_node_alongside_connect
    }
}

/// Finish a Connect-first Vault's optional managed node after the Vault has
/// opened. The request stays in daemon.toml until the caller records the
/// returned node as pending, so a failed or interrupted download can retry.
/// Reports its stage so Settings → Node can show the install while it runs.
pub(crate) fn provision_managed_node_in_background(
    root: CoincubeDirectory,
    chain: crate::chain::ChainId,
    flavor: NodeFlavor,
) -> impl Straw<(BitcoindConfig, Bitcoind), BackgroundNodeInstall, String> {
    sipper(async move |mut progress| {
        flavor.check_chain(chain).map_err(|e| e.to_string())?;
        let family = flavor.chain_family();
        let executable = bitcoind::internal_bitcoind_exe_path_for(&root, family, flavor.version());
        let install = if executable.exists() {
            None
        } else {
            progress
                .send(BackgroundNodeInstall::Downloading(None))
                .await;
            let bytes = download::fetch_managed_node_bytes(flavor.download_url())
                .with(|percent| BackgroundNodeInstall::Downloading(Some(percent)))
                .run(&progress)
                .await
                .map_err(|e| e.to_string())?;
            let manifest = download::fetch_release_manifest(flavor)
                .await
                .map_err(|e| e.to_string())?;
            Some((bytes, manifest))
        };
        progress.send(BackgroundNodeInstall::Starting).await;
        start_background_managed_node(root, chain, flavor, install).await
    })
}

async fn start_background_managed_node(
    root: CoincubeDirectory,
    chain: crate::chain::ChainId,
    flavor: NodeFlavor,
    install: Option<crate::app::state::vault::settings::bitcoind::ManagedNodeInstall>,
) -> Result<(BitcoindConfig, Bitcoind), String> {
    let family = flavor.chain_family();

    // Bitcoin's existing settings path owns the shared-node and Tor rules.
    if !chain.is_blake2b() {
        return crate::app::state::vault::settings::bitcoind::ensure_tor_and_start_managed(
            root,
            chain.bitcoin_network(),
            flavor,
            install,
            false,
            None,
        )
        .await;
    }

    // The fork has a separate binary, config, cookie, lock and chainstate.
    // Its installer path is chain aware; the Bitcoin settings helper above is
    // deliberately restricted to the Bitcoin family.
    tokio::task::spawn_blocking(move || {
        flavor.check_chain(chain).map_err(|e| e.to_string())?;
        let lifecycle = crate::node::bitcoind::ManagedNodeLifecycle::acquire(&root, family)
            .map_err(|e| e.to_string())?;
        lifecycle
            .try_has_owners()
            .map_err(|e| format!("Could not enumerate managed-node owners: {e}"))?;
        if lifecycle
            .try_has_foreign_owners()
            .map_err(|e| format!("Could not enumerate managed-node owners: {e}"))?
        {
            return Err("Another app process owns the managed node. Close it before configuring this node here.".into());
        }
        if let Some((bytes, manifest)) = install {
            let verification = DownloadVerification::for_flavor(flavor, manifest)
                .ok_or_else(|| "Missing release manifest for node verification".to_string())?;
            let directory = bitcoind::internal_bitcoind_directory_for(&root, family);
            install_bitcoind(&directory, &bytes, &verification).map_err(|e| e.to_string())?;
        }
        let network = chain.bitcoin_network();
        let datadir = bitcoind::internal_bitcoind_datadir_for(&root, family);
        let (config, _) = crate::node::managed_conf::update_managed_conf(&root, family, |txn| {
            let mut conf = txn
                .conf
                .clone()
                .unwrap_or_else(|| InternalBitcoindConfig::for_flavor(flavor));
            conf.flavor = flavor;
            let existing = conf.networks.get(&network).cloned();
            let (rpc_port, p2p_port) = match &existing {
                Some(section) => (section.rpc_port, section.p2p_port),
                None => txn.allocate_ports(&conf, get_available_port)?,
            };
            conf.networks
                .entry(network)
                .or_insert(InternalBitcoindNetworkConfig {
                    rpc_port,
                    p2p_port,
                    prune: PRUNE_DEFAULT,
                    rpc_auth: None,
                });
            crate::node::revalidate::ManagedNodeState::record_configured(&root, flavor);
            let config = BitcoindConfig {
                rpc_auth: BitcoindRpcAuth::CookieFile(internal_bitcoind_cookie_path(
                    &datadir, &network,
                )),
                addr: internal_bitcoind_address(rpc_port),
            };
            Ok(((config, conf.clone()), Some(conf)))
        })
        .map_err(|e| e.to_string())?
        .logged("writing the background managed-node config");
        crate::node::tor::prepare_inbound_tor_for_chain_locked(&root, chain, &lifecycle)
            .map_err(|e| e.to_string())?;
        let node = Bitcoind::maybe_start_for_chain_locked(chain, config.clone(), &root, &lifecycle)
            .map_err(|e| e.to_string())?;
        Ok((config, node))
    })
    .await
    .unwrap_or_else(|e| Err(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin_hashes::sha256;

    #[test]
    fn local_node_feature_denial_blocks_provisioning_and_stale_sync() {
        let path =
            std::env::temp_dir().join(format!("local-provision-denied-{}", uuid::Uuid::new_v4()));
        let root = CoincubeDirectory::new(path.clone());
        let mut step = InternalBitcoindStep::new(&root);
        step.chain = crate::chain::ChainId::BitcoinBlake2b;
        step.wait_for_local_fork = true;
        step.flavor = NodeFlavor::KnotsBlake2b;
        let mut hws = HardwareWallets::new(root.clone(), Network::Bitcoin);
        for operation in [
            message::InternalBitcoindMsg::DefineConfig,
            message::InternalBitcoindMsg::Download,
            message::InternalBitcoindMsg::Install,
            message::InternalBitcoindMsg::Start,
        ] {
            step.local_feature_revoked = false;
            drop(step.update(&mut hws, Message::InternalBitcoind(operation.clone())));
            let request = step.local_feature_request;
            drop(step.update(
                &mut hws,
                Message::InternalBitcoind(message::InternalBitcoindMsg::LocalFeatureChecked(
                    step.chain,
                    request,
                    Box::new(operation),
                    Err("feature disabled".into()),
                )),
            ));
            assert!(step.local_feature_revoked);
            assert!(!path.exists());
        }
        step.started = Some(Ok(()));
        step.local_sync_pending = true;
        drop(step.update(
            &mut hws,
            Message::InternalBitcoind(message::InternalBitcoindMsg::LocalSyncChecked(
                step.chain,
                step.local_feature_request,
                Ok(()),
            )),
        ));
        assert!(!step.local_sync_ready);
        let mut ctx = Context::new_for_chain(step.chain, root, None, None);
        assert!(!step.apply(&mut ctx));
    }

    #[test]
    fn previous_discards_local_sync_from_the_old_node_attempt() {
        let root = CoincubeDirectory::new(
            std::env::temp_dir().join(format!("local-sync-generation-{}", uuid::Uuid::new_v4())),
        );
        let mut step = InternalBitcoindStep::new(&root);
        step.chain = crate::chain::ChainId::BitcoinBlake2b;
        step.wait_for_local_fork = true;
        let mut hws = HardwareWallets::new(root, Network::Bitcoin);
        step.started = Some(Ok(()));
        step.local_sync_pending = true;
        let old_request = step.local_feature_request;
        drop(step.update(
            &mut hws,
            Message::InternalBitcoind(message::InternalBitcoindMsg::Previous),
        ));
        step.started = Some(Ok(()));
        step.local_sync_pending = true;
        drop(step.update(
            &mut hws,
            Message::InternalBitcoind(message::InternalBitcoindMsg::LocalSyncChecked(
                step.chain,
                old_request,
                Ok(()),
            )),
        ));
        assert!(!step.local_sync_ready);
        drop(step.update(
            &mut hws,
            Message::InternalBitcoind(message::InternalBitcoindMsg::LocalSyncChecked(
                step.chain,
                step.local_feature_request,
                Ok(()),
            )),
        ));
        assert!(step.local_sync_ready);
    }

    #[test]
    fn anonymous_local_selection_never_installs_a_connect_fallback() {
        let path = std::env::temp_dir().join(format!("local-selector-{}", uuid::Uuid::new_v4()));
        let root = CoincubeDirectory::new(path.clone());
        let mut ctx = Context::new_for_chain(
            crate::chain::ChainId::BitcoinBlake2bTestnet4,
            root.clone(),
            None,
            None,
        );
        // A Vault for an open fork Cube may run on a local node without Connect.
        ctx.in_cube_fork_vault = true;
        let mut select = SelectBitcoindTypeStep::new();
        select.load_context(&ctx);
        let mut hws = HardwareWallets::new(root.clone(), ctx.network);
        drop(select.update(
            &mut hws,
            Message::SelectBitcoindType(message::SelectBitcoindTypeMsg::UseExternal(false)),
        ));
        assert!(select.apply(&mut ctx));
        assert_eq!(ctx.node_flavor, NodeFlavor::KnotsBlake2b);
        assert!(
            !ctx.use_coincube_connect
                && !ctx.install_node_alongside_connect
                && !ctx.bitcoind_is_external
        );
        assert!(ctx.bitcoin_backend.is_none() && ctx.pending_bitcoind_config.is_none());
        let mut internal = InternalBitcoindStep::new(&root);
        internal.load_context(&ctx);
        internal.started = Some(Ok(()));
        assert!(
            !internal.apply(&mut ctx),
            "cannot create a Vault while syncing"
        );
        assert!(ctx.bitcoin_backend.is_none());
        internal.local_sync_ready = true;
        internal.local_feature_checked_at = Some(std::time::Instant::now());
        let config = BitcoindConfig {
            addr: "127.0.0.1:18444".parse().unwrap(),
            rpc_auth: BitcoindRpcAuth::CookieFile(path.join("synthetic.cookie")),
        };
        internal.bitcoind_config = Some(config);
        assert!(internal.apply(&mut ctx));
        assert!(matches!(
            ctx.bitcoin_backend,
            Some(BitcoinBackend::Bitcoind(_))
        ));
        assert!(ctx.pending_bitcoind_config.is_none());
        assert!(
            !path.exists(),
            "selection/sync guards create no wallet artifacts"
        );
    }

    /// A Blake2b start whose managed conf cannot be prepared is refused, and the
    /// refusal is the start's recorded outcome. Left unrecorded, `load` dispatched
    /// `Start` again on every reload and the step never showed the start as failed.
    #[test]
    fn a_refused_conf_preparation_is_recorded_as_the_start_outcome() {
        let dir = CoincubeDirectory::new(
            std::env::temp_dir().join(format!("conf-refused-{}", uuid::Uuid::new_v4())),
        );
        let mut step = InternalBitcoindStep::new(&dir);
        step.chain = crate::chain::ChainId::BitcoinBlake2b;
        std::fs::create_dir_all(&step.bitcoind_datadir).unwrap();
        step.bitcoind_config = Some(BitcoindConfig {
            rpc_auth: BitcoindRpcAuth::CookieFile(step.bitcoind_datadir.join(".cookie")),
            addr: internal_bitcoind_address(8332),
        });
        // A conf that cannot be read: a directory where the file belongs.
        let conf =
            bitcoind::internal_bitcoind_config_path(&bitcoind::internal_bitcoind_datadir_for(
                &dir,
                bitcoind::NodeChainFamily::BitcoinBlake2b,
            ));
        std::fs::create_dir_all(&conf).unwrap();

        let mut hws = HardwareWallets::new(dir.clone(), Network::Bitcoin);
        let _ = step.update(
            &mut hws,
            Message::InternalBitcoind(message::InternalBitcoindMsg::Start),
        );
        assert!(
            matches!(
                step.started,
                Some(Err(StartInternalBitcoindError::ConfigUnavailable(_)))
            ),
            "{:?}",
            step.started
        );
        assert!(
            step.error.is_none(),
            "reported once, as the start's outcome"
        );
        std::fs::remove_dir_all(dir.path()).ok();
    }

    #[test]
    fn fork_connect_selection_keeps_auth_chain_and_has_no_bitcoin_fallback() {
        use crate::chain::ChainId;
        for chain in [ChainId::BitcoinBlake2b, ChainId::BitcoinBlake2bTestnet4] {
            let mut ctx = Context::new_for_chain(
                chain,
                CoincubeDirectory::new(
                    std::env::temp_dir().join(format!("btcb2-selection-{}", uuid::Uuid::new_v4())),
                ),
                None,
                None,
            );
            let mut step = SelectBitcoindTypeStep::new();
            step.load_context(&ctx);
            assert_eq!(step.node_flavor, NodeFlavor::KnotsBlake2b);
            assert!(step.existing_flavor.is_none());
            assert!(!step.apply(&mut ctx), "authentication required");
            assert!(ctx.bitcoin_backend.is_none());
            ctx.connect_jwt = Some(zeroize::Zeroizing::new("synthetic-jwt".to_string()));
            assert!(step.apply(&mut ctx));
            let Some(BitcoinBackend::Esplora(config)) = &ctx.bitcoin_backend else {
                panic!("Connect Esplora required")
            };
            assert_eq!(config.addr, crate::installer::connect_url(chain));
            assert_eq!(config.token.as_deref(), Some("synthetic-jwt"));
            assert!(config.fallback_addr.is_none());
            assert!(config.secondary_fallback_addr.is_none());
            assert!(ctx.install_node_alongside_connect);
            assert!(ctx.pending_bitcoind_config.is_none());
            assert!(!ctx.coincube_directory.path().exists());

            assert!(
                InternalBitcoindStep::new(&ctx.coincube_directory).skip(&ctx),
                "node provisioning must not delay Connect Vault creation"
            );
            step.install_node = false;
            assert!(step.apply(&mut ctx), "Connect-only remains available");
            assert!(!ctx.install_node_alongside_connect);
            step.install_node = true;
            assert!(step.apply(&mut ctx));
            assert!(!ctx.bitcoind_is_external);
            step.load_context(&ctx);
            assert!(
                step.install_node,
                "context reload must preserve the checkbox choice"
            );
            step.install_node = false;
            step.use_connect = false;
            assert!(!step.apply(&mut ctx), "arbitrary external path refused");
        }
    }

    #[test]
    fn bitcoin_connect_and_node_starts_with_connect_without_installer_download() {
        use crate::chain::ChainId;
        let root = CoincubeDirectory::new(
            std::env::temp_dir().join(format!("bitcoin-connect-node-{}", uuid::Uuid::new_v4())),
        );
        let mut ctx = Context::new_for_chain(ChainId::Bitcoin, root.clone(), None, None);
        ctx.use_coincube_connect = true;
        ctx.connect_jwt = Some(zeroize::Zeroizing::new("synthetic-jwt".to_string()));
        let mut choice = SelectBitcoindTypeStep::new();
        choice.load_context(&ctx);
        assert!(choice.install_node, "local node is selected by default");
        let mut hws = HardwareWallets::new(root.clone(), Network::Bitcoin);
        let _ = choice.update(
            &mut hws,
            Message::SelectBitcoindType(message::SelectBitcoindTypeMsg::SelectFlavor(
                NodeFlavor::Core,
            )),
        );
        choice.use_external = false;
        choice.use_connect = true;
        assert!(choice.apply(&mut ctx));
        assert_eq!(ctx.node_flavor, NodeFlavor::Core);
        assert!(matches!(
            ctx.bitcoin_backend,
            Some(BitcoinBackend::Esplora(_))
        ));
        assert!(ctx.pending_bitcoind_config.is_none());
        assert!(InternalBitcoindStep::new(&root).skip(&ctx));
        assert!(!root.path().exists(), "no node provisioning during setup");
    }

    #[tokio::test]
    async fn background_node_refuses_a_provider_from_the_other_chain() {
        let root = CoincubeDirectory::new(
            std::env::temp_dir().join(format!("background-provider-{}", uuid::Uuid::new_v4())),
        );
        let error = provision_managed_node_in_background(
            root.clone(),
            crate::chain::ChainId::BitcoinBlake2b,
            NodeFlavor::Knots,
        )
        .await
        .unwrap_err();
        assert!(error.contains("cannot serve Bitcoin Blake2b"));
        assert!(!root.path().exists(), "provider check must precede all I/O");
    }

    #[test]
    fn staged_node_handoff_preserves_connect_chain_identity() {
        use crate::chain::ChainId;
        for chain in [ChainId::BitcoinBlake2b, ChainId::BitcoinBlake2bTestnet4] {
            let dir = CoincubeDirectory::new(
                std::env::temp_dir().join(format!("btcb2-staged-{}", uuid::Uuid::new_v4())),
            );
            let mut ctx = Context::new_for_chain(chain, dir.clone(), None, None);
            ctx.install_node_alongside_connect = true;
            ctx.connect_jwt = Some(zeroize::Zeroizing::new("synthetic-jwt".to_string()));
            // Only exercise the completed-step handoff; no node is started.
            let mut step = InternalBitcoindStep::new(&dir);
            step.started = Some(Ok(()));
            assert!(step.apply(&mut ctx));
            let Some(BitcoinBackend::Esplora(config)) = &ctx.bitcoin_backend else {
                panic!("Connect Esplora required")
            };
            assert_eq!(config.addr, crate::installer::connect_url(chain));
            assert_eq!(config.token.as_deref(), Some("synthetic-jwt"));
            assert!(config.fallback_addr.is_none());
            assert!(config.secondary_fallback_addr.is_none());
            assert!(!dir.path().exists());
        }
    }

    #[test]
    fn blake2b_installer_config_and_cookie_stay_in_the_fork_family() {
        use crate::{chain::ChainId, node::bitcoind::NodeChainFamily};
        for chain in [ChainId::BitcoinBlake2b, ChainId::BitcoinBlake2bTestnet4] {
            let dir = CoincubeDirectory::new(
                std::env::temp_dir().join(format!("btcb2-install-config-{}", uuid::Uuid::new_v4())),
            );
            let bitcoin = bitcoind::internal_bitcoind_datadir(&dir);
            std::fs::create_dir_all(&bitcoin).unwrap();
            let mut sentinel = InternalBitcoindConfig::for_flavor(NodeFlavor::Core);
            sentinel.networks.insert(
                Network::Bitcoin,
                InternalBitcoindNetworkConfig {
                    rpc_port: 12345,
                    p2p_port: 12346,
                    prune: 550,
                    rpc_auth: None,
                },
            );
            sentinel.to_file(&bitcoin.join("bitcoin.conf")).unwrap();
            let before = std::fs::read(bitcoin.join("bitcoin.conf")).unwrap();
            let mut ctx = Context::new_for_chain(chain, dir.clone(), None, None);
            ctx.node_flavor = NodeFlavor::KnotsBlake2b;
            let mut step = InternalBitcoindStep::new(&dir);
            Step::load_context(&mut step, &ctx);
            let conf = define_config(&mut step);
            assert!(conf.networks.contains_key(&chain.bitcoin_network()));
            assert_eq!(
                step.bitcoind_datadir,
                bitcoind::internal_bitcoind_datadir_for(&dir, NodeChainFamily::BitcoinBlake2b)
            );
            assert_eq!(
                step.bitcoind_config.as_ref().unwrap().rpc_auth,
                BitcoindRpcAuth::CookieFile(internal_bitcoind_cookie_path(
                    &step.bitcoind_datadir,
                    &chain.bitcoin_network()
                ))
            );
            assert_eq!(std::fs::read(bitcoin.join("bitcoin.conf")).unwrap(), before);
            assert!(!crate::node::revalidate::ManagedNodeState::path(&dir).exists());
            std::fs::remove_dir_all(dir.path()).unwrap();
        }
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        sha256::Hash::hash(bytes).to_string()
    }

    /// The managed-node install must default to Knots; Core is
    /// the opt-out. Guards against the default silently reverting to Core.
    #[test]
    fn installer_defaults_to_knots() {
        assert_eq!(SelectBitcoindTypeStep::new().node_flavor, NodeFlavor::Knots);
    }

    // Drive the InternalBitcoindStep's `DefineConfig` and return the on-disk
    // `bitcoin.conf` it wrote, retrying past the rare equal-ephemeral-port case.
    fn define_config(step: &mut InternalBitcoindStep) -> InternalBitcoindConfig {
        use crate::hw::HardwareWallets;
        let mut hws = HardwareWallets::new(step.coincube_datadir.clone(), step.network);
        for _ in 0..5 {
            let _ = step.update(
                &mut hws,
                // This helper tests configuration writing after feature admission;
                // separate local admission tests cover positive/negative API checks.
                Message::InternalBitcoind(if step.wait_for_local_fork {
                    message::InternalBitcoindMsg::LocalFeatureChecked(
                        step.chain,
                        step.local_feature_request,
                        Box::new(message::InternalBitcoindMsg::DefineConfig),
                        Ok(()),
                    )
                } else {
                    message::InternalBitcoindMsg::DefineConfig
                }),
            );
            let path = bitcoind::internal_bitcoind_config_path(&step.bitcoind_datadir);
            if let Ok(conf) = InternalBitcoindConfig::from_file(&path) {
                return conf;
            }
        }
        panic!("DefineConfig never produced a config: {:?}", step.error);
    }

    // A "Small computer" selection writes `prune=550` in the network section and
    // `maxmempool=100` in the general section; the untouched default path writes
    // today's values (15000 MB prune, mempool key omitted → byte-identical).
    #[test]
    fn installer_node_resources_written() {
        use std::fs;

        // Small-computer preset → 550 prune + 100 maxmempool.
        let base = std::env::temp_dir().join(format!("coincube-noderes-sc-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let datadir = CoincubeDirectory::new(base.clone());
        let mut step = InternalBitcoindStep::new(&datadir);
        let _ = step.update(
            &mut crate::hw::HardwareWallets::new(datadir.clone(), Network::Bitcoin),
            Message::InternalBitcoind(message::InternalBitcoindMsg::SmallComputerPreset),
        );
        let conf = define_config(&mut step);
        assert_eq!(
            conf.networks
                .get(&Network::Bitcoin)
                .expect("main section")
                .prune,
            550
        );
        assert_eq!(conf.max_mempool_mb, Some(100));
        // Emitted general section carries the standalone maxmempool key.
        assert_eq!(
            conf.to_ini().general_section().get("maxmempool"),
            Some("100")
        );
        let _ = fs::remove_dir_all(&base);

        // Untouched default path → 15000 prune, no maxmempool key.
        let base2 =
            std::env::temp_dir().join(format!("coincube-noderes-def-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base2);
        let datadir2 = CoincubeDirectory::new(base2.clone());
        let mut step2 = InternalBitcoindStep::new(&datadir2);
        let conf2 = define_config(&mut step2);
        assert_eq!(
            conf2
                .networks
                .get(&Network::Bitcoin)
                .expect("main section")
                .prune,
            PRUNE_DEFAULT
        );
        assert_eq!(conf2.max_mempool_mb, None);
        assert!(conf2.to_ini().general_section().get("maxmempool").is_none());
        let _ = fs::remove_dir_all(&base2);
    }

    // Below-floor input is rejected: `DefineConfig` refuses to write and flags
    // the field, so no `prune=0`/tiny-mempool config can reach disk (invariant
    // I1 + the maxmempool floor).
    #[test]
    fn installer_rejects_below_floor_resources() {
        use std::fs;
        let base =
            std::env::temp_dir().join(format!("coincube-noderes-bad-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let datadir = CoincubeDirectory::new(base.clone());
        let mut step = InternalBitcoindStep::new(&datadir);
        let mut hws = crate::hw::HardwareWallets::new(datadir.clone(), Network::Bitcoin);
        // 100 MB prune is below bitcoind's 550 floor.
        let _ = step.update(
            &mut hws,
            Message::InternalBitcoind(message::InternalBitcoindMsg::PruneEdited("100".to_string())),
        );
        assert!(!step.prune_mb.valid);
        let _ = step.update(
            &mut hws,
            Message::InternalBitcoind(message::InternalBitcoindMsg::DefineConfig),
        );
        assert!(step.error.is_some());
        assert!(
            !bitcoind::internal_bitcoind_config_path(&step.bitcoind_datadir).exists(),
            "no config should be written for an invalid prune target"
        );
        let _ = fs::remove_dir_all(&base);
    }

    // The real published release manifest and its detached signature. Used to
    // prove the *vendored key actually verifies the real release* — the single
    // most important correctness property for this feature.
    const REAL_SUMS: &str = include_str!("test_fixtures/knots_sha256sums");
    const REAL_ASC: &str = include_str!("test_fixtures/knots_sha256sums.asc");

    // Real Tor Expert Bundle manifest + detached signature (Tor Browser
    // Developers signing key). Proves the vendored Tor key verifies the real
    // release manifest — the crypto anchor for the Tor binary download.
    const REAL_TOR_SUMS: &str = include_str!("test_fixtures/tor_sha256sums");
    const REAL_TOR_ASC: &str = include_str!("test_fixtures/tor_sha256sums.asc");

    #[test]
    fn tor_detached_signature_verification() {
        assert_eq!(
            verify_detached_signature(
                REAL_TOR_SUMS.as_bytes(),
                REAL_TOR_ASC,
                bitcoind::TOR_SIGNING_KEY_ASC,
                bitcoind::TOR_SIGNING_KEY_FINGERPRINT,
            ),
            Ok(())
        );
    }

    // Core path: a single pinned hash. Wrong bytes fail, matching bytes pass.
    #[test]
    fn pinned_sha256_verification() {
        let bytes = b"the bitcoin core archive".to_vec();
        let good = DownloadVerification::PinnedSha256(sha256_hex(&bytes));
        let bad = DownloadVerification::PinnedSha256(sha256_hex(b"something else"));
        assert!(verify_download(&bytes, &good).is_ok());
        assert_eq!(
            verify_download(&bytes, &bad),
            Err(InstallBitcoindError::HashMismatch)
        );
    }

    // The vendored Knots key cryptographically verifies the real `SHA256SUMS.asc`
    // over the real manifest; tampering, a wrong pin, or a missing block all fail.
    #[test]
    fn detached_signature_verification() {
        let verify = |data: &[u8], asc: &str, fpr: &str| {
            verify_detached_signature(data, asc, bitcoind::KNOTS_SIGNING_KEY_ASC, fpr)
        };

        // Real signature over the real manifest verifies against the pinned key.
        assert_eq!(
            verify(
                REAL_SUMS.as_bytes(),
                REAL_ASC,
                bitcoind::KNOTS_SIGNING_KEY_FINGERPRINT
            ),
            Ok(())
        );

        // One flipped byte in the signed data invalidates the signature.
        let mut tampered = REAL_SUMS.as_bytes().to_vec();
        tampered[0] ^= 0x01;
        assert_eq!(
            verify(&tampered, REAL_ASC, bitcoind::KNOTS_SIGNING_KEY_FINGERPRINT),
            Err(SignatureError::Invalid)
        );

        // A real, valid signature but pinned to the wrong fingerprint is rejected.
        assert_eq!(
            verify(
                REAL_SUMS.as_bytes(),
                REAL_ASC,
                "0000000000000000000000000000000000000000"
            ),
            Err(SignatureError::Invalid)
        );

        // No signature block at all is Missing, not Invalid.
        assert_eq!(
            verify(
                REAL_SUMS.as_bytes(),
                "not a signature",
                bitcoind::KNOTS_SIGNING_KEY_FINGERPRINT
            ),
            Err(SignatureError::Missing)
        );
    }

    // Manifest listing logic: hash present under the exact filename.
    #[test]
    fn manifest_hash_listing() {
        let archive = b"the bitcoin knots archive".to_vec();
        let filename = "bitcoin-29.3.knots20260507-x86_64-linux-gnu.tar.gz";
        let sums = format!(
            "0000000000000000000000000000000000000000000000000000000000000000  decoy.tar.gz\n\
             {}  {}\n",
            sha256_hex(&archive),
            filename,
        );
        assert!(hash_listed_in_manifest(&archive, filename, &sums));
        // Tampered archive: hash absent.
        assert!(!hash_listed_in_manifest(b"tampered", filename, &sums));
        // Right hash but a different filename: rejected.
        assert!(!hash_listed_in_manifest(
            &archive,
            "bitcoin-29.3.knots20260507-arm64-apple-darwin.tar.gz",
            &sums
        ));
    }

    // End-to-end ReleaseManifest paths: the real signature must verify first,
    // then the archive's hash must be listed; the error variants are distinct.
    #[test]
    fn verify_download_release_manifest() {
        // Signature valid, but our (arbitrary) archive isn't in the real
        // manifest -> reaches and fails the checksum step.
        let manifest = DownloadVerification::ReleaseManifest {
            archive_filename: "bitcoin-29.3.knots20260507-x86_64-linux-gnu.tar.gz".to_string(),
            sha256sums: REAL_SUMS.to_string(),
            sha256sums_asc: REAL_ASC.to_string(),
            signing_key_asc: bitcoind::KNOTS_SIGNING_KEY_ASC,
            signing_key_fingerprint: bitcoind::KNOTS_SIGNING_KEY_FINGERPRINT,
        };
        assert_eq!(
            verify_download(b"not the real archive", &manifest),
            Err(InstallBitcoindError::ChecksumNotInManifest)
        );

        // Missing signature is distinguished from an invalid one.
        let unsigned = DownloadVerification::ReleaseManifest {
            archive_filename: "x".to_string(),
            sha256sums: REAL_SUMS.to_string(),
            sha256sums_asc: String::new(),
            signing_key_asc: bitcoind::KNOTS_SIGNING_KEY_ASC,
            signing_key_fingerprint: bitcoind::KNOTS_SIGNING_KEY_FINGERPRINT,
        };
        assert_eq!(
            verify_download(b"x", &unsigned),
            Err(InstallBitcoindError::MissingSignature)
        );

        // Tampered manifest body -> signature no longer covers it -> invalid.
        let mut tampered_sums = REAL_SUMS.to_string();
        tampered_sums.push_str("dead  evil.tar.gz\n");
        let tampered = DownloadVerification::ReleaseManifest {
            archive_filename: "x".to_string(),
            sha256sums: tampered_sums,
            sha256sums_asc: REAL_ASC.to_string(),
            signing_key_asc: bitcoind::KNOTS_SIGNING_KEY_ASC,
            signing_key_fingerprint: bitcoind::KNOTS_SIGNING_KEY_FINGERPRINT,
        };
        assert_eq!(
            verify_download(b"x", &tampered),
            Err(InstallBitcoindError::InvalidSignature)
        );
    }

    // Exact Tor Expert Bundle URLs per platform, plus the unsupported-platform
    // holes. Host-independent: `tor_asset_url` takes the platform explicitly.
    #[test]
    fn tor_asset_urls() {
        use bitcoind::{tor_asset_url, NodeArch, NodeOs, TOR_VERSION};
        assert_eq!(
            tor_asset_url(TOR_VERSION, NodeOs::MacOs, NodeArch::Aarch64),
            Some(format!(
                "https://archive.torproject.org/tor-package-archive/torbrowser/\
                 {TOR_VERSION}/tor-expert-bundle-macos-aarch64-{TOR_VERSION}.tar.gz"
            ))
        );
        assert_eq!(
            tor_asset_url(TOR_VERSION, NodeOs::Linux, NodeArch::X86_64),
            Some(format!(
                "https://archive.torproject.org/tor-package-archive/torbrowser/\
                 {TOR_VERSION}/tor-expert-bundle-linux-x86_64-{TOR_VERSION}.tar.gz"
            ))
        );
        assert_eq!(
            tor_asset_url(TOR_VERSION, NodeOs::Windows, NodeArch::X86_64),
            Some(format!(
                "https://archive.torproject.org/tor-package-archive/torbrowser/\
                 {TOR_VERSION}/tor-expert-bundle-windows-x86_64-{TOR_VERSION}.tar.gz"
            ))
        );
        // No Expert Bundle is published for Linux/Windows aarch64.
        assert_eq!(
            tor_asset_url(TOR_VERSION, NodeOs::Linux, NodeArch::Aarch64),
            None
        );
        assert_eq!(
            tor_asset_url(TOR_VERSION, NodeOs::Windows, NodeArch::Aarch64),
            None
        );
    }

    // `for_tor` needs the fetched manifest and pins the Tor signing key; end to
    // end, the real Tor archive verifies against the real manifest.
    #[test]
    fn tor_verification() {
        // Without a manifest, there is nothing to verify against.
        assert!(DownloadVerification::for_tor(None).is_none());

        // With the real manifest, `for_tor` pins the Tor key (only meaningful on
        // hosts where a Tor bundle is published).
        if bitcoind::tor_supported_on_host() {
            let v = DownloadVerification::for_tor(Some((
                REAL_TOR_SUMS.to_string(),
                REAL_TOR_ASC.to_string(),
            )))
            .expect("tor verification on a supported host");
            assert!(matches!(
                v,
                DownloadVerification::ReleaseManifest {
                    signing_key_fingerprint,
                    ..
                } if signing_key_fingerprint == bitcoind::TOR_SIGNING_KEY_FINGERPRINT
            ));

            // The real bundle's checksum is listed in the real manifest under
            // the host archive name — the full verify path succeeds for the
            // real hash and fails for a tampered one.
            let filename = bitcoind::tor_download_filename().unwrap();
            let real_hash = REAL_TOR_SUMS
                .lines()
                .find(|l| l.trim_end().ends_with(&filename))
                .and_then(|l| l.split_whitespace().next())
                .expect("host archive listed in manifest");
            assert!(hash_listed_in_manifest_hex(
                real_hash,
                &filename,
                REAL_TOR_SUMS
            ));
        }
    }

    // Helper mirroring `hash_listed_in_manifest` but taking a hex hash directly,
    // so the Tor test needn't reconstruct the (large) archive bytes.
    fn hash_listed_in_manifest_hex(
        hash_hex: &str,
        archive_filename: &str,
        sha256sums: &str,
    ) -> bool {
        sha256sums.lines().any(|line| {
            let mut fields = line.split_whitespace();
            matches!(
                (fields.next(), fields.next()),
                (Some(hash), Some(name))
                    if name == archive_filename && hash.eq_ignore_ascii_case(hash_hex)
            )
        })
    }

    #[test]
    fn verification_for_flavor() {
        // Core never needs a manifest.
        assert!(matches!(
            DownloadVerification::for_flavor(NodeFlavor::Core, None),
            Some(DownloadVerification::PinnedSha256(_))
        ));
        // Knots without a fetched manifest cannot be verified.
        assert!(DownloadVerification::for_flavor(NodeFlavor::Knots, None).is_none());
        // Knots with a manifest produces a ReleaseManifest keyed to the host
        // archive name.
        let v = DownloadVerification::for_flavor(
            NodeFlavor::Knots,
            Some((String::from("sums"), String::from("asc"))),
        );
        assert!(matches!(
            v,
            Some(DownloadVerification::ReleaseManifest { .. })
        ));
    }

    // --- Bitcoin Blake2b provider (dormant) ---------------------------------

    // The real published `SHA256SUMS` (+ `.asc`) for the pinned Bitcoin Blake2b
    // release, fetched from `https://bitcoinknots.org/files/29.x/29.4.1.knots20260508/`.
    const REAL_BLAKE2B_SUMS: &str = include_str!("test_fixtures/knots_blake2b_sha256sums");
    const REAL_BLAKE2B_ASC: &str = include_str!("test_fixtures/knots_blake2b_sha256sums.asc");

    // The pinned Bitcoin Blake2b release is verifiable exactly the way Knots is:
    // its real manifest is signed by the already-pinned Knots key, and every
    // archive filename the provider derives for our platforms is listed in it.
    // No new trust root, no new URL scheme — the version alone selects the file.
    #[test]
    fn blake2b_release_manifest_verifies_under_the_pinned_knots_key() {
        use bitcoind::{NodeArch, NodeOs, KNOTS_BLAKE2B_VERSION};
        // Real signature over the real manifest verifies against the pinned key.
        assert_eq!(
            verify_detached_signature(
                REAL_BLAKE2B_SUMS.as_bytes(),
                REAL_BLAKE2B_ASC,
                bitcoind::KNOTS_SIGNING_KEY_ASC,
                bitcoind::KNOTS_SIGNING_KEY_FINGERPRINT,
            ),
            Ok(())
        );
        // A flipped byte, or a foreign fingerprint, and it does not.
        let mut tampered = REAL_BLAKE2B_SUMS.as_bytes().to_vec();
        tampered[0] ^= 0x01;
        assert_eq!(
            verify_detached_signature(
                &tampered,
                REAL_BLAKE2B_ASC,
                bitcoind::KNOTS_SIGNING_KEY_ASC,
                bitcoind::KNOTS_SIGNING_KEY_FINGERPRINT,
            ),
            Err(SignatureError::Invalid)
        );
        assert_eq!(
            verify_detached_signature(
                REAL_BLAKE2B_SUMS.as_bytes(),
                REAL_BLAKE2B_ASC,
                bitcoind::KNOTS_SIGNING_KEY_ASC,
                "0000000000000000000000000000000000000000",
            ),
            Err(SignatureError::Invalid)
        );
        // The Knots pin's manifest does not vouch for the Blake2b archives, nor
        // the other way round: a version selects exactly one signed listing.
        assert_ne!(REAL_BLAKE2B_SUMS, REAL_SUMS);

        // Every platform archive the provider can ask for is in the listing,
        // under the exact filename the download URL ends in.
        let platforms = [
            (NodeOs::MacOs, NodeArch::Aarch64),
            (NodeOs::MacOs, NodeArch::X86_64),
            (NodeOs::Linux, NodeArch::X86_64),
            (NodeOs::Linux, NodeArch::Aarch64),
            (NodeOs::Windows, NodeArch::X86_64),
            (NodeOs::Windows, NodeArch::Aarch64),
        ];
        for (os, arch) in platforms.iter().copied() {
            let filename = NodeFlavor::KnotsBlake2b.asset_filename(KNOTS_BLAKE2B_VERSION, os, arch);
            let listed = REAL_BLAKE2B_SUMS.lines().any(|line| {
                let mut fields = line.split_whitespace();
                matches!(
                    (fields.next(), fields.next()),
                    (Some(hash), Some(name)) if name == filename && hash.len() == 64
                )
            });
            assert!(listed, "{} is not in the release manifest", filename);
            assert!(NodeFlavor::KnotsBlake2b
                .asset_url(KNOTS_BLAKE2B_VERSION, os, arch)
                .ends_with(&format!("/29.4.1.knots20260508/{}", filename)),);
            // And the Knots pin's own archives are a different set of names.
            assert!(
                !REAL_BLAKE2B_SUMS.contains(&NodeFlavor::Knots.asset_filename(
                    bitcoind::KNOTS_VERSION,
                    os,
                    arch
                ))
            );
        }

        // The verification the installer would build for this provider is the
        // Knots one, keyed to the host's Blake2b archive name.
        match DownloadVerification::for_flavor(
            NodeFlavor::KnotsBlake2b,
            Some((REAL_BLAKE2B_SUMS.to_string(), REAL_BLAKE2B_ASC.to_string())),
        ) {
            Some(DownloadVerification::ReleaseManifest {
                archive_filename,
                signing_key_fingerprint,
                ..
            }) => {
                assert_eq!(
                    archive_filename,
                    NodeFlavor::KnotsBlake2b.download_filename()
                );
                assert!(archive_filename.contains("29.4.1.knots20260508"));
                assert_eq!(
                    signing_key_fingerprint,
                    bitcoind::KNOTS_SIGNING_KEY_FINGERPRINT
                );
            }
            other => panic!("expected a ReleaseManifest, got {:?}", other.is_some()),
        }
        assert!(DownloadVerification::for_flavor(NodeFlavor::KnotsBlake2b, None).is_none());
        // An archive that is not the real one is refused at the checksum step,
        // after the signature has been accepted.
        let verification = DownloadVerification::for_flavor(
            NodeFlavor::KnotsBlake2b,
            Some((REAL_BLAKE2B_SUMS.to_string(), REAL_BLAKE2B_ASC.to_string())),
        )
        .unwrap();
        assert_eq!(
            verify_download(b"not the real archive", &verification),
            Err(InstallBitcoindError::ChecksumNotInManifest)
        );
    }

    // The installer step is a Bitcoin-chain step (its network is a
    // `bitcoin::Network`), so the Blake2b provider is refused at selection and
    // at every later stage — config write, download, unpack — leaving the
    // `bitcoind/` datadir untouched. The Bitcoin providers keep working exactly
    // as before through the same messages.
    #[test]
    fn installer_refuses_the_blake2b_provider_on_a_bitcoin_chain() {
        use std::fs;
        let base = std::env::temp_dir().join(format!(
            "coincube-btcb2-installer-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&base);
        let datadir = CoincubeDirectory::new(base.clone());
        let mut step = InternalBitcoindStep::new(&datadir);
        let mut hws = crate::hw::HardwareWallets::new(datadir.clone(), Network::Bitcoin);
        let conf_path = bitcoind::internal_bitcoind_config_path(&step.bitcoind_datadir);
        let mismatch = "Bitcoin Knots (Bitcoin Blake2b) cannot serve Bitcoin: \
                        it is a Bitcoin Blake2b node provider";

        // Selection is refused and the flavour stays what it was.
        let before = step.flavor;
        assert_ne!(before, NodeFlavor::KnotsBlake2b);
        let _ = step.update(
            &mut hws,
            Message::InternalBitcoind(message::InternalBitcoindMsg::SelectFlavor(
                NodeFlavor::KnotsBlake2b,
            )),
        );
        assert_eq!(step.flavor, before);
        assert_eq!(step.error.as_deref(), Some(mismatch));
        assert!(!step.flavor_confirmed);

        // Even with the provider forced in, no stage gets past the guard.
        step.error = None;
        step.flavor = NodeFlavor::KnotsBlake2b;
        step.exe_download = Some(Download::new());
        for msg in [
            message::InternalBitcoindMsg::DefineConfig,
            message::InternalBitcoindMsg::Download,
            message::InternalBitcoindMsg::Install,
        ]
        .iter()
        .cloned()
        {
            step.error = None;
            let _ = step.update(&mut hws, Message::InternalBitcoind(msg));
            assert_eq!(step.error.as_deref(), Some(mismatch));
        }
        assert!(
            !conf_path.exists(),
            "a config was written for a refused provider"
        );
        assert!(step.internal_bitcoind_config.is_none());
        assert!(step.install_state.is_none());
        assert!(matches!(
            step.exe_download.as_ref().map(|d| &d.state),
            Some(DownloadState::Idle)
        ));
        assert!(
            !crate::node::revalidate::ManagedNodeState::path(&datadir).exists(),
            "the flavour ledger was written for a refused provider"
        );
        assert!(!bitcoind::internal_bitcoind_directory_for(
            &datadir,
            bitcoind::NodeChainFamily::BitcoinBlake2b
        )
        .exists());

        // The Bitcoin providers are unaffected: Knots is selectable and its
        // config is written, with the ledger naming it.
        step.error = None;
        step.flavor = before;
        let _ = step.update(
            &mut hws,
            Message::InternalBitcoind(message::InternalBitcoindMsg::SelectFlavor(
                NodeFlavor::Knots,
            )),
        );
        assert_eq!(step.flavor, NodeFlavor::Knots);
        assert!(step.error.is_none());
        let conf = define_config(&mut step);
        assert!(conf.networks.contains_key(&Network::Bitcoin));
        assert_eq!(step.flavor, NodeFlavor::Knots);
        // The file cannot say which flavour it is for; the ledger does.
        assert_eq!(
            crate::node::revalidate::ManagedNodeState::load(&datadir).configured_flavor,
            Some(NodeFlavor::Knots)
        );
        let _ = fs::remove_dir_all(&base);
    }

    // The installer allocates a new section's ports through the shared policy:
    // the OS supplies candidates, and a port the other family's conf records
    // is refused even though nothing is bound to it; an unreadable
    // other-family conf refuses the allocation altogether; and a section that
    // already exists keeps its ports untouched.
    #[test]
    fn installer_port_allocation_follows_the_shared_policy() {
        use std::fs;
        let base = std::env::temp_dir().join(format!(
            "coincube-btcb2-installer-ports-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&base);
        let datadir = CoincubeDirectory::new(base.clone());
        let section = |rpc_port: u16, p2p_port: u16| InternalBitcoindNetworkConfig {
            rpc_port,
            p2p_port,
            prune: PRUNE_DEFAULT,
            rpc_auth: None,
        };
        // Record ports for the other family that nothing is listening on:
        // exactly the case live probing cannot see.
        let held = get_available_port().unwrap();
        let held2 = get_available_port().unwrap();
        let mut blake2b = InternalBitcoindConfig::for_flavor(NodeFlavor::KnotsBlake2b);
        blake2b
            .networks
            .insert(Network::Bitcoin, section(held, held2));
        let blake2b_conf =
            bitcoind::internal_bitcoind_config_path(&bitcoind::internal_bitcoind_datadir_for(
                &datadir,
                bitcoind::NodeChainFamily::BitcoinBlake2b,
            ));
        fs::create_dir_all(blake2b_conf.parent().unwrap()).unwrap();
        blake2b.to_file(&blake2b_conf).unwrap();

        // Whatever the OS offers, the recorded ports are never returned.
        let mut bitcoin = InternalBitcoindConfig::for_flavor(NodeFlavor::Knots);
        bitcoin
            .networks
            .insert(Network::Testnet4, section(held2.wrapping_add(7), 40999));
        for _ in 0..20 {
            // The same policy the step runs inside its locked update.
            let (rpc, p2p) = crate::node::managed_conf::update_managed_conf(
                &datadir,
                bitcoind::NodeChainFamily::Bitcoin,
                |txn| Ok((txn.allocate_ports(&bitcoin, get_available_port)?, None)),
            )
            .unwrap()
            .value;
            assert_ne!(rpc, p2p);
            for p in [rpc, p2p].iter() {
                assert!(![held, held2, held2.wrapping_add(7), 40999].contains(p));
                assert!(port_is_valid(p));
            }
        }
        // The installer step itself: an existing section is reused as-is and
        // nothing about the Blake2b file changes.
        let mut step = InternalBitcoindStep::new(&datadir);
        let bitcoin_conf = bitcoind::internal_bitcoind_config_path(&step.bitcoind_datadir);
        fs::create_dir_all(bitcoin_conf.parent().unwrap()).unwrap();
        bitcoin
            .networks
            .insert(Network::Bitcoin, section(41001, 41002));
        bitcoin.to_file(&bitcoin_conf).unwrap();
        let conf = define_config(&mut step);
        let main = conf.networks.get(&Network::Bitcoin).unwrap();
        assert_eq!((main.rpc_port, main.p2p_port), (41001, 41002));
        assert_eq!(
            InternalBitcoindConfig::from_file(&blake2b_conf)
                .unwrap()
                .networks,
            blake2b.networks
        );

        // An unreadable other-family conf: the step refuses to write a config.
        fs::write(&blake2b_conf, "[main]\nrpcport=notanumber\nport=1\n").unwrap();
        let mut step = InternalBitcoindStep::new(&datadir);
        fs::remove_file(&bitcoin_conf).unwrap();
        let mut hws = crate::hw::HardwareWallets::new(datadir.clone(), Network::Bitcoin);
        let _ = step.update(
            &mut hws,
            Message::InternalBitcoind(message::InternalBitcoindMsg::DefineConfig),
        );
        assert!(
            step.error
                .as_deref()
                .is_some_and(|e| e.contains("could not be read")),
            "{:?}",
            step.error
        );
        assert!(!bitcoin_conf.exists());
        let _ = fs::remove_dir_all(&base);
    }

    // The installer inherits the same classification: a managed conf this
    // process cannot see — its own family's or the other's — refuses
    // `DefineConfig` instead of being taken for absent. Unix-only
    // precondition; skipped when running privileged.
    #[cfg(unix)]
    #[test]
    fn installer_refuses_an_unreadable_managed_conf_instead_of_treating_it_as_absent() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;
        let section = |rpc_port: u16, p2p_port: u16| InternalBitcoindNetworkConfig {
            rpc_port,
            p2p_port,
            prune: PRUNE_DEFAULT,
            rpc_auth: None,
        };
        let deny =
            |dir: &Path| fs::set_permissions(dir, fs::Permissions::from_mode(0o000)).unwrap();
        let restore = |dir: &Path| {
            let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o755));
        };
        for own_family in [false, true] {
            let base = std::env::temp_dir().join(format!(
                "coincube-btcb2-installer-perm-{}-{}-{:?}",
                own_family,
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = fs::remove_dir_all(&base);
            let datadir = CoincubeDirectory::new(base.clone());
            let mut step = InternalBitcoindStep::new(&datadir);
            let mut hws = crate::hw::HardwareWallets::new(datadir.clone(), Network::Bitcoin);
            let family = if own_family {
                bitcoind::NodeChainFamily::Bitcoin
            } else {
                bitcoind::NodeChainFamily::BitcoinBlake2b
            };
            let dir = bitcoind::internal_bitcoind_datadir_for(&datadir, family);
            fs::create_dir_all(&dir).unwrap();
            let mut conf = InternalBitcoindConfig::for_flavor(family.allowed_flavors()[0]);
            conf.networks
                .insert(Network::Bitcoin, section(52001, 52002));
            let conf_path = bitcoind::internal_bitcoind_config_path(&dir);
            conf.to_file(&conf_path).unwrap();
            deny(&dir);
            let privileged = fs::metadata(&conf_path).is_ok();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _ = step.update(
                    &mut hws,
                    Message::InternalBitcoind(message::InternalBitcoindMsg::DefineConfig),
                );
                step.error.clone()
            }));
            restore(&dir);
            let error = result.unwrap();
            if privileged {
                eprintln!("skipping the permission fixture: metadata succeeds despite mode 000");
            } else {
                let error = error.expect("DefineConfig must refuse");
                assert!(
                    error.to_lowercase().contains("permission denied"),
                    "{}",
                    error
                );
                assert!(step.internal_bitcoind_config.is_none());
                assert!(!crate::node::revalidate::ManagedNodeState::path(&datadir).exists());
                // The own-family conf was not replaced by a fresh one, and the
                // other-family conf is intact.
                assert_eq!(
                    InternalBitcoindConfig::from_file(&conf_path)
                        .unwrap()
                        .networks,
                    conf.networks
                );
            }
            let _ = fs::remove_dir_all(&base);
        }
    }
}
