mod context;
mod descriptor;
mod message;
mod prompt;
mod step;
mod view;

pub use context::{Context, RemoteBackend};
use iced::{clipboard, Subscription, Task};
use liana::miniscript::bitcoin::{self, Network};
use liana_ui::{
    component::network_banner,
    widget::{Column, Element},
};
use lianad::config::Config;
use tracing::{error, info, warn};

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::{
    app::{
        config as gui_config,
        settings::{self as gui_settings, AuthConfig, Settings, SettingsError, WalletSetting},
        wallet::wallet_name,
    },
    backup,
    daemon::DaemonError,
    datadir::create_directory,
    hw::{HardwareWalletConfig, HardwareWallets},
    lianalite::client::{
        auth::AuthError,
        backend::{
            api::payload::{Provider, ProviderKey},
            BackendClient, BackendWalletClient,
        },
    },
    services,
    signer::Signer,
};

pub use descriptor::{KeySource, KeySourceKind, PathKind, PathSequence};
pub use message::Message;
use step::{
    BackupDescriptor, BackupMnemonic, ChooseBackend, ChooseDescriptorTemplate, DefineDescriptor,
    DefineNode, DescriptorTemplateDescription, Final, ImportDescriptor, ImportRemoteWallet,
    InternalBitcoindStep, RecoverMnemonic, RegisterDescriptor, RemoteBackendLogin,
    SelectBitcoindTypeStep, ShareXpubs, Step,
};

#[derive(Debug, Clone)]
pub enum UserFlow {
    CreateWallet,
    AddWallet,
    ShareXpubs,
}

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
    fn previous(&mut self) -> Task<Message> {
        self.hws.reset_watch_list();
        let network = self.network;
        if self.current > 0 {
            self.current -= 1;
        } else {
            return Task::perform(async move { network }, Message::BackToLauncher);
        }
        // skip the previous step according to the current context.
        while self
            .steps
            .get(self.current)
            .expect("There is always a step")
            .skip(&self.context)
        {
            if self.current > 0 {
                self.current -= 1;
            } else {
                return Task::perform(async move { network }, Message::BackToLauncher);
            }
        }

        if let Some(step) = self.steps.get(self.current) {
            step.revert(&mut self.context)
        }
        Task::none()
    }

    pub fn new(
        destination_path: PathBuf,
        network: bitcoin::Network,
        remote_backend: Option<BackendClient>,
        user_flow: UserFlow,
    ) -> (Installer, Task<Message>) {
        let signer = Arc::new(Mutex::new(Signer::generate(network).unwrap()));
        let context = Context::new(
            network,
            destination_path.clone(),
            remote_backend.map(RemoteBackend::WithoutWallet).unwrap_or(
                if matches!(network, Network::Bitcoin | Network::Signet) {
                    RemoteBackend::Undefined
                } else {
                    // The step for choosing the backend will be skipped.
                    RemoteBackend::None
                },
            ),
        );
        let mut installer = Installer {
            network,
            datadir: destination_path.clone(),
            current: 0,
            hws: HardwareWallets::new(destination_path.clone(), network),
            steps: match user_flow {
                UserFlow::CreateWallet => vec![
                    ChooseDescriptorTemplate::default().into(),
                    DescriptorTemplateDescription::default().into(),
                    DefineDescriptor::new(network, signer.clone()).into(),
                    BackupMnemonic::new(signer.clone()).into(),
                    BackupDescriptor::default().into(),
                    RegisterDescriptor::new_create_wallet().into(),
                    ChooseBackend::new(network).into(),
                    RemoteBackendLogin::new(network).into(),
                    SelectBitcoindTypeStep::new().into(),
                    InternalBitcoindStep::new(&context.data_dir).into(),
                    DefineNode::default().into(),
                    Final::new().into(),
                ],
                UserFlow::ShareXpubs => vec![ShareXpubs::new(network, signer.clone()).into()],
                UserFlow::AddWallet => vec![
                    ChooseBackend::new(network).into(),
                    RemoteBackendLogin::new(network).into(),
                    ImportRemoteWallet::new(network).into(),
                    ImportDescriptor::new(network).into(),
                    RecoverMnemonic::default().into(),
                    RegisterDescriptor::new_import_wallet().into(),
                    SelectBitcoindTypeStep::new().into(),
                    InternalBitcoindStep::new(&context.data_dir).into(),
                    DefineNode::default().into(),
                    Final::new().into(),
                ],
            },
            context,
            signer,
        };
        // skip the step according to the current context.
        installer.skip_steps();

        let current_step = installer
            .steps
            .get_mut(installer.current)
            .expect("There is always a step");
        current_step.load_context(&installer.context);
        let command = current_step.load();
        (installer, command)
    }

    pub fn destination_path(&self) -> PathBuf {
        self.context.data_dir.clone()
    }

    pub fn subscription(&self) -> Subscription<Message> {
        self.steps
            .get(self.current)
            .expect("There is always a step")
            .subscription(&self.hws)
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

    fn skip_steps(&mut self) {
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
    }

    fn next(&mut self) -> Task<Message> {
        self.hws.reset_watch_list();
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
                return Task::none();
            }
            // skip the step according to the current context.
            self.skip_steps();

            // calculate new current_step.
            let current_step = self
                .steps
                .get_mut(self.current)
                .expect("There is always a step");
            current_step.load_context(&self.context);
            return current_step.load();
        }
        Task::none()
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::HardwareWallets(msg) => match self.hws.update(msg) {
                Ok(cmd) => cmd.map(Message::HardwareWallets),
                Err(e) => {
                    error!("{}", e);
                    Task::none()
                }
            },
            Message::Clibpboard(s) => clipboard::write(s),
            Message::Next => self.next(),
            Message::Previous => self.previous(),
            Message::Install => {
                let _cmd = self
                    .steps
                    .get_mut(self.current)
                    .expect("There is always a step")
                    .update(&mut self.hws, message, &self.context);
                match &self.context.remote_backend {
                    RemoteBackend::WithoutWallet(backend) => Task::perform(
                        create_remote_wallet(
                            self.context.clone(),
                            self.signer.clone(),
                            backend.clone(),
                        ),
                        Message::Installed,
                    ),
                    RemoteBackend::WithWallet(backend) => Task::perform(
                        import_remote_wallet(self.context.clone(), backend.clone()),
                        Message::Installed,
                    ),
                    RemoteBackend::None => Task::perform(
                        install_local_wallet(self.context.clone(), self.signer.clone()),
                        Message::Installed,
                    ),
                    RemoteBackend::Undefined => unreachable!("Must be defined at this point"),
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
                    .update(&mut self.hws, Message::Installed(Err(e)), &self.context)
            }
            _ => self
                .steps
                .get_mut(self.current)
                .expect("There is always a step")
                .update(&mut self.hws, message, &self.context),
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
                self.context.remote_backend.user_email(),
            );

        if self.network != Network::Bitcoin {
            Column::with_children(vec![network_banner(self.network).into(), content]).into()
        } else {
            content
        }
    }
}

