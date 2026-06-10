#[cfg(target_os = "windows")]
use std::io::{self, Cursor};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::str::FromStr;

use bitcoin_hashes::sha256;
use coincube_core::miniscript::bitcoin::Network;
use coincubed::config::{BitcoinBackend, BitcoindConfig, BitcoindRpcAuth, EsploraConfig};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use flate2::read::GzDecoder;
use iced::{Subscription, Task};
use pgp::composed::{Deserializable, SignedPublicKey, StandaloneSignature};
use pgp::types::KeyDetails;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use tar::Archive;
use tracing::info;

use jsonrpc::{client::Client, simple_http::SimpleHttpTransport};

use coincube_ui::{component::form, widget::*};

use crate::dir::CoincubeDirectory;
use crate::{
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
        internal_bitcoind_directory, Bitcoind, ConfigField, InternalBitcoindConfig,
        InternalBitcoindConfigError, InternalBitcoindNetworkConfig, NodeFlavor, RpcAuthType,
        RpcAuthValues, StartInternalBitcoindError,
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

/// Default prune value used by internal bitcoind.
pub const PRUNE_DEFAULT: u32 = 15_000;
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

/// What a freshly-downloaded managed-node archive is checked against before it
/// is unpacked.
#[derive(Debug, Clone)]
pub enum DownloadVerification {
    /// Bitcoin Core: a single SHA-256 (hex) pinned in code for the current
    /// version's archive on this platform.
    PinnedSha256(String),
    /// Bitcoin Knots: the release `SHA256SUMS` manifest plus its detached
    /// `SHA256SUMS.asc`, both fetched from the release directory.
    /// `archive_filename` is the name the archive is listed under.
    ReleaseManifest {
        archive_filename: String,
        sha256sums: String,
        sha256sums_asc: String,
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
            NodeFlavor::Knots => {
                let (sha256sums, sha256sums_asc) = manifest?;
                Some(Self::ReleaseManifest {
                    archive_filename: flavor.download_filename(),
                    sha256sums,
                    sha256sums_asc,
                })
            }
        }
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
/// validates against the pinned key. Returns [`SignatureError::Missing`] when no
/// signature block is present so callers can distinguish it from an invalid one.
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
    let (signatures, _) =
        StandaloneSignature::from_string_many(asc).map_err(|_| SignatureError::Invalid)?;
    if signatures
        .flatten()
        .any(|sig| sig.verify(&pubkey, data).is_ok())
    {
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
        } => {
            // The manifest's authenticity is anchored by its detached signature
            // against the pinned Knots key; only then do we trust its checksums.
            match verify_detached_signature(
                sha256sums.as_bytes(),
                sha256sums_asc,
                bitcoind::KNOTS_SIGNING_KEY_ASC,
                bitcoind::KNOTS_SIGNING_KEY_FINGERPRINT,
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

pub struct SelectBitcoindTypeStep {
    use_external: bool,
    use_connect: bool,
    install_node: bool,
    show_advanced: bool,
    network: Network,
    connect_authenticated: bool,
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
            use_external: true,
            use_connect: true,
            install_node: true,
            show_advanced: false,
            network: Network::Bitcoin,
            connect_authenticated: false,
        }
    }
}

impl Step for SelectBitcoindTypeStep {
    fn load_context(&mut self, ctx: &Context) {
        self.network = ctx.network;
        self.connect_authenticated = ctx.use_coincube_connect;
        // Expand advanced section by default on non-mainnet networks.
        if ctx.network != Network::Bitcoin {
            self.show_advanced = true;
        }
    }

    fn skip(&self, ctx: &Context) -> bool {
        ctx.remote_backend.is_some()
    }

    fn update(&mut self, _hws: &mut HardwareWallets, message: Message) -> Task<Message> {
        if let Message::SelectBitcoindType(msg) = message {
            match msg {
                message::SelectBitcoindTypeMsg::ContinueWithConnect => {
                    self.use_connect = true;
                    self.use_external = !self.install_node;
                    return Task::perform(async {}, |_| Message::Next);
                }
                message::SelectBitcoindTypeMsg::ToggleInstallNode => {
                    self.install_node = !self.install_node;
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
        if self.use_connect {
            let install_node = self.install_node && !self.use_external;
            ctx.use_coincube_connect = true;
            ctx.install_node_alongside_connect = install_node;
            ctx.bitcoind_is_external = !install_node;
            if install_node {
                // Keep any existing internal_bitcoind_config so the step can restart if needed.
                if ctx.internal_bitcoind_config.is_none() {
                    ctx.bitcoin_backend = None;
                }
            } else {
                // Connect-only: set Esplora backend now. Primary is a public
                // Esplora so wallet sync traffic distributes across user IPs;
                // Connect is the fallback so the safety net still exists when
                // a public provider rate-limits an individual user.
                // `EsploraConfig.token` is a plain `String` (serialized
                // to disk in `coincubed` config), so we copy the inner
                // string out of the `Zeroizing<String>` wrapper here.
                let Some(token) = &ctx.connect_jwt else {
                    return false;
                };
                // Three-tier chain: mempool.space → blockstream.info →
                // Connect (JWT). Picking up `public_esplora_fallback_url`
                // when it's available for this network distributes
                // sync load across two independent public providers
                // before falling back to the metered Connect URL.
                let (
                    fallback_addr,
                    fallback_token,
                    secondary_fallback_addr,
                    secondary_fallback_token,
                ) = match crate::installer::public_esplora_fallback_url(ctx.network) {
                    Some(public_fallback) => (
                        Some(public_fallback),
                        None,
                        Some(crate::installer::connect_url(ctx.network)),
                        Some(token.as_str().to_owned()),
                    ),
                    None => (
                        Some(crate::installer::connect_url(ctx.network)),
                        Some(token.as_str().to_owned()),
                        None,
                        None,
                    ),
                };
                ctx.bitcoin_backend = Some(BitcoinBackend::Esplora(EsploraConfig {
                    addr: crate::installer::public_esplora_url(ctx.network),
                    token: None,
                    fallback_addr,
                    fallback_token,
                    secondary_fallback_addr,
                    secondary_fallback_token,
                }));
                ctx.internal_bitcoind_config = None;
                ctx.pending_bitcoind_config = None;
                ctx.internal_bitcoind = None;
            }
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

    fn view(
        &self,
        _hws: &HardwareWallets,
        progress: (usize, usize),
        _email: Option<&str>,
    ) -> Element<Message> {
        view::select_bitcoind_type(
            progress,
            self.network,
            self.install_node,
            self.show_advanced,
            PRUNE_DEFAULT,
            self.connect_authenticated,
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
    coincube_datadir: CoincubeDirectory,
    bitcoind_datadir: PathBuf,
    network: Network,
    /// Which managed node flavour to install. Defaults to Core; Knots is the
    /// opt-in that enforces BIP-110 (RDTS).
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
}

impl From<InternalBitcoindStep> for Box<dyn Step> {
    fn from(s: InternalBitcoindStep) -> Box<dyn Step> {
        Box::new(s)
    }
}

impl InternalBitcoindStep {
    pub fn new(coincube_datadir: &CoincubeDirectory) -> Self {
        Self {
            coincube_datadir: coincube_datadir.clone(),
            bitcoind_datadir: internal_bitcoind_datadir(coincube_datadir),
            network: Network::Bitcoin,
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
        }
    }
}

impl Step for InternalBitcoindStep {
    fn load_context(&mut self, ctx: &Context) {
        // The installer has no Knots picker, so `self.flavor` defaults to Core.
        // Adopt the flavour already configured on disk (if any) so we don't
        // silently downgrade an existing Knots+RDTS node to Core or fetch the
        // wrong binary. A brand-new install (no conf yet) keeps the step's own
        // default flavour.
        if let Ok(conf) = InternalBitcoindConfig::from_file(
            &bitcoind::internal_bitcoind_config_path(&self.bitcoind_datadir),
        ) {
            self.flavor = conf.flavor;
        }
        if self.exe_path.is_none() {
            // Check if current managed bitcoind version is already installed.
            // For new installations, we ignore any previous managed bitcoind versions that might be installed.
            let exe_path = bitcoind::internal_bitcoind_exe_path(
                &ctx.coincube_directory,
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
            match msg {
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
                    return Task::perform(async {}, |_| Message::Previous);
                }
                message::InternalBitcoindMsg::Reload => {
                    return self.load();
                }
                message::InternalBitcoindMsg::SelectFlavor(flavor) => {
                    // Only meaningful before the binary is fetched.
                    if self.exe_path.is_none() && self.install_state.is_none() {
                        self.flavor = flavor;
                    }
                }
                message::InternalBitcoindMsg::DefineConfig => {
                    let mut conf = match InternalBitcoindConfig::from_file(
                        &bitcoind::internal_bitcoind_config_path(&self.bitcoind_datadir),
                    ) {
                        // Preserve an existing conf's flavour and RDTS setting:
                        // the installer has no Knots picker, so it must not
                        // rewrite an existing Knots `bitcoin.conf` and drop
                        // `consensusrules=rdts`.
                        Ok(conf) => conf,
                        // Fresh install: use the step's selected flavour (Core by
                        // default); `for_flavor` enables RDTS only for Knots.
                        Err(InternalBitcoindConfigError::FileNotFound) => {
                            InternalBitcoindConfig::for_flavor(self.flavor)
                        }
                        Err(e) => {
                            self.error = Some(e.to_string());
                            return Task::none();
                        }
                    };
                    // Keep the step in sync with the flavour actually being
                    // written, so the download / executable / verification paths
                    // match the conf.
                    self.flavor = conf.flavor;
                    let network_conf = conf.networks.get(&self.network);
                    // Use same ports again if there is an existing installation.
                    let (rpc_port, p2p_port) = if let Some(network_conf) = network_conf {
                        (network_conf.rpc_port, network_conf.p2p_port)
                    } else {
                        match (get_available_port(), get_available_port()) {
                            (Ok(rpc_port), Ok(p2p_port)) => {
                                // In case ports are the same, user will need to click button again for another attempt.
                                if rpc_port == p2p_port {
                                    self.error = Some(
                                        "Could not get distinct ports. Please try again."
                                            .to_string(),
                                    );
                                    return Task::none();
                                }
                                (rpc_port, p2p_port)
                            }
                            (Ok(_), Err(e)) | (Err(e), Ok(_)) => {
                                self.error = Some(format!("Could not get available port: {}.", e));
                                return Task::none();
                            }
                            (Err(e1), Err(e2)) => {
                                self.error =
                                    Some(format!("Could not get available ports: {}; {}.", e1, e2));
                                return Task::none();
                            }
                        }
                    };

                    // Use cookie file authentication for new wallets.
                    // For an existing bitcoind, we would not know the RPC password to use without checking
                    // in other daemon.toml files.
                    let cookie_file_auth = BitcoindRpcAuth::CookieFile(
                        internal_bitcoind_cookie_path(&self.bitcoind_datadir, &self.network),
                    );
                    let bitcoind_config = BitcoindConfig {
                        rpc_auth: cookie_file_auth,
                        addr: internal_bitcoind_address(rpc_port),
                    };
                    // Use existing network conf if it exists as it may have rpc_auth field set.
                    // This ensures an existing wallet using username/password authentication will continue to work.
                    let network_conf =
                        network_conf
                            .cloned()
                            .unwrap_or(InternalBitcoindNetworkConfig {
                                rpc_port,
                                p2p_port,
                                prune: PRUNE_DEFAULT,
                                rpc_auth: None, // can be omitted for new bitcoin.conf entries
                            });
                    conf.networks.insert(self.network, network_conf);
                    if let Err(e) = conf.to_file(&bitcoind::internal_bitcoind_config_path(
                        &self.bitcoind_datadir,
                    )) {
                        self.error = Some(e.to_string());
                        return Task::none();
                    }
                    self.error = None;
                    self.internal_bitcoind_config = Some(conf);
                    self.bitcoind_config = Some(bitcoind_config);
                    return Task::perform(async {}, |_| {
                        Message::InternalBitcoind(message::InternalBitcoindMsg::Reload)
                    });
                }
                message::InternalBitcoindMsg::Download => {
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
                        let msg = format!("Failed to fetch release SHA256SUMS manifest: {e}");
                        self.install_state = Some(InstallState::Errored(
                            InstallBitcoindError::MissingSignature,
                        ));
                        self.error = Some(msg);
                        return Task::none();
                    }
                },
                message::InternalBitcoindMsg::Install => {
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
                                &internal_bitcoind_directory(&self.coincube_datadir),
                                bytes,
                                &verification,
                            ) {
                                Ok(_) => {
                                    info!("Installation of bitcoind complete.");
                                    self.install_state = Some(InstallState::Finished);
                                    self.exe_path = Some(bitcoind::internal_bitcoind_exe_path(
                                        &self.coincube_datadir,
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
                    match Bitcoind::maybe_start(
                        self.network,
                        bitcoind_config,
                        &self.coincube_datadir,
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
        Subscription::none()
    }

    fn load(&self) -> Task<Message> {
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
                // wrapper itself. Primary/fallback split: see the
                // Connect-only branch above for the rationale.
                // Same three-tier chain as the Connect-only branch above:
                // mempool.space → blockstream.info (where available) →
                // Connect (JWT).
                let (
                    fallback_addr,
                    fallback_token,
                    secondary_fallback_addr,
                    secondary_fallback_token,
                ) = match crate::installer::public_esplora_fallback_url(ctx.network) {
                    Some(public_fallback) => (
                        Some(public_fallback),
                        None,
                        Some(crate::installer::connect_url(ctx.network)),
                        Some(token.as_str().to_owned()),
                    ),
                    None => (
                        Some(crate::installer::connect_url(ctx.network)),
                        Some(token.as_str().to_owned()),
                        None,
                        None,
                    ),
                };
                ctx.bitcoin_backend = Some(BitcoinBackend::Esplora(EsploraConfig {
                    addr: crate::installer::public_esplora_url(ctx.network),
                    token: None,
                    fallback_addr,
                    fallback_token,
                    secondary_fallback_addr,
                    secondary_fallback_token,
                }));
            } else {
                ctx.bitcoin_backend = bitcoind_config.map(BitcoinBackend::Bitcoind);
            }
            return true;
        }
        false
    }

    fn view(
        &self,
        _hws: &HardwareWallets,
        progress: (usize, usize),
        _email: Option<&str>,
    ) -> Element<Message> {
        view::start_internal_bitcoind(
            progress,
            self.flavor,
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
        ctx.bitcoind_is_external || ctx.remote_backend.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin_hashes::sha256;

    fn sha256_hex(bytes: &[u8]) -> String {
        sha256::Hash::hash(bytes).to_string()
    }

    // The real published release manifest and its detached signature. Used to
    // prove the *vendored key actually verifies the real release* — the single
    // most important correctness property for this feature.
    const REAL_SUMS: &str = include_str!("test_fixtures/knots_sha256sums");
    const REAL_ASC: &str = include_str!("test_fixtures/knots_sha256sums.asc");

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
        let filename = "bitcoin-29.3.knots20260508-x86_64-linux-gnu.tar.gz";
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
            "bitcoin-29.3.knots20260508-arm64-apple-darwin.tar.gz",
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
            archive_filename: "bitcoin-29.3.knots20260508-x86_64-linux-gnu.tar.gz".to_string(),
            sha256sums: REAL_SUMS.to_string(),
            sha256sums_asc: REAL_ASC.to_string(),
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
        };
        assert_eq!(
            verify_download(b"x", &tampered),
            Err(InstallBitcoindError::InvalidSignature)
        );
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
}
