use std::{collections::HashMap, sync::Arc};

use iced::{Subscription, Task};
use tracing::{error, info};
use tracing_subscriber::filter::LevelFilter;
extern crate serde;
extern crate serde_json;

use liana::miniscript::bitcoin;
use liana_ui::widget::Element;
use lianad::commands::ListCoinsResult;

use crate::{
    app::{
        self,
        cache::Cache,
        settings::{update_settings_file, WalletSettings},
        wallet::Wallet,
        App,
    },
    dir::LianaDirectory,
    export::import_backup_at_launch,
    hw::HardwareWalletConfig,
    installer::{self, Installer},
    launcher::{self, Launcher},
    loader::{self, Loader},
    logger::Logger,
    services::connect::{
        client::backend::{api, BackendWalletClient},
        login,
    },
};

pub struct Tab(pub State);

pub enum State {
    Launcher(Box<Launcher>),
    Installer(Box<Installer>),
    Loader(Box<Loader>),
    Login(Box<login::LianaLiteLogin>),
    App(App),
}

#[derive(Debug)]
pub enum Message {
    Launch(Box<launcher::Message>),
    Install(Box<installer::Message>),
    Load(Box<loader::Message>),
    Run(Box<app::Message>),
    Login(Box<login::Message>),
}

impl Tab {
    pub fn new(
        directory: LianaDirectory,
        network: Option<bitcoin::Network>,
    ) -> (Self, Task<Message>) {
        let (launcher, command) = Launcher::new(directory, network);
        (
            Tab(State::Launcher(Box::new(launcher))),
            command.map(|msg| Message::Launch(Box::new(msg))),
        )
    }

    pub fn update(
        &mut self,
        message: Message,
        logger: &Logger,
        log_level: Option<LevelFilter>,
    ) -> Task<Message> {
        match (&mut self.0, message) {
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
                    logger.set_installer_mode(
                        datadir.clone(),
                        log_level.unwrap_or(LevelFilter::INFO),
                    );

                    let (install, command) = Installer::new(datadir, network, None, init);
                    self.0 = State::Installer(Box::new(install));
                    command.map(|msg| Message::Install(Box::new(msg)))
                }
                launcher::Message::Run(datadir_path, cfg, network, settings) => {
                    logger.set_running_mode(
                        datadir_path.clone(),
                        network,
                        log_level.unwrap_or_else(|| cfg.log_level().unwrap_or(LevelFilter::INFO)),
                    );
                    if settings.remote_backend_auth.is_some() {
                        let (login, command) =
                            login::LianaLiteLogin::new(datadir_path, network, settings);
                        self.0 = State::Login(Box::new(login));
                        command.map(|msg| Message::Login(Box::new(msg)))
                    } else {
                        let (loader, command) =
                            Loader::new(datadir_path, cfg, network, None, None, settings);
                        self.0 = State::Loader(Box::new(loader));
                        command.map(|msg| Message::Load(Box::new(msg)))
                    }
                }
                _ => l.update(*msg).map(|msg| Message::Launch(Box::new(msg))),
            },
            (State::Login(l), Message::Login(msg)) => match *msg {
                login::Message::View(login::ViewMessage::BackToLauncher(network)) => {
                    let (launcher, command) = Launcher::new(l.datadir.clone(), Some(network));
                    self.0 = State::Launcher(Box::new(launcher));
                    command.map(|msg| Message::Launch(Box::new(msg)))
                }
                login::Message::Install(remote_backend) => {
                    let (install, command) = Installer::new(
                        l.datadir.clone(),
                        l.network,
                        remote_backend,
                        installer::UserFlow::CreateWallet,
                    );
                    self.0 = State::Installer(Box::new(install));
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
                    logger.set_running_mode(
                        l.datadir.clone(),
                        l.network,
                        config.log_level().unwrap_or(LevelFilter::INFO),
                    );

                    let (app, command) = create_app_with_remote_backend(
                        l.settings.clone(),
                        backend_client,
                        wallet,
                        coins,
                        l.datadir.clone(),
                        l.network,
                        config,
                    );

                    self.0 = State::App(app);
                    command.map(|msg| Message::Run(Box::new(msg)))
                }
                _ => l.update(*msg).map(|msg| Message::Login(Box::new(msg))),
            },
            (State::Installer(i), Message::Install(msg)) => {
                if let installer::Message::Exit(settings, internal_bitcoind, remove_log) = *msg {
                    if settings.remote_backend_auth.is_some() {
                        let (login, command) =
                            login::LianaLiteLogin::new(i.datadir.clone(), i.network, *settings);
                        self.0 = State::Login(Box::new(login));
                        command.map(|msg| Message::Login(Box::new(msg)))
                    } else {
                        let cfg = app::Config::from_file(
                            &i.datadir
                                .network_directory(i.network)
                                .path()
                                .join(app::config::DEFAULT_FILE_NAME),
                        )
                        .expect("A gui configuration file must be present");

                        logger.set_running_mode(
                            i.datadir.clone(),
                            i.network,
                            log_level
                                .unwrap_or_else(|| cfg.log_level().unwrap_or(LevelFilter::INFO)),
                        );
                        if remove_log {
                            logger.remove_install_log_file(i.datadir.clone());
                        }

                        let (loader, command) = Loader::new(
                            i.datadir.clone(),
                            cfg,
                            i.network,
                            internal_bitcoind,
                            i.context.backup.take(),
                            *settings,
                        );
                        self.0 = State::Loader(Box::new(loader));
                        command.map(|msg| Message::Load(Box::new(msg)))
                    }
                } else if let installer::Message::BackToLauncher(network) = *msg {
                    let (launcher, command) = Launcher::new(i.destination_path(), Some(network));
                    self.0 = State::Launcher(Box::new(launcher));
                    command.map(|msg| Message::Launch(Box::new(msg)))
                } else {
                    i.update(*msg).map(|msg| Message::Install(Box::new(msg)))
                }
            }
            (State::Loader(loader), Message::Load(msg)) => match *msg {
                loader::Message::View(loader::ViewMessage::SwitchNetwork) => {
                    let (launcher, command) =
                        Launcher::new(loader.datadir_path.clone(), Some(loader.network));
                    self.0 = State::Launcher(Box::new(launcher));
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
                        let (app, command) = App::new(
                            cache,
                            wallet,
                            loader.gui_config.clone(),
                            daemon,
                            loader.datadir_path.clone(),
                            bitcoind,
                            false,
                        );
                        self.0 = State::App(app);
                        command.map(|msg| Message::Run(Box::new(msg)))
                    }
                }
                loader::Message::App(
                    Ok((cache, wallet, config, daemon, datadir, bitcoind)),
                    restored_from_backup,
                ) => {
                    let (app, command) = App::new(
                        cache,
                        wallet,
                        config,
                        daemon,
                        datadir,
                        bitcoind,
                        restored_from_backup,
                    );
                    self.0 = State::App(app);
                    command.map(|msg| Message::Run(Box::new(msg)))
                }
                loader::Message::App(Err(e), _) => {
                    tracing::error!("Failed to import backup: {e}");
                    Task::none()
                }

