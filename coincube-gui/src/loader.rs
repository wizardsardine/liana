use coincube_ui::locale::Text as CopyText;
use std::convert::From;
use std::fs::File;
use std::io::{BufRead, BufReader, ErrorKind, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use iced::futures::{SinkExt, Stream};
use iced::stream::channel;
use iced::{time, Alignment, Length, Subscription, Task};
use tokio::runtime::Handle;
use tracing::{debug, error, info, warn};

use coincube_core::miniscript::bitcoin;
use iced::widget::image;

use coincube_ui::{
    component::{
        button, notification,
        quote_display::{self, Quote, QuoteDisplayProps, QuoteProvider},
        text::{p2_regular, text},
    },
    theme,
    widget::{Column, ColumnExt, Container, Element, Row},
};
use coincubed::{
    config::{BitcoinBackend, BitcoindConfig, BitcoindRpcAuth, Config, ConfigError},
    StartupError,
};

use crate::app;
use crate::app::cache::DaemonCache;
use crate::app::settings::{CubeSettings, WalletSettings};
use crate::backup::Backup;
use crate::chain::ChainIdExt;
use crate::dir::CoincubeDirectory;
use crate::export::RestoreBackupError;
use crate::{
    app::{
        cache::{coins_to_cache, Cache},
        config::Config as GUIConfig,
        wallet::{Wallet, WalletError},
    },
    daemon::{client, embedded::EmbeddedDaemon, model::GetInfoResult, Daemon, DaemonError},
    node::{
        bitcoind::{
            bitcoind_network_dir, internal_bitcoind_datadir_for, Bitcoind, NodeChainFamily,
            StartInternalBitcoindError,
        },
        NodeType,
    },
};

/// Managed-node resources follow chain family, not shared address encoding.
fn managed_node_datadir(
    root: &CoincubeDirectory,
    chain: crate::chain::ChainId,
) -> std::path::PathBuf {
    internal_bitcoind_datadir_for(root, NodeChainFamily::from_chain(chain))
}

fn managed_node_log_path(
    root: &CoincubeDirectory,
    chain: crate::chain::ChainId,
) -> std::path::PathBuf {
    let mut path = managed_node_datadir(root, chain);
    if let Some(network_dir) = bitcoind_network_dir(&chain.bitcoin_network()) {
        path.push(network_dir);
    }
    path.join("debug.log")
}

fn fork_node_config_is_invalid(
    node: &Bitcoind,
    datadir_path: &CoincubeDirectory,
    network: &bitcoin::Network,
) -> bool {
    !node.config.addr.ip().is_loopback()
        || node.config.rpc_auth
            != coincubed::config::BitcoindRpcAuth::CookieFile(
                crate::node::bitcoind::internal_bitcoind_cookie_path(
                    &internal_bitcoind_datadir_for(datadir_path, NodeChainFamily::BitcoinBlake2b),
                    network,
                ),
            )
}

type Coincubed = client::Coincubed<client::jsonrpc::JsonRPCClient>;
type StartedResult = Result<
    (
        Arc<dyn Daemon + Sync + Send>,
        PendingForkNode,
        GetInfoResult,
    ),
    Error,
>;

pub struct Loader {
    pub datadir_path: CoincubeDirectory,
    pub network: bitcoin::Network,
    pub gui_config: GUIConfig,
    pub daemon_started: bool,
    pub internal_bitcoind: Option<Bitcoind>,
    pub waiting_daemon_bitcoind: bool,
    pub backup: Option<Backup>,
    pub wallet_settings: Option<WalletSettings>,
    pub cube_settings: CubeSettings,
    /// The Cube master signer resolved at unlock, carried to [`app::App::new`]
    /// for the seed-derived features. Never set on Bitcoin Blake2b, which
    /// carries [`Self::cube_encryption_key`] instead.
    pub cube_signer: Option<app::cube_signer::CubeSigner>,
    pub connect_client: Option<crate::services::coincube::CoincubeClient>,
    pub cube_encryption_key: Option<Arc<crate::services::connect::crypto::CubeEncryptionKey>>,
    /// Optional Spark backend loaded alongside `cube_signer` in the PIN
    /// flow. `None` when the cube has no Spark signer or when the bridge
    /// subprocess failed to spawn — panels that depend on Spark surface a
    /// "Spark unavailable" placeholder in that case. Propagated unchanged
    /// from [`crate::gui::tab::Message::WalletsLoadedAfterUnlock`] through
    /// [`Message::WalletsLoaded`] into [`app::App::new`].
    pub spark_backend: Option<std::sync::Arc<app::wallets::SparkBackend>>,
    fork_session_revoked: bool,
    /// The resync this loader started and still waits for, if any.
    fork_resync: Option<u64>,
    /// Set when that resync must not change anything any more: the Connect
    /// session it carries was cleared or revoked, or the loader closed. It is
    /// checked under the resync's locks, before daemon.toml is rewritten.
    fork_resync_cancel: Arc<std::sync::atomic::AtomicBool>,
    fork_task: Option<iced::task::Handle>,
    fork_retry_not_before: Option<std::time::Instant>,
    step: Step,
    quote_provider: QuoteProvider,
    current_quote: Quote,
    current_image_handle: image::Handle,
}

pub enum Step {
    Connecting,
    StartingDaemon {
        progress: f32,
    },
    FullScan {
        progress: f32,
    },
    Syncing {
        daemon: Arc<dyn Daemon + Sync + Send>,
        progress: f64,
        bitcoind_logs: String,
    },
    Error(Box<Error>),
}

#[allow(clippy::type_complexity)]
#[derive(Debug)]
pub enum Message {
    View(ViewMessage),
    Syncing(Result<GetInfoResult, DaemonError>),
    Synced(
        Result<
            (
                Arc<Wallet>,
                Cache,
                Arc<dyn Daemon + Sync + Send>,
                Option<Bitcoind>,
                Option<Backup>,
                CubeSettings,
            ),
            Error,
        >,
    ),
    App(
        Result<
            (
                Cache,
                Arc<Wallet>,
                app::Config,
                Arc<dyn Daemon + Sync + Send>,
                CoincubeDirectory,
                Option<Bitcoind>,
            ),
            Error,
        >,
    ),
    WalletsLoaded {
        cube_signer: Option<app::cube_signer::CubeSigner>,
        spark_backend: Option<std::sync::Arc<app::wallets::SparkBackend>>,
        cache: Cache,
        wallet: Arc<Wallet>,
        config: app::Config,
        daemon: Arc<dyn Daemon + Sync + Send>,
        datadir: CoincubeDirectory,
        bitcoind: Option<Bitcoind>,
        cube_settings: CubeSettings,
    },
    Started(StartedResult),
    /// The Bitcoin Blake2b chain data was deleted for a resync (or why not),
    /// for the resync `token` names.
    ForkNodeResynced {
        token: u64,
        result: Result<(), ForkResyncError>,
    },
    Loaded(Result<(Arc<dyn Daemon + Sync + Send>, GetInfoResult), Error>),
    BitcoindLog(Option<String>),
    LoadingTick,
    Failure(DaemonError),
    None,
}

impl Loader {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        datadir_path: CoincubeDirectory,
        gui_config: GUIConfig,
        network: bitcoin::Network,
        internal_bitcoind: Option<Bitcoind>,
        backup: Option<Backup>,
        wallet_settings: Option<WalletSettings>,
        cube_settings: CubeSettings,
        cube_signer: Option<app::cube_signer::CubeSigner>,
        spark_backend: Option<std::sync::Arc<app::wallets::SparkBackend>>,
    ) -> (Self, Task<Message>) {
        Self::build(
            datadir_path,
            gui_config,
            network,
            internal_bitcoind,
            backup,
            wallet_settings,
            cube_settings,
            cube_signer,
            spark_backend,
            None,
            None,
        )
    }

    /// The ordinary local-wallet loader with the current Connect session
    /// carried only for an imported wallet whose selected Esplora provider is
    /// Connect. No token is written to daemon.toml.
    #[allow(clippy::too_many_arguments)]
    pub fn new_imported(
        datadir_path: CoincubeDirectory,
        gui_config: GUIConfig,
        network: bitcoin::Network,
        internal_bitcoind: Option<Bitcoind>,
        backup: Option<Backup>,
        wallet_settings: WalletSettings,
        cube_settings: CubeSettings,
        cube_signer: Option<app::cube_signer::CubeSigner>,
        spark_backend: Option<std::sync::Arc<app::wallets::SparkBackend>>,
        connect_client: Option<crate::services::coincube::CoincubeClient>,
    ) -> (Self, Task<Message>) {
        Self::build(
            datadir_path,
            gui_config,
            network,
            internal_bitcoind,
            backup,
            Some(wallet_settings),
            cube_settings,
            cube_signer,
            spark_backend,
            connect_client,
            None,
        )
    }

    /// Fork startup uses the current authenticated client and the unlocked Cube's
    /// encryption key, without constructing any Lightning SDK or external node.
    #[allow(clippy::too_many_arguments)]
    pub fn new_for_chain(
        datadir_path: CoincubeDirectory,
        gui_config: GUIConfig,
        wallet_settings: WalletSettings,
        cube_settings: CubeSettings,
        client: crate::services::coincube::CoincubeClient,
        cube_encryption_key: Arc<crate::services::connect::crypto::CubeEncryptionKey>,
    ) -> (Self, Task<Message>) {
        Self::build(
            datadir_path,
            gui_config,
            cube_settings.network.bitcoin_network(),
            None,
            None,
            Some(wallet_settings),
            cube_settings,
            None,
            None,
            Some(client),
            Some(cube_encryption_key),
        )
    }

    /// The loader of a Bitcoin Blake2b Cube whose wallet runs on its local
    /// managed node. The open itself never uses Connect; `connect_client` is
    /// the session the unlock had, if any, carried only so that a resync can
    /// move the Cube back to Connect and reopen it there (see
    /// [`resync_fork_cube`]). A client without a token is not carried.
    pub fn new_local_fork(
        datadir_path: CoincubeDirectory,
        gui_config: GUIConfig,
        wallet_settings: WalletSettings,
        cube_settings: CubeSettings,
        cube_encryption_key: Arc<crate::services::connect::crypto::CubeEncryptionKey>,
        connect_client: Option<crate::services::coincube::CoincubeClient>,
    ) -> (Self, Task<Message>) {
        Self::build(
            datadir_path,
            gui_config,
            cube_settings.network.bitcoin_network(),
            None,
            None,
            Some(wallet_settings),
            cube_settings,
            None,
            None,
            connect_client.filter(|c| c.token().is_some()),
            Some(cube_encryption_key),
        )
    }

    pub(crate) fn fork_revoked(&self) -> bool {
        self.fork_session_revoked
    }

    pub(crate) fn requires_connect(&self) -> bool {
        self.cube_settings.network.is_blake2b()
            && !crate::chain::has_managed_local_fork(
                &self.datadir_path,
                self.cube_settings.network,
                self.wallet_settings.as_ref(),
            )
    }

    #[allow(clippy::too_many_arguments)]
    fn build(
        datadir_path: CoincubeDirectory,
        gui_config: GUIConfig,
        network: bitcoin::Network,
        internal_bitcoind: Option<Bitcoind>,
        backup: Option<Backup>,
        wallet_settings: Option<WalletSettings>,
        cube_settings: CubeSettings,
        cube_signer: Option<app::cube_signer::CubeSigner>,
        spark_backend: Option<Arc<app::wallets::SparkBackend>>,
        connect_client: Option<crate::services::coincube::CoincubeClient>,
        cube_encryption_key: Option<Arc<crate::services::connect::crypto::CubeEncryptionKey>>,
    ) -> (Self, Task<Message>) {
        // A chain this build cannot run is refused here, before the daemon
        // socket is dialled — and its identity must agree with the encoding
        // the caller wants to load under, or the wallet directory, the
        // daemon config and the Cube's own settings would disagree with each
        // other from this point on.
        let local_fork = crate::chain::has_managed_local_fork(
            &datadir_path,
            cube_settings.network,
            wallet_settings.as_ref(),
        );
        let support =
            if (cube_settings.network.is_blake2b() && connect_client.is_some()) || local_fork {
                crate::chain::authenticated_connect_support(cube_settings.network)
            } else {
                cube_settings.network.runtime_support()
            };
        let refusal = match support {
            crate::chain::RuntimeSupport::Dormant { reason } => {
                Some(Error::ChainUnavailable(reason))
            }
            crate::chain::RuntimeSupport::Supported
                if cube_settings.network.bitcoin_network() != network =>
            {
                Some(Error::Unexpected(format!(
                    "this Cube is on {} but was asked to load as {}",
                    cube_settings.network.label(),
                    network
                )))
            }
            crate::chain::RuntimeSupport::Supported
                if cube_settings.network.is_blake2b()
                    && ((!local_fork
                        && connect_client.as_ref().and_then(|c| c.token()).is_none())
                        || internal_bitcoind.as_ref().is_some_and(|node| {
                            fork_node_config_is_invalid(node, &datadir_path, &network)
                        })
                        || cube_encryption_key.is_none()
                        || cube_signer.is_some()
                        || spark_backend.is_some()
                        || backup.is_some()
                        || wallet_settings
                            .as_ref()
                            .is_none_or(|w| w.remote_backend_auth.is_some())) =>
            {
                Some(Error::Unexpected(
                    "Bitcoin Blake2b requires an authenticated Connect Vault session".into(),
                ))
            }
            crate::chain::RuntimeSupport::Supported
                if !cube_settings.network.is_blake2b()
                    && connect_client.is_some()
                    && cube_settings.bitcoin_import.is_none() =>
            {
                Some(Error::Unexpected(
                    "The authenticated fork loader requires Bitcoin Blake2b".into(),
                ))
            }
            crate::chain::RuntimeSupport::Supported => None,
        }
        .or_else(|| {
            validate_imported_open(&datadir_path, &cube_settings, wallet_settings.as_ref()).err()
        });

        let task = Self::initial_task(
            &datadir_path,
            cube_settings.network,
            wallet_settings.as_ref(),
            connect_client.as_ref(),
            refusal.is_some(),
        );

        let (task, fork_task) = if cube_settings.network.is_blake2b() {
            let (task, handle) = task.abortable();
            (task, Some(handle.abort_on_drop()))
        } else {
            (task, None)
        };

        let mut quote_provider = QuoteProvider::new();
        let current_quote = quote_provider.select(if refusal.is_some() {
            "error"
        } else {
            "loading"
        });
        let current_image_handle = quote_display::image_handle_for_context("loading");

        (
            Loader {
                network,
                fork_retry_not_before: (cube_settings.network.is_blake2b() && refusal.is_none())
                    .then(|| std::time::Instant::now() + Duration::from_secs(30)),
                fork_session_revoked: false,
                fork_resync: None,
                fork_resync_cancel: Arc::default(),
                fork_task,
                datadir_path,
                gui_config,
                step: match refusal {
                    Some(e) => Step::Error(Box::new(e)),
                    None => Step::Connecting,
                },
                daemon_started: false,
                internal_bitcoind,
                waiting_daemon_bitcoind: false,
                wallet_settings,
                cube_settings,
                backup,
                cube_signer,
                spark_backend,
                connect_client,
                cube_encryption_key,
                quote_provider,
                current_quote,
                current_image_handle,
            },
            task,
        )
    }

    fn initial_task(
        datadir_path: &CoincubeDirectory,
        chain: crate::chain::ChainId,
        wallet_settings: Option<&WalletSettings>,
        connect_client: Option<&crate::services::coincube::CoincubeClient>,
        refused: bool,
    ) -> Task<Message> {
        if refused {
            Task::none()
        } else if chain.is_blake2b() {
            Task::perform(
                start_fork_daemon(
                    datadir_path.clone(),
                    chain,
                    wallet_settings.cloned().expect("validated Vault settings"),
                    connect_client.cloned(),
                ),
                Message::Started,
            )
        } else if let Some(wallet) = wallet_settings {
            let socket_path = datadir_path
                .network_directory(chain)
                .coincubed_data_directory(&wallet.wallet_id())
                .coincubed_rpc_socket_path();
            Task::perform(connect(socket_path), Message::Loaded)
        } else {
            // No vault configured - loader will show setup screen
            Task::none()
        }
    }

    fn set_quote_context(&mut self, context: &str) {
        self.current_quote = self.quote_provider.select(context);
        self.current_image_handle = quote_display::image_handle_for_context(context);
    }

    fn is_first_esplora_scan(&self, wallet_settings: &WalletSettings) -> bool {
        let data_dir = self
            .datadir_path
            .network_directory(self.cube_settings.network)
            .coincubed_data_directory(&wallet_settings.wallet_id());
        let config_path = data_dir.path().join("daemon.toml");
        let is_esplora = match Config::from_file(Some(config_path)) {
            Ok(config) => matches!(config.bitcoin_backend, Some(BitcoinBackend::Esplora(_))),
            Err(_) => false,
        };
        // Only show the full-scan screen on the very first run, before the
        // SQLite database has been created. On subsequent starts the daemon
        // just does an incremental sync and the regular StartingDaemon step
        // is sufficient.
        is_esplora && !data_dir.sqlite_db_file_path().exists()
    }

    /// Whether this vault's configured RPC backend is the app-managed
    /// (internal) bitcoind — i.e. it authenticates with a cookie file located
    /// inside the managed bitcoind datadir. A genuinely external node (a
    /// user-run bitcoind, `auth = "user:pass"`, or a cookie outside the managed
    /// datadir) returns false, so the `start_internal_bitcoind` flag keeps
    /// governing those.
    fn vault_uses_internal_bitcoind(&self) -> bool {
        let Some(wallet) = &self.wallet_settings else {
            return false;
        };
        let config_path = self
            .datadir_path
            .network_directory(self.cube_settings.network)
            .coincubed_data_directory(&wallet.wallet_id())
            .path()
            .join("daemon.toml");
        backend_is_internal_bitcoind(
            &config_path,
            &managed_node_datadir(&self.datadir_path, self.cube_settings.network),
        )
    }

    fn start_bitcoind(&self) -> bool {
        if self.cube_settings.network.is_blake2b() {
            return false;
        }
        // If the vault's active RPC backend is the app-managed internal
        // bitcoind, always ensure it's running when the cube is opened. This is
        // independent of the `start_internal_bitcoind` flag (which only governs
        // genuinely external nodes) and of any node handle already held for a
        // *different* network: `maybe_start` is network-aware, reusing a node
        // already live on this network's RPC endpoint and otherwise starting a
        // separate one, so this never disturbs another network's cube.
        if self.vault_uses_internal_bitcoind() {
            return true;
        }
        if self.internal_bitcoind.is_some() {
            false
        } else if let Some(wallet) = &self.wallet_settings {
            if let Some(start) = wallet.start_internal_bitcoind {
                start
            } else {
                self.gui_config.start_internal_bitcoind
            }
        } else {
            self.gui_config.start_internal_bitcoind
        }
    }

    fn maybe_skip_syncing(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        info: GetInfoResult,
    ) -> Task<Message> {
        // If no wallet is configured, this shouldn't be called
        let wallet_settings = self
            .wallet_settings
            .clone()
            .expect("wallet_settings must be Some when loading");

        // If the node is not Bitcoin Core or otherwise the wallet was previously synced (blockheight > 0),
        // load the application directly.
        if daemon.backend().node_type() != Some(NodeType::Bitcoind) || info.block_height > 0 {
            let task = Task::perform(
                load_application(LoadApplicationConfig {
                    wallet_settings,
                    cube_settings: self.cube_settings.clone(),
                    daemon,
                    info,
                    datadir_path: self.datadir_path.clone(),
                    network: self.network,
                    internal_bitcoind: self.internal_bitcoind.clone(),
                    backup: self.backup.clone(),
                }),
                Message::Synced,
            );
            if self.cube_settings.network.is_blake2b() {
                let (task, handle) = task.abortable();
                self.fork_task = Some(handle.abort_on_drop());
                return task;
            }
            return task;
        }
        // Otherwise, show the sync progress on the loading screen.
        self.step = Step::Syncing {
            daemon: daemon.clone(),
            progress: 0.0,
            bitcoind_logs: String::new(),
        };
        self.set_quote_context("syncing");
        Task::perform(sync(daemon, false), Message::Syncing)
    }

    fn on_load(
        &mut self,
        res: Result<(Arc<dyn Daemon + Sync + Send>, GetInfoResult), Error>,
    ) -> Task<Message> {
        match res {
            Ok((daemon, info)) => {
                if self.start_bitcoind() {
                    warn!("Coincubed is external, gui will not start internal bitcoind");
                }
                return self.maybe_skip_syncing(daemon, info);
            }
            Err(e) => match e {
                Error::Config(_) => {
                    error!("Loader: config error during connect: {}", e);
                    self.step = Step::Error(Box::new(e));
                    self.set_quote_context("error");
                }
                Error::Daemon(DaemonError::ClientNotSupported)
                | Error::Daemon(DaemonError::RpcSocket(Some(ErrorKind::ConnectionRefused), _))
                | Error::Daemon(DaemonError::RpcSocket(Some(ErrorKind::NotFound), _)) => {
                    let wallet_settings = self
                        .wallet_settings
                        .clone()
                        .expect("wallet_settings must be Some when starting daemon");
                    self.step = if self.is_first_esplora_scan(&wallet_settings) {
                        self.set_quote_context("syncing");
                        Step::FullScan { progress: 0.0 }
                    } else {
                        Step::StartingDaemon { progress: 0.0 }
                    };
                    self.daemon_started = true;
                    self.waiting_daemon_bitcoind = true;
                    return Task::perform(
                        start_bitcoind_and_daemon_with_import_auth(
                            self.datadir_path.clone(),
                            self.start_bitcoind(),
                            self.cube_settings.network,
                            wallet_settings,
                            self.connect_client.clone(),
                        ),
                        Message::Started,
                    );
                }
                _ => {
                    error!("Loader: unexpected error during connect: {}", e);
                    self.step = Step::Error(Box::new(e));
                    self.set_quote_context("error");
                }
            },
        }
        Task::none()
    }

    fn on_log(&mut self, log: Option<String>) -> Task<Message> {
        if let Step::Syncing { bitcoind_logs, .. } = &mut self.step {
            if let Some(l) = log {
                *bitcoind_logs = l;
            }
        }
        Task::none()
    }

    fn on_start(&mut self, res: StartedResult) -> Task<Message> {
        if self.cube_settings.network.is_blake2b() && self.fork_session_revoked {
            if let Ok((daemon, mut bitcoind, _)) = res {
                Self::discard_revoked_fork_start(daemon.as_ref(), &mut bitcoind);
            }
            return Task::none();
        }
        match res {
            Ok((daemon, mut bitcoind, info)) => {
                // bitcoind may have been already started and given to the loader
                // We should not override with None the loader bitcoind field
                if let Some(bitcoind) = bitcoind.node.take() {
                    self.internal_bitcoind = Some(bitcoind);
                }
                self.waiting_daemon_bitcoind = false;
                self.maybe_skip_syncing(daemon, info)
            }
            Err(e) => {
                error!("Loader: start_bitcoind_and_daemon failed: {}", e);
                self.step = Step::Error(Box::new(e));
                self.set_quote_context("error");
                Task::none()
            }
        }
    }

    fn discard_revoked_fork_start(
        daemon: &(dyn Daemon + Sync + Send),
        bitcoind: &mut PendingForkNode,
    ) {
        daemon.invalidate_connect_session();
        if daemon.backend().is_embedded() {
            if let Err(error) = Handle::current().block_on(daemon.stop()) {
                warn!("Revoked fork daemon failed to stop: {error}");
            }
        }
        if let Some(node) = bitcoind.node.take() {
            node.stop();
        }
    }

    fn on_sync(&mut self, res: Result<GetInfoResult, DaemonError>) -> Task<Message> {
        match &mut self.step {
            Step::Syncing {
                daemon, progress, ..
            } => {
                match res {
                    Ok(info) => {
                        if (info.sync - 1.0_f64).abs() < f64::EPSILON {
                            let wallet_settings = self
                                .wallet_settings
                                .clone()
                                .expect("wallet_settings must be Some when syncing");
                            return Task::perform(
                                load_application(LoadApplicationConfig {
                                    wallet_settings,
                                    cube_settings: self.cube_settings.clone(),
                                    daemon: daemon.clone(),
                                    info,
                                    datadir_path: self.datadir_path.clone(),
                                    network: self.network,
                                    internal_bitcoind: self.internal_bitcoind.clone(),
                                    backup: self.backup.clone(),
                                }),
                                Message::Synced,
                            );
                        } else {
                            *progress = info.sync
                        }
                    }
                    Err(e) => {
                        error!("Loader: sync poll failed: {}", e);
                        self.step = Step::Error(Box::new(e.into()));
                        self.set_quote_context("error");
                        return Task::none();
                    }
                }
                Task::perform(sync(daemon.clone(), true), Message::Syncing)
            }
            _ => Task::none(),
        }
    }

    pub fn invalidate_fork_session(&mut self) {
        if !self.cube_settings.network.is_blake2b() {
            return;
        }
        if let Step::Syncing { daemon, .. } = &self.step {
            daemon.invalidate_connect_session();
        }
        if let Some(handle) = self.fork_task.take() {
            handle.abort();
        }
        self.cancel_fork_resync();
        if let Some(node) = self.internal_bitcoind.take() {
            node.stop();
        }
        self.fork_session_revoked = true;
        self.connect_client = None;
        self.cube_encryption_key = None;
        self.step = Step::Error(Box::new(Error::Unexpected(
            "Connect session changed; reopen this Cube".into(),
        )));
    }

    /// Stop a resync in flight from changing anything it has not yet changed.
    fn cancel_fork_resync(&self) {
        self.fork_resync_cancel
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Drop the Connect session a local-backend loader carries for a resync
    /// (any sign-in change), and stop a resync in flight from using it.
    pub(crate) fn clear_connect_session(&mut self) {
        self.connect_client = None;
        self.cancel_fork_resync();
    }

    pub fn stop(&mut self) {
        self.stop_with(false);
    }

    /// [`Self::stop`] for locking a Cube: the daemon, then any managed node,
    /// stops on its own thread (see [`crate::app::App::stop_in_background`]).
    pub fn stop_in_background(&mut self) {
        self.stop_with(true);
    }

    fn stop_with(&mut self, in_background: bool) {
        if let Some(handle) = self.fork_task.take() {
            handle.abort();
        }
        self.cancel_fork_resync();
        info!("Close requested");
        // NOTE: we take() the internal_bitcoind here to make sure the debug.log reader
        // subscription is dropped.
        let bitcoind = self.internal_bitcoind.take();
        if let Step::Syncing { daemon, .. } = &mut self.step {
            if daemon.backend().is_embedded() {
                if in_background {
                    crate::daemon::stopping::stop_in_background(daemon.clone(), move || {
                        if let Some(bitcoind) = bitcoind {
                            bitcoind.stop();
                        }
                    });
                    return;
                }
                Self::stop_embedded_daemon(daemon.as_ref());
            }
        }
        if let Some(bitcoind) = bitcoind {
            bitcoind.stop();
        }
    }

    fn stop_embedded_daemon(daemon: &(dyn Daemon + Sync + Send)) {
        info!("Stopping internal daemon...");
        if let Err(e) = Handle::current().block_on(async { daemon.stop().await }) {
            warn!("Internal daemon failed to stop: {}", e);
        } else {
            info!("Internal daemon stopped");
        }
    }

    /// Surface a refusal raised by the tab on this Loader's behalf (for
    /// example an installer the Cube's chain does not admit) without
    /// starting anything.
    pub(crate) fn fail(&mut self, error: Error) {
        self.step = Step::Error(Box::new(error));
    }

    /// Start over with the same Cube: a fresh Loader and its initial task.
    fn rebuild(&mut self) -> Task<Message> {
        let (loader, cmd) = Self::build(
            self.datadir_path.clone(),
            self.gui_config.clone(),
            self.network,
            self.internal_bitcoind.clone(),
            self.backup.clone(),
            self.wallet_settings.clone(),
            self.cube_settings.clone(),
            self.cube_signer.clone(),
            self.spark_backend.clone(),
            self.connect_client.clone(),
            self.cube_encryption_key.clone(),
        );
        *self = loader;
        cmd
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::View(ViewMessage::Retry) => {
                // After a resync that did not finish, the node's chain data
                // is partly deleted: only finishing the resync may start it.
                if matches!(&self.step, Step::Error(e) if matches!(**e, Error::ForkResyncFailed(_)))
                {
                    return Task::none();
                }
                if self.cube_settings.network.is_blake2b() {
                    // Reopening must establish a fresh session after revocation.
                    // Keep the specific refusal instead of rebuilding without auth.
                    if self.fork_session_revoked {
                        return Task::none();
                    }
                    if let Some(deadline) = self.fork_retry_not_before {
                        let remaining =
                            deadline.saturating_duration_since(std::time::Instant::now());
                        if !remaining.is_zero() {
                            // The resync is not held to this delay; keep its
                            // page (and button) rather than replace it.
                            if matches!(&self.step, Step::Error(e) if e.offers_fork_resync()) {
                                return Task::none();
                            }
                            self.step = Step::Error(Box::new(Error::Unexpected(format!(
                                "Connect startup can be retried in {} seconds. Wait for the backend to be ready, then Retry.", remaining.as_secs() + 1))));
                            return Task::none();
                        }
                    }
                }
                self.rebuild()
            }
            Message::View(ViewMessage::ResyncForkNode) => {
                // Only from the errors it recovers, and never past a revoked
                // session (Retry refuses that too).
                if !self.cube_settings.network.is_blake2b()
                    || self.fork_session_revoked
                    || !matches!(&self.step, Step::Error(e) if e.offers_fork_resync())
                {
                    return Task::none();
                }
                // Results are bound to this loader and this resync: a result
                // that reaches a later loader in the same tab, or an older
                // resync's result, is dropped.
                static NEXT_RESYNC: std::sync::atomic::AtomicU64 =
                    std::sync::atomic::AtomicU64::new(1);
                let token = NEXT_RESYNC.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                self.fork_resync = Some(token);
                let root = self.datadir_path.clone();
                let chain = self.cube_settings.network;
                let wallet = self.wallet_settings.clone();
                let client = self.connect_client.clone();
                let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
                self.fork_resync_cancel = cancel.clone();
                self.step = Step::Connecting;
                self.set_quote_context("loading");
                Task::perform(
                    resync_fork_cube(root, chain, wallet, client, cancel),
                    move |result| Message::ForkNodeResynced { token, result },
                )
            }
            Message::ForkNodeResynced { token, .. }
                if self.fork_session_revoked || self.fork_resync != Some(token) =>
            {
                Task::none()
            }
            // The user asked for this start by deleting the chain data, so it
            // is not held to the Connect retry delay the Retry button applies.
            // A Cube the resync moved to Connect reopens there: the rebuilt
            // loader finds it on Connect and opens it with the session it
            // carries, as `new_for_chain` would. If that session was cleared
            // meanwhile, the rebuild refuses and the Cube opens after sign-in.
            Message::ForkNodeResynced { result: Ok(()), .. } => {
                self.fork_resync = None;
                // Moved to Connect, but the session was cleared after the
                // move: the Cube opens again once signed in.
                if self.requires_connect() && self.connect_client.is_none() {
                    self.step = Step::Error(Box::new(Error::Unexpected(
                        "The Bitcoin Blake2b node is resyncing and this Cube now uses Connect \
                         until it has caught up. Sign in to Connect from Home, then reopen this \
                         Cube."
                            .into(),
                    )));
                    self.set_quote_context("error");
                    return Task::none();
                }
                self.rebuild()
            }
            // A failed resync stays offered: a partial deletion is finished by
            // the next one, while Retry would start the node on half-deleted
            // data. A refused one deleted nothing and would only be refused
            // again, so it is not offered.
            Message::ForkNodeResynced { result: Err(e), .. } => {
                self.fork_resync = None;
                error!("Loader: resyncing the Bitcoin Blake2b node failed: {:?}", e);
                self.step = Step::Error(Box::new(match e {
                    ForkResyncError::Failed(e) => Error::ForkResyncFailed(e),
                    ForkResyncError::Refused(e) => Error::Unexpected(format!(
                        "The Bitcoin Blake2b node was not resynced: {}.",
                        e
                    )),
                }));
                self.set_quote_context("error");
                Task::none()
            }
            Message::Started(res) => self.on_start(res),
            Message::Loaded(res) => self.on_load(res),
            Message::Syncing(res) => self.on_sync(res),
            Message::BitcoindLog(log) => self.on_log(log),
            Message::LoadingTick => {
                match &mut self.step {
                    // ~1% per 500ms tick — daemon usually starts in < 10s so
                    // this will comfortably reach 90% well before it connects.
                    Step::StartingDaemon { progress } => {
                        *progress = (*progress + 0.010).min(0.90);
                    }
                    // ~0.4% per 500ms tick — full scan takes 1-2 minutes.
                    Step::FullScan { progress } => {
                        *progress = (*progress + 0.004).min(0.90);
                    }
                    _ => {}
                }
                Task::none()
            }
            Message::Synced(Err(e)) => {
                error!("Loader: load_application failed: {}", e);
                self.step = Step::Error(Box::new(e));
                self.set_quote_context("error");
                Task::none()
            }
            Message::Failure(_) => {
                self.daemon_started = false;
                Task::none()
            }
            Message::None => Task::none(),
            _ => Task::none(),
        }
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let log_sub = if self.internal_bitcoind.is_some() {
            let log_path = managed_node_log_path(&self.datadir_path, self.cube_settings.network);
            iced::Subscription::run_with(log_path, |log_path| get_bitcoind_log(log_path.clone()))
                .map(Message::BitcoindLog)
        } else {
            Subscription::none()
        };
        let scan_sub = if matches!(
            self.step,
            Step::StartingDaemon { .. } | Step::FullScan { .. }
        ) {
            time::every(Duration::from_millis(500)).map(|_| Message::LoadingTick)
        } else {
            Subscription::none()
        };
        Subscription::batch([log_sub, scan_sub])
    }

    pub fn view(&self) -> Element<Message> {
        let scene = crate::home::launcher_presenter::project_opening(
            &self.cube_settings.name,
            self.cube_settings.network,
        );
        crate::home::launcher_presenter::opening_body(
            scene,
            &self.step,
            &self.current_quote,
            &self.current_image_handle,
            !self.can_retry(),
        )
        .map(Message::View)
    }

    /// The stage line the opening Cube shows for this step, quoted from
    /// [`view_with_retry`]. An error has none: its page owns Back and Retry.
    pub(crate) fn opening_stage(&self) -> Option<crate::home::launcher_opening::Stage> {
        use crate::home::launcher_opening::Stage;
        Some(match &self.step {
            Step::Connecting => Stage::Connecting,
            Step::StartingDaemon { .. } => Stage::StartingDaemon,
            Step::FullScan { .. } => Stage::Scanning,
            Step::Syncing { progress, .. } if *progress > 0.98 => Stage::AlmostThere,
            Step::Syncing { .. } => Stage::Syncing,
            Step::Error(_) => return None,
        })
    }

    fn can_retry(&self) -> bool {
        !self.fork_session_revoked
    }
}

