use std::convert::From;
use std::fs::File;
use std::io::{BufRead, BufReader, ErrorKind, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use iced::futures::{SinkExt, Stream};
use iced::stream::channel;
use iced::{Alignment, Length, Subscription, Task};
use tokio::runtime::Handle;
use tracing::{debug, info, warn};

use liana::miniscript::bitcoin;
use liana_ui::{
    component::{button, notification, text::*},
    icon, theme,
    widget::*,
};
use lianad::{
    config::{BitcoinBackend, Config, ConfigError},
    StartupError,
};

use crate::app;
use crate::app::settings::WalletSettings;
use crate::backup::Backup;
use crate::dir::LianaDirectory;
use crate::export::RestoreBackupError;
use crate::{
    app::{
        cache::{coins_to_cache, Cache},
        config::Config as GUIConfig,
        wallet::{Wallet, WalletError},
    },
    daemon::{client, embedded::EmbeddedDaemon, model::*, Daemon, DaemonError},
    node::bitcoind::{internal_bitcoind_debug_log_path, Bitcoind, StartInternalBitcoindError},
};

const SYNCING_PROGRESS_1: &str = "Bitcoin Core is synchronising the blockchain. A full synchronisation typically takes a few days and is resource-intensive. Once the initial synchronisation is done, the next ones will be much faster.";
const SYNCING_PROGRESS_2: &str = "Bitcoin Core is synchronising the blockchain. This will take a while, depending on the last time it was done, your internet connection, and your computer performance.";
const SYNCING_PROGRESS_3: &str = "Bitcoin Core is synchronising the blockchain. This may take a few minutes, depending on the last time it was done, your internet connection, and your computer performance.";

type Lianad = client::Lianad<client::jsonrpc::JsonRPCClient>;
type StartedResult = Result<
    (
        Arc<dyn Daemon + Sync + Send>,
        Option<Bitcoind>,
        GetInfoResult,
    ),
    Error,
>;

pub struct Loader {
    pub datadir_path: LianaDirectory,
    pub network: bitcoin::Network,
    pub gui_config: GUIConfig,
    pub daemon_started: bool,
    pub internal_bitcoind: Option<Bitcoind>,
    pub waiting_daemon_bitcoind: bool,
    pub backup: Option<Backup>,
    pub wallet_settings: WalletSettings,
    step: Step,
}

pub enum Step {
    Connecting,
    StartingDaemon,
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
                LianaDirectory,
                Option<Bitcoind>,
            ),
            Error,
        >,
        /* restored_from_backup */ bool,
    ),
    Started(StartedResult),
    Loaded(Result<(Arc<dyn Daemon + Sync + Send>, GetInfoResult), Error>),
    BitcoindLog(Option<String>),
    Failure(DaemonError),
    None,
}

impl Loader {
    pub fn new(
        datadir_path: LianaDirectory,
        gui_config: GUIConfig,
        network: bitcoin::Network,
        internal_bitcoind: Option<Bitcoind>,
        backup: Option<Backup>,
        wallet_settings: WalletSettings,
    ) -> (Self, Task<Message>) {
        let socket_path = datadir_path
            .network_directory(network)
            .lianad_data_directory(&wallet_settings.wallet_id())
            .lianad_rpc_socket_path();
        (
            Loader {
                network,
                datadir_path,
                gui_config,
                step: Step::Connecting,
                daemon_started: false,
                internal_bitcoind,
                waiting_daemon_bitcoind: false,
                wallet_settings,
                backup,
            },
            Task::perform(connect(socket_path), Message::Loaded),
        )
    }

    fn start_bitcoind(&self) -> bool {
        if self.internal_bitcoind.is_some() {
            false
        } else if let Some(start) = self.wallet_settings.start_internal_bitcoind {
            start
        } else {
            self.gui_config.start_internal_bitcoind
        }
    }

    fn maybe_skip_syncing(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        info: GetInfoResult,
    ) -> Task<Message> {
        // If the wallet was previously synced (blockheight > 0), load the
        // application directly.
        if info.block_height > 0 {
            return Task::perform(
                load_application(
                    self.wallet_settings.clone(),
                    daemon,
                    info,
                    self.datadir_path.clone(),
                    self.network,
                    self.internal_bitcoind.clone(),
                    self.backup.clone(),
                ),
                Message::Synced,
            );
        }
        // Otherwise, show the sync progress on the loading screen.
        self.step = Step::Syncing {
            daemon: daemon.clone(),
            progress: 0.0,
            bitcoind_logs: String::new(),
        };
        Task::perform(sync(daemon, false), Message::Syncing)
    }

