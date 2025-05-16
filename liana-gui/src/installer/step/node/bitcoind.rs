#[cfg(target_os = "windows")]
use std::io::{self, Cursor};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::str::FromStr;

use bitcoin_hashes::{sha256, Hash};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use flate2::read::GzDecoder;
use iced::{Subscription, Task};
use liana::miniscript::bitcoin::Network;
use lianad::config::{BitcoinBackend, BitcoindConfig, BitcoindRpcAuth};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use tar::Archive;
use tracing::info;

use jsonrpc::{client::Client, simple_http::SimpleHttpTransport};

use liana_ui::{component::form, widget::*};

use crate::dir::LianaDirectory;
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
        InternalBitcoindConfigError, InternalBitcoindNetworkConfig, RpcAuthType, RpcAuthValues,
        StartInternalBitcoindError, VERSION,
    },
};

// The approach for tracking download progress is taken from
// https://github.com/iced-rs/iced/blob/master/examples/download_progress/src/main.rs.
#[derive(Debug)]
struct Download {
    id: usize,
    state: DownloadState,
}

#[derive(Debug)]
pub enum DownloadState {
    Idle,
    Downloading { progress: f32 },
    Finished(Vec<u8>),
    Errored(download::DownloadError),
}

impl Download {
    pub fn new(id: usize) -> Self {
        Download {
            id,
            state: DownloadState::Idle,
        }
    }

    pub fn start(&mut self) {
        match self.state {
            DownloadState::Idle { .. }
            | DownloadState::Finished { .. }
            | DownloadState::Errored { .. } => {
                self.state = DownloadState::Downloading { progress: 0.0 };
            }
            _ => {}
        }
    }

    pub fn progress(&mut self, new_progress: Result<download::Progress, download::DownloadError>) {
        if let DownloadState::Downloading { progress } = &mut self.state {
            match new_progress {
                Ok(download::Progress::Downloading(percentage)) => {
                    *progress = percentage;
                }
                Ok(download::Progress::Finished(bytes)) => {
                    self.state = DownloadState::Finished(bytes);
                }
                Err(e) => {
                    self.state = DownloadState::Errored(e);
                }
            }
        }
    }

    pub fn subscription(&self) -> Subscription<Message> {
        match self.state {
            DownloadState::Downloading { .. } => download::file(self.id, bitcoind::download_url())
                .map(|(_, progress)| {
                    Message::InternalBitcoind(message::InternalBitcoindMsg::DownloadProgressed(
                        progress,
                    ))
                }),
            _ => Subscription::none(),
        }
    }
}

/// Default prune value used by internal bitcoind.
pub const PRUNE_DEFAULT: u32 = 15_000;
/// Default ports used by bitcoind across all networks.
pub const BITCOIND_DEFAULT_PORTS: [u16; 8] = [8332, 8333, 18332, 18333, 18443, 18444, 38332, 38333];

#[derive(Debug)]
pub enum InstallState {
    InProgress,
    Finished,
    Errored(InstallBitcoindError),
}

/// Possible errors when installing bitcoind.
#[derive(PartialEq, Eq, Debug, Clone)]
pub enum InstallBitcoindError {
    HashMismatch,
    UnpackingError(String),
}

impl std::fmt::Display for InstallBitcoindError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::HashMismatch => {
                write!(f, "Hashes do not match.")
            }
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

/// Verify the download hash against the expected value.
fn verify_hash(bytes: &[u8]) -> bool {
    let bytes_hash = sha256::Hash::hash(bytes);
    info!("Download hash: '{}'.", bytes_hash);
    let expected_hash = sha256::Hash::from_str(bitcoind::SHA256SUM).expect("This cannot fail.");
    expected_hash == bytes_hash
}

/// Install bitcoind by verifying the download hash and unpacking in the specified directory.
fn install_bitcoind(install_dir: &PathBuf, bytes: &[u8]) -> Result<(), InstallBitcoindError> {
    if !verify_hash(bytes) {
        return Err(InstallBitcoindError::HashMismatch);
    };
    unpack_bitcoind(install_dir, bytes)
}

