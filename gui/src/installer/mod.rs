mod context;
mod message;
mod prompt;
mod step;
mod view;

use iced::{clipboard, Command, Subscription};
use liana::miniscript::bitcoin;
use liana_ui::widget::Element;
use tracing::{error, info, warn};

use context::Context;

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::{
    app::config::InternalBitcoindExeConfig,
    app::{config as gui_config, settings as gui_settings},
    bitcoind::stop_internal_bitcoind,
    signer::Signer,
};

pub use message::Message;
use step::{
    BackupDescriptor, BackupMnemonic, DefineBitcoind, DefineDescriptor, Final, ImportDescriptor,
    InternalBitcoindStep, ParticipateXpub, RecoverMnemonic, RegisterDescriptor,
    SelectBitcoindTypeStep, Step, Welcome,
};

pub struct Installer {
    current: usize,
    steps: Vec<Box<dyn Step>>,
    signer: Arc<Mutex<Signer>>,

    /// Context is data passed through each step.
    context: Context,
}

impl Installer {
    fn previous(&mut self) {
        if self.current > 0 {
            self.current -= 1;
        }
        // skip the previous step according to the current context.
        while self.current > 0
            && self
                .steps
                .get(self.current)
                .expect("There is always a step")
                .skip(&self.context)
        {
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
                signer: Arc::new(Mutex::new(Signer::generate(network).unwrap())),
            },
            Command::none(),
        )
    }

    pub fn subscription(&self) -> Subscription<Message> {
        Subscription::none()
    }

    pub fn stop(&mut self) {
        // Use current step's `stop()` method for any changes not yet written to context.
        self.steps
            .get_mut(self.current)
            .expect("There is always a step")
            .stop();
        // Now use context to determine what to stop.
        if self.context.internal_bitcoind_config.is_some() {
            if let Some(bitcoind_config) = &self.context.bitcoind_config {
                stop_internal_bitcoind(bitcoind_config);
            }
        }
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
        let hot_signer_fingerprint = self.signer.lock().unwrap().fingerprint();
        match message {
            Message::CreateWallet => {
                self.steps = vec![
                    Welcome::default().into(),
                    DefineDescriptor::new(self.signer.clone()).into(),
                    BackupMnemonic::new(self.signer.clone()).into(),
                    BackupDescriptor::default().into(),
                    RegisterDescriptor::new_create_wallet().into(),
                    SelectBitcoindTypeStep::new().into(),
                    InternalBitcoindStep::new(&self.context.data_dir).into(),
                    DefineBitcoind::new().into(),
                    Final::new(hot_signer_fingerprint).into(),
                ];
                self.next()
            }
            Message::ParticipateWallet => {
                self.steps = vec![
                    Welcome::default().into(),
                    ParticipateXpub::new(self.signer.clone()).into(),
                    ImportDescriptor::new(false).into(),
                    BackupMnemonic::new(self.signer.clone()).into(),
                    BackupDescriptor::default().into(),
                    RegisterDescriptor::new_import_wallet().into(),
                    SelectBitcoindTypeStep::new().into(),
                    InternalBitcoindStep::new(&self.context.data_dir).into(),
                    DefineBitcoind::new().into(),
                    Final::new(hot_signer_fingerprint).into(),
                ];
                self.next()
            }
            Message::ImportWallet => {
                self.steps = vec![
                    Welcome::default().into(),
                    ImportDescriptor::new(true).into(),
                    RecoverMnemonic::default().into(),
                    RegisterDescriptor::new_import_wallet().into(),
                    SelectBitcoindTypeStep::new().into(),
                    InternalBitcoindStep::new(&self.context.data_dir).into(),
                    DefineBitcoind::new().into(),
                    Final::new(hot_signer_fingerprint).into(),
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
                let _cmd = self
                    .steps
                    .get_mut(self.current)
                    .expect("There is always a step")
                    .update(message);
                Command::perform(
                    install(self.context.clone(), self.signer.clone()),
                    Message::Installed,
                )
            }
            Message::Installed(Err(e)) => {
                let mut data_dir = self.context.data_dir.clone();
                data_dir.push(self.context.bitcoin_config.network.to_string());
                // In case of failure during install, block the thread to
                // deleted the data_dir/network directory in order to start clean again.
                warn!("Installation failed. Cleaning up the leftover data directory.");
                if let Err(e) = std::fs::remove_dir_all(&data_dir) {
                    error!(
                        "Failed to completely delete the data directory (path: '{}'): {}",
                        data_dir.to_string_lossy(),
                        e
                    );
                } else {
                    warn!(
                        "Successfully deleted data directory at '{}'.",
                        data_dir.to_string_lossy()
                    );
                };
                self.steps
                    .get_mut(self.current)
                    .expect("There is always a step")
                    .update(Message::Installed(Err(e)))
            }
            _ => self
                .steps
                .get_mut(self.current)
                .expect("There is always a step")
                .update(message),
        }
    }

    /// Some steps are skipped because of contextual choice of the user, this
    /// code is giving a correct progress summary to the user.
    fn progress(&self) -> (usize, usize) {
        let mut current = self.current;
        let mut total = 0;
        for (i, step) in self.steps.iter().enumerate() {
            if step.skip(&self.context) {
                if i < self.current {
                    current -= 1;
                }
            } else {
                total += 1
            }
        }
        (current, total - 1)
    }

    pub fn view(&self) -> Element<Message> {
        self.steps
            .get(self.current)
            .expect("There is always a step")
            .view(self.progress())
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

/// Data directory used by internal bitcoind.
pub fn internal_bitcoind_datadir(liana_datadir: &PathBuf) -> PathBuf {
    let mut datadir = PathBuf::from(liana_datadir);
    datadir.push("bitcoind_datadir");
    datadir
}

pub async fn install(ctx: Context, signer: Arc<Mutex<Signer>>) -> Result<PathBuf, Error> {
    let mut cfg: liana::config::Config = ctx.extract_daemon_config();
    let data_dir = cfg.data_dir.unwrap();

    let data_dir = data_dir
        .canonicalize()
        .map_err(|e| Error::Unexpected(format!("Failed to canonicalize datadir path: {}", e)))?;
    cfg.data_dir = Some(data_dir.clone());

    daemon_check(cfg.clone())?;

    info!("daemon checked");

    let mut network_datadir_path = data_dir;
    network_datadir_path.push(cfg.bitcoin_config.network.to_string());

    // Step needed because of ValueAfterTable error in the toml serialize implementation.
    let daemon_config = toml::Value::try_from(&cfg)
        .map_err(|e| Error::Unexpected(format!("Failed to serialize daemon config: {}", e)))?;

    // create lianad configuration file
    let daemon_config_path = create_and_write_file(
        network_datadir_path.clone(),
        "daemon.toml",
        daemon_config.to_string().as_bytes(),
    )?;

    info!("Daemon configuration file created");

    if cfg
        .main_descriptor
        .to_string()
        .contains(&signer.lock().unwrap().fingerprint().to_string())
    {
        signer
            .lock()
            .unwrap()
            .store(
                &cfg.data_dir().expect("Already checked"),
                cfg.bitcoin_config.network,
            )
            .map_err(|e| Error::Unexpected(format!("Failed to store mnemonic: {}", e)))?;

        info!("Hot signer mnemonic stored");
    }

    if let Some(signer) = &ctx.recovered_signer {
        signer
            .store(
                &cfg.data_dir().expect("Already checked"),
                cfg.bitcoin_config.network,
            )
            .map_err(|e| Error::Unexpected(format!("Failed to store mnemonic: {}", e)))?;

        info!("Recovered signer mnemonic stored");
    }

    // create liana GUI configuration file
    let gui_config_path = create_and_write_file(
        network_datadir_path.clone(),
        gui_config::DEFAULT_FILE_NAME,
        toml::to_string(&gui_config::Config::new(
            daemon_config_path.canonicalize().map_err(|e| {
                Error::Unexpected(format!("Failed to canonicalize daemon config path: {}", e))
            })?,
            ctx.internal_bitcoind_exe_config.clone(),
        ))
        .map_err(|e| Error::Unexpected(format!("Failed to serialize gui config: {}", e)))?
        .as_bytes(),
    )?;

    info!("Gui configuration file created");

    // create liana GUI settings file
    let settings: gui_settings::Settings = ctx.extract_gui_settings();
    create_and_write_file(
        network_datadir_path,
        gui_settings::DEFAULT_FILE_NAME,
        serde_json::to_string_pretty(&settings)
            .map_err(|e| Error::Unexpected(format!("Failed to serialize settings: {}", e)))?
            .as_bytes(),
    )?;

    info!("Settings file created");

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
    Bitcoind(String),
    CannotCreateDatadir(String),
    CannotCreateFile(String),
    CannotWriteToFile(String),
    CannotGetAvailablePort(String),
    Unexpected(String),
    HardwareWallet(async_hwi::Error),
}

impl From<jsonrpc::simple_http::Error> for Error {
    fn from(error: jsonrpc::simple_http::Error) -> Self {
        Error::Bitcoind(error.to_string())
    }
}

impl From<jsonrpc::Error> for Error {
    fn from(error: jsonrpc::Error) -> Self {
        Error::Bitcoind(error.to_string())
    }
}

impl From<async_hwi::Error> for Error {
    fn from(error: async_hwi::Error) -> Self {
        Error::HardwareWallet(error)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Bitcoind(e) => write!(f, "Failed to ping bitcoind: {}", e),
            Self::CannotCreateDatadir(e) => write!(f, "Failed to create datadir: {}", e),
            Self::CannotGetAvailablePort(e) => write!(f, "Failed to get available port: {}", e),
            Self::CannotWriteToFile(e) => write!(f, "Failed to write to file: {}", e),
            Self::CannotCreateFile(e) => write!(f, "Failed to create file: {}", e),
            Self::Unexpected(e) => write!(f, "Unexpected: {}", e),
            Self::HardwareWallet(e) => write!(f, "Hardware Wallet: {}", e),
        }
    }
}