    fn on_load(
        &mut self,
        res: Result<(Arc<dyn Daemon + Sync + Send>, GetInfoResult), Error>,
    ) -> Task<Message> {
        match res {
            Ok((daemon, info)) => {
                if self.gui_config.start_internal_bitcoind {
                    warn!("Lianad is external, gui will not start internal bitcoind");
                }
                return self.maybe_skip_syncing(daemon, info);
            }
            Err(e) => match e {
                Error::Config(_) => {
                    self.step = Step::Error(Box::new(e));
                }
                Error::Daemon(DaemonError::ClientNotSupported)
                | Error::Daemon(DaemonError::RpcSocket(Some(ErrorKind::ConnectionRefused), _))
                | Error::Daemon(DaemonError::RpcSocket(Some(ErrorKind::NotFound), _)) => {
                    self.step = Step::StartingDaemon;
                    self.daemon_started = true;
                    self.waiting_daemon_bitcoind = true;
                    return Task::perform(
                        start_bitcoind_and_daemon(
                            self.datadir_path.clone(),
                            self.start_bitcoind(),
                            self.network,
                            self.wallet_settings.clone(),
                        ),
                        Message::Started,
                    );
                }
                _ => {
                    self.step = Step::Error(Box::new(e));
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
        match res {
            Ok((daemon, bitcoind, info)) => {
                // bitcoind may have been already started and given to the loader
                // We should not override with None the loader bitcoind field
                if let Some(bitcoind) = bitcoind {
                    self.internal_bitcoind = Some(bitcoind);
                }
                self.waiting_daemon_bitcoind = false;
                self.maybe_skip_syncing(daemon, info)
            }
            Err(e) => {
                self.step = Step::Error(Box::new(e));
                Task::none()
            }
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
                            return Task::perform(
                                load_application(
                                    self.wallet_settings.clone(),
                                    daemon.clone(),
                                    info,
                                    self.datadir_path.clone(),
                                    self.network,
                                    self.internal_bitcoind.clone(),
                                    self.backup.clone(),
                                ),
                                Message::Synced,
                            );
                        } else {
                            *progress = info.sync
                        }
                    }
                    Err(e) => {
                        self.step = Step::Error(Box::new(e.into()));
                        return Task::none();
                    }
                };
                Task::perform(sync(daemon.clone(), true), Message::Syncing)
            }
            _ => Task::none(),
        }
    }

    pub fn stop(&mut self) {
        info!("Close requested");
        if let Step::Syncing { daemon, .. } = &mut self.step {
            if daemon.backend().is_embedded() {
                info!("Stopping internal daemon...");
                if let Err(e) = Handle::current().block_on(async { daemon.stop().await }) {
                    warn!("Internal daemon failed to stop: {}", e);
                } else {
                    info!("Internal daemon stopped");
                }
            }
        }

        // NOTE: we take() the internal_bitcoind here to make sure the debug.log reader
        // subscription is dropped.
        if let Some(bitcoind) = self.internal_bitcoind.take() {
            bitcoind.stop();
        }
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::View(ViewMessage::Retry) => {
                let (loader, cmd) = Self::new(
                    self.datadir_path.clone(),
                    self.gui_config.clone(),
                    self.network,
                    self.internal_bitcoind.clone(),
                    self.backup.clone(),
                    self.wallet_settings.clone(),
                );
                *self = loader;
                cmd
            }
            Message::Started(res) => self.on_start(res),
            Message::Loaded(res) => self.on_load(res),
            Message::Syncing(res) => self.on_sync(res),
            Message::BitcoindLog(log) => self.on_log(log),
            Message::Synced(Err(e)) => {
                self.step = Step::Error(Box::new(e));
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
        if self.internal_bitcoind.is_some() {
            let log_path = internal_bitcoind_debug_log_path(&self.datadir_path, self.network);
            iced::Subscription::run_with_id("bitcoind_log", get_bitcoind_log(log_path))
                .map(Message::BitcoindLog)
        } else {
            Subscription::none()
        }
    }

    pub fn view(&self) -> Element<Message> {
        view(&self.step).map(Message::View)
    }
}

