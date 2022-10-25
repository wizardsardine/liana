mod config;
mod message;
mod step;
mod view;

use iced::pure::Element;
use iced::{Command, Subscription};
use iced_native::{window, Event};
use minisafe::miniscript::bitcoin;

use std::convert::TryInto;
use std::io::Write;
use std::path::PathBuf;

use crate::{app::config as gui_config, installer::config::DEFAULT_FILE_NAME};

pub use message::Message;
use step::{Context, DefineBitcoind, DefineDescriptor, Final, Step, Welcome};

pub struct Installer {
    should_exit: bool,
    current: usize,
    steps: Vec<Box<dyn Step>>,

    /// Context is data passed through each step.
    context: Context,
}

impl Installer {
    fn next(&mut self) {
        if self.current < self.steps.len() - 1 {
            self.current += 1;
        }
    }

    fn previous(&mut self) {
        if self.current > 0 {
            self.current -= 1;
        }
    }

    pub fn new(
        destination_path: PathBuf,
        network: bitcoin::Network,
    ) -> (Installer, Command<Message>) {
        (
            Installer {
                should_exit: false,
                current: 0,
                steps: vec![
                    Welcome::new(network).into(),
                    DefineDescriptor::new().into(),
                    DefineBitcoind::new().into(),
                    Final::new().into(),
                ],
                context: Context::new(network, Some(destination_path)),
            },
            Command::none(),
        )
    }

    pub fn subscription(&self) -> Subscription<Message> {
        iced_native::subscription::events().map(Message::Event)
    }

    pub fn should_exit(&self) -> bool {
        self.should_exit
    }

    pub fn stop(&mut self) {
        self.should_exit = true;
    }

    pub fn update(&mut self, message: Message) -> Command<Message> {
        match message {
            Message::Next => {
                let current_step = self
                    .steps
                    .get_mut(self.current)
                    .expect("There is always a step");
                if current_step.apply(&mut self.context) {
                    self.next();
                    // skip the step according to the current context.
                    while self
                        .steps
                        .get(self.current)
                        .expect("There is always a step")
                        .skip(&self.context)
                    {
                        self.next();
                    }
                    // calculate new current_step.
                    let current_step = self
                        .steps
                        .get_mut(self.current)
                        .expect("There is always a step");
                    current_step.load_context(&self.context);
                }
                Command::none()
            }
            Message::Previous => {
                self.previous();
                Command::none()
            }
            Message::Install => {
                self.steps
                    .get_mut(self.current)
                    .expect("There is always a step")
                    .update(message);
                Command::perform(install(self.context.clone()), Message::Installed)
            }
            Message::Event(Event::Window(window::Event::CloseRequested)) => {
                self.stop();
                Command::none()
            }
            _ => self
                .steps
                .get_mut(self.current)
                .expect("There is always a step")
                .update(message),
        }
    }

    pub fn view(&self) -> Element<Message> {
        self.steps
            .get(self.current)
            .expect("There is always a step")
            .view()
    }
}

pub async fn install(ctx: Context) -> Result<PathBuf, Error> {
    let mut cfg: minisafe::config::Config = ctx
        .try_into()
        .expect("Everything should be checked at this point");
    // Start Daemon to check correctness of installation
    let daemon = minisafe::DaemonHandle::start_default(cfg.clone()).map_err(|e| {
        Error::Unexpected(format!("Failed to start daemon with entered config: {}", e))
    })?;
    daemon.shutdown();

    cfg.data_dir =
        Some(cfg.data_dir.unwrap().canonicalize().map_err(|e| {
            Error::Unexpected(format!("Failed to canonicalize datadir path: {}", e))
        })?);

    let mut datadir_path = cfg.data_dir.clone().unwrap();
    datadir_path.push(cfg.bitcoin_config.network.to_string());

    // create minisafed configuration file
    let mut minisafed_config_path = datadir_path.clone();
    minisafed_config_path.push(DEFAULT_FILE_NAME);
    let mut minisafed_config_file = std::fs::File::create(&minisafed_config_path)
        .map_err(|e| Error::CannotCreateFile(e.to_string()))?;

    // Step needed because of ValueAfterTable error in the toml serialize implementation.
    let minisafed_config =
        toml::Value::try_from(&cfg).expect("daemon::Config has a proper Serialize implementation");

    minisafed_config_file
        .write_all(minisafed_config.to_string().as_bytes())
        .map_err(|e| Error::CannotWriteToFile(e.to_string()))?;

    // create minisafe GUI configuration file
    let mut gui_config_path = datadir_path;
    gui_config_path.push(gui_config::DEFAULT_FILE_NAME);
    let mut gui_config_file = std::fs::File::create(&gui_config_path)
        .map_err(|e| Error::CannotCreateFile(e.to_string()))?;

    gui_config_file
        .write_all(
            toml::to_string(&gui_config::Config::new(
                minisafed_config_path.canonicalize().map_err(|e| {
                    Error::Unexpected(format!(
                        "Failed to canonicalize minisafed config path: {}",
                        e
                    ))
                })?,
            ))
            .unwrap()
            .as_bytes(),
        )
        .map_err(|e| Error::CannotWriteToFile(e.to_string()))?;

    Ok(gui_config_path)
}

#[derive(Debug, Clone)]
pub enum Error {
    CannotCreateDatadir(String),
    CannotCreateFile(String),
    CannotWriteToFile(String),
    Unexpected(String),
    HardwareWallet(async_hwi::Error),
}

impl From<async_hwi::Error> for Error {
    fn from(error: async_hwi::Error) -> Self {
        Error::HardwareWallet(error)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::CannotCreateDatadir(e) => write!(f, "Failed to create datadir: {}", e),
            Self::CannotWriteToFile(e) => write!(f, "Failed to write to file: {}", e),
            Self::CannotCreateFile(e) => write!(f, "Failed to create file: {}", e),
            Self::Unexpected(e) => write!(f, "Unexpected: {}", e),
            Self::HardwareWallet(e) => write!(f, "Hardware Wallet: {}", e),
        }
    }
}