pub fn get_bitcoind_log(log_path: PathBuf) -> impl Stream<Item = Option<String>> {
    channel(5, async move |mut output| {
        loop {
            // Reduce the io load.
            tokio::time::sleep(Duration::from_millis(500)).await;

            // Open the log file and seek to its end, with some breathing room to make sure
            // we don't skip all "UpdateTip" lines. This is to avoid making BufReader read
            // the whole file every single time below.
            let mut file = match File::open(&log_path) {
                Ok(file) => file,
                Err(e) => {
                    log::warn!("Opening bitcoind log file: {}", e);
                    continue;
                }
            };
            seek_bitcoind_log_tail(&mut file);

            // Find the latest tip update line in bitcoind's debug.log. BufReader is only
            // used to facilitates searching through the lines.
            let reader = BufReader::new(file);
            let last_update_tip = reader
                .lines()
                .filter(|l| {
                    l.as_ref()
                        .map(|l| l.contains("UpdateTip") || l.contains("blockheaders"))
                        .unwrap_or(false)
                })
                .last();
            match last_update_tip {
                Some(Ok(line)) => {
                    let _ = output.send(Some(line)).await;
                }
                res => {
                    if let Some(Err(e)) = res {
                        log::error!("Reading bitcoind log file: {}", e);
                    } else {
                        log::warn!("Couldn't find an UpdateTip line in bitcoind log file.");
                    }
                }
            }
        }
    })
}

fn seek_bitcoind_log_tail(file: &mut File) {
    match file.metadata() {
        Ok(m) => {
            let file_len = m.len();
            let offset = 1024 * 1024;
            if file_len > offset {
                if let Err(e) = file.seek(SeekFrom::Start(file_len.saturating_sub(offset))) {
                    log::error!("Seeking to end of bitcoind log file: {}", e);
                }
            }
        }
        Err(e) => {
            log::error!("Getting bitcoind log file metadata: {}", e);
        }
    }
}

pub struct LoadApplicationConfig {
    pub wallet_settings: WalletSettings,
    pub cube_settings: CubeSettings,
    pub daemon: Arc<dyn Daemon + Sync + Send>,
    pub info: GetInfoResult,
    pub datadir_path: CoincubeDirectory,
    pub network: bitcoin::Network,
    pub internal_bitcoind: Option<Bitcoind>,
    pub backup: Option<Backup>,
}

pub async fn load_application(
    config: LoadApplicationConfig,
) -> Result<
    (
        Arc<Wallet>,
        Cache,
        Arc<dyn Daemon + Sync + Send>,
        Option<Bitcoind>,
        Option<Backup>,
        CubeSettings,
    ),
    Error,
> {
    let bitcoin_unit = config.cube_settings.unit_setting.display_unit;

    let wallet = if let Some(import) = &config.cube_settings.bitcoin_import {
        let attachment = import.active_attachment().ok_or_else(|| {
            Error::Unexpected("Finish importing this wallet before opening it".into())
        })?;
        let imported = config.info.descriptors.main.as_imported().ok_or_else(|| {
            Error::Unexpected("The running Bitcoin wallet is not this Cube's import".into())
        })?;
        let status = config.info.imported_wallet.as_ref().ok_or_else(|| {
            Error::Unexpected("The running Bitcoin wallet has no import identity".into())
        })?;
        if config.info.chain_id != Some(config.cube_settings.network)
            || imported != &attachment.descriptor
            || status.chain_id != config.cube_settings.network
            || attachment.wallet_id.descriptor_checksum
                != format!("imported-v1-{}", status.wallet_identity)
            || status.birthday != attachment.birthday
            || status.initial_lookahead != attachment.initial_lookahead
            || config.wallet_settings.wallet_id() != attachment.wallet_id
        {
            return Err(Error::Unexpected(
                "The running Bitcoin wallet does not match this Cube's imported descriptors".into(),
            ));
        }
        // The Cube's seed secures its Spark account and Recovery Kit.
        // It is never an implicit signer for the imported Bitcoin descriptors.
        Wallet::from_imported_cube(&config.cube_settings)
            .map_err(Error::Unexpected)?
            .load_from_settings(config.wallet_settings)?
            .with_name(config.cube_settings.name.clone())
    } else {
        // A Vault hot signer is encrypted under the same credential the Cube
        // unlocks with. Only a strict Vault descriptor may load those seeds.
        let seed_password = crate::app::session::seed_file_password(&config.cube_settings);
        let descriptor = config
            .info
            .descriptors
            .main
            .as_vault()
            .cloned()
            .ok_or_else(|| Error::Unexpected("The running wallet is not a Vault".into()))?;
        Wallet::new(descriptor)
            .with_chain(config.cube_settings.network)
            .load_from_settings(config.wallet_settings)?
            .load_hotsigners(
                &config.datadir_path,
                config.cube_settings.network,
                &config.cube_settings.id,
                seed_password.as_deref().map(|p| p.as_str()),
            )?
    };

    let coins = coins_to_cache(config.daemon.clone())
        .await
        .map(|res| res.coins)?;

    let display_mode = crate::app::settings::Settings::from_file(
        &config
            .datadir_path
            .network_directory(config.cube_settings.network),
    )
    .ok()
    .map(|s| s.display_mode)
    .unwrap_or_default();
    // Both last poll fields start with the same value.
    let cache = Cache {
        app_generation: crate::app::cache::AppGeneration::next(),
        unswept_notice: None,
        node_history: crate::app::node_history::Status::default(),
        node_history_switch_requested: false,
        local_node_transfer_visible: false,
        background_node_install: Default::default(),
        connect_transport_key: None,
        cube_encryption_key: None,
        cube_switcher_open: false,
        connect_panel_open: false,
        cube_lock_confirmation: false,
        datadir_path: config.datadir_path,
        network: config.info.network,
        fiat_chain: config.cube_settings.network,
        btcb2_price_request: None,
        // Recomputed from the P2P panel's Mostro config once panels are built.
        p2p_test_coordinator: false,
        // Fail-closed until `/connect/features` loads and the account panel
        // mirrors the real flags in (see `App::update`'s ConnectAccount arm).
        marketplace_flags: crate::app::features::MarketplaceServerFlags::OFF,
        // Same fail-closed stance for the fork account grant.
        btcb2_server_enabled: false,
        // Resolved from disk in `App::new_inner`.
        btcb2_already_claimed: false,
        entangled: std::collections::HashMap::new(),
        chain_proofs: std::collections::HashMap::new(),
        split_records: Vec::new(),
        splitting: std::collections::HashMap::new(),
        split_unconfirmed: std::collections::HashSet::new(),
        separated: std::collections::HashMap::new(),
        last_poll_at_startup: config.info.last_poll_timestamp,
        daemon_cache: DaemonCache {
            blockheight: config.info.block_height,
            coins,
            sync_progress: config.info.sync,
            last_poll_timestamp: config.info.last_poll_timestamp,
            history_sync: config.info.history_sync.clone(),
            ..Default::default()
        },
        fiat_price: None,
        bitcoin_unit,
        display_mode,
        node_bitcoind_sync_progress: None,
        node_bitcoind_sync_heights: None,
        node_bitcoind_ibd: None,
        node_bitcoind_subversion: None,
        node_bitcoind_pruning: None,
        local_switch_history: None,
        daemon_switch_in_progress: false,
        node_bitcoind_last_log: None,
        node_net_stats: None,
        connect_authenticated: false,
        sidebar_connect_signed_in: false,
        // Local-daemon launch has no Connect session until the user signs in
        // via the Connect tab; `App::new` flips this on if it restores one.
        has_connect_session: false,
        restorable_connect_session: false,
        has_vault: !wallet.is_imported(),
        has_imported_bitcoin: wallet.is_imported(),
        imported_stored_keys: None,
        imported_node: None,
        vault_lacks_keychain_key: wallet.lacks_keychain_key(),
        cube_name: config.cube_settings.name.clone(),
        cube_name_sync: Default::default(),
        spark_nickname_save: Default::default(),
        wallet_names: Default::default(),
        cube_connect_owner: config.cube_settings.connect_owner.clone(),
        cube_owned_by_other_account: false,
        current_cube_backed_up: config.cube_settings.backed_up,
        spark_notice: None,
        has_p2p: false, // Set later by App::new based on mnemonic availability
        theme_mode: coincube_ui::theme::palette::ThemeMode::default(),
        reduce_motion: false,
        connect_light_dim: false,
        lightning_address_setup: crate::app::cache::LightningAddressSetup::Idle,
        balance_privacy: None,
        btc_usd_price: None,
        btc_usd_requested_at: None,
        show_direction_badges: true,
        lightning_address: None,
        avatar_handle: None,
        avatar_regenerations_remaining: None,
        cube_id: config.cube_settings.id.clone(),
        current_cube_server_id: None,
        current_descriptor_fingerprint: None,
        recovery_kit_last_backed_up_descriptor_fingerprint: config
            .cube_settings
            .recovery_kit_last_backed_up_descriptor_fingerprint
            .clone(),
        recovery_kit_last_backed_up_keychain_descriptor_fingerprint: config
            .cube_settings
            .recovery_kit_last_backed_up_keychain_descriptor_fingerprint
            .clone(),
        recovery_kit_password_backed_up: config.cube_settings.recovery_kit_password_backed_up,
        // Local-daemon path has no Connect tokens or gRPC URL; the user
        // would have to sign in via the Connect tab to populate these.
        connect_grpc_url: None,
        connect_tokens: None,
        connect_stream_status: crate::app::ConnectionStatus::default(),
        connect_signed_out_by: None,
        vault_alert: None,
        vault_via_connect: false,
        connect_device_id: None,
        connect_email: None,
    };

    Ok((
        Arc::new(wallet),
        cache,
        config.daemon,
        config.internal_bitcoind,
        config.backup,
        config.cube_settings,
    ))
}

#[derive(Clone, Debug)]
pub enum ViewMessage {
    Retry,
    /// Delete the Bitcoin Blake2b node's chain data and start again, offered
    /// only for [`StartInternalBitcoindError::PrunedRevalidationRequired`].
    ResyncForkNode,
    SwitchNetwork,
    SetupVault,
}

pub fn view<'a>(
    step: &'a Step,
    quote: &'a Quote,
    image_handle: &'a image::Handle,
) -> Element<'a, ViewMessage> {
    view_with_retry(step, quote, image_handle, true)
}

