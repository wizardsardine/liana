mod context;
mod message;
mod prompt;
mod step;
mod view;

use iced::{clipboard, Command, Subscription};
use liana::{
    config::Config,
    miniscript::bitcoin::{self, Network},
};
use liana_ui::{
    component::network_banner,
    widget::{Column, Element},
};
use tracing::{error, info, warn};

use context::{Context, RemoteBackend};

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::{
    app::{
        config as gui_config, settings as gui_settings,
        settings::{AuthConfig, Settings, SettingsError, WalletSetting},
        wallet::wallet_name,
    },
    daemon::DaemonError,
    datadir::create_directory,
    hw::{HardwareWalletConfig, HardwareWallets},
    lianalite::client::{
        auth::AuthError,
        backend::{BackendClient, BackendWalletClient},
    },
    signer::Signer,
};

pub use message::Message;
use step::{
    BackupDescriptor, BackupMnemonic, ChooseBackend, DefineBitcoind, DefineDescriptor, Final,
    ImportDescriptor, ImportRemoteWallet, InternalBitcoindStep, RecoverMnemonic,
    RegisterDescriptor, SelectBitcoindTypeStep, ShareXpubs, Step, Welcome,
};

pub struct Installer {
    pub network: bitcoin::Network,
    pub datadir: PathBuf,

    current: usize,
    steps: Vec<Box<dyn Step>>,
    hws: HardwareWallets,
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
        remote_backend: Option<BackendClient>,
    ) -> (Installer, Command<Message>) {
        (
            Installer {
                network,
                datadir: destination_path.clone(),
                current: 0,
                hws: HardwareWallets::new(destination_path.clone(), network),
                steps: vec![Welcome::default().into()],
                context: Context::new(
                    network,
                    destination_path,
                    remote_backend.map(RemoteBackend::WithoutWallet),
                ),
                signer: Arc::new(Mutex::new(Signer::generate(network).unwrap())),
            },
            Command::none(),
        )
    }

    pub fn destination_path(&self) -> PathBuf {
        self.context.data_dir.clone()
    }

    pub fn subscription(&self) -> Subscription<Message> {
        if self.current > 0 {
            self.steps
                .get(self.current)
                .expect("There is always a step")
                .subscription(&self.hws)
        } else {
            Subscription::none()
        }
    }

    pub fn stop(&mut self) {
        // Use current step's `stop()` method for any changes not yet written to context.
        self.steps
            .get_mut(self.current)
            .expect("There is always a step")
            .stop();
        // Now use context to determine what to stop.
        if let Some(bitcoind) = &self.context.internal_bitcoind {
            bitcoind.stop();
        }
        self.context.internal_bitcoind = None;
    }

    fn next(&mut self) -> Command<Message> {
        let current_step = self
            .steps
            .get_mut(self.current)
            .expect("There is always a step");
        if current_step.apply(&mut self.context) {
            if self.current < self.steps.len() - 1 {
                self.current += 1;
            } else {
                // The step is already the last current step.
                // No need to reload the current step.
                return Command::none();
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
                    DefineDescriptor::new(self.network, self.signer.clone()).into(),
                    BackupMnemonic::new(self.signer.clone()).into(),
                    BackupDescriptor::default().into(),
                    RegisterDescriptor::new_create_wallet().into(),
                    ChooseBackend::new(self.network).into(),
                    SelectBitcoindTypeStep::new().into(),
                    InternalBitcoindStep::new(&self.context.data_dir).into(),
                    DefineBitcoind::new().into(),
                    Final::new().into(),
                ];
                self.next()
            }
            Message::ShareXpubs => {
                self.steps = vec![
                    Welcome::default().into(),
                    ShareXpubs::new(self.network, self.signer.clone()).into(),
                ];
                self.next()
            }
            Message::ImportWallet => {
                self.steps = vec![
                    Welcome::default().into(),
                    ChooseBackend::new(self.network).into(),
                    ImportRemoteWallet::new(self.network).into(),
                    ImportDescriptor::new(self.network).into(),
                    RecoverMnemonic::default().into(),
                    RegisterDescriptor::new_import_wallet().into(),
                    SelectBitcoindTypeStep::new().into(),
                    InternalBitcoindStep::new(&self.context.data_dir).into(),
                    DefineBitcoind::new().into(),
                    Final::new().into(),
                ];

                self.next()
            }
            Message::HardwareWallets(msg) => match self.hws.update(msg) {
                Ok(cmd) => cmd.map(Message::HardwareWallets),
                Err(e) => {
                    error!("{}", e);
                    Command::none()
                }
            },
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
                    .update(&mut self.hws, message);
                match &self.context.remote_backend {
                    Some(RemoteBackend::WithoutWallet(backend)) => Command::perform(
                        create_remote_wallet(
                            self.context.clone(),
                            self.signer.clone(),
                            backend.clone(),
                        ),
                        Message::Installed,
                    ),
                    Some(RemoteBackend::WithWallet(backend)) => Command::perform(
                        import_remote_wallet(self.context.clone(), backend.clone()),
                        Message::Installed,
                    ),
                    None => Command::perform(
                        install_local_wallet(self.context.clone(), self.signer.clone()),
                        Message::Installed,
                    ),
                }
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
                    .update(&mut self.hws, Message::Installed(Err(e)))
            }
            _ => self
                .steps
                .get_mut(self.current)
                .expect("There is always a step")
                .update(&mut self.hws, message),
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
        let content = self
            .steps
            .get(self.current)
            .expect("There is always a step")
            .view(
                &self.hws,
                self.progress(),
                self.context.remote_backend.as_ref().map(|b| b.user_email()),
            );

        if self.network != Network::Bitcoin {
            Column::with_children(vec![network_banner(self.network).into(), content]).into()
        } else {
            content
        }
    }
}

