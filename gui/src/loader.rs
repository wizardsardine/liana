use std::convert::From;
use std::fs::File;
use std::io::{BufRead, BufReader, ErrorKind, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use iced::{Alignment, Command, Length, Subscription};
use tokio::runtime::Handle;
use tracing::{debug, info, warn};

use liana::{
    commands::CoinStatus,
    config::{BitcoinBackend, Config, ConfigError},
    miniscript::bitcoin,
    StartupError,
};
use liana_ui::{
    color,
    component::{button, notification, text::*},
    icon,
    widget::*,
};

use crate::daemon::DaemonBackend;
use crate::{
    app::{
        cache::Cache,
        config::Config as GUIConfig,
        wallet::{Wallet, WalletError},
    },
    daemon::{client, embedded::EmbeddedDaemon, model::*, Daemon, DaemonError},
    node::bitcoind::{
        internal_bitcoind_debug_log_path, stop_bitcoind, Bitcoind, StartInternalBitcoindError,
    },
};

const SYNCING_PROGRESS_1: &str = "Bitcoin Core is synchronising the blockchain. A full synchronisation typically takes a few days and is resource-intensive. Once the initial synchronisation is done, the next ones will be much faster.";
const SYNCING_PROGRESS_2: &str = "Bitcoin Core is synchronising the blockchain. This will take a while, depending on the last time it was done, your internet connection, and your computer performance.";
const SYNCING_PROGRESS_3: &str = "Bitcoin Core is synchronising the blockchain. This may take a few minutes, depending on the last time it was done, your internet connection, and your computer performance.";

type Lianad = client::Lianad<client::jsonrpc::JsonRPCClient>;

pub struct Loader {
    pub datadir_path: PathBuf,
    pub network: bitcoin::Network,
    pub gui_config: GUIConfig,
    pub daemon_started: bool,
    pub internal_bitcoind: Option<Bitcoind>,
    pub waiting_daemon_bitcoind: bool,

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
            ),
            Error,
        >,
    ),
    Started(Result<(Arc<dyn Daemon + Sync + Send>, Option<Bitcoind>), Error>),
    Loaded(Result<Arc<dyn Daemon + Sync + Send>, Error>),
    BitcoindLog(Option<String>),
    Failure(DaemonError),
    None,
}

impl Loader {
    pub fn new(
        datadir_path: PathBuf,
        gui_config: GUIConfig,
        network: bitcoin::Network,
        internal_bitcoind: Option<Bitcoind>,
    ) -> (Self, Command<Message>) {
        let path = gui_config
            .daemon_rpc_path
            .clone()
            .unwrap_or_else(|| socket_path(&datadir_path, network));
        (
            Loader {
                network,
                datadir_path,
                gui_config,
                step: Step::Connecting,
                daemon_started: false,
                internal_bitcoind,
                waiting_daemon_bitcoind: false,
            },
            Command::perform(connect(path), Message::Loaded),
        )
    }

    fn on_load(&mut self, res: Result<Arc<dyn Daemon + Sync + Send>, Error>) -> Command<Message> {
        match res {
            Ok(daemon) => {
                self.step = Step::Syncing {
                    daemon: daemon.clone(),
                    progress: 0.0,
                    bitcoind_logs: String::new(),
                };
                if self.gui_config.start_internal_bitcoind {
                    warn!("Lianad is external, gui will not start internal bitcoind");
                }
                return Command::perform(sync(daemon, false), Message::Syncing);
            }
            Err(e) => match e {
                Error::Config(_) => {
                    self.step = Step::Error(Box::new(e));
                }
                Error::Daemon(DaemonError::ClientNotSupported)
                | Error::Daemon(DaemonError::RpcSocket(Some(ErrorKind::ConnectionRefused), _))
                | Error::Daemon(DaemonError::RpcSocket(Some(ErrorKind::NotFound), _)) => {
                    if let Some(daemon_config_path) = self.gui_config.daemon_config_path.clone() {
                        self.step = Step::StartingDaemon;
                        self.daemon_started = true;
                        self.waiting_daemon_bitcoind = true;
                        return Command::perform(
                            start_bitcoind_and_daemon(
                                daemon_config_path,
                                self.datadir_path.clone(),
                                self.gui_config.start_internal_bitcoind
                                    && self.internal_bitcoind.is_none(),
                            ),
                            Message::Started,
                        );
                    } else {
                        self.step = Step::Error(Box::new(e));
                    }
                }
                _ => {
                    self.step = Step::Error(Box::new(e));
                }
            },
        }
        Command::none()
    }