pub(crate) fn view_with_retry<'a>(
    step: &'a Step,
    quote: &'a Quote,
    image_handle: &'a image::Handle,
    allow_retry: bool,
) -> Element<'a, ViewMessage> {
    match &step {
        Step::StartingDaemon { .. } => cover(
            None,
            Column::new()
                .width(Length::Fill)
                .spacing(20)
                .align_x(Alignment::Center)
                .push(quote_display::display(&QuoteDisplayProps::new(
                    "loading",
                    quote,
                    image_handle,
                )))
                .push(crate::loading::loading_indicator(None))
                .push(text(CopyText::StartingDaemon.get()).style(theme::text::secondary)),
        ),
        Step::FullScan { .. } => cover(
            None,
            Column::new()
                .width(Length::Fill)
                .spacing(10)
                .align_x(Alignment::Center)
                .push(quote_display::display(&QuoteDisplayProps::new(
                    "syncing",
                    quote,
                    image_handle,
                )))
                .push(crate::loading::loading_indicator(None))
                .push(text(CopyText::ScanningTheBlockchain.get()).style(theme::text::secondary))
                .push_maybe(
                    coincube_ui::worldview_copy::VoiceLine::Syncing
                        .get()
                        .map(|line| p2_regular(line).style(theme::text::secondary)),
                )
                .push(p2_regular(CopyText::InitialEsploraScan.get()).style(theme::text::secondary)),
        ),
        Step::Connecting => cover(
            None,
            Column::new()
                .width(Length::Fill)
                .spacing(20)
                .align_x(Alignment::Center)
                .push(quote_display::display(&QuoteDisplayProps::new(
                    "loading",
                    quote,
                    image_handle,
                )))
                .push(crate::loading::loading_indicator(None))
                .push(text(CopyText::ConnectingToDaemon.get()).style(theme::text::secondary)),
        ),
        Step::Syncing {
            progress,
            bitcoind_logs,
            ..
        } => cover(
            None,
            Column::new()
                .width(Length::Fill)
                .spacing(5)
                .align_x(Alignment::Center)
                .push(quote_display::display(&QuoteDisplayProps::new(
                    "syncing",
                    quote,
                    image_handle,
                )))
                .push(crate::loading::loading_indicator(None))
                .push(
                    text(if *progress > 0.98 {
                        CopyText::AlmostThere.get()
                    } else {
                        CopyText::SyncingBlockchain.get()
                    })
                    .style(theme::text::secondary),
                )
                .push_maybe(
                    coincube_ui::worldview_copy::VoiceLine::Syncing
                        .get()
                        .map(|line| p2_regular(line).style(theme::text::secondary)),
                )
                .push(p2_regular(bitcoind_logs).style(theme::text::secondary)),
        ),
        Step::Error(error) => {
            let headline = match error.as_ref() {
                Error::Daemon(DaemonError::Start(StartupError::Bitcoind(_))) => {
                    CopyText::TenshuFailedToStartPleaseCheckIfBitcoindIs.get()
                }
                Error::Daemon(DaemonError::Start(StartupError::Esplora(_))) => {
                    CopyText::StartupEsploraUnreachable.get()
                }
                Error::Daemon(DaemonError::Start(StartupError::Electrum(_))) => {
                    CopyText::StartupElectrumUnreachable.get()
                }
                _ => CopyText::TenshuFailedToStart.get(),
            };
            cover(
                Some((CopyText::ErrorWhileStartingTenshu.get(), error)),
                Column::new()
                    .spacing(20)
                    .width(Length::Fill)
                    .align_x(Alignment::Center)
                    .push(quote_display::display(&QuoteDisplayProps::new(
                        "error",
                        quote,
                        image_handle,
                    )))
                    .push(text(headline))
                    .push({
                        let actions = Row::new().spacing(10).push(
                            button::secondary(None, CopyText::Back.get())
                                .width(Length::Fixed(200.0))
                                .on_press(ViewMessage::SwitchNetwork),
                        );
                        if allow_retry {
                            actions
                                .push(
                                    button::secondary(None, CopyText::Retry.get())
                                        .width(Length::Fixed(200.0))
                                        .on_press(ViewMessage::Retry),
                                )
                                .push(error.offers_fork_resync().then(|| {
                                    button::primary(None, CopyText::ResyncForkNodeChainData.get())
                                        .width(Length::Fixed(260.0))
                                        .on_press(ViewMessage::ResyncForkNode)
                                }))
                        } else {
                            actions
                        }
                    }),
            )
        }
    }
}

pub fn cover<'a, T: 'a + Clone, C: Into<Element<'a, T>>>(
    warn: Option<(&'static str, &Error)>,
    content: C,
) -> Element<'a, T> {
    Column::new()
        .push(warn.map(|w| notification::warning(w.0.to_string(), w.1.to_string())))
        .push(
            Container::new(content)
                .center_x(iced::Length::Fill)
                .center_y(iced::Length::Fill)
                .padding(50),
        )
        .into()
}

async fn connect(
    socket_path: PathBuf,
) -> Result<(Arc<dyn Daemon + Sync + Send>, GetInfoResult), Error> {
    let client = client::jsonrpc::JsonRPCClient::new(socket_path);
    let daemon = Coincubed::new(client);

    debug!("Searching for external daemon");
    let info = daemon.get_info().await?;
    info!("Connected to external daemon");

    Ok((Arc::new(daemon), info))
}

// Daemon can start only if a config path is given.
/// Whether the `daemon.toml` at `config_path` selects the app-managed internal
/// bitcoind as its RPC backend — that is, a cookie file located inside
/// `internal_datadir` (the managed bitcoind datadir). External nodes return
/// false: `auth = "user:pass"`, a cookie outside the managed datadir, or any
/// non-bitcoind backend. A missing or unparseable config also returns false.
/// See [`Loader::start_bitcoind`].
fn backend_is_internal_bitcoind(config_path: &Path, internal_datadir: &Path) -> bool {
    match Config::from_file(Some(config_path.to_path_buf())) {
        Ok(config) => matches!(
            config.bitcoin_backend,
            Some(BitcoinBackend::Bitcoind(BitcoindConfig {
                rpc_auth: BitcoindRpcAuth::CookieFile(path),
                ..
            })) if path.starts_with(internal_datadir)
        ),
        Err(_) => false,
    }
}

/// Dedicated fork restart path, preserving exact-chain managed companions.
/// Generic external sockets and config migration remain unavailable.
#[cfg(test)]
pub(crate) async fn start_connect_daemon(
    root: CoincubeDirectory,
    chain: crate::chain::ChainId,
    settings: WalletSettings,
    client: crate::services::coincube::CoincubeClient,
) -> StartedResult {
    if client.token().is_none() {
        return Err(Error::Unexpected(
            "Authenticated Connect fork Vault required".into(),
        ));
    }
    start_fork_daemon(root, chain, settings, Some(client)).await
}

// Worker results retain cleanup ownership when the async recipient is canceled.
#[derive(Debug)]
pub struct PendingForkNode {
    node: Option<Bitcoind>,
    cleanup: bool,
}
#[cfg(test)]
impl PendingForkNode {
    pub(crate) fn is_none(&self) -> bool {
        self.node.is_none()
    }
}
impl Drop for PendingForkNode {
    fn drop(&mut self) {
        if self.cleanup {
            if let Some(node) = self.node.take() {
                node.stop();
            }
        }
    }
}

/// Why a Cube whose wallet runs on its local node is not resynced: there is no
/// Connect session to reopen it on.
///
/// Such a Cube cannot be admitted to a node in initial block download, so a
/// resync from scratch first moves it back to Connect with the node pending
/// (see [`connect_demoted_config`]). Its loader then reopens it through
/// Connect, which needs a session. Without one the Cube could not reopen, so
/// nothing is deleted or changed.
const LOCAL_FORK_RESYNC_NEEDS_CONNECT: &str =
    "this Cube's wallet runs on its local node. Resyncing moves it to Connect until the \
     node has caught up, which needs a Connect session. Nothing was deleted. Sign in to \
     Connect from Home, reopen this Cube, then resync";

/// Why a resync did not complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForkResyncError {
    /// Refused for this Cube before anything was deleted; repeating it would
    /// be refused the same way.
    Refused(String),
    /// Did not finish (the node is still running, a deletion failed, …); a
    /// repeat can complete it.
    Failed(String),
}

/// The configuration of a Cube whose wallet runs on its local node, moved
/// back to Connect for a resync: the node becomes the pending one, adopted
/// (`auto_switch_to_pending = Some(true)`) so the App promotes it again once
/// it has caught up. This is the inverse of the App's promotion
/// (`synced_bitcoind_config`) and the state of a Cube before its first one.
/// The Connect endpoint is rebuilt for the chain, as a switch to Connect does,
/// and the session token is never persisted ([`Config::for_persistence`]).
fn connect_demoted_config(cfg: &Config) -> Option<Config> {
    let Some(BitcoinBackend::Bitcoind(node)) = &cfg.bitcoin_backend else {
        return None;
    };
    let chain = cfg.bitcoin_config.chain;
    let mut esplora = crate::installer::connect_esplora_config(chain, "");
    esplora.token = None;
    let mut demoted = cfg.clone();
    demoted.pending_bitcoind = Some(node.clone());
    demoted.bitcoin_backend = Some(BitcoinBackend::Esplora(esplora));
    demoted.fallback_esplora = None;
    demoted.pending_managed_node_install = None;
    demoted.auto_switch_to_pending = Some(true);
    demoted.bitcoin_config.poll_interval_secs =
        Duration::from_secs(coincubed::config::ESPLORA_POLL_INTERVAL_SECS);
    Some(demoted.for_persistence())
}

/// Resync a Bitcoin Blake2b Cube's node. See [`resync_fork_cube_with`]; the
/// Connect check is [`confirm_connect_reopen`].
async fn resync_fork_cube(
    root: CoincubeDirectory,
    chain: crate::chain::ChainId,
    wallet: Option<WalletSettings>,
    client: Option<crate::services::coincube::CoincubeClient>,
    cancel: Arc<std::sync::atomic::AtomicBool>,
) -> Result<(), ForkResyncError> {
    resync_fork_cube_with(
        root,
        chain,
        wallet,
        client,
        cancel,
        |chain, client, root| async move { confirm_connect_reopen(chain, client, root).await },
    )
    .await
}

/// Whether the Connect start that reopens a Cube moved to Connect would be
/// admitted now: Bitcoin Blake2b is enabled for the account
/// ([`crate::chain::require_connect_feature`], as the start checks) and
/// Connect's anchor is available and trusted (as the authenticated daemon
/// start requires).
async fn confirm_connect_reopen(
    chain: crate::chain::ChainId,
    client: crate::services::coincube::CoincubeClient,
    root: CoincubeDirectory,
) -> Result<(), String> {
    crate::chain::require_connect_feature(chain, &client, &root).await?;
    client.anchor_admits_start(chain).await
}

/// Resync a Bitcoin Blake2b Cube's node: delete its chain data so it starts
/// from scratch. A repeat after a partial failure finishes the job: missing
/// directories are not an error.
///
/// A Cube whose wallet runs on its local node must be able to reopen after
/// the deletion, so before anything is deleted:
/// - it needs an authenticated Connect `client`, or it is refused;
/// - `verify_connect` (whether the reopen would be admitted) must pass;
/// - its daemon.toml is moved to Connect ([`connect_demoted_config`]) under
///   the resync's locks, once every refusal has passed, only if the file is
///   still what was read and `cancel` is not set. If any of this fails,
///   nothing is deleted.
///
/// A Cube already on Connect is resynced without touching its configuration.
/// Any other node backend is refused: deleting its chain data would leave a
/// wallet that cannot be admitted to it and cannot move to Connect from here.
async fn resync_fork_cube_with<V, F>(
    root: CoincubeDirectory,
    chain: crate::chain::ChainId,
    wallet: Option<WalletSettings>,
    client: Option<crate::services::coincube::CoincubeClient>,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    verify_connect: V,
) -> Result<(), ForkResyncError>
where
    V: FnOnce(
        crate::chain::ChainId,
        crate::services::coincube::CoincubeClient,
        CoincubeDirectory,
    ) -> F,
    F: std::future::Future<Output = Result<(), String>>,
{
    let mut demotion = None;
    if let Some(wallet) = wallet {
        let path = root
            .network_directory(chain)
            .coincubed_data_directory(&wallet.wallet_id())
            .path()
            .join("daemon.toml");
        let read = std::fs::read(&path).map_err(|e| {
            ForkResyncError::Failed(format!("could not read this Cube's configuration: {e}"))
        })?;
        let cfg = Config::from_file(Some(path.clone())).map_err(|e| {
            ForkResyncError::Failed(format!("could not read this Cube's configuration: {e}"))
        })?;
        if crate::chain::is_managed_local_fork(&cfg, &root) {
            let client = client
                .filter(|c| c.token().is_some())
                .ok_or_else(|| ForkResyncError::Refused(LOCAL_FORK_RESYNC_NEEDS_CONNECT.into()))?;
            // Refused, not offered again: a session that fails now (an
            // expired token, Connect unavailable) needs the Cube reopened.
            verify_connect(chain, client, root.clone())
                .await
                .map_err(|e| {
                    ForkResyncError::Refused(format!(
                        "this Cube's wallet runs on its local node and must move to Connect \
                         while the node resyncs, but Connect cannot reopen it now ({e}). Nothing \
                         was deleted. Reopen this Cube to try again"
                    ))
                })?;
            let demoted = connect_demoted_config(&cfg).ok_or_else(|| {
                ForkResyncError::Failed("this Cube's local node configuration is missing".into())
            })?;
            let content = toml::to_string(&demoted).map_err(|e| {
                ForkResyncError::Failed(format!("could not write this Cube's configuration: {e}"))
            })?;
            demotion = Some((path, read, content));
        } else if matches!(cfg.bitcoin_backend, Some(BitcoinBackend::Bitcoind(_))) {
            return Err(ForkResyncError::Refused(
                "this Cube's node is not the managed Bitcoin Blake2b node, so nothing was deleted"
                    .into(),
            ));
        }
    }
    tokio::task::spawn_blocking(move || {
        crate::node::bitcoind::resync_managed_fork_chain_data_then(&root, chain, move || {
            let Some((path, read, content)) = demotion else {
                return Ok(());
            };
            if cancel.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(
                    "the Connect session changed or the Cube was closed during the resync, so \
                     nothing was deleted"
                        .into(),
                );
            }
            if std::fs::read(&path).ok().as_deref() != Some(read.as_slice()) {
                return Err(
                    "this Cube's configuration changed during the resync, so nothing was deleted"
                        .into(),
                );
            }
            crate::services::chain_status::store::write_atomically(&path, content.as_bytes())
                .map_err(|e| {
                    format!("could not move this Cube to Connect ({e}), so nothing was deleted")
                })?;
            info!("Moved this Cube to Connect for the Bitcoin Blake2b resync");
            Ok(())
        })
    })
    .await
    .unwrap_or_else(|e| Err(e.to_string()))
    .map_err(ForkResyncError::Failed)
}

/// Install the pinned Bitcoin Blake2b binary when it is missing (see
/// [`crate::installer::step::node::bitcoind::ensure_pinned_fork_binary`]).
#[cfg(not(test))]
async fn provision_pinned_fork_binary(
    root: CoincubeDirectory,
    chain: crate::chain::ChainId,
) -> Result<bool, String> {
    crate::installer::step::node::bitcoind::ensure_pinned_fork_binary(root, chain).await
}

/// Unit tests never reach the network: the provisioning decision and its
/// verification are tested through `ensure_pinned_fork_binary_with`, and the
/// loader's start paths keep reporting an absent binary as they always did.
#[cfg(test)]
async fn provision_pinned_fork_binary(
    _root: CoincubeDirectory,
    _chain: crate::chain::ChainId,
) -> Result<bool, String> {
    Ok(false)
}

async fn start_fork_daemon(
    root: CoincubeDirectory,
    chain: crate::chain::ChainId,
    settings: WalletSettings,
    client: Option<crate::services::coincube::CoincubeClient>,
) -> StartedResult {
    if let crate::chain::RuntimeSupport::Dormant { reason } =
        crate::chain::authenticated_connect_support(chain)
    {
        return Err(Error::ChainUnavailable(reason));
    }
    if !chain.is_blake2b() || settings.remote_backend_auth.is_some() {
        return Err(Error::Unexpected(
            "Authenticated Connect fork Vault required".into(),
        ));
    }

    let expected_dir = root
        .network_directory(chain)
        .coincubed_data_directory(&settings.wallet_id());
    let cfg =
        Config::from_file(Some(expected_dir.path().join("daemon.toml"))).map_err(Error::Config)?;
    if cfg.bitcoin_config.chain != chain {
        return Err(Error::ChainMismatch {
            cube: chain,
            config: cfg.bitcoin_config.chain,
        });
    }
    let expected =
        std::path::absolute(expected_dir.path()).map_err(|e| Error::Unexpected(e.to_string()))?;
    if cfg
        .data_directory()
        .is_none_or(|d| !crate::chain::same_path(d.path(), &expected))
    {
        return Err(Error::Unexpected(
            "Connect daemon datadir differs from this Cube".into(),
        ));
    }
    let local = match &cfg.bitcoin_backend {
        Some(BitcoinBackend::Bitcoind(node)) => Some(node.clone()),
        Some(BitcoinBackend::Esplora(_)) => cfg.pending_bitcoind.clone(),
        _ => {
            return Err(Error::Unexpected(
                "Unsupported Bitcoin Blake2b backend".into(),
            ))
        }
    };
    let local_backend = crate::chain::is_managed_local_fork(&cfg, &root);
    if matches!(cfg.bitcoin_backend, Some(BitcoinBackend::Bitcoind(_))) && !local_backend {
        return Err(Error::Unexpected(
            "Bitcoin Blake2b requires its isolated managed node".into(),
        ));
    }
    if local_backend {
        crate::chain::require_local_feature(chain, client.clone(), &root)
            .await
            .map_err(Error::Unexpected)?;
    } else {
        let client = client.as_ref().ok_or_else(|| {
            Error::Unexpected("Connect authentication is required for this backend".into())
        })?;
        crate::chain::require_connect_feature(chain, client, &root)
            .await
            .map_err(Error::Unexpected)?;
    }
    let internal = if let Some(node) = local {
        // Only the pinned Blake2b build is ever started, so after a pin bump an
        // existing install has no binary to start: fetch and verify the pinned
        // release first, instead of failing the open with `ExecutableNotFound`.
        // A failed fetch does not stop the start: a pinned node that is already
        // running needs no binary on disk. Its reason is reported only if the
        // start then needs the binary it could not install.
        let provision_error = provision_pinned_fork_binary(root.clone(), chain)
            .await
            .err();
        if let Some(e) = &provision_error {
            warn!("Could not install the pinned Bitcoin Blake2b node: {}", e);
        }
        let root = root.clone();
        tokio::task::spawn_blocking(move || {
            Bitcoind::preflight_for_chain(chain, &root)?;
            let lifecycle = crate::node::bitcoind::ManagedNodeLifecycle::acquire(
                &root,
                crate::node::bitcoind::NodeChainFamily::from_chain(chain),
            )?;
            crate::node::tor::prepare_inbound_tor_for_chain_locked(&root, chain, &lifecycle)
                .map_err(|e| {
                    crate::node::bitcoind::StartInternalBitcoindError::ConfigUnavailable(
                        e.to_string(),
                    )
                })?;
            Bitcoind::maybe_start_for_chain_locked(chain, node, &root, &lifecycle).map(|node| {
                PendingForkNode {
                    node: Some(node),
                    cleanup: true,
                }
            })
        })
        .await
        .map_err(|e| Error::Unexpected(e.to_string()))?
        .map_err(|e| match (e, provision_error) {
            (crate::node::bitcoind::StartInternalBitcoindError::ExecutableNotFound, Some(why)) => {
                Error::Unexpected(why)
            }
            (e, _) => Error::Bitcoind(e),
        })?
    } else {
        PendingForkNode {
            node: None,
            cleanup: true,
        }
    };
    let daemon: Arc<dyn Daemon + Sync + Send> =
        if matches!(cfg.bitcoin_backend, Some(BitcoinBackend::Bitcoind(_))) {
            Arc::new(
                tokio::task::spawn_blocking(move || EmbeddedDaemon::start_local_fork(cfg))
                    .await
                    .map_err(|e| Error::Unexpected(e.to_string()))?
                    .map_err(Error::Daemon)?,
            )
        } else {
            Arc::new(
                EmbeddedDaemon::start_authenticated(
                    cfg,
                    client.expect("authenticated backend checked"),
                )
                .await
                .map_err(Error::Daemon)?,
            )
        };
    let info = daemon.get_info().await.map_err(Error::Daemon)?;
    Ok((daemon, internal, info))
}

/// Compare the Cube's durable imported attachment with the wallet row and
/// daemon config before connecting to an existing socket or starting a node.
/// A shared Loader must never let a stale local wallet directory stand in for
/// the descriptor pair the user attached to this Cube.
fn validate_imported_open(
    root: &CoincubeDirectory,
    cube: &CubeSettings,
    wallet_settings: Option<&WalletSettings>,
) -> Result<(), Error> {
    let Some(import) = &cube.bitcoin_import else {
        return Ok(());
    };
    let Some(attachment) = import.active_attachment() else {
        return if wallet_settings.is_none() {
            Ok(())
        } else {
            Err(Error::Unexpected(
                "An unfinished Bitcoin import cannot open a wallet yet".into(),
            ))
        };
    };
    if cube.network.is_blake2b() {
        crate::services::bitcoin_import::pairing::validate_target_record(cube)
            .map_err(Error::Unexpected)?;
    }
    attachment
        .validate(cube.network)
        .map_err(Error::Unexpected)?;
    if cube.vault_wallet_id.is_some() {
        return Err(Error::Unexpected(
            "This Cube has conflicting Bitcoin wallet attachments".into(),
        ));
    }
    let wallet_settings = wallet_settings.ok_or_else(|| {
        Error::Unexpected("The imported wallet's local registration is missing".into())
    })?;
    if wallet_settings.wallet_id() != attachment.wallet_id
        || wallet_settings.remote_backend_auth.is_some()
    {
        return Err(Error::Unexpected(
            "The imported wallet registration does not match this Cube".into(),
        ));
    }
    let expected_directory = root
        .network_directory(cube.network)
        .coincubed_data_directory(&attachment.wallet_id);
    let config = Config::from_file(Some(expected_directory.path().join("daemon.toml")))
        .map_err(Error::Config)?;
    if config.bitcoin_config.chain != cube.network
        || config.data_directory().as_ref().map(|dir| dir.path()) != Some(expected_directory.path())
        || config.main_descriptor.as_imported() != Some(&attachment.descriptor)
        || config.import_birthday != Some(attachment.birthday)
        || config.import_initial_lookahead != Some(attachment.initial_lookahead)
    {
        return Err(Error::Unexpected(
            "The imported wallet's daemon settings do not match this Cube".into(),
        ));
    }
    Ok(())
}

pub async fn start_bitcoind_and_daemon(
    coincube_datadir_path: CoincubeDirectory,
    start_internal_bitcoind: bool,
    chain: crate::chain::ChainId,
    settings: WalletSettings,
) -> StartedResult {
    start_bitcoind_and_daemon_with_import_auth(
        coincube_datadir_path,
        start_internal_bitcoind,
        chain,
        settings,
        None,
    )
    .await
}

