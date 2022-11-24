mod config;
mod message;
mod step;
mod view;

use iced::{clipboard, Command, Element, Subscription};
use iced_native::{window, Event};
use liana::miniscript::bitcoin;

use std::convert::TryInto;
use std::io::Write;
use std::path::PathBuf;

use crate::{
    app::config as gui_config, hw::HardwareWalletConfig, installer::config::DEFAULT_FILE_NAME,
};

pub use message::Message;
use step::{
    Context, DefineBitcoind, DefineDescriptor, Final, ImportDescriptor, RegisterDescriptor, Step,
    Welcome,
};

pub struct Installer {
    should_exit: bool,
    current: usize,
    steps: Vec<Box<dyn Step>>,

    /// Context is data passed through each step.
    context: Context,
}

impl Installer {
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
                steps: vec![Welcome::default().into()],
                context: Context::new(network, destination_path),
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

    fn next(&mut self) -> Command<Message> {
        let current_step = self
            .steps
            .get_mut(self.current)
            .expect("There is always a step");
        if current_step.apply(&mut self.context) {
            if self.current < self.steps.len() - 1 {
                self.current += 1;
            }
            // skip the step according to the current context.
            while self
                .steps
                .get(self.current)
                .expect("There is always a step")
                .skip(&self.context)
            {
                if self.current < self.steps.len() - 1 {
                    self.current += 1;
                }
            }
            // calculate new current_step.
            let current_step = self
                .steps
                .get_mut(self.current)
                .expect("There is always a step");
            current_step.load_context(&self.context);
            return current_step.load();
        }
        Command::none()
    }

    pub fn update(&mut self, message: Message) -> Command<Message> {
        match message {
            Message::CreateWallet => {
                self.steps = vec![
                    Welcome::default().into(),
                    DefineDescriptor::new().into(),
                    RegisterDescriptor::default().into(),
                    DefineBitcoind::new().into(),
                    Final::new().into(),
                ];
                self.next()
            }
            Message::ImportWallet => {
                self.steps = vec![
                    Welcome::default().into(),
                    ImportDescriptor::new().into(),
                    RegisterDescriptor::default().into(),
                    DefineBitcoind::new().into(),
                    Final::new().into(),
                ];
                self.next()
            }
            Message::Clibpboard(s) => clipboard::write(s),
            Message::Next => self.next(),
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
            Message::Installed(Err(e)) => {
                let mut data_dir = self.context.data_dir.clone();
                data_dir.push(self.context.bitcoin_config.network.to_string());
                // In case of failure during install, block the thread to
                // deleted the data_dir/network directory in order to start clean again.
                std::fs::remove_dir_all(data_dir).expect("Correctly deleted");
                self.steps
                    .get_mut(self.current)
                    .expect("There is always a step")
                    .update(Message::Installed(Err(e)));
                Command::none()
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
    let hardware_wallets = ctx
        .hw_tokens
        .iter()
        .map(|(kind, fingerprint, token)| HardwareWalletConfig::new(kind, fingerprint, token))
        .collect();

    let mut cfg: liana::config::Config = ctx
        .try_into()
        .expect("Everything should be checked at this point");
    // Start Daemon to check correctness of installation
    let daemon = liana::DaemonHandle::start_default(cfg.clone()).map_err(|e| {
        Error::Unexpected(format!("Failed to start daemon with entered config: {}", e))
    })?;
    daemon.shutdown();

    cfg.data_dir =
        Some(cfg.data_dir.unwrap().canonicalize().map_err(|e| {
            Error::Unexpected(format!("Failed to canonicalize datadir path: {}", e))
        })?);

    let mut datadir_path = cfg.data_dir.clone().unwrap();
    datadir_path.push(cfg.bitcoin_config.network.to_string());

    // create lianad configuration file
    let mut daemon_config_path = datadir_path.clone();
    daemon_config_path.push(DEFAULT_FILE_NAME);
    let mut daemon_config_file = std::fs::File::create(&daemon_config_path)
        .map_err(|e| Error::CannotCreateFile(e.to_string()))?;

    // Step needed because of ValueAfterTable error in the toml serialize implementation.
    let daemon_config =
        toml::Value::try_from(&cfg).expect("daemon::Config has a proper Serialize implementation");

    daemon_config_file
        .write_all(daemon_config.to_string().as_bytes())
        .map_err(|e| Error::CannotWriteToFile(e.to_string()))?;

    // create liana GUI configuration file
    let mut gui_config_path = datadir_path;
    gui_config_path.push(gui_config::DEFAULT_FILE_NAME);
    let mut gui_config_file = std::fs::File::create(&gui_config_path)
        .map_err(|e| Error::CannotCreateFile(e.to_string()))?;

    gui_config_file
        .write_all(
            toml::to_string(&gui_config::Config::new(
                daemon_config_path.canonicalize().map_err(|e| {
                    Error::Unexpected(format!("Failed to canonicalize daemon config path: {}", e))
                })?,
                hardware_wallets,
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