/// RPC address for internal bitcoind.
fn internal_bitcoind_address(rpc_port: u16) -> SocketAddr {
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

fn bitcoind_default_address(network: &Network) -> String {
    match network {
        Network::Bitcoin => "127.0.0.1:8332".to_string(),
        Network::Testnet => "127.0.0.1:18332".to_string(),
        Network::Regtest => "127.0.0.1:18443".to_string(),
        Network::Signet => "127.0.0.1:38332".to_string(),
        _ => "127.0.0.1:8332".to_string(),
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
        Self { use_external: true }
    }
}

impl Step for SelectBitcoindTypeStep {
    fn skip(&self, ctx: &Context) -> bool {
        ctx.remote_backend.is_some()
    }
    fn update(&mut self, _hws: &mut HardwareWallets, message: Message) -> Task<Message> {
        if let Message::SelectBitcoindType(msg) = message {
            match msg {
                message::SelectBitcoindTypeMsg::UseExternal(selected) => {
                    self.use_external = selected;
                }
            };
            return Task::perform(async {}, |_| Message::Next);
        };
        Task::none()
    }

    fn apply(&mut self, ctx: &mut Context) -> bool {
        if !self.use_external {
            if ctx.internal_bitcoind_config.is_none() {
                ctx.bitcoin_backend = None; // Ensures internal bitcoind can be restarted in case user has switched selection.
            }
        } else {
            ctx.internal_bitcoind_config = None;
        }
        ctx.bitcoind_is_external = self.use_external;
        true
    }

    fn view(
        &self,
        _hws: &HardwareWallets,
        progress: (usize, usize),
        _email: Option<&str>,
    ) -> Element<Message> {
        view::select_bitcoind_type(progress)
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
        client.send_request(client.build_request("echo", &[]))?;
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
            };
        };
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
                ctx.bitcoin_backend =
                    Some(lianad::config::BitcoinBackend::Bitcoind(BitcoindConfig {
                        rpc_auth,
                        addr,
                    }));
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
    liana_datadir: LianaDirectory,
    bitcoind_datadir: PathBuf,
    network: Network,
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
    pub fn new(liana_datadir: &LianaDirectory) -> Self {
        Self {
            liana_datadir: liana_datadir.clone(),
            bitcoind_datadir: internal_bitcoind_datadir(liana_datadir),
            network: Network::Bitcoin,
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
        if self.exe_path.is_none() {
            // Check if current managed bitcoind version is already installed.
            // For new installations, we ignore any previous managed bitcoind versions that might be installed.
            let exe_path = bitcoind::internal_bitcoind_exe_path(&ctx.liana_directory, VERSION);
            if exe_path.exists() {
                self.exe_path = Some(exe_path)
            } else if self.exe_download.is_none() {
                self.exe_download = Some(Download::new(0));
            };
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
                message::InternalBitcoindMsg::DefineConfig => {
                    let mut conf = match InternalBitcoindConfig::from_file(
                        &bitcoind::internal_bitcoind_config_path(&self.bitcoind_datadir),
                    ) {
                        Ok(conf) => conf,
                        Err(InternalBitcoindConfigError::FileNotFound) => {
                            InternalBitcoindConfig::new()
                        }
                        Err(e) => {
                            self.error = Some(e.to_string());
                            return Task::none();
                        }
                    };
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
                    };
                    self.error = None;
                    self.internal_bitcoind_config = Some(conf);
                    self.bitcoind_config = Some(bitcoind_config);
                    return Task::perform(async {}, |_| {
                        Message::InternalBitcoind(message::InternalBitcoindMsg::Reload)
                    });
                }
                message::InternalBitcoindMsg::Download => {
                    if let Some(download) = &mut self.exe_download {
                        if let DownloadState::Idle = download.state {
                            info!("Downloading bitcoind version {}...", &bitcoind::VERSION);
                            download.start();
                        }
                    }
                }
                message::InternalBitcoindMsg::DownloadProgressed(progress) => {
                    if let Some(download) = self.exe_download.as_mut() {
                        download.progress(progress);
                        if let DownloadState::Finished(_) = &download.state {
                            info!("Download of bitcoind complete.");
                            return Task::perform(async {}, |_| {
                                Message::InternalBitcoind(message::InternalBitcoindMsg::Install)
                            });
                        }
                    }
                }
                message::InternalBitcoindMsg::Install => {
                    if let Some(download) = &self.exe_download {
                        if let DownloadState::Finished(bytes) = &download.state {
                            info!("Installing bitcoind...");
                            self.install_state = Some(InstallState::InProgress);
                            match install_bitcoind(
                                &internal_bitcoind_directory(&self.liana_datadir),
                                bytes,
                            ) {
                                Ok(_) => {
                                    info!("Installation of bitcoind complete.");
                                    self.install_state = Some(InstallState::Finished);
                                    self.exe_path = Some(bitcoind::internal_bitcoind_exe_path(
                                        &self.liana_datadir,
                                        VERSION,
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
                    };
                    let bitcoind_config = self
                        .bitcoind_config
                        .as_ref()
                        .expect("already added")
                        .clone();
                    match Bitcoind::maybe_start(self.network, bitcoind_config, &self.liana_datadir)
                    {
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
                    };
                }
            };
        };
        Task::none()
    }

    fn subscription(&self, _hws: &HardwareWallets) -> Subscription<Message> {
        if let Some(download) = self.exe_download.as_ref() {
            return download.subscription();
        }
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
            ctx.bitcoin_backend = self
                .bitcoind_config
                .as_ref()
                .map(|bitcoind_config| BitcoinBackend::Bitcoind(bitcoind_config.clone()));
            ctx.internal_bitcoind_config
                .clone_from(&self.internal_bitcoind_config);
            ctx.internal_bitcoind.clone_from(&self.internal_bitcoind);
            self.error = None;
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
    use super::verify_hash;

    #[test]
    fn hash() {
        let bytes = "this is not bitcoin".as_bytes().to_vec();
        assert!(!verify_hash(&bytes));
    }
}