async fn start_bitcoind_and_daemon_with_import_auth(
    coincube_datadir_path: CoincubeDirectory,
    start_internal_bitcoind: bool,
    chain: crate::chain::ChainId,
    settings: WalletSettings,
    connect_client: Option<crate::services::coincube::CoincubeClient>,
) -> StartedResult {
    // The last gate before anything runs: no `daemon.toml` is read or
    // migrated, no node is provisioned and no daemon is started for a chain
    // this build cannot run. Keyed on the chain's *identity* so the wallet
    // directory below is the chain's own, never its encoding twin's.
    if let crate::chain::RuntimeSupport::Dormant { reason } = chain.runtime_support() {
        return Err(Error::ChainUnavailable(reason));
    }
    let mut config_path = coincube_datadir_path
        .network_directory(chain)
        .coincubed_data_directory(&settings.wallet_id())
        .path()
        .to_path_buf();
    config_path.push("daemon.toml");
    // The `daemon.toml` in this Cube's directory must be for this Cube's chain — checked
    // on the file as found, before the migration below gets to rewrite it, so a mismatched
    // file is refused byte-for-byte untouched. The daemon checks an *existing* database
    // against its config; this is the only check for a Cube whose database does not exist
    // yet, and it runs before any node or daemon is started.
    let found = Config::from_file(Some(config_path.clone())).map_err(Error::Config)?;
    if found.bitcoin_config.chain != chain {
        return Err(Error::ChainMismatch {
            cube: chain,
            config: found.bitcoin_config.chain,
        });
    }
    // Reopening a Cube right after locking it: let the previous daemon finish
    // stopping before anything here touches its directory or node.
    if let Some(dir) = found.data_directory() {
        if !crate::daemon::stopping::wait_for_async(dir.path().to_path_buf()).await {
            return Err(Error::Daemon(DaemonError::Unexpected(
                crate::daemon::stopping::STILL_STOPPING.to_string(),
            )));
        }
    }
    // Promote pre-fallback vaults: rewrite `daemon.toml` so the
    // Connect URL moves to `fallback_addr` and `mempool.space`
    // becomes the primary `addr`. Idempotent (skips when
    // `fallback_addr` is already present or `addr` isn't the
    // Connect URL), so subsequent starts no-op. An I/O failure
    // here is surfaced as a startup error rather than swallowed
    // — if we can't migrate the file we shouldn't pretend we
    // did.
    let mut config = if found.main_descriptor.as_imported().is_some() {
        // The imported wallet's provider order is an explicit user choice.
        // The Vault-only legacy migration must never rewrite it.
        found
    } else {
        match crate::installer::migration::migrate_esplora_config(&config_path) {
            // Rewritten: read back what is now on disk.
            Ok(true) => Config::from_file(Some(config_path)).map_err(Error::Config)?,
            Ok(false) => found,
            Err(e) => {
                tracing::warn!(
                "esplora-config migration failed for {}: {} — continuing with the existing daemon.toml",
                config_path.display(),
                e,
            );
                found
            }
        }
    };
    if config.main_descriptor.as_imported().is_some() {
        authenticate_imported_connect_provider(
            &mut config,
            chain,
            connect_client.as_ref().and_then(|client| client.token()),
        )?;
    }
    let bitcoind = match (start_internal_bitcoind, &config.bitcoin_backend) {
        (true, Some(BitcoinBackend::Bitcoind(bitcoind_config))) => {
            Some(start_active_bitcoind(&coincube_datadir_path, chain, bitcoind_config).await?)
        }
        _ => None,
    };

    // If the active backend is not internal bitcoind but there is a pending
    // internal node, start it so it keeps syncing in the background.
    let bitcoind = if bitcoind.is_none() {
        start_pending_bitcoind(
            &coincube_datadir_path,
            chain,
            config.pending_bitcoind.as_ref(),
        )?
    } else {
        bitcoind
    };

    debug!("starting coincube daemon");

    let daemon = EmbeddedDaemon::start(config)?;
    let info = daemon.get_info().await?;

    Ok((
        Arc::new(daemon),
        PendingForkNode {
            node: bitcoind,
            cleanup: false,
        },
        info,
    ))
}

async fn start_active_bitcoind(
    coincube_datadir_path: &CoincubeDirectory,
    chain: crate::chain::ChainId,
    bitcoind_config: &BitcoindConfig,
) -> Result<Bitcoind, Error> {
    // The provider the ledger names must serve this chain *before* Tor
    // is provisioned, the managed conf rewritten for inbound, or a
    // running node stopped below — `maybe_start` refuses again at its
    // own boundary, but by then those would have run.
    Bitcoind::preflight_for_chain(chain, coincube_datadir_path).map_err(Error::Bitcoind)?;
    // A default-ON node self-provisions the Tor binary on first launch
    // (best-effort; failure just means inbound is unavailable this run).
    // The managed Tor bundle/registry belongs to Bitcoin today. Never
    // provision or stop it for a BTCB2 start; the chain-aware preparation
    // below refuses that unsupported lifecycle before touching it.
    if !chain.is_blake2b() {
        crate::node::tor::ensure_tor_installed_if_wanted(coincube_datadir_path).await;
    }
    // Bring up inbound-over-Tor (if the user enabled it) and reconcile
    // bitcoin.conf *before* starting bitcoind, so bitcoind reads the
    // fresh onion/proxy config. Fail-safe on any Tor issue (the conf is
    // left outbound-only and the node starts exactly as it does today)
    // but not on a conf issue: if the file could not be read, locked or
    // replaced it may still name a Tor that is not running, so the
    // start is refused — retryably — rather than made from stale
    // privacy configuration.
    let lifecycle = crate::node::bitcoind::ManagedNodeLifecycle::acquire(
        coincube_datadir_path,
        crate::node::bitcoind::NodeChainFamily::from_chain(chain),
    )
    .map_err(Error::Bitcoind)?;
    let inbound_up = crate::node::tor::prepare_inbound_tor_for_chain_locked(
        coincube_datadir_path,
        chain,
        &lifecycle,
    )
    .map_err(|e| {
        Error::Bitcoind(
            crate::node::bitcoind::StartInternalBitcoindError::ConfigUnavailable(e.to_string()),
        )
    })?;
    // A fresh tor gets fresh control/SOCKS ports each run, which a
    // *reused* bitcoind (one that survived a previous session) wouldn't
    // pick up — `maybe_start` would just reattach to it. When inbound is
    // being applied, stop any running node first so `maybe_start` starts
    // it clean against the new onion/proxy config. No-op (and no restart)
    // when nothing is running, i.e. the normal clean-quit case. Safe
    // here: the embedded daemon isn't up yet, so there's no live poller
    // to disturb.
    if inbound_up {
        crate::node::bitcoind::stop_and_wait_managed_bitcoind(bitcoind_config);
    }
    Bitcoind::maybe_start_for_chain_locked(
        chain,
        bitcoind_config.clone(),
        coincube_datadir_path,
        &lifecycle,
    )
    .map_err(Error::Bitcoind)
}

fn start_pending_bitcoind(
    coincube_datadir_path: &CoincubeDirectory,
    chain: crate::chain::ChainId,
    pending: Option<&BitcoindConfig>,
) -> Result<Option<Bitcoind>, Error> {
    let bitcoind = if let Some(pending_cfg) = pending {
        let internal_datadir = managed_node_datadir(coincube_datadir_path, chain);
        let is_internal = match &pending_cfg.rpc_auth {
            BitcoindRpcAuth::CookieFile(path) => path.starts_with(&internal_datadir),
            _ => false,
        };
        if is_internal {
            Some(
                Bitcoind::maybe_start_for_chain(chain, pending_cfg.clone(), coincube_datadir_path)
                    .map_err(Error::Bitcoind)?,
            )
        } else {
            None
        }
    } else {
        None
    };
    Ok(bitcoind)
}

pub(crate) fn authenticate_imported_connect_provider(
    config: &mut Config,
    chain: crate::chain::ChainId,
    token: Option<&str>,
) -> Result<(), Error> {
    let Some(BitcoinBackend::Esplora(esplora)) = config.bitcoin_backend.as_mut() else {
        return Ok(());
    };
    let connect_url = crate::installer::connect_url(chain);
    let uses_connect = esplora.addr == connect_url
        || esplora.fallback_addr.as_deref() == Some(connect_url.as_str())
        || esplora.secondary_fallback_addr.as_deref() == Some(connect_url.as_str());
    if !uses_connect {
        return Ok(());
    }
    let token = token.ok_or_else(|| {
        Error::Unexpected(
            "Sign in to Connect from Home to open this imported Bitcoin wallet".into(),
        )
    })?;
    if esplora.addr == connect_url {
        esplora.token = Some(token.to_owned());
    }
    if esplora.fallback_addr.as_deref() == Some(connect_url.as_str()) {
        esplora.fallback_token = Some(token.to_owned());
    }
    if esplora.secondary_fallback_addr.as_deref() == Some(connect_url.as_str()) {
        esplora.secondary_fallback_token = Some(token.to_owned());
    }
    Ok(())
}