pub fn daemon_check(cfg: liana::config::Config) -> Result<(), Error> {
    // Start Daemon to check correctness of installation
    match liana::DaemonHandle::start_default(cfg) {
        Ok(daemon) => daemon
            .stop()
            .map_err(|e| Error::Unexpected(format!("Failed to stop Liana daemon: {}", e))),
        Err(e) => Err(Error::Unexpected(format!(
            "Failed to start Liana daemon: {}",
            e
        ))),
    }
}

pub async fn install_local_wallet(
    ctx: Context,
    signer: Arc<Mutex<Signer>>,
) -> Result<PathBuf, Error> {
    let mut cfg: liana::config::Config = extract_daemon_config(&ctx);
    let data_dir = cfg.data_dir.unwrap();

    let data_dir = data_dir
        .canonicalize()
        .map_err(|e| Error::Unexpected(format!("Failed to canonicalize datadir path: {}", e)))?;
    cfg.data_dir = Some(data_dir.clone());

    daemon_check(cfg.clone())?;

    info!("daemon checked");

    let mut network_datadir_path = data_dir;
    network_datadir_path.push(cfg.bitcoin_config.network.to_string());
    create_directory(&network_datadir_path)
        .map_err(|e| Error::Unexpected(format!("Failed to create datadir path: {}", e)))?;

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
            // Installer started a bitcoind, it is expected that gui will start it on on startup
            ctx.internal_bitcoind.is_some(),
        ))
        .map_err(|e| Error::Unexpected(format!("Failed to serialize gui config: {}", e)))?
        .as_bytes(),
    )?;

    info!("Gui configuration file created");

    // create liana GUI settings file
    let settings: gui_settings::Settings = extract_local_gui_settings(&ctx).await;
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

