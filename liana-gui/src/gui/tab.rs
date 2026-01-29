use std::{collections::HashMap, marker::PhantomData, sync::Arc, time::Instant};

use iced::{Subscription, Task};
use tracing::{error, info};
extern crate serde;
extern crate serde_json;

use liana::miniscript::bitcoin;
use liana_ui::widget::Element;
use lianad::commands::ListCoinsResult;

use crate::{
    app::{
        self,
        cache::{Cache, DaemonCache},
        settings::{
            self, update_settings_file, LianaSettings, LianaWalletSettings, SettingsError,
            SettingsTrait,
        },
        wallet::Wallet,
    },
    dir::LianaDirectory,
    export::import_backup_at_launch,
    hw::HardwareWalletConfig,
    installer::{self, Installer},
    launcher::{self, Launcher},
    loader::{self, Loader},
    services::connect::{
        client::{
            auth::AuthClient,
            backend::{api, BackendClient, BackendWalletClient},
            cache as connect_cache,
        },
        login,
    },
};

pub enum State<I, S, M>
where
    M: Clone + Send + 'static,
    I: for<'a> Installer<'a, M>,
    S: SettingsTrait,
{
    Launcher(Box<Launcher>),
    Installer(I),
    Loader(Box<Loader>),
    Login(Box<login::LianaLiteLogin>),
    App(app::App<S>),
    #[doc(hidden)]
    _Phantom(PhantomData<M>),
}

impl<I, S, M> State<I, S, M>
where
    M: Clone + Send + 'static,
    I: for<'a> Installer<'a, M>,
    S: SettingsTrait,
{
    pub fn new(
        directory: LianaDirectory,
        network: Option<bitcoin::Network>,
    ) -> (Self, Task<Message<M>>) {
        if I::skip_launcher() {
            // Start directly with the Installer (e.g., for liana-business where auth is mandatory)
            let net = network.unwrap_or(bitcoin::Network::Signet);
            let (install, command) =
                I::new(directory, net, None, installer::UserFlow::CreateWallet);
            (
                State::Installer(*install),
                command.map(|msg| Message::Install(Box::new(msg))),
            )
        } else {
            // Normal flow: start with Launcher
            let (launcher, command) = Launcher::new(directory, network, I::backend_type());
            (
                State::Launcher(Box::new(launcher)),
                command.map(|msg| Message::Launch(Box::new(msg))),
            )
        }
    }
}

#[derive(Debug)]
pub enum Message<M>
where
    M: Clone + Send + 'static,
{
    Launch(Box<launcher::Message>),
    Install(Box<M>),
    Load(Box<loader::Message>),
    Run(Box<app::Message>),
    Login(Box<login::Message>),
    /// Result of connecting to backend for RunLianaBusiness flow
    BusinessConnected(BusinessConnectResult),
}

/// Result of attempting to connect to backend for liana-business
#[derive(Debug)]
pub struct BusinessConnectResult {
    pub datadir: LianaDirectory,
    pub network: bitcoin::Network,
    pub wallet_id: String,
    pub email: String,
    pub result: Result<
        (
            crate::services::connect::client::backend::BackendWalletClient,
            crate::services::connect::client::backend::api::Wallet,
            lianad::commands::ListCoinsResult,
        ),
        login::Error,
    >,
}

pub struct Tab<I, S, M>
where
    M: Clone + Send + 'static,
    I: for<'a> Installer<'a, M>,
    S: SettingsTrait,
{
    pub id: usize,
    pub state: State<I, S, M>,
    _phantom: PhantomData<M>,
}