async fn sync(
    daemon: Arc<dyn Daemon + Sync + Send>,
    sleep: bool,
) -> Result<GetInfoResult, DaemonError> {
    if sleep {
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    daemon.get_info().await
}

#[derive(Debug)]
pub enum Error {
    Wallet(WalletError),
    Config(ConfigError),
    Daemon(DaemonError),
    Bitcoind(StartInternalBitcoindError),
    BitcoindLogs(std::io::Error),
    RestoreBackup(RestoreBackupError),
    /// The Cube's chain is one this build only knows the identity of
    /// ([`crate::chain::RuntimeSupport::Dormant`]). Raised before any daemon
    /// socket is dialled, any `daemon.toml` is read or migrated, and any node
    /// is started. The `gui::tab` open gate normally stops such a Cube earlier;
    /// this is the loader's own copy of the refusal for any path that reaches
    /// it directly.
    ChainUnavailable(&'static str),
    /// The Cube's `daemon.toml` names another chain than the Cube record it sits under
    /// (a file copied between chain directories, say). Raised before any node or daemon
    /// is started, whether or not a database exists yet.
    ChainMismatch {
        cube: crate::chain::ChainId,
        config: crate::chain::ChainId,
    },
    /// A Bitcoin Blake2b resync could not finish; it is offered again.
    ForkResyncFailed(String),
    Unexpected(String),
}

impl Error {
    /// The Bitcoin Blake2b node cannot revalidate its chain because the blocks
    /// it needs are pruned.
    pub(crate) fn is_pruned_revalidation(&self) -> bool {
        matches!(
            self,
            Error::Bitcoind(StartInternalBitcoindError::PrunedRevalidationRequired(_))
        )
    }

    /// The errors a resync recovers: the pruned revalidation itself, and a
    /// resync that did not finish.
    pub(crate) fn offers_fork_resync(&self) -> bool {
        self.is_pruned_revalidation() || matches!(self, Error::ForkResyncFailed(_))
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Config(e) => write!(f, "Config error: {}", e),
            Self::Wallet(e) => write!(f, "Wallet error: {}", e),
            Self::Daemon(e) => write!(f, "COINCUBE Desktop daemon error: {}", e),
            Self::Bitcoind(e) => write!(f, "Bitcoind error: {}", e),
            Self::BitcoindLogs(e) => write!(f, "Bitcoind logs error: {}", e),
            Self::RestoreBackup(e) => write!(f, "Restore backup: {e}"),
            Self::ChainUnavailable(reason) => f.write_str(reason),
            Self::ChainMismatch { cube, config } => write!(
                f,
                "This Cube is on {} but its daemon configuration is for {}. Nothing was started \
                 or modified; the configuration file needs to be fixed or restored.",
                cube.label(),
                config.label()
            ),
            Self::ForkResyncFailed(e) => write!(
                f,
                "The Bitcoin Blake2b node could not be resynced: {}. Its chain data still has to \
                 be deleted before the node can start.",
                e
            ),
            Self::Unexpected(e) => write!(f, "Unexpected error: {}", e),
        }
    }
}

impl From<WalletError> for Error {
    fn from(error: WalletError) -> Self {
        Error::Wallet(error)
    }
}

impl From<ConfigError> for Error {
    fn from(error: ConfigError) -> Self {
        Error::Config(error)
    }
}

impl From<DaemonError> for Error {
    fn from(error: DaemonError) -> Self {
        Error::Daemon(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

    // A mainnet descriptor (so `Config::check()`'s xpub-network validation
    // passes against `network = "bitcoin"`), borrowed from coincubed's own
    // config tests.
    pub(super) const MAINNET_DESC: &str = "tr([abcdef01]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*,and_v(v:pk([abcdef01]xpub688Hn4wScQAAiYJLPg9yH27hUpfZAUnmJejRQBCiwfP5PEDzjWMNW1wChcninxr5gyavFqbbDjdV1aK5USJz8NDVjUy7FRQaaqqXHh5SbXe/<0;1>/*),older(52560)))#0mt7e93c";

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// Minimal RAII temp directory — avoids pulling in `tempfile` just for this
    /// test. Removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "coincube-loader-test-{}-{}",
                std::process::id(),
                n
            ));
            fs::create_dir_all(&path).unwrap();
            TempDir(path)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn imported_loader_preflights_exact_cube_wallet_and_config() {
        use coincube_core::miniscript::bitcoin::{
            bip32::{DerivationPath, Xpriv, Xpub},
            secp256k1::Secp256k1,
        };
        use coincube_core::wallet_descriptor::{ImportedWalletDescriptor, WalletDescriptor};
        use std::str::FromStr;

        let tmp = TempDir::new();
        let root = CoincubeDirectory::new(tmp.path().to_path_buf());
        let chain = crate::chain::ChainId::Bitcoin;
        let secp = Secp256k1::new();
        let master = Xpriv::new_master(bitcoin::Network::Bitcoin, &[41; 32]).unwrap();
        let account = master
            .derive_priv(&secp, &DerivationPath::from_str("m/84h/0h/0h").unwrap())
            .unwrap();
        let key = format!(
            "[{}/84h/0h/0h]{}",
            master.fingerprint(&secp),
            Xpub::from_priv(&secp, &account)
        );
        let descriptor = ImportedWalletDescriptor::parse(
            &format!("wpkh({key}/0/*)"),
            Some(&format!("wpkh({key}/1/*)")),
            chain,
        )
        .unwrap();
        let attachment =
            app::settings::ImportedWalletAttachment::new(descriptor.clone(), chain, 0, 20).unwrap();
        let mut cube = CubeSettings::new("Imported".into(), chain);
        cube.bitcoin_import = Some(Box::new(app::settings::BitcoinImport {
            operation_id: uuid::Uuid::new_v4(),
            stage: app::settings::BitcoinImportStage::Syncing {
                wallet: attachment.clone(),
            },
            last_error: None,
        }));
        let wallet = WalletSettings {
            name: "Imported".into(),
            alias: None,
            descriptor_checksum: attachment.wallet_id.descriptor_checksum.clone(),
            pinned_at: None,
            keys: Vec::new(),
            hardware_wallets: Vec::new(),
            remote_backend_auth: None,
            start_internal_bitcoind: Some(false),
            pending_rescan: None,
            keychain_keys_recorded: false,
        };
        let directory = root
            .network_directory(chain)
            .coincubed_data_directory(&attachment.wallet_id);
        fs::create_dir_all(directory.path()).unwrap();
        let mut config = Config::new(
            coincubed::config::BitcoinConfig::new(chain, Duration::from_secs(30)),
            Some(BitcoinBackend::Esplora(coincubed::config::EsploraConfig {
                addr: "https://example.com/api".into(),
                token: None,
                fallback_addr: None,
                fallback_token: None,
                secondary_fallback_addr: None,
                secondary_fallback_token: None,
            })),
            log::LevelFilter::Info,
            WalletDescriptor::Imported(descriptor),
            coincubed::datadir::DataDirectory::new(directory.path().to_path_buf()),
        );
        config.import_birthday = Some(0);
        config.import_initial_lookahead = Some(20);
        let config_path = directory.path().join("daemon.toml");
        fs::write(&config_path, toml::to_string_pretty(&config).unwrap()).unwrap();

        validate_imported_open(&root, &cube, Some(&wallet)).unwrap();
        config.import_birthday = Some(1);
        fs::write(&config_path, toml::to_string_pretty(&config).unwrap()).unwrap();
        assert!(validate_imported_open(&root, &cube, Some(&wallet)).is_err());
        config.import_birthday = Some(0);
        fs::write(&config_path, toml::to_string_pretty(&config).unwrap()).unwrap();
        let mut wrong_wallet = wallet.clone();
        wrong_wallet.descriptor_checksum = "different".into();
        assert!(validate_imported_open(&root, &cube, Some(&wrong_wallet)).is_err());
        assert!(validate_imported_open(&root, &cube, None).is_err());
    }

    #[test]
    fn imported_connect_provider_keeps_configured_priority_and_requires_session() {
        use coincube_core::miniscript::bitcoin::{
            bip32::{DerivationPath, Xpriv, Xpub},
            secp256k1::Secp256k1,
        };
        use coincube_core::wallet_descriptor::{ImportedWalletDescriptor, WalletDescriptor};
        use std::str::FromStr;

        let chain = crate::chain::ChainId::Bitcoin;
        let secp = Secp256k1::new();
        let master = Xpriv::new_master(bitcoin::Network::Bitcoin, &[43; 32]).unwrap();
        let account = master
            .derive_priv(&secp, &DerivationPath::from_str("m/84h/0h/0h").unwrap())
            .unwrap();
        let key = format!(
            "[{}/84h/0h/0h]{}",
            master.fingerprint(&secp),
            Xpub::from_priv(&secp, &account)
        );
        let imported = ImportedWalletDescriptor::parse(
            &format!("wpkh({key}/0/*)"),
            Some(&format!("wpkh({key}/1/*)")),
            chain,
        )
        .unwrap();
        let mut config = Config::new(
            coincubed::config::BitcoinConfig::new(chain, Duration::from_secs(30)),
            Some(BitcoinBackend::Esplora(coincubed::config::EsploraConfig {
                addr: "https://example.com/api".into(),
                token: None,
                fallback_addr: None,
                fallback_token: None,
                secondary_fallback_addr: None,
                secondary_fallback_token: None,
            })),
            log::LevelFilter::Info,
            WalletDescriptor::Imported(imported),
            coincubed::datadir::DataDirectory::new(std::env::temp_dir().join("import-auth-test")),
        );
        let url = crate::installer::connect_url(chain);
        if let Some(BitcoinBackend::Esplora(esplora)) = config.bitcoin_backend.as_mut() {
            esplora.addr = "https://example.com/api".into();
            esplora.fallback_addr = Some(url.clone());
            esplora.secondary_fallback_addr = Some(url.clone());
        } else {
            panic!("expected Esplora config");
        }
        let original = config.clone();
        let error = authenticate_imported_connect_provider(&mut config, chain, None).unwrap_err();
        assert!(error.to_string().contains("Sign in to Connect"));
        assert_eq!(format!("{config:?}"), format!("{original:?}"));

        let mut client = crate::services::coincube::CoincubeClient::for_test("http://127.0.0.1:1");
        client.set_token("session-token");
        authenticate_imported_connect_provider(&mut config, chain, client.token()).unwrap();
        let Some(BitcoinBackend::Esplora(esplora)) = config.bitcoin_backend.as_ref() else {
            panic!("expected Esplora config");
        };
        assert_eq!(esplora.addr, "https://example.com/api");
        assert_eq!(esplora.fallback_addr.as_deref(), Some(url.as_str()));
        assert_eq!(esplora.fallback_token.as_deref(), Some("session-token"));
        assert_eq!(
            esplora.secondary_fallback_addr.as_deref(),
            Some(url.as_str())
        );
        assert_eq!(
            esplora.secondary_fallback_token.as_deref(),
            Some("session-token")
        );
    }

    /// Write a `daemon.toml` whose backend section is `backend`, returning its
    /// path. `backend` is a full TOML table, e.g. `"[bitcoind_config]\n..."`.
    fn write_daemon_toml(dir: &Path, backend: &str) -> PathBuf {
        let path = dir.join("daemon.toml");
        fs::write(
            &path,
            format!(
                "main_descriptor = \"{MAINNET_DESC}\"\n\n\
                 [bitcoin_config]\nnetwork = \"bitcoin\"\n\n\
                 {backend}\n"
            ),
        )
        .unwrap();
        path
    }

    #[test]
    fn managed_paths_follow_chain_family_and_keep_bitcoin_layout() {
        use crate::chain::ChainId;
        let root = CoincubeDirectory::new(PathBuf::from("synthetic-root"));
        for chain in ChainId::ALL {
            let path = managed_node_datadir(&root, chain);
            let log = managed_node_log_path(&root, chain);
            if chain.is_blake2b() {
                assert!(path.ends_with("bitcoind-blake2b/datadir"));
                assert!(!path.starts_with(crate::node::bitcoind::internal_bitcoind_datadir(&root)));
            } else {
                assert_eq!(
                    path,
                    crate::node::bitcoind::internal_bitcoind_datadir(&root)
                );
                assert_eq!(
                    log,
                    crate::node::bitcoind::internal_bitcoind_debug_log_path(
                        &root,
                        chain.bitcoin_network()
                    )
                );
            }
            assert!(log.starts_with(path));
            assert!(log.ends_with("debug.log"));
        }
        assert_ne!(
            managed_node_log_path(&root, ChainId::BitcoinBlake2b),
            managed_node_log_path(&root, ChainId::BitcoinBlake2bTestnet4)
        );
    }

    #[test]
    fn cookie_inside_managed_datadir_is_internal() {
        let tmp = TempDir::new();
        let internal_datadir = tmp.path().join("bitcoind/datadir");
        let cookie = internal_datadir.join(".cookie");
        // Embed the path with a TOML *literal* string (single quotes): on
        // Windows `cookie.display()` yields backslashes, which a basic
        // (double-quoted) string treats as escape sequences (e.g. `\U`),
        // making the config unparseable. Literal strings pass backslashes
        // through verbatim — the same round-trip `toml::to_string_pretty`
        // gives real configs.
        let backend = format!(
            "[bitcoind_config]\ncookie_path = '{}'\naddr = \"127.0.0.1:8332\"",
            cookie.display()
        );
        let config_path = write_daemon_toml(tmp.path(), &backend);
        assert!(backend_is_internal_bitcoind(
            &config_path,
            &internal_datadir
        ));
    }

    #[test]
    fn cookie_outside_managed_datadir_is_external() {
        let tmp = TempDir::new();
        let internal_datadir = tmp.path().join("bitcoind/datadir");
        // A user-run node whose cookie lives elsewhere on disk: the flag, not
        // this check, should govern whether we manage it.
        let backend =
            "[bitcoind_config]\ncookie_path = \"/home/user/.bitcoin/.cookie\"\naddr = \"127.0.0.1:8332\"";
        let config_path = write_daemon_toml(tmp.path(), backend);
        assert!(!backend_is_internal_bitcoind(
            &config_path,
            &internal_datadir
        ));
    }

    #[test]
    fn userpass_auth_is_external() {
        let tmp = TempDir::new();
        let internal_datadir = tmp.path().join("bitcoind/datadir");
        let backend = "[bitcoind_config]\nauth = \"user:password\"\naddr = \"127.0.0.1:8332\"";
        let config_path = write_daemon_toml(tmp.path(), backend);
        assert!(!backend_is_internal_bitcoind(
            &config_path,
            &internal_datadir
        ));
    }

    #[test]
    fn esplora_backend_is_not_internal_bitcoind() {
        let tmp = TempDir::new();
        let internal_datadir = tmp.path().join("bitcoind/datadir");
        let backend = "[esplora_config]\naddr = \"https://mempool.space/api\"";
        let config_path = write_daemon_toml(tmp.path(), backend);
        assert!(!backend_is_internal_bitcoind(
            &config_path,
            &internal_datadir
        ));
    }

    #[test]
    fn missing_or_unparseable_config_is_not_internal() {
        let tmp = TempDir::new();
        let internal_datadir = tmp.path().join("bitcoind/datadir");
        // Missing file.
        let missing = tmp.path().join("nope/daemon.toml");
        assert!(!backend_is_internal_bitcoind(&missing, &internal_datadir));
        // Present but not valid TOML.
        let garbage = tmp.path().join("daemon.toml");
        fs::write(&garbage, "not valid toml : : :").unwrap();
        assert!(!backend_is_internal_bitcoind(&garbage, &internal_datadir));
    }

    fn cube_settings() -> CubeSettings {
        CubeSettings::new_with_raw_id(
            "cube-uuid".to_string(),
            "Family Vault".to_string(),
            bitcoin::Network::Bitcoin,
        )
    }

    fn loader_without_wallet(start_internal_bitcoind: bool) -> Loader {
        Loader::new(
            CoincubeDirectory::new(PathBuf::new()),
            GUIConfig::new(start_internal_bitcoind),
            bitcoin::Network::Bitcoin,
            None,
            None,
            None,
            cube_settings(),
            None,
            None,
        )
        .0
    }

    #[test]
    fn fork_retry_is_user_triggered_and_rate_bounded() {
        let mut loader = loader_without_wallet(false);
        loader.cube_settings.network = crate::chain::ChainId::BitcoinBlake2b;
        loader.fork_retry_not_before = Some(std::time::Instant::now() + Duration::from_secs(30));
        let _ = loader.update(Message::View(ViewMessage::Retry));
        assert!(
            matches!(&loader.step, Step::Error(error) if error.to_string().contains("retried in"))
        );
        assert!(loader.fork_task.is_none());
        assert!(!loader.daemon_started);
    }

    fn pruned_revalidation_error() -> Error {
        Error::Bitcoind(StartInternalBitcoindError::PrunedRevalidationRequired(
            crate::chain::ChainId::BitcoinBlake2bTestnet4,
        ))
    }

    // The resync is offered for, and acts on, the pruned-revalidation error
    // of a Bitcoin Blake2b Cube only.
    #[test]
    fn resync_acts_only_on_a_pruned_revalidation_of_a_fork_cube() {
        // Any other error: the message is ignored.
        let mut loader = loader_without_wallet(false);
        loader.cube_settings.network = crate::chain::ChainId::BitcoinBlake2bTestnet4;
        loader.step = Step::Error(Box::new(Error::Unexpected("other".into())));
        let _ = loader.update(Message::View(ViewMessage::ResyncForkNode));
        assert!(matches!(&loader.step, Step::Error(e) if !e.is_pruned_revalidation()));

        // A Bitcoin Cube never resyncs through this path.
        let mut loader = loader_without_wallet(false);
        loader.step = Step::Error(Box::new(pruned_revalidation_error()));
        let _ = loader.update(Message::View(ViewMessage::ResyncForkNode));
        assert!(matches!(&loader.step, Step::Error(e) if e.is_pruned_revalidation()));

        // A revoked session keeps its refusal, as Retry does.
        let mut loader = loader_without_wallet(false);
        loader.cube_settings.network = crate::chain::ChainId::BitcoinBlake2bTestnet4;
        loader.fork_session_revoked = true;
        loader.step = Step::Error(Box::new(pruned_revalidation_error()));
        let _ = loader.update(Message::View(ViewMessage::ResyncForkNode));
        assert!(matches!(&loader.step, Step::Error(e) if e.is_pruned_revalidation()));

        // The fork Cube's pruned error starts the resync.
        let mut loader = loader_without_wallet(false);
        loader.cube_settings.network = crate::chain::ChainId::BitcoinBlake2bTestnet4;
        loader.step = Step::Error(Box::new(pruned_revalidation_error()));
        let _ = loader.update(Message::View(ViewMessage::ResyncForkNode));
        assert!(matches!(loader.step, Step::Connecting));
        let token = loader.fork_resync.expect("resync in flight");

        // Another resync's result (or one meant for another loader) is dropped.
        let _ = loader.update(Message::ForkNodeResynced {
            token: token + 1000,
            result: Err(ForkResyncError::Failed("foreign".into())),
        });
        assert!(matches!(loader.step, Step::Connecting));

        // A refused resync is shown, with the reason, and stays offered.
        let _ = loader.update(Message::ForkNodeResynced {
            token,
            result: Err(ForkResyncError::Failed("node still running".into())),
        });
        assert!(matches!(&loader.step, Step::Error(e)
            if e.to_string().contains("node still running") && e.offers_fork_resync()));
        assert!(loader.fork_resync.is_none());
        // A late duplicate of that result is ignored.
        let _ = loader.update(Message::ForkNodeResynced {
            token,
            result: Err(ForkResyncError::Failed("late".into())),
        });
        assert!(matches!(&loader.step, Step::Error(e) if !e.to_string().contains("late")));
        // ... and the resync can be started again from the failure.
        let _ = loader.update(Message::View(ViewMessage::ResyncForkNode));
        assert!(matches!(loader.step, Step::Connecting));
        let token = loader.fork_resync.expect("resync in flight");

        // A refusal (nothing deleted, would be refused again) is shown with
        // its reason and no longer offers the resync.
        let _ = loader.update(Message::ForkNodeResynced {
            token,
            result: Err(ForkResyncError::Refused("runs on its local node".into())),
        });
        assert!(matches!(&loader.step, Step::Error(e)
            if e.to_string().contains("runs on its local node") && !e.offers_fork_resync()));
        let _ = loader.update(Message::View(ViewMessage::ResyncForkNode));
        assert!(matches!(&loader.step, Step::Error(_)));
    }

    // A result reaching a loader that never started a resync (the tab now
    // shows another Cube) is dropped.
    #[test]
    fn a_resync_result_never_reaches_a_loader_that_did_not_start_it() {
        let mut loader = loader_without_wallet(false);
        loader.cube_settings.network = crate::chain::ChainId::BitcoinBlake2bTestnet4;
        for result in [
            Ok(()),
            Err(ForkResyncError::Failed("other Cube's failure".to_string())),
        ] {
            let _ = loader.update(Message::ForkNodeResynced { token: 1, result });
            assert!(matches!(loader.step, Step::Connecting));
            assert!(loader.fork_resync.is_none());
        }
    }

    // The Retry cooldown does not hide the resync page: the resync is not
    // subject to it.
    #[test]
    fn the_retry_cooldown_keeps_the_resync_page() {
        let mut loader = loader_without_wallet(false);
        loader.cube_settings.network = crate::chain::ChainId::BitcoinBlake2bTestnet4;
        loader.fork_retry_not_before = Some(std::time::Instant::now() + Duration::from_secs(30));
        for error in [
            pruned_revalidation_error(),
            Error::ForkResyncFailed("busy".into()),
        ] {
            loader.step = Step::Error(Box::new(error));
            let _ = loader.update(Message::View(ViewMessage::Retry));
            assert!(matches!(&loader.step, Step::Error(e) if e.offers_fork_resync()));
        }
        // Once the cooldown has passed, Retry still never starts the node on
        // partly deleted data after a resync that did not finish.
        loader.fork_retry_not_before = None;
        loader.step = Step::Error(Box::new(Error::ForkResyncFailed("busy".into())));
        let task = loader.update(Message::View(ViewMessage::Retry));
        assert_eq!(task.units(), 0);
        assert!(matches!(&loader.step, Step::Error(e)
            if matches!(**e, Error::ForkResyncFailed(_))));
    }

    // A session revoked while the chain data was being deleted stays latched:
    // the resync's result neither rebuilds the open without auth nor replaces
    // the reopen guidance.
    #[test]
    fn a_resync_finishing_after_revocation_keeps_the_refusal() {
        for result in [
            Ok(()),
            Err(ForkResyncError::Failed("deleted nothing".to_string())),
        ] {
            let mut loader = loader_without_wallet(false);
            loader.cube_settings.network = crate::chain::ChainId::BitcoinBlake2bTestnet4;
            loader.step = Step::Error(Box::new(pruned_revalidation_error()));
            let _ = loader.update(Message::View(ViewMessage::ResyncForkNode));
            assert!(matches!(loader.step, Step::Connecting));
            let token = loader.fork_resync.expect("resync in flight");
            loader.invalidate_fork_session();
            let shown = match &loader.step {
                Step::Error(e) => e.to_string(),
                _ => "not an error".to_string(),
            };
            let _ = loader.update(Message::ForkNodeResynced {
                token,
                result: result.clone(),
            });
            assert!(loader.fork_session_revoked, "{:?}", result);
            assert!(loader.fork_task.is_none(), "{:?}", result);
            let after = match &loader.step {
                Step::Error(e) => e.to_string(),
                _ => "not an error".to_string(),
            };
            assert_eq!(after, shown, "{:?}", result);
        }
    }

    #[test]
    fn the_pruned_revalidation_page_offers_the_resync() {
        use crate::app::view::shared::landing::capture;
        let mut quote_provider = QuoteProvider::new();
        let quote = quote_provider.select("error");
        let image = quote_display::image_handle_for_context("error");
        let label = CopyText::ResyncForkNodeChainData.get();

        let step = Step::Error(Box::new(pruned_revalidation_error()));
        let shown = capture::inspect(
            "loader-pruned-revalidation",
            view(&step, &quote, &image),
            (1024, 768),
            "dark",
            theme::Theme::dark(),
        );
        assert!(shown.contains(label));
        assert!(shown.contains("block data it needs has been pruned"));

        let step = Step::Error(Box::new(Error::Unexpected("other".into())));
        let shown = capture::inspect(
            "loader-other-error",
            view(&step, &quote, &image),
            (1024, 768),
            "dark",
            theme::Theme::dark(),
        );
        assert!(!shown.contains(label));
    }

    #[test]
    fn fork_invalidation_clears_pending_auth_and_encryption_key() {
        let mut loader = loader_without_wallet(false);
        loader.cube_settings.network = crate::chain::ChainId::BitcoinBlake2b;
        let mut client = crate::services::coincube::CoincubeClient::for_test("http://127.0.0.1:1");
        client.set_token("synthetic-token");
        loader.connect_client = Some(client);
        let signer =
            coincube_core::signer::MasterSigner::generate(bitcoin::Network::Bitcoin).unwrap();
        loader.cube_encryption_key = Some(Arc::new(
            crate::services::connect::crypto::CubeEncryptionKey::derive(
                &signer,
                bitcoin::Network::Bitcoin,
            ),
        ));
        loader.invalidate_fork_session();
        assert!(loader.connect_client.is_none());
        assert!(loader.cube_encryption_key.is_none());
        let _ = loader.on_start(Err(Error::Unexpected("late startup".into())));
        assert!(
            matches!(&loader.step, Step::Error(error) if error.to_string().contains("session changed"))
        );
    }

    #[test]
    fn the_opening_cube_quotes_the_loader_stage_and_leaves_errors_to_its_page() {
        use crate::home::launcher_opening::Stage;
        let mut loader = loader_without_wallet(true);
        for (step, stage) in [
            (Step::Connecting, Some(Stage::Connecting)),
            (
                Step::StartingDaemon { progress: 0.4 },
                Some(Stage::StartingDaemon),
            ),
            (Step::FullScan { progress: 0.4 }, Some(Stage::Scanning)),
            (
                Step::Error(Box::new(Error::Unexpected("refused".into()))),
                None,
            ),
        ] {
            loader.step = step;
            assert_eq!(loader.opening_stage(), stage);
        }
        // The stage line reads exactly as the loader page's own status.
        assert_eq!(Stage::Connecting.text(), "Connecting to daemon...");
        assert_eq!(Stage::StartingDaemon.text(), "Starting daemon...");
        assert_eq!(Stage::Scanning.text(), "Scanning the blockchain...");
    }

    #[test]
    fn loader_without_wallet_uses_config_for_bitcoind_and_none_subscriptions() {
        let mut loader = loader_without_wallet(true);
        assert!(loader.start_bitcoind());
        assert!(!loader.vault_uses_internal_bitcoind());
        let _ = loader.subscription();
        let _ = loader.view();

        loader.step = Step::StartingDaemon { progress: 0.895 };
        let _ = loader.update(Message::LoadingTick);
        assert!(matches!(
            loader.step,
            Step::StartingDaemon { progress } if (progress - 0.9).abs() < f32::EPSILON
        ));
        let _ = loader.subscription();

        loader.step = Step::FullScan { progress: 0.899 };
        let _ = loader.update(Message::LoadingTick);
        assert!(matches!(
            loader.step,
            Step::FullScan { progress } if (progress - 0.9).abs() < f32::EPSILON
        ));

        let _ = loader.update(Message::Synced(Err(Error::Unexpected("boom".to_string()))));
        assert!(matches!(loader.step, Step::Error(_)));
        let _ = loader.update(Message::Failure(DaemonError::NoAnswer));
        assert!(!loader.daemon_started);
        let _ = loader.update(Message::None);
    }

    #[test]
    fn loader_standalone_views_cover_steps_and_error_copy() {
        let mut quote_provider = QuoteProvider::new();
        let quote = quote_provider.select("loading");
        let image_handle = quote_display::image_handle_for_context("loading");

        let _ = view(&Step::Connecting, &quote, &image_handle);
        let _ = view(
            &Step::StartingDaemon { progress: 0.5 },
            &quote,
            &image_handle,
        );
        let _ = view(&Step::FullScan { progress: 0.5 }, &quote, &image_handle);
        let _ = view(
            &Step::Error(Box::new(Error::Config(ConfigError::FileNotFound))),
            &quote,
            &image_handle,
        );
        let _ = view(
            &Step::Error(Box::new(Error::Daemon(DaemonError::NoAnswer))),
            &quote,
            &image_handle,
        );
        let _ = cover::<ViewMessage, _>(
            Some(("warn", &Error::Unexpected("details".to_string()))),
            text("body"),
        );
        let _ = cover::<ViewMessage, _>(None, text("body"));
    }

    #[test]
    fn worldview_voice_sits_alongside_fallback_scan_guidance() {
        use crate::app::view::shared::landing::capture;
        use coincube_ui::{theme::WorldviewMode, worldview_copy};

        let mut quote_provider = QuoteProvider::new();
        let quote = quote_provider.select("loading");
        let image = quote_display::image_handle_for_context("loading");
        let step = Step::FullScan { progress: 0.5 };
        let rendered =
            worldview_copy::render_in(WorldviewMode::Luke, || view(&step, &quote, &image));
        let shown = capture::inspect(
            "worldview-voice-fallback-scan",
            rendered,
            (1024, 768),
            "dark",
            theme::Theme::dark().with_worldview(WorldviewMode::Luke),
        );
        assert!(shown.contains(CopyText::ScanningTheBlockchain.get()));
        assert!(shown.contains("Consulting the ancient rules…"));
        assert!(shown.contains(CopyText::InitialEsploraScan.get()));
    }

    #[test]
    fn loader_error_display_and_from_impls_are_stable() {
        assert_eq!(
            Error::Config(ConfigError::FileNotFound).to_string(),
            "Config error: Could not locate the configuration file."
        );
        assert!(Error::Daemon(DaemonError::NoAnswer)
            .to_string()
            .contains("COINCUBE Desktop daemon error"));
        assert!(Error::BitcoindLogs(std::io::Error::other("log"))
            .to_string()
            .contains("Bitcoind logs error"));
        assert_eq!(
            Error::Unexpected("odd".to_string()).to_string(),
            "Unexpected error: odd"
        );

        assert!(matches!(
            Error::from(ConfigError::FileNotFound),
            Error::Config(ConfigError::FileNotFound)
        ));
        assert!(matches!(
            Error::from(DaemonError::NoAnswer),
            Error::Daemon(DaemonError::NoAnswer)
        ));
    }
}

#[cfg(test)]
mod chain_identity_tests {
    //! The loader's own refusal of a chain this build cannot run (BTCB2 plan
    //! PR 2, dormant slice). The `gui::tab` open gate normally stops such a
    //! Cube first; these pin that the loader and the daemon-start path refuse
    //! on their own, before a socket is dialled or a `daemon.toml` is read.

    use super::tests::MAINNET_DESC;
    use super::*;
    use crate::chain::{ChainId, BTCB2_DORMANT_REASON};
    use std::path::PathBuf;

    fn cube(chain: ChainId) -> CubeSettings {
        CubeSettings::new_with_raw_id("cube-uuid".to_string(), "Fork".to_string(), chain)
    }

    fn wallet() -> WalletSettings {
        WalletSettings {
            name: "Coincube-kt6ht0kt".to_string(),
            alias: None,
            descriptor_checksum: "kt6ht0kt".to_string(),
            pinned_at: Some(1_720_000_000),
            keys: Vec::new(),
            hardware_wallets: Vec::new(),
            remote_backend_auth: None,
            start_internal_bitcoind: Some(true),
            pending_rescan: None,
            keychain_keys_recorded: false,
        }
    }

    fn temp_root(tag: &str) -> CoincubeDirectory {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "coincube-loader-chain-{}-{}-{}",
            tag,
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&path).unwrap();
        CoincubeDirectory::new(path)
    }

    fn tree(root: &std::path::Path) -> Vec<PathBuf> {
        fn walk(p: &std::path::Path, out: &mut Vec<PathBuf>) {
            if let Ok(rd) = std::fs::read_dir(p) {
                for e in rd.flatten() {
                    out.push(e.path());
                    walk(&e.path(), out);
                }
            }
        }
        let mut out = Vec::new();
        walk(root, &mut out);
        out.sort();
        out
    }

    #[test]
    fn a_blake2b_cube_lands_in_the_error_step_without_dialling_the_daemon() {
        for chain in [ChainId::BitcoinBlake2b, ChainId::BitcoinBlake2bTestnet4] {
            let (loader, _task) = Loader::new(
                CoincubeDirectory::new(PathBuf::from("/nonexistent")),
                GUIConfig::new(true),
                chain.bitcoin_network(),
                None,
                None,
                Some(wallet()),
                cube(chain),
                None,
                None,
            );
            match &loader.step {
                Step::Error(e) => assert_eq!(e.to_string(), BTCB2_DORMANT_REASON, "{:?}", chain),
                other => panic!(
                    "expected the error step for {:?}, got {:?}",
                    chain,
                    std::mem::discriminant(other)
                ),
            }
            assert!(!loader.daemon_started);
            // The error view renders the reason, not a daemon/config error.
            let _ = loader.view();
        }
    }

    #[test]
    fn a_cube_whose_identity_disagrees_with_the_requested_encoding_is_refused() {
        // A Signet Cube asked to load as mainnet: the two would then disagree
        // on every path below, so the loader stops.
        let (loader, _task) = Loader::new(
            CoincubeDirectory::new(PathBuf::from("/nonexistent")),
            GUIConfig::new(false),
            bitcoin::Network::Bitcoin,
            None,
            None,
            None,
            cube(ChainId::Signet),
            None,
            None,
        );
        assert!(matches!(&loader.step, Step::Error(e) if matches!(**e, Error::Unexpected(_))));
        // The agreeing case is untouched.
        let (loader, _task) = Loader::new(
            CoincubeDirectory::new(PathBuf::from("/nonexistent")),
            GUIConfig::new(false),
            bitcoin::Network::Signet,
            None,
            None,
            None,
            cube(ChainId::Signet),
            None,
            None,
        );
        assert!(matches!(&loader.step, Step::Connecting));
    }

    #[tokio::test]
    async fn the_daemon_start_path_refuses_a_blake2b_chain_before_touching_daemon_toml() {
        let root = temp_root("start");
        let before = tree(root.path());
        for chain in [ChainId::BitcoinBlake2b, ChainId::BitcoinBlake2bTestnet4] {
            let result = start_bitcoind_and_daemon(root.clone(), true, chain, wallet()).await;
            match result {
                Err(Error::ChainUnavailable(reason)) => assert_eq!(reason, BTCB2_DORMANT_REASON),
                Err(other) => panic!("wrong refusal for {:?}: {}", chain, other),
                Ok(_) => panic!("started a daemon for {:?}", chain),
            }
        }
        // No wallet directory, no daemon.toml, no migration side effect, no
        // node datadir — the tree is byte-for-byte what it was.
        assert_eq!(tree(root.path()), before);
        assert!(!root.path().join("bitcoin-blake2b").exists());
        assert!(!root.path().join("bitcoin").exists());

        // A supported chain with no daemon.toml fails *later*, on the config
        // read — proving the dormant refusal is a distinct, earlier gate.
        let result =
            start_bitcoind_and_daemon(root.clone(), false, ChainId::Signet, wallet()).await;
        assert!(
            matches!(result, Err(Error::Config(_))),
            "expected a config error"
        );
        let _ = std::fs::remove_dir_all(root.path());
    }

    // A Bitcoin-family Vault whose managed-node ledger names the Blake2b
    // provider: the dormant-chain gate passes (the chain is Testnet4), so the
    // refusal has to come from the provider check — and it has to come before
    // the loader provisions Tor, rewrites the managed conf for inbound, or
    // stops a running node, all of which sit ahead of `maybe_start`'s own
    // guard. No node runs here: the conf is seeded with the inbound/proxy keys
    // `prepare_inbound_tor` always strips on a non-mainnet chain, so
    // byte-identical files after the call are the evidence that nothing in
    // that sequence ran. The matched control below (a Knots ledger) shows the
    // same call *does* reach and rewrite the conf when the provider is right.
    #[tokio::test]
    async fn a_mismatched_persisted_provider_is_refused_before_tor_config_or_stop() {
        // The control run reaches `prepare_inbound_tor`, which touches the
        // process-global Tor registry.
        let _registry = crate::node::tor::registry_test_guard();
        use crate::node::bitcoind::{
            internal_bitcoind_config_path, internal_bitcoind_cookie_path,
            internal_bitcoind_datadir, InternalBitcoindConfig, InternalBitcoindNetworkConfig,
            NodeFlavor, ProviderChainMismatch, StartInternalBitcoindError, PRUNE_DEFAULT,
        };
        use crate::node::revalidate::ManagedNodeState;
        use crate::node::tor::InboundTorPreference;
        let root = temp_root("provider");
        let chain = ChainId::Testnet4;
        let wallet = wallet();

        // The managed Bitcoin node's files: a conf carrying inbound/proxy
        // state, a ledger naming the Blake2b provider, and Tor switched off.
        let bitcoind_datadir = internal_bitcoind_datadir(&root);
        std::fs::create_dir_all(&bitcoind_datadir).unwrap();
        let conf_path = internal_bitcoind_config_path(&bitcoind_datadir);
        let mut conf = InternalBitcoindConfig::for_flavor(NodeFlavor::Knots);
        conf.networks.insert(
            bitcoin::Network::Testnet4,
            InternalBitcoindNetworkConfig {
                rpc_port: 46001,
                p2p_port: 46002,
                prune: PRUNE_DEFAULT,
                rpc_auth: None,
            },
        );
        conf.inbound_tor = true;
        conf.outbound_via_tor = true;
        conf.tor_control_port = Some(9051);
        conf.tor_socks_port = Some(9050);
        conf.to_file(&conf_path).unwrap();
        let conf_bytes = std::fs::read(&conf_path).unwrap();
        assert!(String::from_utf8_lossy(&conf_bytes).contains("listenonion=1"));
        ManagedNodeState {
            configured_flavor: Some(NodeFlavor::KnotsBlake2b),
            ..Default::default()
        }
        .save(&root)
        .unwrap();
        let ledger_bytes = std::fs::read(ManagedNodeState::path(&root)).unwrap();
        InboundTorPreference {
            enabled: false,
            ..Default::default()
        }
        .save(&root)
        .unwrap();

        // The Vault's daemon.toml on that node.
        let cookie = internal_bitcoind_cookie_path(&bitcoind_datadir, &bitcoin::Network::Testnet4);
        let wallet_dir = root
            .network_directory(chain)
            .coincubed_data_directory(&wallet.wallet_id())
            .path()
            .to_path_buf();
        std::fs::create_dir_all(&wallet_dir).unwrap();
        let daemon_toml = wallet_dir.join("daemon.toml");
        std::fs::write(
            &daemon_toml,
            format!(
                "data_dir = {:?}\nlog_level = \"TRACE\"\nmain_descriptor = \"{}\"\n\n\
                 [bitcoin_config]\nnetwork = \"testnet4\"\npoll_interval_secs = 18\n\n\
                 [bitcoind_config]\ncookie_path = {:?}\naddr = \"127.0.0.1:1\"\n",
                root.path().display().to_string(),
                "wsh(andor(pk([aabbccdd]tpubDEN9WSToTyy9ZQfaYqSKfmVqmq1VVLNtYfj3Vkqh67et57eJ5sTKZQBkHqSwPUsoSskJeaYnPttHe2VrkCsKA27kUaN9SDc5zhqeLzKa1rr/<0;1>/*),older(10000),pk([aabbccdd]tpubD8LYfn6njiA2inCoxwM7EuN3cuLVcaHAwLYeups13dpevd3nHLRdK9NdQksWXrhLQVxcUZRpnp5CkJ1FhE61WRAsHxDNAkvGkoQkAeWDYjV/<0;1>/*)))#dw4ulnrs",
                cookie.display().to_string(),
            ),
        )
        .unwrap();
        let daemon_toml_bytes = std::fs::read(&daemon_toml).unwrap();
        let before = tree(root.path());

        let result = start_bitcoind_and_daemon(root.clone(), true, chain, wallet.clone()).await;
        match result {
            Err(Error::Bitcoind(StartInternalBitcoindError::ProviderChainMismatch(e))) => {
                assert_eq!(
                    e,
                    ProviderChainMismatch {
                        flavor: NodeFlavor::KnotsBlake2b,
                        chain,
                    }
                )
            }
            Err(other) => panic!("wrong refusal: {}", other),
            Ok(_) => panic!("started for a mismatched provider"),
        }
        // Byte-identical conf (still carrying the inbound/proxy keys), ledger
        // and daemon.toml; no Tor state, no identity marker, no cookie
        // directory, nothing at all added to the tree.
        assert_eq!(std::fs::read(&conf_path).unwrap(), conf_bytes);
        assert_eq!(
            std::fs::read(ManagedNodeState::path(&root)).unwrap(),
            ledger_bytes
        );
        assert_eq!(std::fs::read(&daemon_toml).unwrap(), daemon_toml_bytes);
        assert_eq!(tree(root.path()), before);

        // Matched control: with a provider of this chain's family the same
        // call gets past the guard, `prepare_inbound_tor` rewrites the conf
        // outbound-only (the keys above are gone), and the start then fails on
        // the absent binary — the ordinary Bitcoin path this build ships.
        ManagedNodeState {
            configured_flavor: Some(NodeFlavor::Knots),
            ..Default::default()
        }
        .save(&root)
        .unwrap();
        let result = start_bitcoind_and_daemon(root.clone(), true, chain, wallet).await;
        assert!(
            matches!(
                result,
                Err(Error::Bitcoind(
                    StartInternalBitcoindError::ExecutableNotFound
                ))
            ),
            "expected ExecutableNotFound"
        );
        let rewritten = std::fs::read(&conf_path).unwrap();
        assert_ne!(rewritten, conf_bytes);
        let rewritten = String::from_utf8_lossy(&rewritten);
        assert!(!rewritten.contains("listenonion=1"), "{}", rewritten);
        assert!(rewritten.contains("listenonion=0"), "{}", rewritten);
        let _ = std::fs::remove_dir_all(root.path());
    }

    /// A testnet-family descriptor for every encoding but mainnet, so that `Config::check`'s
    /// xpub-network validation passes and the chain comparison is the gate under test.
    pub(super) const TESTNET_DESC: &str = "wsh(andor(pk([aabbccdd]tpubDEN9WSToTyy9ZQfaYqSKfmVqmq1VVLNtYfj3Vkqh67et57eJ5sTKZQBkHqSwPUsoSskJeaYnPttHe2VrkCsKA27kUaN9SDc5zhqeLzKa1rr/<0;1>/*),older(10000),pk([aabbccdd]tpubD8LYfn6njiA2inCoxwM7EuN3cuLVcaHAwLYeups13dpevd3nHLRdK9NdQksWXrhLQVxcUZRpnp5CkJ1FhE61WRAsHxDNAkvGkoQkAeWDYjV/<0;1>/*)))#dw4ulnrs";

    fn desc_for(network: &str) -> &'static str {
        match network {
            "bitcoin" | "bitcoin-blake2b" => MAINNET_DESC,
            _ => TESTNET_DESC,
        }
    }

    /// The Cube's daemon directory under `root` for `chain` (created).
    fn cube_daemon_dir(root: &CoincubeDirectory, chain: ChainId) -> PathBuf {
        let dir = root
            .network_directory(chain)
            .coincubed_data_directory(&wallet().wallet_id())
            .path()
            .to_path_buf();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The `data_directory` line every fixture below carries, pinning the daemon to the
    /// Cube's own temp directory. Without it `Config::data_directory()` falls through to the
    /// real config folder (`config_folder_path()/<chain>`), and a control that reaches the
    /// daemon would create — or migrate — a database there. `data_directory` is used
    /// verbatim (the installer's own key); the legacy `data_dir` would append the chain
    /// segment. A TOML literal string, so Windows paths pass through unescaped.
    fn data_directory_line(dir: &std::path::Path) -> String {
        format!("data_directory = '{}'\n", dir.display())
    }

    /// The fixture at `dir` resolves its data directory to `dir` itself — never to the real
    /// config folder. Checked on the parsed file, the way the daemon will resolve it.
    fn assert_pinned_to(dir: &std::path::Path) {
        let config = Config::from_file(Some(dir.join("daemon.toml"))).unwrap();
        assert_eq!(config.data_directory().unwrap().path(), dir);
    }

    /// A `daemon.toml` for `network` in the Cube's daemon directory, with a bitcoind backend
    /// pointing nowhere. Returns the directory that would hold the database (absent).
    fn write_cube_daemon_toml(root: &CoincubeDirectory, chain: ChainId, network: &str) -> PathBuf {
        let dir = cube_daemon_dir(root, chain);
        std::fs::write(
            dir.join("daemon.toml"),
            format!(
                "{}main_descriptor = \"{}\"\n\n\
                 [bitcoin_config]\nnetwork = \"{network}\"\n\n\
                 [bitcoind_config]\ncookie_path = '/nonexistent/.cookie'\naddr = \"127.0.0.1:1\"\n",
                data_directory_line(&dir),
                desc_for(network)
            ),
        )
        .unwrap();
        dir
    }

    /// A deterministic, network-free stop for a control that is *meant* to get past the
    /// loader's chain comparison: a stray SQLite sidecar in the Cube's daemon directory makes
    /// the daemon refuse at its own preflight (`OrphanSidecar`) — after the loader's compare
    /// and `daemon.toml` migration, but before `data_dir.init`, any bitcoind RPC, any
    /// database creation and any Esplora client. So the control proves the comparison passed
    /// without a node, a database or a socket.
    fn plant_orphan_sidecar(dir: &std::path::Path) -> PathBuf {
        let stray = dir.join("coincubed.sqlite3-shm");
        std::fs::write(&stray, b"remains").unwrap();
        stray
    }

    /// The daemon refused at its preflight because of the planted sidecar — the only
    /// acceptable outcome for a control that got past the loader's comparison.
    fn assert_stopped_by_planted_sidecar(result: &StartedResult, stray: &std::path::Path) {
        match result {
            Err(Error::Daemon(DaemonError::Start(StartupError::DbPreflight(
                coincubed::PreflightError::OrphanSidecar(p),
            )))) => assert_eq!(p, stray),
            Err(other) => panic!("expected the planted sidecar refusal, got: {}", other),
            Ok(_) => panic!("started a daemon"),
        }
    }

    /// A *legacy* Esplora `daemon.toml` for `network` — Connect as the only `addr`, no
    /// fallback, a pre-Esplora poll interval — i.e. exactly the shape
    /// `migrate_esplora_config` rewrites when it runs.
    fn write_legacy_esplora_daemon_toml(
        root: &CoincubeDirectory,
        chain: ChainId,
        network: &str,
        config_chain: ChainId,
    ) -> PathBuf {
        let dir = cube_daemon_dir(root, chain);
        std::fs::write(
            dir.join("daemon.toml"),
            format!(
                "{}main_descriptor = \"{}\"\n\n\
                 [bitcoin_config]\nnetwork = \"{network}\"\npoll_interval_secs = 10\n\n\
                 [esplora_config]\naddr = \"{}\"\ntoken = \"jwt\"\n",
                data_directory_line(&dir),
                desc_for(network),
                crate::installer::connect_url(config_chain)
            ),
        )
        .unwrap();
        dir
    }

    /// Every file under `root` with its bytes, so "nothing changed" is a comparison of
    /// contents, not of path lists.
    fn snapshot(root: &std::path::Path) -> Vec<(PathBuf, Vec<u8>)> {
        tree(root)
            .into_iter()
            .map(|p| {
                let bytes = if p.is_file() {
                    std::fs::read(&p).unwrap()
                } else {
                    Vec::new()
                };
                (p, bytes)
            })
            .collect()
    }

    // A Bitcoin Cube whose `daemon.toml` names another chain — a file copied between chain
    // directories, or edited by hand. No database exists yet, so the daemon's own preflight
    // would have nothing to compare against and would create a database for the *config's*
    // chain inside the *Cube's* directory. The loader refuses first, and nothing is created.
    #[tokio::test]
    async fn a_daemon_toml_for_another_chain_is_refused_before_anything_starts() {
        for (cube_chain, config_network, config_chain) in [
            (ChainId::Bitcoin, "signet", ChainId::Signet),
            (ChainId::Signet, "bitcoin", ChainId::Bitcoin),
            // The encoding twin, which an older build could not even tell apart.
            (ChainId::Bitcoin, "bitcoin-blake2b", ChainId::BitcoinBlake2b),
        ] {
            let root = temp_root("mismatch");
            let dir = write_cube_daemon_toml(&root, cube_chain, config_network);
            assert_pinned_to(&dir);
            let before = snapshot(root.path());
            let result = start_bitcoind_and_daemon(root.clone(), false, cube_chain, wallet()).await;
            match result {
                Err(Error::ChainMismatch { cube, config }) => {
                    assert_eq!((cube, config), (cube_chain, config_chain));
                }
                Err(other) => panic!("wrong refusal for {:?}: {}", cube_chain, other),
                Ok(_) => panic!("started a daemon for {:?}", cube_chain),
            }
            // No database, no lock, no node datadir, and every file byte-identical.
            assert_eq!(snapshot(root.path()), before, "{:?}", cube_chain);
            assert!(!dir.join("coincubed.sqlite3").exists());
            let _ = std::fs::remove_dir_all(root.path());
        }
    }

    // A mismatched `daemon.toml` that the Esplora-config migration *would* rewrite (Connect as
    // the sole `addr`, a 10 s poll interval) must be refused before that migration runs, so
    // the refusal's "nothing was modified" is true down to the bytes. The control below shows
    // the same file for the matching chain does get rewritten by the same call.
    #[tokio::test]
    async fn a_mismatched_legacy_daemon_toml_is_refused_before_it_is_migrated() {
        let root = temp_root("legacy-mismatch");
        let dir =
            write_legacy_esplora_daemon_toml(&root, ChainId::Bitcoin, "signet", ChainId::Signet);
        let config_path = dir.join("daemon.toml");
        assert_pinned_to(&dir);
        let before_bytes = std::fs::read(&config_path).unwrap();
        let before = snapshot(root.path());
        // The fixture is one the migration acts on: it carries the Connect URL as `addr`.
        assert!(String::from_utf8_lossy(&before_bytes)
            .contains(&crate::installer::connect_url(ChainId::Signet)));

        let result =
            start_bitcoind_and_daemon(root.clone(), false, ChainId::Bitcoin, wallet()).await;
        match result {
            Err(Error::ChainMismatch { cube, config }) => {
                assert_eq!((cube, config), (ChainId::Bitcoin, ChainId::Signet));
            }
            Err(other) => panic!("wrong refusal: {}", other),
            Ok(_) => panic!("started a daemon"),
        }
        assert_eq!(
            std::fs::read(&config_path).unwrap(),
            before_bytes,
            "daemon.toml was rewritten"
        );
        assert_eq!(snapshot(root.path()), before);
        let _ = std::fs::remove_dir_all(root.path());

        // Control: the matching chain passes the comparison and the migration then rewrites
        // the file (Connect demoted to a fallback slot). The daemon is stopped right after,
        // at its own preflight, by a planted sidecar — so no database is created and no
        // Esplora client is built.
        let root = temp_root("legacy-match");
        let dir =
            write_legacy_esplora_daemon_toml(&root, ChainId::Signet, "signet", ChainId::Signet);
        let config_path = dir.join("daemon.toml");
        let before_bytes = std::fs::read(&config_path).unwrap();
        let stray = plant_orphan_sidecar(&dir);
        assert_pinned_to(&dir);
        let result =
            start_bitcoind_and_daemon(root.clone(), false, ChainId::Signet, wallet()).await;
        assert_stopped_by_planted_sidecar(&result, &stray);
        let after = std::fs::read_to_string(&config_path).unwrap();
        assert_ne!(
            after.as_bytes(),
            before_bytes.as_slice(),
            "migration did not run"
        );
        assert!(after.contains("fallback_addr"), "{}", after);
        // The migrated file still pins the daemon to the temp directory.
        assert_pinned_to(&dir);
        assert!(!dir.join("coincubed.sqlite3").exists());
        let _ = std::fs::remove_dir_all(root.path());
    }

    // The control: the same Cube with a matching `daemon.toml` gets past the comparison and
    // is stopped one gate later — at the daemon's own preflight, by a planted sidecar —
    // proving the mismatch refusal is its own gate, without reaching a node, a database or
    // a socket.
    #[tokio::test]
    async fn a_matching_daemon_toml_passes_the_chain_comparison() {
        let root = temp_root("match");
        let dir = write_cube_daemon_toml(&root, ChainId::Signet, "signet");
        let stray = plant_orphan_sidecar(&dir);
        assert_pinned_to(&dir);
        let result =
            start_bitcoind_and_daemon(root.clone(), false, ChainId::Signet, wallet()).await;
        assert_stopped_by_planted_sidecar(&result, &stray);
        assert!(!dir.join("coincubed.sqlite3").exists());
        let _ = std::fs::remove_dir_all(root.path());
    }

    // The conf lock is busy for the whole bounded wait while the loader is
    // reconciling inbound-over-Tor: the start is refused — retryably, as
    // `ConfigUnavailable` — instead of proceeding from a conf that may still
    // name a Tor that is not running. Nothing on disk changes and no binary
    // is looked for.
    #[tokio::test]
    async fn a_busy_conf_lock_refuses_the_start_instead_of_using_stale_privacy_config() {
        // Both starts reach `prepare_inbound_tor` (process-global Tor registry).
        let _registry = crate::node::tor::registry_test_guard();
        use crate::node::bitcoind::{
            internal_bitcoind_config_path, internal_bitcoind_cookie_path,
            internal_bitcoind_datadir, InternalBitcoindConfig, InternalBitcoindNetworkConfig,
            ManagedNodeLifecycle, NodeChainFamily, NodeFlavor, StartInternalBitcoindError,
            PRUNE_DEFAULT,
        };
        use crate::node::managed_conf::ManagedConfLock;
        use crate::node::revalidate::ManagedNodeState;
        use crate::node::tor::InboundTorPreference;
        let root = temp_root("busy");
        let chain = ChainId::Testnet4;
        let wallet = wallet();

        let bitcoind_datadir = internal_bitcoind_datadir(&root);
        std::fs::create_dir_all(&bitcoind_datadir).unwrap();
        let conf_path = internal_bitcoind_config_path(&bitcoind_datadir);
        let mut conf = InternalBitcoindConfig::for_flavor(NodeFlavor::Knots);
        conf.networks.insert(
            bitcoin::Network::Testnet4,
            InternalBitcoindNetworkConfig {
                rpc_port: 46001,
                p2p_port: 46002,
                prune: PRUNE_DEFAULT,
                rpc_auth: None,
            },
        );
        // Stale inbound lines from a previous run.
        conf.inbound_tor = true;
        conf.tor_control_port = Some(9051);
        conf.tor_socks_port = Some(9050);
        conf.to_file(&conf_path).unwrap();
        let conf_bytes = std::fs::read(&conf_path).unwrap();
        ManagedNodeState {
            configured_flavor: Some(NodeFlavor::Knots),
            ..Default::default()
        }
        .save(&root)
        .unwrap();
        InboundTorPreference {
            enabled: false,
            ..Default::default()
        }
        .save(&root)
        .unwrap();
        let cookie = internal_bitcoind_cookie_path(&bitcoind_datadir, &bitcoin::Network::Testnet4);
        let wallet_dir = root
            .network_directory(chain)
            .coincubed_data_directory(&wallet.wallet_id())
            .path()
            .to_path_buf();
        std::fs::create_dir_all(&wallet_dir).unwrap();
        std::fs::write(
            wallet_dir.join("daemon.toml"),
            format!(
                "data_dir = {:?}\nlog_level = \"TRACE\"\nmain_descriptor = \"{}\"\n\n\
                 [bitcoin_config]\nnetwork = \"testnet4\"\npoll_interval_secs = 18\n\n\
                 [bitcoind_config]\ncookie_path = {:?}\naddr = \"127.0.0.1:1\"\n",
                root.path().display().to_string(),
                "wsh(andor(pk([aabbccdd]tpubDEN9WSToTyy9ZQfaYqSKfmVqmq1VVLNtYfj3Vkqh67et57eJ5sTKZQBkHqSwPUsoSskJeaYnPttHe2VrkCsKA27kUaN9SDc5zhqeLzKa1rr/<0;1>/*),older(10000),pk([aabbccdd]tpubD8LYfn6njiA2inCoxwM7EuN3cuLVcaHAwLYeups13dpevd3nHLRdK9NdQksWXrhLQVxcUZRpnp5CkJ1FhE61WRAsHxDNAkvGkoQkAeWDYjV/<0;1>/*)))#dw4ulnrs",
                cookie.display().to_string(),
            ),
        )
        .unwrap();
        // Prime the stable lifecycle lock before the byte-for-byte snapshot;
        // acquiring it is allowed even when the later conf lock is busy.
        drop(ManagedNodeLifecycle::acquire(&root, NodeChainFamily::Bitcoin).unwrap());
        // The conf lock file is created by the holder below, so snapshot after it.
        let held = ManagedConfLock::acquire(&root).unwrap();
        let before = tree(root.path());
        // The start is polled on this thread (current-thread runtime), so the
        // thread-local quick bound applies to its lock attempt.
        let result = crate::node::managed_conf::with_quick_lock_bound_async(
            start_bitcoind_and_daemon(root.clone(), true, chain, wallet.clone()),
        )
        .await;
        drop(held);
        match result {
            Err(Error::Bitcoind(StartInternalBitcoindError::ConfigUnavailable(e))) => {
                assert!(e.contains("another setup is updating"), "{}", e)
            }
            Err(other) => panic!("wrong refusal: {}", other),
            Ok(_) => panic!("started from a conf that could not be reconciled"),
        }
        assert_eq!(std::fs::read(&conf_path).unwrap(), conf_bytes);
        assert_eq!(tree(root.path()), before);

        // With the lock free the same start reconciles the conf (the stale
        // inbound lines go) and proceeds to the ordinary absent-binary failure.
        let result = start_bitcoind_and_daemon(root.clone(), true, chain, wallet).await;
        assert!(matches!(
            result,
            Err(Error::Bitcoind(
                StartInternalBitcoindError::ExecutableNotFound
            ))
        ));
        assert!(
            !String::from_utf8_lossy(&std::fs::read(&conf_path).unwrap()).contains("torcontrol")
        );
        let _ = std::fs::remove_dir_all(root.path());
    }
}

