use std::convert::From;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::sync::Arc;

use iced::pure::{column, text, Element};
use iced::{Alignment, Command, Subscription};
use iced_native::{window, Event};
use log::{debug, info};

use minisafe::{
    config::{Config, ConfigError},
    miniscript::bitcoin,
};

use crate::{
    app::config::{default_datadir, Config as GUIConfig},
    daemon::{client, embedded::EmbeddedDaemon, model::*, Daemon, DaemonError},
};

type Minisafed = client::Minisafed<client::jsonrpc::JsonRPCClient>;

pub struct Loader {
    pub gui_config: GUIConfig,
    pub daemon_started: bool,

    should_exit: bool,
    step: Step,
}

enum Step {
    Connecting,
    StartingDaemon,
    Syncing {
        daemon: Arc<dyn Daemon + Sync + Send>,
        progress: f64,
    },
    Error(Box<Error>),
}

#[derive(Debug)]
pub enum Message {
    Event(iced_native::Event),
    Syncing(Result<GetInfoResult, DaemonError>),
    Synced(GetInfoResult, Vec<Coin>, Arc<dyn Daemon + Sync + Send>),
    Started(Result<Arc<dyn Daemon + Sync + Send>, Error>),
    Loaded(Result<Arc<dyn Daemon + Sync + Send>, Error>),
    Failure(DaemonError),
}

impl Loader {
    pub fn new(gui_config: GUIConfig, daemon_config: Config) -> (Self, Command<Message>) {
        let path = socket_path(
            &daemon_config.data_dir,
            daemon_config.bitcoin_config.network,
        )
        .unwrap();
        (
            Loader {
                gui_config,
                step: Step::Connecting,
                should_exit: false,
                daemon_started: false,
            },
            Command::perform(connect(path, daemon_config), Message::Loaded),
        )
    }

    fn on_load(&mut self, res: Result<Arc<dyn Daemon + Sync + Send>, Error>) -> Command<Message> {
        match res {
            Ok(daemon) => {
                self.step = Step::Syncing {
                    daemon: daemon.clone(),
                    progress: 0.0,
                };
                return Command::perform(sync(daemon, false), Message::Syncing);
            }
            Err(e) => match e {
                Error::Config(_) => {
                    self.step = Step::Error(Box::new(e));
                }
                Error::Daemon(DaemonError::Transport(Some(ErrorKind::ConnectionRefused), _))
                | Error::Daemon(DaemonError::Transport(Some(ErrorKind::NotFound), _)) => {
                    self.step = Step::StartingDaemon;
                    self.daemon_started = true;
                    return Command::perform(
                        start_daemon(self.gui_config.minisafed_config_path.clone()),
                        Message::Started,
                    );
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
            Ok(minisafed) => {
                self.step = Step::Syncing {
                    daemon: minisafed.clone(),
                    progress: 0.0,
                };
                Command::perform(sync(minisafed, false), Message::Syncing)
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
                            let daemon = daemon.clone();
                            return Command::perform(
                                async move {
                                    let coins = daemon
                                        .list_coins()
                                        .map(|res| res.coins)
                                        .unwrap_or_else(|_| Vec::new());
                                    (info, coins, daemon)
                                },
                                |res| Message::Synced(res.0, res.1, res.2),
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
        log::info!("Close requested");
        if let Step::Syncing { daemon, .. } = &mut self.step {
            if !daemon.is_external() {
                log::info!("Stopping internal daemon...");
                if let Some(d) = Arc::get_mut(daemon) {
                    d.stop().expect("Daemon is internal");
                    log::info!("Internal daemon stopped");
                    self.should_exit = true;
                }
            } else {
                self.should_exit = true;
            }
        } else {
            self.should_exit = true;
        }
    }

    pub fn update(&mut self, message: Message) -> Command<Message> {
        match message {
            Message::Started(res) => self.on_start(res),
            Message::Loaded(res) => self.on_load(res),
            Message::Syncing(res) => self.on_sync(res),
            Message::Failure(_) => {
                self.daemon_started = false;
                Command::none()
            }
            Message::Event(Event::Window(window::Event::CloseRequested)) => {
                self.stop();
                Command::none()
            }
            _ => Command::none(),
        }
    }

    pub fn subscription(&self) -> Subscription<Message> {
        iced_native::subscription::events().map(Message::Event)
    }

    pub fn should_exit(&self) -> bool {
        self.should_exit
    }

    pub fn view(&self) -> Element<Message> {
        match &self.step {
            Step::StartingDaemon => cover(text("Starting daemon...")),
            Step::Connecting => cover(text("Connecting to daemon...")),
            Step::Syncing { progress, .. } => cover(text(&format!("Syncing... {}%", progress))),
            Step::Error(error) => cover(text(&format!("Error: {}", error))),
        }
    }
}

pub fn cover<'a, T: 'a, C: Into<Element<'a, T>>>(content: C) -> Element<'a, T> {
    column()
        .push(content)
        .width(iced::Length::Fill)
        .height(iced::Length::Fill)
        .padding(50)
        .spacing(50)
        .align_items(Alignment::Center)
        .into()
}

async fn connect(
    socket_path: PathBuf,
    config: Config,
) -> Result<Arc<dyn Daemon + Sync + Send>, Error> {
    let client = client::jsonrpc::JsonRPCClient::new(socket_path);
    let minisafed = Minisafed::new(client, config);

    debug!("Searching for external daemon");
    minisafed.get_info()?;
    info!("Connected to external daemon");

    Ok(Arc::new(minisafed))
}

// Daemon can start only if a config path is given.
pub async fn start_daemon(config_path: PathBuf) -> Result<Arc<dyn Daemon + Sync + Send>, Error> {
    debug!("starting minisafe daemon");

    let config = Config::from_file(Some(config_path))
        .map_err(|e| DaemonError::Start(format!("Error parsing config: {}", e)))?;

    let mut daemon = EmbeddedDaemon::new(config);
    daemon.start()?;

    Ok(Arc::new(daemon))
}

async fn sync(
    minisafed: Arc<dyn Daemon + Sync + Send>,
    sleep: bool,
) -> Result<GetInfoResult, DaemonError> {
    if sleep {
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    minisafed.get_info()
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum Error {
    Config(ConfigError),
    Daemon(DaemonError),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Config(e) => write!(f, "Config error: {}", e),
            Self::Daemon(e) => write!(f, "Minisafed error: {}", e),
        }
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

/// default minisafed socket path is .minisafe/bitcoin/minisafed_rpc
fn socket_path(
    datadir: &Option<PathBuf>,
    network: bitcoin::Network,
) -> Result<PathBuf, ConfigError> {
    let mut path = if let Some(ref datadir) = datadir {
        datadir.clone()
    } else {
        default_datadir().map_err(|_| ConfigError::DatadirNotFound)?
    };
    path.push(network.to_string());
    path.push("minisafed_rpc");
    Ok(path)
}