impl<I, S, M> Tab<I, S, M>
where
    M: Clone + Send + 'static,
    I: for<'a> Installer<'a, M>,
    S: SettingsTrait,
{
    pub fn new(id: usize, state: State<I, S, M>) -> Self {
        Tab {
            id,
            state,
            _phantom: PhantomData,
        }
    }

    pub fn cache(&self) -> Option<&Cache> {
        if let State::App(ref app) = self.state {
            Some(app.cache())
        } else {
            None
        }
    }

    pub fn wallet(&self) -> Option<&Wallet> {
        if let State::App(ref app) = self.state {
            Some(app.wallet())
        } else {
            None
        }
    }

    pub fn title(&self) -> &str {
        match &self.state {
            State::Installer(_) => "Installer",
            State::Loader(_) => "Loading...",
            State::Launcher(_) => "Launcher",
            State::Login(_) => "Login",
            State::App(a) => a.title(),
            State::_Phantom(_) => unreachable!(),
        }
    }

    pub fn on_tick(&mut self) -> Task<Message<M>> {
        // currently the Tick is only used by the app
        if let State::App(app) = &mut self.state {
            app.on_tick().map(|msg| Message::Run(Box::new(msg)))
        } else {
            Task::none()
        }
    }

    pub fn update(&mut self, message: Message<M>) -> Task<Message<M>> {
        match (&mut self.state, message) {
            (State::Launcher(l), Message::Launch(msg)) => match *msg {
                launcher::Message::Install(datadir, network, init) => {
                    if !datadir.exists() {
                        // datadir is created right before launching the installer
                        // so logs can go in <datadir_path>/installer.log
                        if let Err(e) = datadir.init() {
                            error!("Failed to create datadir: {}", e);
                        } else {
                            info!(
                                "Created a fresh data directory at {}",
                                &datadir.path().to_string_lossy()
                            );
                        }
                    }
                    let (install, command) = Installer::new(datadir, network, None, init);
                    self.state = State::Installer(*install);
                    command.map(|msg| Message::Install(Box::new(msg)))
                }
                launcher::Message::Run(datadir_path, cfg, network, settings) => {
                    let wallet_id = settings.wallet_id();
                    if let Some(auth_cfg) = settings.remote_backend_auth {
                        let (login, command) =
                            login::LianaLiteLogin::new(datadir_path, network, wallet_id, auth_cfg, I::backend_type());
                        self.state = State::Login(Box::new(login));
                        command.map(|msg| Message::Login(Box::new(msg)))
                    } else {
                        let (loader, command) =
                            Loader::new(datadir_path, cfg, network, None, None, settings);
                        self.state = State::Loader(Box::new(loader));
                        command.map(|msg| Message::Load(Box::new(msg)))
                    }
                }
                _ => l.update(*msg).map(|msg| Message::Launch(Box::new(msg))),
            },
            (State::Login(l), Message::Login(msg)) => match *msg {
                login::Message::View(login::ViewMessage::BackToLauncher(network)) => {
                    let (launcher, command) = Launcher::new(l.datadir.clone(), Some(network), I::backend_type());
                    self.state = State::Launcher(Box::new(launcher));
                    command.map(|msg| Message::Launch(Box::new(msg)))
                }
                login::Message::Install(remote_backend) => {
                    let (install, command) = I::new(
                        l.datadir.clone(),
                        l.network,
                        remote_backend,
                        installer::UserFlow::CreateWallet,
                    );
                    self.state = State::Installer(*install);
                    command.map(|msg| Message::Install(Box::new(msg)))
                }
                login::Message::Run(Ok((backend_client, wallet, coins))) => {
                    let config = app::Config::from_file(
                        &l.datadir
                            .network_directory(l.network)
                            .path()
                            .join(app::config::DEFAULT_FILE_NAME),
                    )
                    .expect("A gui configuration file must be present");

                    // Use the trait method - returns None if S doesn't support remote backend
                    let result = S::create_app_for_remote_backend(
                        l.directory_wallet_id.clone(),
                        backend_client,
                        wallet,
                        coins,
                        l.datadir.clone(),
                        l.network,
                        config,
                    );

                    let (app, command) = match result {
                        Some(Ok((app, command))) => (app, command),
                        Some(Err(e)) => {
                            tracing::error!("{}", e);
                            return Task::none();
                        }
                        None => {
                            // This should never happen - Login state only exists for LianaSettings
                            tracing::error!("Login state reached for settings type that doesn't support remote backend");
                            return Task::none();
                        }
                    };

                    self.state = State::App(app);
                    command.map(|msg| Message::Run(Box::new(msg)))
                }
                _ => l.update(*msg).map(|msg| Message::Login(Box::new(msg))),
            },
            (State::Installer(i), Message::Install(msg)) => {
                if let Some(next_state) = i.exit_maybe(&msg) {
                    match next_state {
                        installer::NextState::LoginLianaLite {
                            datadir,
                            network,
                            directory_wallet_id,
                            auth_cfg,
                        } => {
                            let (login, command) = login::LianaLiteLogin::new(
                                datadir,
                                network,
                                directory_wallet_id,
                                auth_cfg,
                                I::backend_type(),
                            );
                            self.state = State::Login(Box::new(login));
                            command.map(|msg| Message::Login(Box::new(msg)))
                        }
                        installer::NextState::Loader {
                            datadir: datadir_path,
                            network,
                            internal_bitcoind,
                            backup,
                            wallet_settings,
                        } => {
                            let cfg = app::Config::from_file(
                                &datadir_path
                                    .network_directory(network)
                                    .path()
                                    .join(app::config::DEFAULT_FILE_NAME),
                            )
                            .expect("A gui configuration file must be present");

                            let (loader, command) = Loader::new(
                                datadir_path,
                                cfg,
                                network,
                                internal_bitcoind,
                                backup,
                                wallet_settings,
                            );
                            self.state = State::Loader(Box::new(loader));
                            command.map(|msg| Message::Load(Box::new(msg)))
                        }
                        installer::NextState::Launcher { network, datadir } => {
                            let (launcher, command) = Launcher::new(datadir, Some(network), I::backend_type());
                            self.state = State::Launcher(Box::new(launcher));
                            command.map(|msg| Message::Launch(Box::new(msg)))
                        }
                        installer::NextState::RunLianaBusiness {
                            datadir,
                            network,
                            wallet_id,
                            email,
                        } => {
                            // Spawn async task to connect using cached tokens
                            let datadir_clone = datadir.clone();
                            let wallet_id_clone = wallet_id.clone();
                            let email_clone = email.clone();
                            Task::perform(
                                async move {
                                    connect_for_business(
                                        datadir_clone.clone(),
                                        network,
                                        wallet_id_clone.clone(),
                                        email_clone.clone(),
                                        I::backend_type(),
                                    )
                                    .await
                                },
                                move |result| {
                                    Message::BusinessConnected(BusinessConnectResult {
                                        datadir: datadir.clone(),
                                        network,
                                        wallet_id: wallet_id.clone(),
                                        email: email.clone(),
                                        result,
                                    })
                                },
                            )
                        }
                    }
                } else {
                    i.update(*msg).map(|msg| Message::Install(Box::new(msg)))
                }
            }
            (State::Loader(loader), Message::Load(msg)) => match *msg {
                loader::Message::View(loader::ViewMessage::SwitchNetwork) => {
                    let (launcher, command) =
                        Launcher::new(loader.datadir_path.clone(), Some(loader.network), I::backend_type());
                    self.state = State::Launcher(Box::new(launcher));
                    command.map(|msg| Message::Launch(Box::new(msg)))
                }
                loader::Message::Synced(Ok((wallet, cache, daemon, bitcoind, backup))) => {
                    if let Some(backup) = backup {
                        let config = loader.gui_config.clone();
                        let datadir = loader.datadir_path.clone();
                        Task::perform(
                            async move {
                                import_backup_at_launch(
                                    cache, wallet, config, daemon, datadir, bitcoind, backup,
                                )
                                .await
                            },
                            |r| {
                                let r = r.map_err(loader::Error::RestoreBackup);
                                Message::Load(Box::new(loader::Message::App(
                                    r, /* restored_from_backup */ true,
                                )))
                            },
                        )
                    } else {
                        let (app, command) = app::App::<S>::new(
                            cache,
                            wallet,
                            loader.gui_config.clone(),
                            daemon,
                            loader.datadir_path.clone(),
                            bitcoind,
                            false,
                        );
                        self.state = State::App(app);
                        command.map(|msg| Message::Run(Box::new(msg)))
                    }
                }
                loader::Message::App(
                    Ok((cache, wallet, config, daemon, datadir, bitcoind)),
                    restored_from_backup,
                ) => {
                    let (app, command) = app::App::<S>::new(
                        cache,
                        wallet,
                        config,
                        daemon,
                        datadir,
                        bitcoind,
                        restored_from_backup,
                    );
                    self.state = State::App(app);
                    command.map(|msg| Message::Run(Box::new(msg)))
                }
                loader::Message::App(Err(e), _) => {
                    tracing::error!("Failed to import backup: {e}");
                    Task::none()
                }

                _ => loader.update(*msg).map(|msg| Message::Load(Box::new(msg))),
            },
            (State::App(i), Message::Run(msg)) => {
                if matches!(*msg, app::Message::RedirectLianaConnectLogin) {
                    let (login, command) = login::LianaLiteLogin::new(
                        i.cache().datadir_path.clone(),
                        i.cache().network,
                        i.wallet_id(),
                        i.wallet()
                            .remote_backend_auth
                            .clone()
                            .expect("Must be a liana-connect wallet"),
                        I::backend_type(),
                    );
                    self.state = State::Login(Box::new(login));
                    command.map(|msg| Message::Login(Box::new(msg)))
                } else {
                    i.update(*msg).map(|msg| Message::Run(Box::new(msg)))
                }
            }
            // Handle result of RunLianaBusiness connection attempt
            (State::Installer(_), Message::BusinessConnected(conn_result)) => {
                let BusinessConnectResult {
                    datadir,
                    network,
                    wallet_id,
                    email,
                    result,
                } = conn_result;

                match result {
                    Ok((backend_client, wallet, coins)) => {
                        // Success! Create App directly
                        let config_path = datadir
                            .network_directory(network)
                            .path()
                            .join(app::config::DEFAULT_FILE_NAME);
                        let config = match app::Config::from_file(&config_path) {
                            Ok(c) => c,
                            Err(e) => {
                                tracing::warn!(
                                    "Failed to load config from {:?}, creating default: {}",
                                    config_path,
                                    e
                                );
                                // Create a minimal config for remote backend (no bitcoind)
                                app::Config::new(false)
                            }
                        };

                        // Create WalletId for the directory
                        let directory_wallet_id =
                            crate::app::settings::WalletId::new(wallet_id.clone(), None);

                        // Use the trait method to create App
                        let result = S::create_app_for_remote_backend(
                            directory_wallet_id,
                            backend_client,
                            wallet,
                            coins,
                            datadir.clone(),
                            network,
                            config,
                        );

                        match result {
                            Some(Ok((app, command))) => {
                                self.state = State::App(app);
                                command.map(|msg| Message::Run(Box::new(msg)))
                            }
                            Some(Err(e)) => {
                                tracing::error!("Failed to create app: {}", e);
                                // Fall back to login flow
                                let auth_cfg = crate::app::settings::AuthConfig {
                                    email,
                                    wallet_id,
                                    refresh_token: None,
                                };
                                let directory_wallet_id = crate::app::settings::WalletId::new(
                                    auth_cfg.wallet_id.clone(),
                                    None,
                                );
                                let (login, command) = login::LianaLiteLogin::new(
                                    datadir,
                                    network,
                                    directory_wallet_id,
                                    auth_cfg,
                                    I::backend_type(),
                                );
                                self.state = State::Login(Box::new(login));
                                command.map(|msg| Message::Login(Box::new(msg)))
                            }
                            None => {
                                tracing::error!("Settings type doesn't support remote backend");
                                Task::none()
                            }
                        }
                    }
                    Err(e) => {
                        // Connection failed, fall back to login flow
                        tracing::warn!("Business connection failed, falling back to login: {}", e);
                        let auth_cfg = crate::app::settings::AuthConfig {
                            email,
                            wallet_id,
                            refresh_token: None,
                        };
                        let directory_wallet_id =
                            crate::app::settings::WalletId::new(auth_cfg.wallet_id.clone(), None);
                        let (login, command) = login::LianaLiteLogin::new(
                            datadir,
                            network,
                            directory_wallet_id,
                            auth_cfg,
                            I::backend_type(),
                        );
                        self.state = State::Login(Box::new(login));
                        command.map(|msg| Message::Login(Box::new(msg)))
                    }
                }
            }
            _ => Task::none(),
        }
    }

    pub fn subscription(&self) -> Subscription<Message<M>> {
        Subscription::batch(vec![match &self.state {
            State::Installer(v) => v.subscription().map(|msg| Message::Install(Box::new(msg))),
            State::Loader(v) => v.subscription().map(|msg| Message::Load(Box::new(msg))),
            State::App(v) => v.subscription().map(|msg| Message::Run(Box::new(msg))),
            State::Launcher(v) => v.subscription().map(|msg| Message::Launch(Box::new(msg))),
            State::Login(_) => Subscription::none(),
            State::_Phantom(_) => unreachable!(),
        }])
    }

    pub fn view(&self) -> Element<Message<M>> {
        match &self.state {
            State::Installer(v) => v.view().map(|msg| Message::Install(Box::new(msg))),
            State::App(v) => v.view().map(|msg| Message::Run(Box::new(msg))),
            State::Launcher(v) => v.view().map(|msg| Message::Launch(Box::new(msg))),
            State::Loader(v) => v.view().map(|msg| Message::Load(Box::new(msg))),
            State::Login(v) => v.view().map(|msg| Message::Login(Box::new(msg))),
            State::_Phantom(_) => unreachable!(),
        }
    }

    pub fn stop(&mut self) {
        match &mut self.state {
            State::Loader(s) => s.stop(),
            State::Launcher(s) => s.stop(),
            State::Installer(s) => s.stop(),
            State::App(s) => s.stop(),
            State::Login(_) => {}
            State::_Phantom(_) => unreachable!(),
        }
    }
}