#[cfg(test)]
mod bootstrap_backend_selection_tests {
    //! Which Bitcoin backend the launcher selects when a Cube is opened, and
    //! which companions the authenticated Bitcoin Blake2b start refuses.
    //!
    //! Two properties, both settled without a node, a database or a socket:
    //!
    //! 1. A fork Cube never selects the app-managed node — not when its own
    //!    persisted `daemon.toml` names the managed cookie as its backend, and
    //!    not when either internal-node preference asks for one. Its only
    //!    backend is the authenticated Connect route, whose start path returns
    //!    no node handle at all ([`start_connect_daemon`] yields `None` for it).
    //!    [`Loader::start_bitcoind`] is the value [`Loader::on_load`] hands to
    //!    [`start_bitcoind_and_daemon`] as `start_internal_bitcoind`, i.e. the
    //!    only place the launcher can ask for a managed node, so `false` there
    //!    is what keeps a fork Cube from requesting one.
    //! 2. The authenticated fork start refuses each wrong companion on its own,
    //!    before a socket is dialled or anything is written.
    //!
    //! Headless by construction: these drive [`Loader`] and
    //! [`start_connect_daemon`] directly. A pass here is *not* rendered-GUI,
    //! PIN-entry, passkey or hardware-signer evidence, and it is not an
    //! end-to-end App bootstrap run — it pins the launcher's selection and
    //! refusal decisions only.

    use super::chain_identity_tests::TESTNET_DESC;
    use super::tests::MAINNET_DESC;
    use super::*;
    use crate::app::settings::AuthConfig;
    use crate::chain::ChainId;
    use std::path::PathBuf;

    const SESSION_REFUSAL: &str =
        "Unexpected error: Bitcoin Blake2b requires an authenticated Connect Vault session";
    const NOT_A_FORK_CUBE: &str =
        "Unexpected error: The authenticated fork loader requires Bitcoin Blake2b";
    const FORK_VAULT_REQUIRED: &str = "Authenticated Connect fork Vault required";

    fn cube(chain: ChainId) -> CubeSettings {
        CubeSettings::new_with_raw_id("cube-uuid".to_string(), "Fork".to_string(), chain)
    }

    fn wallet() -> WalletSettings {
        WalletSettings {
            name: "Coincube-kt6ht0kt".to_string(),
            alias: None,
            descriptor_checksum: "kt6ht0kt".to_string(),
            pinned_at: Some(1_720_000_000),
            keys: Vec::new(),
            hardware_wallets: Vec::new(),
            remote_backend_auth: None,
            start_internal_bitcoind: Some(true),
            pending_rescan: None,
            keychain_keys_recorded: false,
        }
    }

    /// A Vault held on the remote backend: the fork route is for a local Vault
    /// under an authenticated Connect *session*, never for a remote-backend one.
    fn remote_backend_wallet() -> WalletSettings {
        WalletSettings {
            remote_backend_auth: Some(AuthConfig::new(
                "someone@example.com".to_string(),
                "wallet-uuid".to_string(),
            )),
            ..wallet()
        }
    }