                _ => loader.update(*msg).map(|msg| Message::Load(Box::new(msg))),
            },
            (State::App(i), Message::Run(msg)) => {
                i.update(*msg).map(|msg| Message::Run(Box::new(msg)))
            }
            _ => Task::none(),
        }
    }

    pub fn subscription(&self) -> Subscription<Message> {
        Subscription::batch(vec![match &self.0 {
            State::Installer(v) => v.subscription().map(|msg| Message::Install(Box::new(msg))),
            State::Loader(v) => v.subscription().map(|msg| Message::Load(Box::new(msg))),
            State::App(v) => v.subscription().map(|msg| Message::Run(Box::new(msg))),
            State::Launcher(v) => v.subscription().map(|msg| Message::Launch(Box::new(msg))),
            State::Login(_) => Subscription::none(),
        }])
    }

    pub fn view(&self) -> Element<Message> {
        match &self.0 {
            State::Installer(v) => v.view().map(|msg| Message::Install(Box::new(msg))),
            State::App(v) => v.view().map(|msg| Message::Run(Box::new(msg))),
            State::Launcher(v) => v.view().map(|msg| Message::Launch(Box::new(msg))),
            State::Loader(v) => v.view().map(|msg| Message::Load(Box::new(msg))),
            State::Login(v) => v.view().map(|msg| Message::Login(Box::new(msg))),
        }
    }

    pub fn stop(&mut self) {
        match &mut self.0 {
            State::Loader(s) => s.stop(),
            State::Launcher(s) => s.stop(),
            State::Installer(s) => s.stop(),
            State::App(s) => s.stop(),
            State::Login(_) => {}
        }
    }
}

pub fn create_app_with_remote_backend(
    wallet_settings: WalletSettings,
    remote_backend: BackendWalletClient,
    wallet: api::Wallet,
    coins: ListCoinsResult,
    liana_dir: LianaDirectory,
    network: bitcoin::Network,
    config: app::Config,
) -> (app::App, iced::Task<app::Message>) {
    // If someone modified the wallet_alias on Liana-Connect,
    // then the new alias is imported and stored in the settings file.
    if wallet.metadata.wallet_alias != wallet_settings.alias {
        let network_directory = liana_dir.network_directory(network);
        if let Err(e) = tokio::runtime::Handle::current().block_on(async {
            update_settings_file(&network_directory, |mut settings| {
                if let Some(w) = settings
                    .wallets
                    .iter_mut()
                    .find(|w| w.wallet_id() == wallet_settings.wallet_id())
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

    App::new(
        Cache {
            network,
            coins: coins.coins,
            rescan_progress: None,
            sync_progress: 1.0, // Remote backend is always synced
            datadir_path: liana_dir.clone(),
            blockheight: wallet.tip_height.unwrap_or(0),
            // We ignore last poll fields for remote backend.
            last_poll_timestamp: None,
            last_poll_at_startup: None,
        },
        Arc::new(
            Wallet::new(wallet.descriptor)
                .with_name(wallet.name)
                .with_alias(wallet.metadata.wallet_alias)
                .with_pinned_at(wallet_settings.pinned_at)
                .with_key_aliases(aliases)
                .with_provider_keys(provider_keys)
                .with_hardware_wallets(hws)
                .load_hotsigners(&liana_dir, network)
                .expect("Datadir should be conform"),
        ),
        config,
        Arc::new(remote_backend),
        liana_dir,
        None,
        false,
    )
}