fn get_bitcoind_log(log_path: PathBuf) -> impl Stream<Item = Option<String>> {
    channel(5, move |mut output| async move {
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
            match file.metadata() {
                Ok(m) => {
                    let file_len = m.len();
                    let offset = 1024 * 1024;
                    if file_len > offset {
                        if let Err(e) = file.seek(SeekFrom::Start(file_len.saturating_sub(offset)))
                        {
                            log::error!("Seeking to end of bitcoind log file: {}", e);
                        }
                    }
                }
                Err(e) => {
                    log::error!("Getting bitcoind log file metadata: {}", e);
                }
            };

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

pub async fn load_application(
    wallet_settings: WalletSettings,
    daemon: Arc<dyn Daemon + Sync + Send>,
    info: GetInfoResult,
    datadir_path: LianaDirectory,
    network: bitcoin::Network,
    internal_bitcoind: Option<Bitcoind>,
    backup: Option<Backup>,
) -> Result<
    (
        Arc<Wallet>,
        Cache,
        Arc<dyn Daemon + Sync + Send>,
        Option<Bitcoind>,
        Option<Backup>,
    ),
    Error,
> {
    let wallet = Wallet::new(info.descriptors.main)
        .load_from_settings(wallet_settings)?
        .load_hotsigners(&datadir_path, network)?;

    let coins = coins_to_cache(daemon.clone()).await.map(|res| res.coins)?;

    let cache = Cache {
        datadir_path,
        network: info.network,
        blockheight: info.block_height,
        coins,
        sync_progress: info.sync,
        // Both last poll fields start with the same value.
        last_poll_timestamp: info.last_poll_timestamp,
        last_poll_at_startup: info.last_poll_timestamp,
        ..Default::default()
    };

    Ok((Arc::new(wallet), cache, daemon, internal_bitcoind, backup))
}

#[derive(Clone, Debug)]
pub enum ViewMessage {
    Retry,
    SwitchNetwork,
}

pub fn view(step: &Step) -> Element<ViewMessage> {
    match &step {
        Step::StartingDaemon => cover(
            None,
            Column::new()
                .width(Length::Fill)
                .push(ProgressBar::new(0.0..=1.0, 0.0).width(Length::Fill))
                .push(text("Starting daemon...")),
        ),
        Step::Connecting => cover(
            None,
            Column::new()
                .width(Length::Fill)
                .push(ProgressBar::new(0.0..=1.0, 0.0).width(Length::Fill))
                .push(text("Connecting to daemon...")),
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
                .push(text(format!("Progress {:.2}%", 100.0 * *progress)))
                .push(ProgressBar::new(0.0..=1.0, *progress as f32).width(Length::Fill))
                .push(text(if *progress > 0.98 {
                    SYNCING_PROGRESS_3
                } else if *progress > 0.9 {
                    SYNCING_PROGRESS_2
                } else {
                    SYNCING_PROGRESS_1
                }))
                .push(p2_regular(bitcoind_logs).style(theme::text::secondary)),
        ),
        Step::Error(error) => cover(
            Some(("Error while starting the internal daemon", error)),
            Column::new()
                .spacing(20)
                .width(Length::Fill)
                .align_x(Alignment::Center)
                .push(icon::plug_icon().size(100).width(Length::Fixed(300.0)))
                .push(
                    if matches!(
                        error.as_ref(),
                        Error::Daemon(DaemonError::Start(StartupError::Bitcoind(_)))
                    ) {
                        text("Liana failed to start, please check if bitcoind is running")
                    } else {
                        text("Liana failed to start")
                    },
                )
                .push(
                    Row::new()
                        .spacing(10)
                        .push(
                            button::secondary(None, "Use another Bitcoin network")
                                .on_press(ViewMessage::SwitchNetwork),
                        )
                        .push(
                            button::secondary(None, "Retry")
                                .width(Length::Fixed(200.0))
                                .on_press(ViewMessage::Retry),
                        ),
                ),
        ),
    }
}

pub fn cover<'a, T: 'a + Clone, C: Into<Element<'a, T>>>(
    warn: Option<(&'static str, &Error)>,
    content: C,
) -> Element<'a, T> {
    Column::new()
        .push_maybe(warn.map(|w| notification::warning(w.0.to_string(), w.1.to_string())))
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
    let daemon = Lianad::new(client);

    debug!("Searching for external daemon");
    let info = daemon.get_info().await?;
    info!("Connected to external daemon");

    Ok((Arc::new(daemon), info))
}

// Daemon can start only if a config path is given.
pub async fn start_bitcoind_and_daemon(
    liana_datadir_path: LianaDirectory,
    start_internal_bitcoind: bool,
    network: bitcoin::Network,
    settings: WalletSettings,
) -> StartedResult {
    let mut config_path = liana_datadir_path
        .network_directory(network)
        .lianad_data_directory(&settings.wallet_id())
        .path()
        .to_path_buf();
    config_path.push("daemon.toml");
    let config = Config::from_file(Some(config_path)).map_err(Error::Config)?;
    let bitcoind = match (start_internal_bitcoind, &config.bitcoin_backend) {
        (true, Some(BitcoinBackend::Bitcoind(bitcoind_config))) => Some(
            Bitcoind::maybe_start(
                config.bitcoin_config.network,
                bitcoind_config.clone(),
                &liana_datadir_path,
            )
            .map_err(Error::Bitcoind)?,
        ),
        _ => None,
    };

    debug!("starting liana daemon");

    let daemon = EmbeddedDaemon::start(config)?;
    let info = daemon.get_info().await?;

    Ok((Arc::new(daemon), bitcoind, info))
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

#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum Error {
    Wallet(WalletError),
    Config(ConfigError),
    Daemon(DaemonError),
    Bitcoind(StartInternalBitcoindError),
    BitcoindLogs(std::io::Error),
    RestoreBackup(RestoreBackupError),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Config(e) => write!(f, "Config error: {}", e),
            Self::Wallet(e) => write!(f, "Wallet error: {}", e),
            Self::Daemon(e) => write!(f, "Liana daemon error: {}", e),
            Self::Bitcoind(e) => write!(f, "Bitcoind error: {}", e),
            Self::BitcoindLogs(e) => write!(f, "Bitcoind logs error: {}", e),
            Self::RestoreBackup(e) => write!(f, "Restore backup: {e}"),
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