    fn temp_root(tag: &str) -> CoincubeDirectory {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "coincube-loader-backend-{}-{}-{}",
            tag,
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&path).unwrap();
        CoincubeDirectory::new(path)
    }

    /// Every directory and file under `root`, including exact file bytes.
    /// The active managed-node owner lock is empty but cannot be read while
    /// locked on Windows; its path and length are checked instead. All other
    /// files retain byte-for-byte comparison, including config and ledger data.
    fn tree(root: &Path) -> Vec<(PathBuf, Option<Vec<u8>>)> {
        fn walk(p: &Path, out: &mut Vec<(PathBuf, Option<Vec<u8>>)>) {
            for entry in std::fs::read_dir(p).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                let kind = entry.file_type().unwrap();
                if kind.is_dir() {
                    out.push((path.clone(), None));
                    walk(&path, out);
                } else if kind.is_file() {
                    #[cfg(windows)]
                    let bytes = if path.extension() == Some(std::ffi::OsStr::new("lock"))
                        && path
                            .parent()
                            .and_then(Path::parent)
                            .and_then(Path::file_name)
                            == Some(std::ffi::OsStr::new("locks"))
                    {
                        assert_eq!(std::fs::metadata(&path).unwrap().len(), 0, "{:?}", path);
                        Vec::new()
                    } else {
                        std::fs::read(&path).unwrap()
                    };
                    #[cfg(not(windows))]
                    let bytes = std::fs::read(&path).unwrap();
                    out.push((path.clone(), Some(bytes)));
                } else {
                    panic!("unsupported filesystem entry in test snapshot: {:?}", path);
                }
            }
        }
        let mut out = Vec::new();
        walk(root, &mut out);
        out.sort();
        out
    }

    fn desc_for(chain: ChainId) -> &'static str {
        if chain.bitcoin_network() == bitcoin::Network::Bitcoin {
            MAINNET_DESC
        } else {
            TESTNET_DESC
        }
    }

    /// The Cube's daemon directory under `root` for `chain` (created).
    fn cube_daemon_dir(root: &CoincubeDirectory, chain: ChainId) -> PathBuf {
        let dir = root
            .network_directory(chain)
            .coincubed_data_directory(&wallet().wallet_id())
            .path()
            .to_path_buf();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// This Cube's `daemon.toml`, with a bitcoind backend whose cookie lives
    /// *inside this chain's managed-node datadir* — the persisted shape that
    /// means "the app-managed node is my backend" (see
    /// [`backend_is_internal_bitcoind`]). `config_chain` is what the file
    /// claims, so a caller can also write one for the wrong chain.
    fn write_managed_cookie_daemon_toml(
        root: &CoincubeDirectory,
        chain: ChainId,
        config_chain: ChainId,
    ) -> PathBuf {
        let dir = cube_daemon_dir(root, chain);
        let cookie = managed_node_datadir(root, config_chain).join(".cookie");
        std::fs::write(
            dir.join("daemon.toml"),
            format!(
                "data_directory = '{}'\nmain_descriptor = \"{}\"\n\n\
                 [bitcoin_config]\nnetwork = \"{}\"\n\n\
                 [bitcoind_config]\ncookie_path = '{}'\naddr = \"127.0.0.1:1\"\n",
                dir.display(),
                desc_for(config_chain),
                config_chain,
                cookie.display(),
            ),
        )
        .unwrap();
        dir
    }

    /// An authenticated Connect client pointed at a closed port: enough for
    /// every guard that must refuse *before* a request is made. A guard that
    /// reaches the network would fail with a transport error instead, which is
    /// a different assertion and would be visible as such.
    fn authenticated_client() -> crate::services::coincube::CoincubeClient {
        let mut client = crate::services::coincube::CoincubeClient::for_test("http://127.0.0.1:1");
        client.set_token("synthetic-token");
        client
    }

    fn tokenless_client() -> crate::services::coincube::CoincubeClient {
        crate::services::coincube::CoincubeClient::for_test("http://127.0.0.1:1")
    }

    fn encryption_key() -> Arc<crate::services::connect::crypto::CubeEncryptionKey> {
        let signer =
            coincube_core::signer::MasterSigner::generate(bitcoin::Network::Bitcoin).unwrap();
        Arc::new(crate::services::connect::crypto::CubeEncryptionKey::derive(
            &signer,
            bitcoin::Network::Bitcoin,
        ))
    }

    #[test]
    fn retry_preserves_a_revoked_fork_session_and_its_reopen_guidance() {
        let root = temp_root("revoked-retry");
        let (mut loader, startup) = Loader::new_for_chain(
            root.clone(),
            GUIConfig::new(false),
            wallet(),
            cube(ChainId::BitcoinBlake2b),
            authenticated_client(),
            encryption_key(),
        );
        assert!(matches!(loader.step, Step::Connecting));
        assert!(startup.units() > 0);
        drop(startup); // Admission only: never poll a daemon or network worker.
        loader.invalidate_fork_session();
        assert!(loader.fork_session_revoked);
        assert!(loader.connect_client.is_none());
        assert!(loader.cube_encryption_key.is_none());
        let original = match &loader.step {
            Step::Error(error) => error.to_string(),
            _ => panic!("invalidation must refuse the session"),
        };
        assert!(original.contains("reopen this Cube"));
        for deadline in [
            Some(std::time::Instant::now() + Duration::from_secs(30)),
            None, // The real cooldown has elapsed; never wait in the test.
        ] {
            loader.fork_retry_not_before = deadline;
            for _ in 0..2 {
                let task = loader.update(Message::View(ViewMessage::Retry));
                assert_eq!(task.units(), 0);
                assert!(loader.fork_session_revoked);
                assert!(loader.connect_client.is_none());
                assert!(loader.cube_encryption_key.is_none());
                assert!(matches!(&loader.step, Step::Error(error)
                    if error.to_string() == original));
            }
        }
        // Late worker delivery must still see the revocation latch after Retry.
        let task = loader.update(Message::Started(Err(Error::Unexpected(
            "late startup".into(),
        ))));
        assert_eq!(task.units(), 0);
        assert!(matches!(&loader.step, Step::Error(error)
            if error.to_string() == original));
        // Capability revocation may install a more specific refusal afterwards.
        let disabled =
            "Bitcoin Blake2b is disabled. Check Global Settings before reopening this Cube.";
        loader.fail(Error::Unexpected(disabled.into()));
        let task = loader.update(Message::View(ViewMessage::Retry));
        assert_eq!(task.units(), 0);
        assert!(loader.fork_session_revoked);
        assert!(matches!(&loader.step, Step::Error(error)
            if error.to_string() == format!("Unexpected error: {disabled}")));
        std::fs::remove_dir_all(root.path()).unwrap();
    }

    #[test]
    fn nonrevoked_connect_and_local_fork_retries_keep_their_admission() {
        for local in [false, true] {
            let root = temp_root("healthy-retry");
            let chain = ChainId::BitcoinBlake2b;
            let key = encryption_key();
            let (loader, startup) = if local {
                let (_, wallet) = crate::chain::managed_local_fixture(&root, chain);
                // Carried for a resync only, and dropped on any auth change.
                Loader::new_local_fork(
                    root.clone(),
                    GUIConfig::new(false),
                    wallet,
                    cube(chain),
                    key.clone(),
                    Some(authenticated_client()),
                )
            } else {
                Loader::new_for_chain(
                    root.clone(),
                    GUIConfig::new(false),
                    wallet(),
                    cube(chain),
                    authenticated_client(),
                    key.clone(),
                )
            };
            drop(startup);
            let mut tab = crate::gui::tab::Tab::new(1, crate::gui::tab::State::Loader(loader));
            if local {
                // Local admission is independent of sibling Connect auth changes.
                for change in [
                    crate::gui::tab::AuthChange::LogOut,
                    crate::gui::tab::AuthChange::SignIn {
                        user_id: 7,
                        token: zeroize::Zeroizing::new("new-synthetic-token".into()),
                    },
                ] {
                    assert_eq!(tab.invalidate_fork_session(change, false).units(), 0);
                }
            }
            let crate::gui::tab::State::Loader(loader) = &mut tab.state else {
                panic!("auth-independent local loader must stay open");
            };
            assert!(!loader.fork_session_revoked);
            loader.fork_retry_not_before = None;
            let task = loader.update(Message::View(ViewMessage::Retry));
            assert!(
                task.units() > 0,
                "healthy local={} must retry startup",
                local
            );
            assert!(matches!(loader.step, Step::Connecting));
            assert!(!loader.fork_session_revoked);
            assert_eq!(loader.connect_client.is_some(), !local);
            assert!(Arc::ptr_eq(
                loader.cube_encryption_key.as_ref().unwrap(),
                &key
            ));
            drop(task);
            drop(tab);
            std::fs::remove_dir_all(root.path()).unwrap();
        }
    }

    /// A managed-node handle with no node behind it, to stand in for one the
    /// caller already holds. Its lock file lands under `root`, so a caller
    /// snapshotting the tree must snapshot after this.
    fn managed_node_handle(root: &CoincubeDirectory) -> Bitcoind {
        Bitcoind::for_test(
            BitcoindConfig {
                rpc_auth: BitcoindRpcAuth::CookieFile(
                    managed_node_datadir(root, ChainId::Bitcoin).join(".cookie"),
                ),
                addr: "127.0.0.1:1".parse().unwrap(),
            },
            root,
            bitcoin::Network::Bitcoin,
        )
    }

    fn a_backup() -> Backup {
        Backup {
            name: None,
            alias: None,
            accounts: Vec::new(),
            network: bitcoin::Network::Bitcoin,
            chain: Some(ChainId::BitcoinBlake2b),
            date: None,
            proprietary: serde_json::Map::new(),
            version: 0,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn build_loader(
        root: &CoincubeDirectory,
        chain: ChainId,
        wallet_settings: Option<WalletSettings>,
        client: Option<crate::services::coincube::CoincubeClient>,
        key: Option<Arc<crate::services::connect::crypto::CubeEncryptionKey>>,
        internal_bitcoind: Option<Bitcoind>,
        backup: Option<Backup>,
        start_internal_bitcoind: bool,
    ) -> Loader {
        Loader::build(
            root.clone(),
            GUIConfig::new(start_internal_bitcoind),
            chain.bitcoin_network(),
            internal_bitcoind,
            backup,
            wallet_settings,
            cube(chain),
            None,
            None,
            client,
            key,
        )
        .0
    }

    /// A fork Cube whose own `daemon.toml` names the app-managed node as its
    /// backend still must not start one: the selection is keyed on the chain,
    /// ahead of the config. The Bitcoin control carries the identical fixture
    /// and *does* select the managed node, so the fork assertion is about the
    /// chain and not about a fixture that never selected anything.
    #[test]
    fn a_fork_cube_never_selects_the_managed_node_its_own_config_names() {
        for chain in [ChainId::BitcoinBlake2b, ChainId::BitcoinBlake2bTestnet4] {
            let root = temp_root("fork-cookie");
            write_managed_cookie_daemon_toml(&root, chain, chain);
            let loader = build_loader(
                &root,
                chain,
                Some(wallet()),
                Some(authenticated_client()),
                Some(encryption_key()),
                None,
                None,
                true,
            );
            assert!(
                matches!(&loader.step, Step::Connecting),
                "the authenticated fork session must be admitted for {}, got another step",
                chain
            );
            assert!(
                loader.vault_uses_internal_bitcoind(),
                "fixture must name the managed cookie for {}",
                chain
            );
            assert!(
                !loader.start_bitcoind(),
                "{} must not select the managed node its config names",
                chain
            );
        }

        let root = temp_root("bitcoin-cookie");
        write_managed_cookie_daemon_toml(&root, ChainId::Bitcoin, ChainId::Bitcoin);
        let loader = build_loader(
            &root,
            ChainId::Bitcoin,
            Some(wallet()),
            None,
            None,
            None,
            None,
            // Off on purpose: a config-named managed node is started whatever
            // the preference says, which is the behaviour the fork must not
            // inherit.
            false,
        );
        assert!(loader.vault_uses_internal_bitcoind());
        assert!(
            loader.start_bitcoind(),
            "control: Bitcoin selects the managed node its config names"
        );
    }

    /// The two internal-node preferences — the Vault's own and the GUI default —
    /// govern an external node. Neither may start one for a fork Cube, at any
    /// combination.
    #[test]
    fn a_fork_cube_ignores_both_internal_node_preferences() {
        for chain in [ChainId::BitcoinBlake2b, ChainId::BitcoinBlake2bTestnet4] {
            for wallet_pref in [Some(true), Some(false), None] {
                for gui_pref in [true, false] {
                    let root = temp_root("fork-pref");
                    let loader = build_loader(
                        &root,
                        chain,
                        Some(WalletSettings {
                            start_internal_bitcoind: wallet_pref,
                            ..wallet()
                        }),
                        Some(authenticated_client()),
                        Some(encryption_key()),
                        None,
                        None,
                        gui_pref,
                    );
                    assert!(
                        !loader.vault_uses_internal_bitcoind(),
                        "no config on disk for {}",
                        chain
                    );
                    assert!(
                        !loader.start_bitcoind(),
                        "{} started the managed node for wallet={:?} gui={}",
                        chain,
                        wallet_pref,
                        gui_pref
                    );
                }
            }
        }

        let root = temp_root("bitcoin-pref");
        let loader = build_loader(
            &root,
            ChainId::Bitcoin,
            Some(wallet()),
            None,
            None,
            None,
            None,
            true,
        );
        assert!(
            loader.start_bitcoind(),
            "control: the preference does start a node on Bitcoin"
        );
    }

    /// Each companion the authenticated fork start refuses, one at a time, so no
    /// arm can pass behind another's refusal. Every arm must land in the error
    /// step with the session copy, arm no retry window, hold no abortable fork
    /// task, and leave the datadir exactly as it found it.
    ///
    /// Bounded coverage, stated rather than implied: the same `||` predicate
    /// also refuses a Cube signer and a Spark backend. Neither is constructed
    /// here — `SparkBackend` spawns the bridge subprocess, which would make
    /// this a live-SDK test. Those arms are untested.
    #[test]
    fn an_authenticated_fork_start_refuses_each_wrong_companion() {
        let chain = ChainId::BitcoinBlake2b;

        /// One companion swapped for a wrong one; everything else is the shape
        /// the fork start accepts.
        struct Arm {
            name: &'static str,
            vault: fn() -> Option<WalletSettings>,
            client: fn() -> Option<crate::services::coincube::CoincubeClient>,
            with_key: bool,
            with_node: bool,
            with_backup: bool,
        }

        const ACCEPTED: Arm = Arm {
            name: "",
            vault: || Some(wallet()),
            client: || Some(authenticated_client()),
            with_key: true,
            with_node: false,
            with_backup: false,
        };

        let arms = [
            Arm {
                name: "no Connect token",
                client: || Some(tokenless_client()),
                ..ACCEPTED
            },
            Arm {
                name: "no Cube encryption key",
                with_key: false,
                ..ACCEPTED
            },
            Arm {
                name: "no Vault settings",
                vault: || None,
                ..ACCEPTED
            },
            Arm {
                name: "a remote-backend Vault",
                vault: || Some(remote_backend_wallet()),
                ..ACCEPTED
            },
            Arm {
                name: "a managed-node handle",
                with_node: true,
                ..ACCEPTED
            },
            Arm {
                name: "a pending backup restore",
                with_backup: true,
                ..ACCEPTED
            },
        ];

        for Arm {
            name,
            vault,
            client,
            with_key,
            with_node,
            with_backup,
        } in arms
        {
            let root = temp_root("fork-companion");
            let node = with_node.then(|| managed_node_handle(&root));
            // After the handle's lock file, so the snapshot is of the tree the
            // loader is actually handed.
            let before = tree(root.path());
            let loader = build_loader(
                &root,
                chain,
                vault(),
                client(),
                with_key.then(encryption_key),
                node,
                with_backup.then(a_backup),
                true,
            );
            match &loader.step {
                Step::Error(e) => assert_eq!(e.to_string(), SESSION_REFUSAL, "arm: {}", name),
                other => panic!(
                    "arm {} was admitted: {:?}",
                    name,
                    std::mem::discriminant(other)
                ),
            }
            // Deliberately not asserted here: `fork_task`. Every fork-chain
            // task is wrapped in the abortable handle, refusal or not, so a
            // `Some` says nothing about whether work was dispatched — the
            // refused arm holds an abort handle for an empty task. What a
            // refusal *does* have to leave alone is the retry window, the
            // daemon and the datadir, below.
            assert!(
                loader.fork_retry_not_before.is_none(),
                "arm {} armed a retry window for a session that never started",
                name
            );
            assert!(!loader.daemon_started, "arm {} started a daemon", name);
            assert_eq!(
                tree(root.path()),
                before,
                "arm {} wrote to the datadir while refusing",
                name
            );
        }
    }

    /// The mirror refusal: a Bitcoin-family Cube handed an authenticated Connect
    /// client is refused too, rather than quietly loading through the fork's
    /// dedicated start path.
    #[test]
    fn the_fork_loader_refuses_a_bitcoin_family_cube() {
        for chain in [ChainId::Bitcoin, ChainId::Signet, ChainId::Testnet4] {
            let root = temp_root("bitcoin-on-fork-path");
            let before = tree(root.path());
            let loader = build_loader(
                &root,
                chain,
                Some(wallet()),
                Some(authenticated_client()),
                Some(encryption_key()),
                None,
                None,
                true,
            );
            match &loader.step {
                Step::Error(e) => assert_eq!(e.to_string(), NOT_A_FORK_CUBE, "{}", chain),
                other => panic!(
                    "{} was admitted to the fork path: {:?}",
                    chain,
                    std::mem::discriminant(other)
                ),
            }
            assert!(loader.fork_task.is_none(), "{} armed a fork task", chain);
            assert!(!loader.daemon_started);
            assert_eq!(tree(root.path()), before, "{} wrote to the datadir", chain);
        }
    }

    /// The fork daemon start's own guards, at the same boundary but called
    /// directly: each refuses before Connect is dialled, before `daemon.toml` is
    /// read and without creating a managed datadir.
    #[tokio::test]
    async fn the_fork_daemon_start_refuses_before_it_dials_connect() {
        // A Bitcoin-family chain on the fork's dedicated start path.
        let root = temp_root("start-bitcoin");
        let before = tree(root.path());
        let err = start_connect_daemon(
            root.clone(),
            ChainId::Bitcoin,
            wallet(),
            authenticated_client(),
        )
        .await
        .expect_err("a Bitcoin chain has no authenticated fork start");
        assert!(
            matches!(&err, Error::Unexpected(m) if m.as_str() == FORK_VAULT_REQUIRED),
            "got {}",
            err
        );
        assert_eq!(tree(root.path()), before);

        // An unauthenticated client.
        let root = temp_root("start-tokenless");
        let before = tree(root.path());
        let err = start_connect_daemon(
            root.clone(),
            ChainId::BitcoinBlake2b,
            wallet(),
            tokenless_client(),
        )
        .await
        .expect_err("an unauthenticated session cannot start the fork daemon");
        assert!(
            matches!(&err, Error::Unexpected(m) if m.as_str() == FORK_VAULT_REQUIRED),
            "got {}",
            err
        );
        assert_eq!(tree(root.path()), before);

        // A remote-backend Vault.
        let root = temp_root("start-remote");
        let before = tree(root.path());
        let err = start_connect_daemon(
            root.clone(),
            ChainId::BitcoinBlake2b,
            remote_backend_wallet(),
            authenticated_client(),
        )
        .await
        .expect_err("a remote-backend Vault cannot start the fork daemon");
        assert!(
            matches!(&err, Error::Unexpected(m) if m.as_str() == FORK_VAULT_REQUIRED),
            "got {}",
            err
        );
        assert_eq!(tree(root.path()), before);
    }

    /// A local stub answering the one `/connect/features` request
    /// [`crate::chain::require_connect_feature`] makes, with Bitcoin Blake2b
    /// enabled. Returns its address; the thread ends after that request.
    fn features_stub_enabling_the_fork() -> std::net::SocketAddr {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("local_addr");
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut scratch = [0u8; 2048];
                let _ = stream.read(&mut scratch);
                let body = "{\"data\":{\"plans\":[],\"bitcoin_blake2b_enabled\":true}}";
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.flush();
                let _ = stream.shutdown(std::net::Shutdown::Write);
            }
        });
        addr
    }

    fn client_for(addr: std::net::SocketAddr) -> crate::services::coincube::CoincubeClient {
        let mut client =
            crate::services::coincube::CoincubeClient::for_test(format!("http://{addr}"));
        client.set_token("synthetic-token");
        client
    }

    /// Past Connect admission, the fork start still refuses a `daemon.toml`
    /// written for another chain — and refuses it before the daemon, its
    /// database or a managed datadir exist.
    #[tokio::test]
    async fn an_admitted_fork_session_refuses_a_daemon_toml_for_another_chain() {
        let root = temp_root("start-chain-mismatch");
        write_managed_cookie_daemon_toml(&root, ChainId::BitcoinBlake2b, ChainId::Bitcoin);
        let before = tree(root.path());
        let err = start_connect_daemon(
            root.clone(),
            ChainId::BitcoinBlake2b,
            wallet(),
            client_for(features_stub_enabling_the_fork()),
        )
        .await
        .expect_err("a foreign-chain daemon.toml must not start this Cube");
        match &err {
            Error::ChainMismatch { cube, config } => {
                assert_eq!(*cube, ChainId::BitcoinBlake2b);
                assert_eq!(*config, ChainId::Bitcoin);
            }
            other => panic!("expected the chain mismatch, got {}", other),
        }
        assert_eq!(
            tree(root.path()),
            before,
            "the refusal created or rewrote something"
        );
        assert!(!root.path().join("bitcoind-blake2b").exists());
    }

    /// The same boundary for a `daemon.toml` whose `data_directory` is some
    /// other Cube's: refused, so the fork daemon can never be pointed at a
    /// database that is not this Cube's own.
    #[tokio::test]
    async fn an_admitted_fork_session_refuses_a_foreign_data_directory() {
        let root = temp_root("start-datadir-mismatch");
        let elsewhere = temp_root("start-datadir-elsewhere");
        let dir = cube_daemon_dir(&root, ChainId::BitcoinBlake2b);
        std::fs::write(
            dir.join("daemon.toml"),
            format!(
                "data_directory = '{}'\nmain_descriptor = \"{}\"\n\n\
                 [bitcoin_config]\nnetwork = \"{}\"\n\n\
                 [bitcoind_config]\ncookie_path = '/nonexistent/.cookie'\naddr = \"127.0.0.1:1\"\n",
                elsewhere.path().display(),
                MAINNET_DESC,
                ChainId::BitcoinBlake2b,
            ),
        )
        .unwrap();
        let before = tree(root.path());
        let before_elsewhere = tree(elsewhere.path());
        let err = start_connect_daemon(
            root.clone(),
            ChainId::BitcoinBlake2b,
            wallet(),
            client_for(features_stub_enabling_the_fork()),
        )
        .await
        .expect_err("a foreign data directory must not be adopted");
        assert!(
            matches!(&err, Error::Unexpected(m) if m.as_str() == "Connect daemon datadir differs from this Cube"),
            "got {}",
            err
        );
        assert_eq!(tree(root.path()), before);
        assert_eq!(
            tree(elsewhere.path()),
            before_elsewhere,
            "the other Cube's directory was written to"
        );
    }
}