    fn on_log(&mut self, log: Option<String>) -> Command<Message> {
        if let Step::Syncing { bitcoind_logs, .. } = &mut self.step {
            if let Some(l) = log {
                *bitcoind_logs = l;
            }
        }
        Command::none()
    }

    fn on_start(
        &mut self,
        res: Result<(Arc<dyn Daemon + Sync + Send>, Option<Bitcoind>), Error>,
    ) -> Command<Message> {
        match res {
            Ok((daemon, bitcoind)) => {
                // bitcoind may have been already started and given to the loader
                // We should not override with None the loader bitcoind field
                if let Some(bitcoind) = bitcoind {
                    self.internal_bitcoind = Some(bitcoind);
                }
                self.waiting_daemon_bitcoind = false;
                self.step = Step::Syncing {
                    daemon: daemon.clone(),
                    progress: 0.0,
                    bitcoind_logs: String::new(),
                };
                Command::perform(sync(daemon, false), Message::Syncing)
            }
            Err(e) => {
                self.step = Step::Error(Box::new(e));
                Command::none()
            }
        }
    }

    fn on_sync(&mut self, res: Result<GetInfoResult, DaemonError>) -> Command<Message> {
        match &mut self.step {
            Step::Syncing {
                daemon, progress, ..
            } => {
                match res {
                    Ok(info) => {
                        if (info.sync - 1.0_f64).abs() < f64::EPSILON {
                            return Command::perform(
                                load_application(
                                    daemon.clone(),
                                    info,
                                    self.datadir_path.clone(),
                                    self.network,
                                    self.internal_bitcoind.clone(),
                                ),
                                Message::Synced,
                            );
                        } else {
                            *progress = info.sync
                        }
                    }
                    Err(e) => {
                        self.step = Step::Error(Box::new(e.into()));
                        return Command::none();
                    }
                };
                Command::perform(sync(daemon.clone(), true), Message::Syncing)
            }
            _ => Command::none(),
        }
    }

