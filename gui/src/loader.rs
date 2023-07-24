use std::convert::From;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use iced::{Alignment, Command, Length, Subscription};
use tracing::{debug, info, warn};

use liana::{
    config::{Config, ConfigError},
    miniscript::bitcoin,
    StartupError,
};
use liana_ui::{
    component::{button, notification, text::*},
    icon,
    util::Collection,
    widget::*,
};

use crate::{
    app::{
        cache::Cache,
        config::{Config as GUIConfig, InternalBitcoindExeConfig},
        wallet::{Wallet, WalletError},
    },
    bitcoind::{start_internal_bitcoind, stop_internal_bitcoind, StartInternalBitcoindError},
    daemon::{client, embedded::EmbeddedDaemon, model::*, Daemon, DaemonError},
    utils,
};

type Lianad = client::Lianad<client::jsonrpc::JsonRPCClient>;

pub struct Loader {
    pub datadir_path: PathBuf,
    pub network: bitcoin::Network,
    pub gui_config: GUIConfig,
    pub daemon_started: bool,

    step: Step,
}

pub enum Step {
    Connecting,
    StartingDaemon,
    Syncing {
        daemon: Arc<dyn Daemon + Sync + Send>,
        progress: f64,
    },
    Error(Box<Error>),
}

#[allow(clippy::type_complexity)]
#[derive(Debug)]
pub enum Message {
    View(ViewMessage),
    Syncing(Result<GetInfoResult, DaemonError>),
    Synced(Result<(Arc<Wallet>, Cache, Arc<dyn Daemon + Sync + Send>), Error>),
    Started(Result<Arc<dyn Daemon + Sync + Send>, Error>),
    Loaded(Result<Arc<dyn Daemon + Sync + Send>, Error>),
    Failure(DaemonError),
}