pub fn daemon_check(cfg: lianad::config::Config) -> Result<(), Error> {
    // Start Daemon to check correctness of installation
    match lianad::DaemonHandle::start_default(cfg, false) {
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
    let mut cfg: lianad::config::Config = extract_daemon_config(&ctx);
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
            // Installer started a bitcoind, it is expected that gui will start it on startup
            ctx.internal_bitcoind.is_some(),
        ))
        .map_err(|e| Error::Unexpected(format!("Failed to serialize gui config: {}", e)))?
        .as_bytes(),
    )?;

    info!("Gui configuration file created");

    // create liana GUI settings file
    let settings: gui_settings::Settings = extract_local_gui_settings(&ctx);
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

    let pks: Vec<_> = ctx
        .keys
        .values()
        .filter_map(|key| {
            key.provider_key.as_ref().map(|pk| ProviderKey {
                fingerprint: key.master_fingerprint.to_string(),
                uuid: pk.uuid.clone(),
                token: pk.token.clone(),
                provider: Provider {
                    uuid: pk.provider.uuid.clone(),
                    name: pk.provider.name.clone(),
                },
            })
        })
        .collect();
    let wallet = remote_backend
        .create_wallet(&wallet_name(descriptor), descriptor, &pks)
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
        .values()
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

pub fn extract_local_gui_settings(ctx: &Context) -> Settings {
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
            keys: ctx.keys.values().cloned().collect(),
            hardware_wallets,
            remote_backend_auth: None,
        }],
    }
}

pub fn extract_daemon_config(ctx: &Context) -> Config {
    Config {
        log_level: log::LevelFilter::Info,
        main_descriptor: ctx
            .descriptor
            .clone()
            .expect("Context must have a descriptor at this point"),
        data_dir: Some(ctx.data_dir.clone()),
        bitcoin_config: ctx.bitcoin_config.clone(),
        bitcoin_backend: ctx.bitcoin_backend.clone(),
    }
}

#[derive(Debug, Clone)]
pub enum Error {
    Auth(AuthError),
    // DaemonError does not implement Clone.
    // TODO: maybe Arc is overkill
    Backend(Arc<DaemonError>),
    Services(services::Error),
    Settings(SettingsError),
    Bitcoind(String),
    Electrum(String),
    CannotCreateDatadir(String),
    CannotCreateFile(String),
    CannotWriteToFile(String),
    CannotGetAvailablePort(String),
    Unexpected(String),
    HardwareWallet(async_hwi::Error),
    Backup(backup::Error),
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
            Self::Auth(e) => write!(f, "Authentication error: {}", e),
            Self::Backend(e) => write!(f, "Remote backend error: {}", e),
            Self::Services(e) => write!(f, "Services error: {}", e),
            Self::Settings(e) => write!(f, "Settings file error: {}", e),
            Self::Bitcoind(e) => write!(f, "Failed to ping bitcoind: {}", e),
            Self::Electrum(e) => write!(f, "Failed to ping Electrum: {}", e),
            Self::CannotCreateDatadir(e) => write!(f, "Failed to create datadir: {}", e),
            Self::CannotGetAvailablePort(e) => write!(f, "Failed to get available port: {}", e),
            Self::CannotWriteToFile(e) => write!(f, "Failed to write to file: {}", e),
            Self::CannotCreateFile(e) => write!(f, "Failed to create file: {}", e),
            Self::Unexpected(e) => write!(f, "Unexpected: {}", e),
            Self::HardwareWallet(e) => write!(f, "Hardware Wallet: {}", e),
            Self::Backup(e) => write!(f, "Backup: {:?}", e),
        }
    }
}