#[cfg(test)]
mod local_start_cancel_tests {
    use super::*;
    #[test]
    fn dropping_completed_local_started_message_releases_its_node_owner() {
        let path =
            std::env::temp_dir().join(format!("local-start-delivery-{}", uuid::Uuid::new_v4()));
        let root = CoincubeDirectory::new(path.clone());
        let (cfg, _) =
            crate::chain::managed_local_fixture(&root, crate::chain::ChainId::BitcoinBlake2b);
        let Some(BitcoinBackend::Bitcoind(node)) = cfg.bitcoin_backend.clone() else {
            unreachable!()
        };
        let owner = Bitcoind::local_fork_for_test(node, &root, bitcoin::Network::Bitcoin);
        let locks = crate::node::bitcoind::internal_bitcoind_datadir_for(
            &root,
            crate::node::bitcoind::NodeChainFamily::BitcoinBlake2b,
        )
        .join("locks/bitcoin");
        let other = locks.join("other-owner.lock");
        std::fs::write(&other, "").unwrap();
        let info = GetInfoResult {
            version: "synthetic".into(),
            network: bitcoin::Network::Bitcoin,
            chain_id: Some(cfg.bitcoin_config.chain),
            block_height: 0,
            sync: 1.0,
            descriptors: coincubed::commands::GetInfoDescriptors {
                main: cfg.main_descriptor.clone(),
            },
            imported_wallet: None,
            rescan_progress: None,
            refused_reorg_depth: None,
            chain_divergence: false,
            timestamp: 0,
            last_poll_timestamp: None,
            history_sync: Default::default(),
            receive_index: 0,
            change_index: 0,
        };
        let daemon: Arc<dyn Daemon + Sync + Send> =
            Arc::new(EmbeddedDaemon::unstarted_for_test(cfg, None));
        let completed = Message::Started(Ok((
            daemon,
            PendingForkNode {
                node: Some(owner),
                cleanup: true,
            },
            info,
        )));
        drop(completed);
        assert!(crate::node::bitcoind::wait_for_managed_node_cleanup(
            std::time::Duration::from_secs(120)
        ));
        assert_eq!(std::fs::read_dir(&locks).unwrap().count(), 1);
        assert!(other.exists());
        std::fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn canceled_local_start_releases_only_its_managed_node_owner() {
        let path =
            std::env::temp_dir().join(format!("local-start-cancel-{}", uuid::Uuid::new_v4()));
        let root = CoincubeDirectory::new(path.clone());
        let (cfg, _) =
            crate::chain::managed_local_fixture(&root, crate::chain::ChainId::BitcoinBlake2b);
        let Some(BitcoinBackend::Bitcoind(node)) = cfg.bitcoin_backend else {
            unreachable!()
        };
        let owner = Bitcoind::local_fork_for_test(node, &root, bitcoin::Network::Bitcoin);
        let locks = crate::node::bitcoind::internal_bitcoind_datadir_for(
            &root,
            crate::node::bitcoind::NodeChainFamily::BitcoinBlake2b,
        )
        .join("locks/bitcoin");
        let other = locks.join("other-owner.lock");
        std::fs::write(&other, "").unwrap();
        let (ready, received) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let _owned = PendingForkNode {
                node: Some(owner),
                cleanup: true,
            };
            let _ = ready.send(());
            std::future::pending::<()>().await;
        });
        received.await.unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(crate::node::bitcoind::wait_for_managed_node_cleanup(
            std::time::Duration::from_secs(120)
        ));
        assert_eq!(std::fs::read_dir(&locks).unwrap().count(), 1);
        assert!(other.exists());
        std::fs::remove_dir_all(path).unwrap();
    }
}

#[cfg(test)]
mod local_fork_resync_tests {
    //! Resyncing a pruned Bitcoin Blake2b Cube whose wallet runs on its local
    //! node: it moves to Connect with the node pending, then its loader
    //! reopens it there; or it is refused with nothing deleted or changed.
    //! Real wallet settings and daemon.toml (`managed_local_fixture`), a real
    //! managed-node layout and a mocked Connect API; no node is started.
    use super::*;
    use crate::chain::ChainId;
    use crate::services::coincube::CoincubeClient;
    use httpmock::prelude::*;

    const CHAIN: ChainId = ChainId::BitcoinBlake2bTestnet4;
    struct Fixture {
        root: CoincubeDirectory,
        wallet: WalletSettings,
        node: coincubed::config::BitcoindConfig,
        daemon_toml: PathBuf,
        blocks: PathBuf,
        rpc_port: u16,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(self.root.path());
        }
    }

    /// A local-backend Cube with chain data, its node on a port nothing
    /// listens on (the fixture's fixed port may be taken on a shared host),
    /// and the Bitcoin Blake2b beta enabled.
    fn local_cube(tag: &str) -> Fixture {
        use crate::node::bitcoind::{
            internal_bitcoind_config_path, internal_bitcoind_datadir_for, InternalBitcoindConfig,
            NodeChainFamily,
        };
        let path = std::env::temp_dir().join(format!(
            "coincube-local-resync-{tag}-{}",
            uuid::Uuid::new_v4()
        ));
        let root = CoincubeDirectory::new(path);
        let (mut cfg, wallet) = crate::chain::managed_local_fixture(&root, CHAIN);
        let rpc_port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let datadir = internal_bitcoind_datadir_for(&root, NodeChainFamily::BitcoinBlake2b);
        let conf_path = internal_bitcoind_config_path(&datadir);
        let mut managed = InternalBitcoindConfig::from_file(&conf_path).unwrap();
        managed
            .networks
            .get_mut(&CHAIN.bitcoin_network())
            .unwrap()
            .rpc_port = rpc_port;
        managed.to_ini().write_to_file(&conf_path).unwrap();
        let Some(BitcoinBackend::Bitcoind(node)) = cfg.bitcoin_backend.as_mut() else {
            panic!("the fixture is a local backend");
        };
        node.addr.set_port(rpc_port);
        let node = node.clone();
        let daemon_toml = root
            .network_directory(CHAIN)
            .coincubed_data_directory(&wallet.wallet_id())
            .path()
            .join("daemon.toml");
        std::fs::write(&daemon_toml, toml::to_string(&cfg).unwrap()).unwrap();
        assert!(crate::chain::is_managed_local_fork(&cfg, &root));
        let blocks = datadir.join("testnet4").join("blocks");
        std::fs::create_dir_all(&blocks).unwrap();
        crate::app::settings::global::GlobalSettings::update_bitcoin_blake2b_beta(
            &crate::app::settings::global::GlobalSettings::path(&root),
            true,
        )
        .unwrap();
        Fixture {
            root,
            wallet,
            node,
            daemon_toml,
            blocks,
            rpc_port,
        }
    }

    fn cube() -> CubeSettings {
        CubeSettings::new_with_raw_id("cube-uuid".to_string(), "Local".to_string(), CHAIN)
    }

    fn client(server: &MockServer) -> CoincubeClient {
        let mut client = CoincubeClient::for_test(server.base_url());
        client.set_token("synthetic-session-token");
        client
    }

    fn encryption_key() -> Arc<crate::services::connect::crypto::CubeEncryptionKey> {
        let signer =
            coincube_core::signer::MasterSigner::generate(bitcoin::Network::Testnet).unwrap();
        Arc::new(crate::services::connect::crypto::CubeEncryptionKey::derive(
            &signer,
            bitcoin::Network::Testnet,
        ))
    }

    async fn features(server: &MockServer, enabled: bool) -> httpmock::Mock<'_> {
        server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/api/v1/connect/features")
                    .header("authorization", "Bearer synthetic-session-token");
                then.status(200).json_body(serde_json::json!({
                    "data": {"plans": [], "bitcoin_blake2b_enabled": enabled}
                }));
            })
            .await
    }

    async fn anchor(server: &MockServer, available: bool) -> httpmock::Mock<'_> {
        let body = if available {
            serde_json::json!({"success": true, "data": {
            "network": "bitcoin-blake2b-testnet4", "state": "available", "anchor": {
                "tip_hash": "11".repeat(32), "tip_height": 973029,
                "tip_median_time_past": 1800000000,
                "observed_at": std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(),
                "observation": {"tip_height": 973029,
                    "fork": {"height": 972000, "active": true},
                    "rdts": {"state": "flagday", "flagday": {
                        "height": 972000, "expiry_time": 1800010000_i64, "active": false}}}
            }}})
        } else {
            serde_json::json!({"success": false,
                "error": {"code": "SERVICE_UNAVAILABLE"},
                "data": {"network": "bitcoin-blake2b-testnet4", "state": "syncing"}})
        };
        server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/api/v1/connect/networks/bitcoin-blake2b-testnet4/anchor");
                then.status(if available { 200 } else { 503 })
                    .json_body(body);
            })
            .await
    }

    fn no_cancel() -> Arc<std::sync::atomic::AtomicBool> {
        Arc::default()
    }

    fn pruned() -> Error {
        Error::Bitcoind(StartInternalBitcoindError::PrunedRevalidationRequired(
            CHAIN,
        ))
    }

    // The whole recovery: the pruned page's resync moves the Cube to Connect
    // with its node pending and adopted, deletes the chain data, and the
    // result reopens the Cube on Connect with the session it carried.
    #[tokio::test]
    async fn a_local_backend_cube_resyncs_through_connect_and_reopens_there() {
        let fx = local_cube("reopen");
        let server = MockServer::start_async().await;
        let enabled = features(&server, true).await;
        let available = anchor(&server, true).await;
        let session = client(&server);

        let (mut loader, startup) = Loader::new_local_fork(
            fx.root.clone(),
            GUIConfig::new(false),
            fx.wallet.clone(),
            cube(),
            encryption_key(),
            Some(session.clone()),
        );
        drop(startup); // Never start a node or daemon here.
        assert!(!loader.requires_connect());
        loader.step = Step::Error(Box::new(pruned()));
        let task = loader.update(Message::View(ViewMessage::ResyncForkNode));
        assert!(task.units() > 0);
        drop(task); // The same future, run directly below.
        assert!(matches!(loader.step, Step::Connecting));
        let token = loader.fork_resync.expect("resync in flight");

        resync_fork_cube(
            fx.root.clone(),
            CHAIN,
            Some(fx.wallet.clone()),
            loader.connect_client.clone(),
            loader.fork_resync_cancel.clone(),
        )
        .await
        .unwrap();
        enabled.assert_hits_async(1).await;
        available.assert_hits_async(1).await;
        assert!(!fx.blocks.exists());

        // On Connect, the node pending and adopted, no token on disk.
        let raw = std::fs::read_to_string(&fx.daemon_toml).unwrap();
        assert!(!raw.contains("synthetic-session-token"));
        let cfg = Config::from_file(Some(fx.daemon_toml.clone())).unwrap();
        assert!(!crate::chain::is_managed_local_fork(&cfg, &fx.root));
        match &cfg.bitcoin_backend {
            Some(BitcoinBackend::Esplora(esplora)) => {
                assert_eq!(
                    esplora.addr,
                    crate::installer::connect_esplora_config(CHAIN, "").addr
                );
                assert!(esplora.token.is_none());
            }
            other => panic!("not on Connect: {:?}", other),
        }
        assert_eq!(cfg.pending_bitcoind, Some(fx.node.clone()));
        assert_eq!(cfg.auto_switch_to_pending, Some(true));
        assert!(cfg.fallback_esplora.is_none());
        assert_eq!(
            cfg.bitcoin_config.poll_interval_secs,
            Duration::from_secs(coincubed::config::ESPLORA_POLL_INTERVAL_SECS)
        );
        assert_eq!(cfg.bitcoin_config.chain, CHAIN);
        assert!(cfg.pending_managed_node_install.is_none());

        // The result reopens it on Connect, admitted with the carried session.
        let task = loader.update(Message::ForkNodeResynced {
            token,
            result: Ok(()),
        });
        assert!(task.units() > 0, "the reopen must start");
        drop(task);
        assert!(matches!(loader.step, Step::Connecting));
        assert!(loader.requires_connect());
        assert!(!loader.fork_session_revoked);
        assert_eq!(
            loader.connect_client.as_ref().and_then(|c| c.token()),
            Some("synthetic-session-token")
        );
        assert!(loader.cube_encryption_key.is_some());

        // Without that session the Cube would not reopen: why it is required
        // before anything is deleted.
        let (bare, startup) = Loader::new_local_fork(
            fx.root.clone(),
            GUIConfig::new(false),
            fx.wallet.clone(),
            cube(),
            encryption_key(),
            None,
        );
        assert_eq!(startup.units(), 0, "refused before anything starts");
        assert!(
            matches!(&bare.step, Step::Error(_)),
            "a Connect Cube without a session must be refused"
        );
    }

    // Every way the recovery can be refused leaves the chain data and
    // daemon.toml exactly as they were.
    #[tokio::test]
    async fn a_local_backend_resync_is_refused_before_anything_changes() {
        let fx = local_cube("refused");
        let server = MockServer::start_async().await;
        let before = std::fs::read(&fx.daemon_toml).unwrap();
        let unchanged = |fx: &Fixture| {
            assert!(fx.blocks.exists());
            assert_eq!(std::fs::read(&fx.daemon_toml).unwrap(), before);
        };
        let wallet = Some(fx.wallet.clone());
        let never = |_: ChainId, _: CoincubeClient, _: CoincubeDirectory| async {
            panic!("Connect checked without a session")
        };
        let ok = |_: ChainId, _: CoincubeClient, _: CoincubeDirectory| async { Ok(()) };

        // No session, or one without a token: refused, not offered again.
        for session in [None, Some(CoincubeClient::for_test(server.base_url()))] {
            let refused = resync_fork_cube_with(
                fx.root.clone(),
                CHAIN,
                wallet.clone(),
                session,
                no_cancel(),
                never,
            )
            .await
            .unwrap_err();
            assert!(
                matches!(&refused, ForkResyncError::Refused(e)
                    if e.contains("Nothing was deleted") && e.contains("Sign in to Connect")),
                "{:?}",
                refused
            );
            unchanged(&fx);
        }

        // Connect would not admit the reopen: the chain is not enabled for
        // the account, or its anchor is unavailable. Refused, nothing deleted.
        let disabled = features(&server, false).await;
        let refused = resync_fork_cube(
            fx.root.clone(),
            CHAIN,
            wallet.clone(),
            Some(client(&server)),
            no_cancel(),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(&refused, ForkResyncError::Refused(e)
                if e.contains("isn't enabled") && e.contains("Nothing was deleted")),
            "{:?}",
            refused
        );
        disabled.assert_hits_async(1).await;
        disabled.delete_async().await;
        unchanged(&fx);
        let enabled = features(&server, true).await;
        let syncing = anchor(&server, false).await;
        let refused = resync_fork_cube(
            fx.root.clone(),
            CHAIN,
            wallet.clone(),
            Some(client(&server)),
            no_cancel(),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(&refused, ForkResyncError::Refused(e)
                if e.contains("not available") && e.contains("Nothing was deleted")),
            "{:?}",
            refused
        );
        syncing.assert_hits_async(1).await;
        enabled.delete_async().await;
        syncing.delete_async().await;
        unchanged(&fx);

        // The node still answers: the resync is refused before daemon.toml
        // is moved, not after.
        let listener = std::net::TcpListener::bind(("127.0.0.1", fx.rpc_port)).unwrap();
        let failed = resync_fork_cube_with(
            fx.root.clone(),
            CHAIN,
            wallet.clone(),
            Some(client(&server)),
            no_cancel(),
            ok,
        )
        .await
        .unwrap_err();
        assert!(
            matches!(&failed, ForkResyncError::Failed(e) if e.contains("RPC port")),
            "{:?}",
            failed
        );
        drop(listener);
        unchanged(&fx);

        // The session was cleared (or the loader closed) during the check.
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let failed = resync_fork_cube_with(
            fx.root.clone(),
            CHAIN,
            wallet.clone(),
            Some(client(&server)),
            cancel,
            ok,
        )
        .await
        .unwrap_err();
        assert!(
            matches!(&failed, ForkResyncError::Failed(e) if e.contains("session changed")),
            "{:?}",
            failed
        );
        unchanged(&fx);

        // daemon.toml changed after it was read: not overwritten, nothing
        // deleted.
        let path = fx.daemon_toml.clone();
        let edited = format!("{}\n", String::from_utf8(before.clone()).unwrap());
        let edit = edited.clone();
        let failed = resync_fork_cube_with(
            fx.root.clone(),
            CHAIN,
            wallet.clone(),
            Some(client(&server)),
            no_cancel(),
            move |_: ChainId, _: CoincubeClient, _: CoincubeDirectory| async move {
                std::fs::write(&path, edit).unwrap();
                Ok(())
            },
        )
        .await
        .unwrap_err();
        assert!(
            matches!(&failed, ForkResyncError::Failed(e) if e.contains("changed")),
            "{:?}",
            failed
        );
        assert!(fx.blocks.exists());
        assert_eq!(std::fs::read_to_string(&fx.daemon_toml).unwrap(), edited);
        std::fs::write(&fx.daemon_toml, &before).unwrap();

        // A node backend that is not the managed one (here: another port)
        // fails closed rather than being deleted without the move.
        let mut cfg = Config::from_file(Some(fx.daemon_toml.clone())).unwrap();
        let Some(BitcoinBackend::Bitcoind(node)) = cfg.bitcoin_backend.as_mut() else {
            panic!("local backend");
        };
        node.addr.set_port(fx.rpc_port.wrapping_add(1));
        std::fs::write(&fx.daemon_toml, toml::to_string(&cfg).unwrap()).unwrap();
        let other = std::fs::read(&fx.daemon_toml).unwrap();
        let refused = resync_fork_cube_with(
            fx.root.clone(),
            CHAIN,
            wallet.clone(),
            Some(client(&server)),
            no_cancel(),
            never,
        )
        .await
        .unwrap_err();
        assert!(
            matches!(&refused, ForkResyncError::Refused(e) if e.contains("not the managed")),
            "{:?}",
            refused
        );
        assert!(fx.blocks.exists());
        assert_eq!(std::fs::read(&fx.daemon_toml).unwrap(), other);
    }

    // A Cube already on Connect with the node pending needs no session and
    // no Connect check: its chain data is deleted, its configuration untouched.
    #[tokio::test]
    async fn a_connect_cube_with_the_node_pending_is_resynced_untouched() {
        let fx = local_cube("pending");
        let cfg = Config::from_file(Some(fx.daemon_toml.clone())).unwrap();
        let mut pending = connect_demoted_config(&cfg).unwrap();
        pending.auto_switch_to_pending = Some(false);
        std::fs::write(&fx.daemon_toml, toml::to_string(&pending).unwrap()).unwrap();
        let before = std::fs::read(&fx.daemon_toml).unwrap();
        resync_fork_cube_with(
            fx.root.clone(),
            CHAIN,
            Some(fx.wallet.clone()),
            None,
            no_cancel(),
            |_: ChainId, _: CoincubeClient, _: CoincubeDirectory| async {
                panic!("a Connect Cube needs no reopen check")
            },
        )
        .await
        .unwrap();
        assert!(!fx.blocks.exists());
        assert_eq!(std::fs::read(&fx.daemon_toml).unwrap(), before);
    }

    // A sign-in change while the resync checks Connect: the local loader
    // drops its session and the resync changes nothing; a result arriving
    // once the Cube is on Connect without a session asks for sign-in rather
    // than rebuilding into a misleading refusal.
    #[tokio::test]
    async fn a_sign_in_change_during_the_check_stops_the_resync() {
        let fx = local_cube("sign-in-change");
        let server = MockServer::start_async().await;
        let before = std::fs::read(&fx.daemon_toml).unwrap();
        let (mut loader, startup) = Loader::new_local_fork(
            fx.root.clone(),
            GUIConfig::new(false),
            fx.wallet.clone(),
            cube(),
            encryption_key(),
            Some(client(&server)),
        );
        drop(startup);
        loader.step = Step::Error(Box::new(pruned()));
        drop(loader.update(Message::View(ViewMessage::ResyncForkNode)));
        let token = loader.fork_resync.expect("resync in flight");
        let session = loader.connect_client.clone();
        let cancel = loader.fork_resync_cancel.clone();
        let mut tab = crate::gui::tab::Tab::new(1, crate::gui::tab::State::Loader(loader));
        drop(tab.invalidate_fork_session(crate::gui::tab::AuthChange::LogOut, false));
        let failed = resync_fork_cube_with(
            fx.root.clone(),
            CHAIN,
            Some(fx.wallet.clone()),
            session.clone(),
            cancel,
            |_: ChainId, _: CoincubeClient, _: CoincubeDirectory| async { Ok(()) },
        )
        .await
        .unwrap_err();
        assert!(
            matches!(&failed, ForkResyncError::Failed(_)),
            "{:?}",
            failed
        );
        assert!(fx.blocks.exists());
        assert_eq!(std::fs::read(&fx.daemon_toml).unwrap(), before);

        // Had the move already happened, the result asks for sign-in.
        let crate::gui::tab::State::Loader(loader) = &mut tab.state else {
            panic!("a local loader stays open across sign-in changes");
        };
        assert!(loader.connect_client.is_none());
        resync_fork_cube_with(
            fx.root.clone(),
            CHAIN,
            Some(fx.wallet.clone()),
            session,
            no_cancel(),
            |_: ChainId, _: CoincubeClient, _: CoincubeDirectory| async { Ok(()) },
        )
        .await
        .unwrap();
        let task = loader.update(Message::ForkNodeResynced {
            token,
            result: Ok(()),
        });
        assert_eq!(task.units(), 0);
        assert!(matches!(&loader.step, Step::Error(e)
            if e.to_string().contains("Sign in to Connect from Home, then reopen this Cube")));
    }

    // A sign-out once the Cube is on Connect revokes the loader like any
    // Connect loader's: the resync's late result does not reopen it, and the
    // Cube opens on Connect after the next sign-in.
    #[tokio::test]
    async fn a_sign_out_after_the_move_to_connect_revokes_the_reopen() {
        let fx = local_cube("sign-out");
        let server = MockServer::start_async().await;
        let _enabled = features(&server, true).await;
        let _available = anchor(&server, true).await;
        let (mut loader, startup) = Loader::new_local_fork(
            fx.root.clone(),
            GUIConfig::new(false),
            fx.wallet.clone(),
            cube(),
            encryption_key(),
            Some(client(&server)),
        );
        drop(startup);
        loader.step = Step::Error(Box::new(pruned()));
        drop(loader.update(Message::View(ViewMessage::ResyncForkNode)));
        let token = loader.fork_resync.expect("resync in flight");
        resync_fork_cube(
            fx.root.clone(),
            CHAIN,
            Some(fx.wallet.clone()),
            loader.connect_client.clone(),
            loader.fork_resync_cancel.clone(),
        )
        .await
        .unwrap();

        let mut tab = crate::gui::tab::Tab::new(1, crate::gui::tab::State::Loader(loader));
        drop(tab.invalidate_fork_session(crate::gui::tab::AuthChange::LogOut, false));
        let crate::gui::tab::State::Loader(loader) = &mut tab.state else {
            panic!("the loader stays, refused");
        };
        assert!(loader.fork_revoked());
        assert!(loader.connect_client.is_none());
        let task = loader.update(Message::ForkNodeResynced {
            token,
            result: Ok(()),
        });
        assert_eq!(task.units(), 0);
        assert!(
            matches!(&loader.step, Step::Error(e) if e.to_string().contains("reopen this Cube"))
        );
        // Still on Connect: the next open goes through Connect sign-in.
        let cfg = Config::from_file(Some(fx.daemon_toml.clone())).unwrap();
        assert!(matches!(
            cfg.bitcoin_backend,
            Some(BitcoinBackend::Esplora(_))
        ));
        assert!(cfg.pending_bitcoind.is_some());
    }
}