impl Loader {
    pub fn new(
        datadir_path: PathBuf,
        gui_config: GUIConfig,
        network: bitcoin::Network,
    ) -> (Self, Command<Message>) {
        let path = gui_config
            .daemon_rpc_path
            .clone()
            .unwrap_or_else(|| socket_path(&datadir_path, network));
        let network = network;
        (
            Loader {
                network,
                datadir_path,
                gui_config,
                step: Step::Connecting,
                daemon_started: false,
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
                };
                if self.gui_config.internal_bitcoind_exe_config.is_some() {
                    warn!("Ignoring internal bitcoind config because Liana daemon is external.");
                }
                return Command::perform(sync(daemon, false), Message::Syncing);
            }
            Err(e) => match e {
                Error::Config(_) => {
                    self.step = Step::Error(Box::new(e));
                }
                Error::Daemon(DaemonError::ClientNotSupported)
                | Error::Daemon(DaemonError::Transport(Some(ErrorKind::ConnectionRefused), _))
                | Error::Daemon(DaemonError::Transport(Some(ErrorKind::NotFound), _)) => {
                    if let Some(daemon_config_path) = self.gui_config.daemon_config_path.clone() {
                        self.step = Step::StartingDaemon;
                        self.daemon_started = true;
                        return Command::perform(
                            start_bitcoind_and_daemon(
                                daemon_config_path,
                                self.gui_config.internal_bitcoind_exe_config.clone(),
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

    fn on_start(&mut self, res: Result<Arc<dyn Daemon + Sync + Send>, Error>) -> Command<Message> {
        match res {
            Ok(daemon) => {
                self.step = Step::Syncing {
                    daemon: daemon.clone(),
                    progress: 0.0,
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
                                    self.gui_config.clone(),
                                    self.datadir_path.clone(),
                                    self.network,
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
            if !daemon.is_external() {
                info!("Stopping internal daemon...");
                daemon.stop();
                info!("Internal daemon stopped");
                if self.gui_config.internal_bitcoind_exe_config.is_some() {
                    if let Some(daemon_config) = daemon.config() {
                        if let Some(bitcoind_config) = &daemon_config.bitcoind_config {
                            stop_internal_bitcoind(bitcoind_config);
                        }
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
                );
                *self = loader;
                cmd
            }
            Message::Started(res) => self.on_start(res),
            Message::Loaded(res) => self.on_load(res),
            Message::Syncing(res) => self.on_sync(res),
            Message::Synced(Err(e)) => {
                self.step = Step::Error(Box::new(e));
                Command::none()
            }
            Message::Failure(_) => {
                self.daemon_started = false;
                Command::none()
            }
            _ => Command::none(),
        }
    }

    pub fn subscription(&self) -> Subscription<Message> {
        Subscription::none()
    }

    pub fn view(&self) -> Element<Message> {
        view(&self.step).map(Message::View)
    }
}

pub async fn load_application(
    daemon: Arc<dyn Daemon + Sync + Send>,
    info: GetInfoResult,
    gui_config: GUIConfig,
    datadir_path: PathBuf,
    network: bitcoin::Network,
) -> Result<(Arc<Wallet>, Cache, Arc<dyn Daemon + Sync + Send>), Error> {
    let coins = daemon.list_coins().map(|res| res.coins)?;
    let spend_txs = daemon.list_spend_transactions()?;
    let cache = Cache {
        network: info.network,
        blockheight: info.block_height,
        coins,
        spend_txs,
        ..Default::default()
    };

    let wallet =
        Wallet::new(info.descriptors.main).load_settings(&gui_config, &datadir_path, network)?;

    Ok((Arc::new(wallet), cache, daemon))
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
        Step::Syncing { progress, .. } => cover(
            None,
            Column::new()
                .width(Length::Fill)
                .push(ProgressBar::new(0.0..=1.0, *progress as f32).width(Length::Fill))
                .push(text("Syncing the wallet with the blockchain...")),
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
                            button::primary(None, "Retry")
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
    daemon.get_info()?;
    info!("Connected to external daemon");

    Ok(Arc::new(daemon))
}

// Daemon can start only if a config path is given.
pub async fn start_bitcoind_and_daemon(
    config_path: PathBuf,
    bitcoind_exe_config: Option<InternalBitcoindExeConfig>,
) -> Result<Arc<dyn Daemon + Sync + Send>, Error> {
    let config = Config::from_file(Some(config_path)).map_err(Error::Config)?;
    if let Some(exe_config) = bitcoind_exe_config {
        if let Some(bitcoind_config) = &config.bitcoind_config {
            // Check if bitcoind is already running before trying to start it.
            if liana::BitcoinD::new(bitcoind_config, "internal_bitcoind_start".to_string()).is_ok()
            {
                info!("Internal bitcoind is already running");
            } else {
                info!("Starting internal bitcoind");
                start_internal_bitcoind(&config.bitcoin_config.network, exe_config)
                    .map_err(Error::Bitcoind)?;
                if !utils::poll_for_file(&bitcoind_config.cookie_path, 200, 15) {
                    return Err(Error::Bitcoind(
                        StartInternalBitcoindError::CookieFileNotFound(
                            bitcoind_config.cookie_path.to_string_lossy().into_owned(),
                        ),
                    ));
                }
                liana::BitcoinD::new(bitcoind_config, "internal_bitcoind_start".to_string())
                    .map_err(|e| {
                        Error::Bitcoind(StartInternalBitcoindError::BitcoinDError(e.to_string()))
                    })?;
            }
        }
    }

    debug!("starting liana daemon");

    let daemon = EmbeddedDaemon::start(config)?;

    Ok(Arc::new(daemon))
}

async fn sync(
    daemon: Arc<dyn Daemon + Sync + Send>,
    sleep: bool,
) -> Result<GetInfoResult, DaemonError> {
    if sleep {
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    daemon.get_info()
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum Error {
    Wallet(WalletError),
    Config(ConfigError),
    Daemon(DaemonError),
    Bitcoind(StartInternalBitcoindError),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Config(e) => write!(f, "Config error: {}", e),
            Self::Wallet(e) => write!(f, "Wallet error: {}", e),
            Self::Daemon(e) => write!(f, "Liana daemon error: {}", e),
            Self::Bitcoind(e) => write!(f, "Bitcoind error: {}", e),
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
