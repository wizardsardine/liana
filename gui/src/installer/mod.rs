mod context;
mod message;
mod prompt;
mod step;
mod view;

use iced::{clipboard, Command, Element, Subscription};
use liana::miniscript::bitcoin;

use context::Context;
use std::io::Write;
use std::path::PathBuf;

use crate::app::{config as gui_config, settings as gui_settings};

pub use message::Message;
use step::{
    BackupDescriptor, DefineBitcoind, DefineDescriptor, Final, ImportDescriptor, ParticipateXpub,
    RegisterDescriptor, Step, Welcome,
};

pub struct Installer {
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
                current: 0,
                steps: vec![Welcome::default().into()],
                context: Context::new(network, destination_path),
            },
            Command::none(),
        )
    }

    pub fn subscription(&self) -> Subscription<Message> {
        Subscription::none()
    }

    pub fn stop(&mut self) {}

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
                    BackupDescriptor::default().into(),
                    RegisterDescriptor::default().into(),
                    DefineBitcoind::new().into(),
                    Final::new().into(),
                ];
                self.next()
            }
            Message::ParticipateWallet => {
                self.steps = vec![
                    Welcome::default().into(),
                    ParticipateXpub::new().into(),
                    ImportDescriptor::new(false).into(),
                    BackupDescriptor::default().into(),
                    RegisterDescriptor::default().into(),
                    DefineBitcoind::new().into(),
                    Final::new().into(),
                ];
                self.next()
            }
            Message::ImportWallet => {
                self.steps = vec![
                    Welcome::default().into(),
                    ImportDescriptor::new(true).into(),
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
                log::warn!("Installation failed. Cleaning up the leftover data directory.");
                if let Err(e) = std::fs::remove_dir_all(&data_dir) {
                    log::error!(
                        "Failed to completely delete the data directory (path: '{}'): {}",
                        data_dir.to_string_lossy(),
                        e
                    );
                } else {
                    log::warn!(
                        "Successfully deleted data directory at '{}'.",
                        data_dir.to_string_lossy()
                    );
                };
                self.steps
                    .get_mut(self.current)
                    .expect("There is always a step")
                    .update(Message::Installed(Err(e)));
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
            .view((self.current, self.steps.len() - 1))
    }
}

pub fn daemon_check(cfg: liana::config::Config) -> Result<(), Error> {
    // Start Daemon to check correctness of installation
    match liana::DaemonHandle::start_default(cfg) {
        Ok(daemon) => {
            daemon.shutdown();
            Ok(())
        }
        Err(e) => Err(Error::Unexpected(format!(
            "Failed to start Liana daemon: {}",
            e
        ))),
    }
}

pub async fn install(ctx: Context) -> Result<PathBuf, Error> {
    log::info!("installing");
    let mut cfg: liana::config::Config = ctx.extract_daemon_config();
    daemon_check(cfg.clone())?;
    log::info!("daemon checked");

    cfg.data_dir =
        Some(cfg.data_dir.unwrap().canonicalize().map_err(|e| {
            Error::Unexpected(format!("Failed to canonicalize datadir path: {}", e))
        })?);

    let mut datadir_path = cfg.data_dir.clone().unwrap();
    datadir_path.push(cfg.bitcoin_config.network.to_string());

    // Step needed because of ValueAfterTable error in the toml serialize implementation.
    let daemon_config = toml::Value::try_from(&cfg)
        .map_err(|e| Error::Unexpected(format!("Failed to serialize daemon config: {}", e)))?;

    // create lianad configuration file
    let daemon_config_path = create_and_write_file(
        datadir_path.clone(),
        "daemon.toml",
        daemon_config.to_string().as_bytes(),
    )?;

    log::info!("Daemon config file created");

    // create liana GUI configuration file
    let gui_config_path = create_and_write_file(
        datadir_path.clone(),
        gui_config::DEFAULT_FILE_NAME,
        toml::to_string(&gui_config::Config::new(
            daemon_config_path.canonicalize().map_err(|e| {
                Error::Unexpected(format!("Failed to canonicalize daemon config path: {}", e))
            })?,
        ))
        .map_err(|e| Error::Unexpected(format!("Failed to serialize gui config: {}", e)))?
        .as_bytes(),
    )?;

    log::info!("Gui config file created");

    // create liana GUI settings file
    let settings: gui_settings::Settings = ctx.extract_gui_settings();
    create_and_write_file(
        datadir_path,
        gui_settings::DEFAULT_FILE_NAME,
        serde_json::to_string_pretty(&settings)
            .map_err(|e| Error::Unexpected(format!("Failed to serialize settings: {}", e)))?
            .as_bytes(),
    )?;

    log::info!("Settings file created");

    Ok(gui_config_path)
}

pub fn create_and_write_file(
    mut network_datadir: PathBuf,
    file_name: &str,
    data: &[u8],
) -> Result<PathBuf, Error> {
    network_datadir.push(file_name);
    let path = network_datadir;
    let mut file =
        std::fs::File::create(&path).map_err(|e| Error::CannotCreateFile(e.to_string()))?;
    file.write_all(data)
        .map_err(|e| Error::CannotWriteToFile(e.to_string()))?;
    Ok(path)
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