    pub fn stop(&mut self) {
        info!("Close requested");
        if let Step::Syncing { daemon, .. } = &mut self.step {
            if daemon.backend() == DaemonBackend::EmbeddedLianad {
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
            log::info!("Stopping managed bitcoind..");
            bitcoind.stop();
            log::info!("Managed bitcoind stopped.");
        } else if self.waiting_daemon_bitcoind && self.gui_config.start_internal_bitcoind {
            if let Ok(config) = Config::from_file(self.gui_config.daemon_config_path.clone()) {
                if let Some(BitcoinBackend::Bitcoind(bitcoind_config)) = &config.bitcoin_backend {
                    let mut retry = 0;
                    while !stop_bitcoind(bitcoind_config) && retry < 10 {
                        std::thread::sleep(std::time::Duration::from_millis(500));
                        retry += 1;
                    }
                }
            }
        }
    }

    pub fn update(&mut self, message: Message) -> Command<Message> {
        match message {
            Message::View(ViewMessage::Retry) => {
                let (loader, cmd) = Self::new(
                    self.datadir_path.clone(),
                    self.gui_config.clone(),
                    self.network,
                    self.internal_bitcoind.clone(),
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
                Command::none()
            }
            Message::Failure(_) => {
                self.daemon_started = false;
                Command::none()
            }
            Message::None => Command::none(),
            _ => Command::none(),
        }
    }

    pub fn subscription(&self) -> Subscription<Message> {
        if self.internal_bitcoind.is_some() {
            let log_path = internal_bitcoind_debug_log_path(&self.datadir_path, self.network);
            iced::subscription::unfold(0, log_path, move |log_path| async move {
                // Reduce the io load.
                tokio::time::sleep(Duration::from_millis(500)).await;

                // Open the log file and seek to its end, with some breathing room to make sure
                // we don't skip all "UpdateTip" lines. This is to avoid making BufReader read
                // the whole file every single time below.
                let mut file = match File::open(&log_path) {
                    Ok(file) => file,
                    Err(e) => {
                        log::warn!("Opening bitcoind log file: {}", e);
                        return (Message::None, log_path);
                    }
                };
                match file.metadata() {
                    Ok(m) => {
                        let file_len = m.len();
                        let offset = 1024 * 1024;
                        if file_len > offset {
                            if let Err(e) =
                                file.seek(SeekFrom::Start(file_len.saturating_sub(offset)))
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
                    Some(Ok(line)) => (Message::BitcoindLog(Some(line)), log_path),
                    res => {
                        if let Some(Err(e)) = res {
                            log::error!("Reading bitcoind log file: {}", e);
                        } else {
                            log::warn!("Couldn't find an UpdateTip line in bitcoind log file.");
                        }
                        (Message::None, log_path)
                    }
                }
            })
        } else {
            Subscription::none()
        }
    }

    pub fn view(&self) -> Element<Message> {
        view(&self.step).map(Message::View)
    }
}

pub async fn load_application(
    daemon: Arc<dyn Daemon + Sync + Send>,
    info: GetInfoResult,
    datadir_path: PathBuf,
    network: bitcoin::Network,
    internal_bitcoind: Option<Bitcoind>,
) -> Result<
    (
        Arc<Wallet>,
        Cache,
        Arc<dyn Daemon + Sync + Send>,
        Option<Bitcoind>,
    ),
    Error,
> {
    let wallet = Wallet::new(info.descriptors.main)
        .load_from_settings(&datadir_path, network)?
        .load_hotsigners(&datadir_path, network)?;

    let coins = daemon
        .list_coins(&[CoinStatus::Unconfirmed, CoinStatus::Confirmed], &[])
        .await
        .map(|res| res.coins)?;

    let cache = Cache {
        datadir_path,
        network: info.network,
        blockheight: info.block_height,
        coins,
        ..Default::default()
    };

    Ok((Arc::new(wallet), cache, daemon, internal_bitcoind))
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
                .push(p2_regular(bitcoind_logs).style(color::GREY_3)),
        ),
        Step::Error(error) => cover(
            Some(("Error while starting the internal daemon", error)),
            Column::new()
                .spacing(20)
                .width(Length::Fill)
                .align_items(Alignment::Center)
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
                            button::border(None, "Use another Bitcoin network")
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
                .width(iced::Length::Fill)
                .height(iced::Length::Fill)
                .center_x()
                .center_y()
                .padding(50),
        )
        .into()
}

async fn connect(socket_path: PathBuf) -> Result<Arc<dyn Daemon + Sync + Send>, Error> {
    let client = client::jsonrpc::JsonRPCClient::new(socket_path);
    let daemon = Lianad::new(client);

    debug!("Searching for external daemon");
    daemon.get_info().await?;
    info!("Connected to external daemon");

    Ok(Arc::new(daemon))
}

// Daemon can start only if a config path is given.
pub async fn start_bitcoind_and_daemon(
    config_path: PathBuf,
    liana_datadir_path: PathBuf,
    start_internal_bitcoind: bool,
) -> Result<(Arc<dyn Daemon + Sync + Send>, Option<Bitcoind>), Error> {
    let config = Config::from_file(Some(config_path)).map_err(Error::Config)?;
    let mut bitcoind: Option<Bitcoind> = None;
    if start_internal_bitcoind {
        if let Some(BitcoinBackend::Bitcoind(bitcoind_config)) = &config.bitcoin_backend {
            // Check if bitcoind is already running before trying to start it.
            if liana::BitcoinD::new(bitcoind_config, "internal_bitcoind_start".to_string()).is_ok()
            {
                info!("Internal bitcoind is already running");
            } else {
                info!("Starting internal bitcoind");
                bitcoind = Some(
                    Bitcoind::start(
                        &config.bitcoin_config.network,
                        bitcoind_config.clone(),
                        &liana_datadir_path,
                    )
                    .map_err(Error::Bitcoind)?,
                );
            }
        }
    }

    debug!("starting liana daemon");

    let daemon = EmbeddedDaemon::start(config)?;

    Ok((Arc::new(daemon), bitcoind))
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
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Config(e) => write!(f, "Config error: {}", e),
            Self::Wallet(e) => write!(f, "Wallet error: {}", e),
            Self::Daemon(e) => write!(f, "Liana daemon error: {}", e),
            Self::Bitcoind(e) => write!(f, "Bitcoind error: {}", e),
            Self::BitcoindLogs(e) => write!(f, "Bitcoind logs error: {}", e),
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

/// default lianad socket path is .liana/bitcoin/lianad_rpc
fn socket_path(datadir: &Path, network: bitcoin::Network) -> PathBuf {
    let mut path = datadir.to_path_buf();
    path.push(network.to_string());
    path.push("lianad_rpc");
    path
}