pub fn create_app_with_remote_backend(
    wallet_id: settings::WalletId,
    remote_backend: BackendWalletClient,
    wallet: api::Wallet,
    coins: ListCoinsResult,
    liana_dir: LianaDirectory,
    network: bitcoin::Network,
    config: app::Config,
) -> Result<(app::App, iced::Task<app::Message>), settings::SettingsError> {
    let network_directory = liana_dir.network_directory(network);
    let wallet_settings =
        LianaWalletSettings::from_file(&network_directory, |w| w.wallet_id() == wallet_id)?
            .ok_or_else(|| SettingsError::Unexpected("Wallet not found".to_string()))?;

    // If someone modified the wallet_alias on Liana-Connect,
    // then the new alias is imported and stored in the settings file.
    if wallet.metadata.wallet_alias != wallet_settings.alias {
        if let Err(e) = tokio::runtime::Handle::current().block_on(async {
            update_settings_file(&network_directory, |mut settings: LianaSettings| {
                if let Some(w) = settings
                    .wallets
                    .iter_mut()
                    .find(|w| w.wallet_id() == wallet_id)
                {
                    w.alias = wallet.metadata.wallet_alias.clone();
                    tracing::info!("Wallet alias was changed. Settings updated.");
                }
                settings
            })
            .await
        }) {
            tracing::error!("Failed to update wallet settings with remote alias: {}", e);
        }
    }

    let hws: Vec<HardwareWalletConfig> = wallet
        .metadata
        .ledger_hmacs
        .into_iter()
        .map(|ledger_hmac| HardwareWalletConfig {
            kind: async_hwi::DeviceKind::Ledger.to_string(),
            fingerprint: ledger_hmac.fingerprint,
            token: ledger_hmac.hmac,
        })
        .collect();

    let aliases: HashMap<bitcoin::bip32::Fingerprint, String> = wallet
        .metadata
        .fingerprint_aliases
        .into_iter()
        .filter_map(|a| {
            if a.user_id == remote_backend.user_id() {
                Some((a.fingerprint, a.alias))
            } else {
                None
            }
        })
        .collect();

    let provider_keys: HashMap<_, _> = wallet
        .metadata
        .provider_keys
        .into_iter()
        .map(|pk| (pk.fingerprint, pk.into()))
        .collect();

    Ok(app::App::new(
        Cache {
            network,
            datadir_path: liana_dir.clone(),
            // We ignore last poll fields for remote backend.
            last_poll_at_startup: None,
            daemon_cache: DaemonCache {
                coins: coins.coins,
                rescan_progress: None,
                sync_progress: 1.0, // Remote backend is always synced
                blockheight: wallet.tip_height.unwrap_or(0),
                // We ignore last poll fields for remote backend.
                last_poll_timestamp: None,
                last_tick: Instant::now(),
            },
            fiat_price: None,
        },
        Arc::new(
            Wallet::new(wallet.descriptor)
                .with_name(wallet.name)
                .with_alias(wallet.metadata.wallet_alias)
                .with_pinned_at(wallet_settings.pinned_at)
                .with_key_aliases(aliases)
                .with_provider_keys(provider_keys)
                .with_hardware_wallets(hws)
                .with_remote_backend_auth(
                    wallet_settings
                        .remote_backend_auth
                        .expect("This is a liana-connect wallet"),
                )
                .with_fiat_price_setting(wallet_settings.fiat_price)
                .load_hotsigners(&liana_dir, network)
                .expect("Datadir should be conform"),
        ),
        config,
        Arc::new(remote_backend),
        liana_dir,
        None,
        false,
    ))
}