pub async fn create_remote_wallet(
    ctx: Context,
    signer: Arc<Mutex<Signer>>,
    remote_backend: BackendClient,
) -> Result<PathBuf, Error> {
    let data_dir = ctx
        .data_dir
        .canonicalize()
        .map_err(|e| Error::Unexpected(format!("Failed to canonicalize datadir path: {}", e)))?;

    let mut network_datadir_path = data_dir.clone();
    network_datadir_path.push(ctx.network.to_string());
    create_directory(&network_datadir_path)
        .map_err(|e| Error::Unexpected(format!("Failed to create datadir path: {}", e)))?;

    let descriptor = ctx
        .descriptor
        .as_ref()
        .expect("There must be a descriptor at this point");

    if descriptor
        .to_string()
        .contains(&signer.lock().unwrap().fingerprint().to_string())
    {
        signer
            .lock()
            .unwrap()
            .store(&data_dir, ctx.network)
            .map_err(|e| Error::Unexpected(format!("Failed to store mnemonic: {}", e)))?;

        info!("Hot signer mnemonic stored");
    }

    if let Some(signer) = &ctx.recovered_signer {
        signer
            .store(&data_dir, ctx.network)
            .map_err(|e| Error::Unexpected(format!("Failed to store mnemonic: {}", e)))?;

        info!("Recovered signer mnemonic stored");
    }

    let mut network_datadir_path = data_dir;
    network_datadir_path.push(ctx.network.to_string());

    // create liana GUI configuration file
    let gui_config_path = create_and_write_file(
        network_datadir_path.clone(),
        gui_config::DEFAULT_FILE_NAME,
        toml::to_string(&gui_config::Config {
            daemon_config_path: None,
            daemon_rpc_path: None,
            log_level: Some("info".to_string()),
            debug: Some(false),
            start_internal_bitcoind: false,
        })
        .map_err(|e| Error::Unexpected(format!("Failed to serialize gui config: {}", e)))?
        .as_bytes(),
    )?;

    info!("Gui configuration file created");

    let wallet = remote_backend
        .create_wallet(&wallet_name(descriptor), descriptor)
        .await
        .map_err(|e| Error::Unexpected(e.to_string()))?;

    let hws: Vec<HardwareWalletConfig> = ctx
        .hws
        .iter()
        .filter_map(|(kind, fingerprint, token)| {
            token
                .as_ref()
                .map(|token| HardwareWalletConfig::new(kind, *fingerprint, token))
        })
        .collect();
    let descriptor_str = descriptor.to_string();
    let aliases = ctx
        .keys
        .iter()
        .filter_map(|k| {
            if descriptor_str.contains(&k.master_fingerprint.to_string()) {
                Some((k.master_fingerprint, k.name.to_string()))
            } else {
                None
            }
        })
        .collect();
    remote_backend
        .update_wallet_metadata(&wallet.id, &aliases, &hws)
        .await
        .map_err(|e| Error::Unexpected(e.to_string()))?;

    let remote_backend = remote_backend.connect_wallet(wallet).0;

    // create liana GUI settings file
    let settings: gui_settings::Settings = extract_remote_gui_settings(&ctx, &remote_backend).await;
    create_and_write_file(
        network_datadir_path.clone(),
        gui_settings::DEFAULT_FILE_NAME,
        serde_json::to_string_pretty(&settings)
            .map_err(|e| Error::Unexpected(format!("Failed to serialize settings: {}", e)))?
            .as_bytes(),
    )?;

    info!("Settings file created");

    Ok(gui_config_path)
}

pub async fn import_remote_wallet(
    ctx: Context,
    backend: BackendWalletClient,
) -> Result<PathBuf, Error> {
    tracing::info!("Importing wallet from remote backend");

    let data_dir = ctx
        .data_dir
        .canonicalize()
        .map_err(|e| Error::Unexpected(format!("Failed to canonicalize datadir path: {}", e)))?;

    if let Some(signer) = &ctx.recovered_signer {
        signer
            .store(&data_dir, ctx.network)
            .map_err(|e| Error::Unexpected(format!("Failed to store mnemonic: {}", e)))?;

        info!("Recovered signer mnemonic stored");
    }

    let mut network_datadir_path = data_dir;
    network_datadir_path.push(ctx.network.to_string());
    create_directory(&network_datadir_path)
        .map_err(|e| Error::Unexpected(format!("Failed to create datadir path: {}", e)))?;

    // create liana GUI settings file
    let settings: gui_settings::Settings = extract_remote_gui_settings(&ctx, &backend).await;
    create_and_write_file(
        network_datadir_path.clone(),
        gui_settings::DEFAULT_FILE_NAME,
        serde_json::to_string_pretty(&settings)
            .map_err(|e| Error::Unexpected(format!("Failed to serialize settings: {}", e)))?
            .as_bytes(),
    )?;

    info!("Settings file created");

    // create liana GUI configuration file
    let gui_config_path = create_and_write_file(
        network_datadir_path.clone(),
        gui_config::DEFAULT_FILE_NAME,
        toml::to_string(&gui_config::Config {
            daemon_config_path: None,
            daemon_rpc_path: None,
            log_level: Some("info".to_string()),
            debug: Some(false),
            start_internal_bitcoind: false,
        })
        .map_err(|e| Error::Unexpected(format!("Failed to serialize gui config: {}", e)))?
        .as_bytes(),
    )?;

    info!("Gui configuration file created");

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

// if the wallet is using the remote backend, then the hardware wallet settings and
// keys will be store on the remote backend side and not in the settings file.
pub async fn extract_remote_gui_settings(ctx: &Context, backend: &BackendWalletClient) -> Settings {
    let descriptor = ctx
        .descriptor
        .as_ref()
        .expect("Context must have a descriptor at this point");

    let descriptor_checksum = descriptor
        .to_string()
        .split_once('#')
        .map(|(_, checksum)| checksum)
        .expect("LianaDescriptor.to_string() always include the checksum")
        .to_string();

    let auth = backend.inner_client().auth.read().await;

    Settings {
        wallets: vec![WalletSetting {
            name: wallet_name(descriptor),
            descriptor_checksum,
            keys: Vec::new(),
            hardware_wallets: Vec::new(),
            remote_backend_auth: Some(AuthConfig {
                email: backend.user_email().to_string(),
                wallet_id: backend.wallet_id(),
                refresh_token: auth.refresh_token.clone(),
            }),
        }],
    }
}

pub async fn extract_local_gui_settings(ctx: &Context) -> Settings {
    let descriptor = ctx
        .descriptor
        .as_ref()
        .expect("Context must have a descriptor at this point");

    let descriptor_checksum = descriptor
        .to_string()
        .split_once('#')
        .map(|(_, checksum)| checksum)
        .expect("LianaDescriptor.to_string() always include the checksum")
        .to_string();

    let hardware_wallets = ctx
        .hws
        .iter()
        .filter_map(|(kind, fingerprint, token)| {
            token
                .as_ref()
                .map(|token| HardwareWalletConfig::new(kind, *fingerprint, token))
        })
        .collect();
    Settings {
        wallets: vec![WalletSetting {
            name: wallet_name(descriptor),
            descriptor_checksum,
            keys: ctx.keys.clone(),
            hardware_wallets,
            remote_backend_auth: None,
        }],
    }
}

pub fn extract_daemon_config(ctx: &Context) -> Config {
    Config {
        #[cfg(unix)]
        daemon: false,
        log_level: log::LevelFilter::Info,
        main_descriptor: ctx
            .descriptor
            .clone()
            .expect("Context must have a descriptor at this point"),
        data_dir: Some(ctx.data_dir.clone()),
        bitcoin_config: ctx.bitcoin_config.clone(),
        bitcoind_config: ctx.bitcoind_config.clone(),
    }
}

#[derive(Debug, Clone)]
pub enum Error {
    Auth(AuthError),
    // DaemonError does not implement Clone.
    // TODO: maybe Arc is overkill
    Backend(Arc<DaemonError>),
    Settings(SettingsError),
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

impl From<DaemonError> for Error {
    fn from(value: DaemonError) -> Self {
        Self::Backend(Arc::new(value))
    }
}

impl From<AuthError> for Error {
    fn from(value: AuthError) -> Self {
        Self::Auth(value)
    }
}

impl From<SettingsError> for Error {
    fn from(value: SettingsError) -> Self {
        Self::Settings(value)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Auth(e) => write!(f, "Authentification error: {}", e),
            Self::Backend(e) => write!(f, "Remote backend error: {}", e),
            Self::Settings(e) => write!(f, "Settings file error: {}", e),
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