/// Connect to backend for liana-business using cached tokens.
/// This is similar to `login::connect_with_credentials` but uses the liana-business API URLs.
async fn connect_for_business(
    datadir: LianaDirectory,
    network: bitcoin::Network,
    wallet_id: String,
    email: String,
    backend_type: crate::services::connect::client::BackendType,
) -> Result<
    (
        BackendWalletClient,
        api::Wallet,
        lianad::commands::ListCoinsResult,
    ),
    login::Error,
> {
    use crate::app::cache::coins_to_cache;

    // Get service config
    let service_config =
        crate::services::connect::client::get_service_config(network, backend_type)
            .await
            .map_err(|e| login::Error::Unexpected(e.to_string()))?;

    // Create auth client
    let auth_client = AuthClient::new(
        service_config.auth_api_url,
        service_config.auth_api_public_key,
        email.clone(),
        backend_type.user_agent(),
    );

    // Get tokens from cache
    let network_dir = datadir.network_directory(network);
    let mut tokens = connect_cache::Account::from_cache(&network_dir, &email)
        .map_err(|e| match e {
            connect_cache::ConnectCacheError::NotFound => login::Error::CredentialsMissing,
            _ => e.into(),
        })?
        .ok_or(login::Error::CredentialsMissing)?
        .tokens;

    // Refresh if expired
    if tokens.expires_at < chrono::Utc::now().timestamp() {
        tokens =
            connect_cache::update_connect_cache(&network_dir, &tokens, &auth_client, true).await?;
    }

    // Connect to backend
    let client =
        BackendClient::connect(auth_client, service_config.backend_api_url, tokens, network)
            .await
            .map_err(|e| login::Error::Unexpected(e.to_string()))?;

    // Find the wallet
    let wallet = client
        .list_wallets()
        .await
        .map_err(|e| login::Error::Unexpected(e.to_string()))?
        .into_iter()
        .find(|w| w.id == wallet_id)
        .ok_or_else(|| login::Error::Unexpected(format!("Wallet {} not found", wallet_id)))?;

    // Create wallet client
    let (wallet_client, wallet) = client.connect_wallet(wallet);

    // Get coins
    let coins = coins_to_cache(std::sync::Arc::new(wallet_client.clone()))
        .await
        .map_err(|e| login::Error::Unexpected(e.to_string()))?;

    Ok((wallet_client, wallet, coins))
}
