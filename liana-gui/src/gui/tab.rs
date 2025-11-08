use std::{collections::HashMap, sync::Arc, time::Instant};

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
        settings::{update_settings_file, WalletSettings},
        wallet::Wallet,
        App,
    },
    dir::LianaDirectory,
    export::import_backup_at_launch,
    hw::HardwareWalletConfig,
    installer::{self, Installer, UserFlow},
    launcher::{self, Launcher},
    loader::{self, Loader},
    services::connect::{
        client::backend::{api, BackendWalletClient},
        login,
    },
};

pub enum State {
    Launcher(Box<Launcher>),
    Installer(Box<Installer>),
    Loader(Box<Loader>),
    Login(Box<login::LianaLiteLogin>),
    PinEntry(Box<crate::pin_entry::PinEntry>),
    App(App),
}

impl State {
    pub fn new(
        directory: LianaDirectory,
        network: Option<bitcoin::Network>,
    ) -> (Self, Task<Message>) {
        let (launcher, command) = Launcher::new(directory, network);
        (
            State::Launcher(Box::new(launcher)),
            command.map(|msg| Message::Launch(Box::new(msg))),
        )
    }
}

#[derive(Debug)]
pub enum Message {
    Launch(Box<launcher::Message>),
    Install(Box<installer::Message>),
    Load(Box<loader::Message>),
    Run(Box<app::Message>),
    Login(Box<login::Message>),
    PinEntry(Box<crate::pin_entry::Message>),
}

pub struct Tab {
    pub id: usize,
    pub state: State,
}

impl Tab {
    pub fn new(id: usize, state: State) -> Self {
        Tab { id, state }
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
            app.wallet()
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
            State::PinEntry(_) => "Enter PIN",
            State::App(a) => a.title(),
        }
    }

    pub fn on_tick(&mut self) -> Task<Message> {
        // currently the Tick is only used by the app
        if let State::App(app) = &mut self.state {
            app.on_tick().map(|msg| Message::Run(Box::new(msg)))
        } else {
            Task::none()
        }
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
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
                    let (install, command) = Installer::new(datadir, network, None, init, false, None);
                    self.state = State::Installer(Box::new(install));
                    command.map(|msg| Message::Install(Box::new(msg)))
                }
                launcher::Message::Run(datadir_path, cfg, network, cube) => {
                    // Check if cube has PIN protection
                    if cube.has_pin() {
                        // Determine the success action based on cube configuration
                        let on_success = if let Some(vault_id) = &cube.vault_wallet_id {
                            // Load wallet settings to determine next state
                            let network_dir = datadir_path.network_directory(network);
                            match app::settings::Settings::from_file(&network_dir) {
                                Ok(s) => {
                                    if let Some(wallet_settings) = s.wallets.iter().find(|w| w.wallet_id() == *vault_id) {
                                        if wallet_settings.remote_backend_auth.is_some() {
                                            // Will go to Login after PIN verification
                                            crate::pin_entry::PinEntrySuccess::LoadLoader {
                                                datadir: datadir_path,
                                                config: cfg,
                                                network,
                                                internal_bitcoind: None,
                                                backup: None,
                                                wallet_settings: Some(wallet_settings.clone()),
                                            }
                                        } else {
                                            // Will go to Loader after PIN verification
                                            crate::pin_entry::PinEntrySuccess::LoadLoader {
                                                datadir: datadir_path,
                                                config: cfg,
                                                network,
                                                internal_bitcoind: None,
                                                backup: None,
                                                wallet_settings: Some(wallet_settings.clone()),
                                            }
                                        }
                                    } else {
                                        // Vault wallet not found, load app without wallet
                                        crate::pin_entry::PinEntrySuccess::LoadApp {
                                            datadir: datadir_path,
                                            config: cfg,
                                            network,
                                        }
                                    }
                                }
                                Err(_) => {
                                    // No settings file, load app without wallet
                                    crate::pin_entry::PinEntrySuccess::LoadApp {
                                        datadir: datadir_path,
                                        config: cfg,
                                        network,
                                    }
                                }
                            }
                        } else {
                            // No vault configured - will load app without wallet after PIN
                            crate::pin_entry::PinEntrySuccess::LoadApp {
                                datadir: datadir_path,
                                config: cfg,
                                network,
                            }
                        };
                        
                        let pin_entry = crate::pin_entry::PinEntry::new(cube, on_success);
                        self.state = State::PinEntry(Box::new(pin_entry));
                        Task::none()
                    } else {
                        // No PIN protection - proceed directly
                        // Check if cube has a vault wallet configured
                        if let Some(vault_id) = &cube.vault_wallet_id {
                            // Load wallet settings from settings.json
                            let network_dir = datadir_path.network_directory(network);
                            match app::settings::Settings::from_file(&network_dir) {
                                Ok(s) => {
                                    if let Some(wallet_settings) = s.wallets.iter().find(|w| w.wallet_id() == *vault_id) {
                                        if wallet_settings.remote_backend_auth.is_some() {
                                            let (login, command) =
                                                login::LianaLiteLogin::new(datadir_path, network, wallet_settings.clone());
                                            self.state = State::Login(Box::new(login));
                                            return command.map(|msg| Message::Login(Box::new(msg)));
                                        } else {
                                            let (loader, command) =
                                                Loader::new(datadir_path, cfg, network, None, None, Some(wallet_settings.clone()), cube);
                                            self.state = State::Loader(Box::new(loader));
                                            return command.map(|msg| Message::Load(Box::new(msg)));
                                        }
                                    }
                                }
                                Err(_) => {
                                    // Wallet settings not found, fall through to load without vault
                                }
                            }
                        }
                        
                        // No vault configured - load app without wallet
                        let (app, command) = App::new_without_wallet(
                            cfg,
                            datadir_path,
                            network,
                            cube,
                        );
                        self.state = State::App(app);
                        command.map(|msg| Message::Run(Box::new(msg)))
                    }
                }
                _ => l.update(*msg).map(|msg| Message::Launch(Box::new(msg))),
            },
            (State::Login(l), Message::Login(msg)) => match *msg {
                login::Message::View(login::ViewMessage::BackToLauncher(network)) => {
                    let (launcher, command) = Launcher::new(l.datadir.clone(), Some(network));
                    self.state = State::Launcher(Box::new(launcher));
                    command.map(|msg| Message::Launch(Box::new(msg)))
                }
                login::Message::Install(remote_backend) => {
                    let (install, command) = Installer::new(
                        l.datadir.clone(),
                        l.network,
                        remote_backend,
                        installer::UserFlow::CreateWallet,
                        false,
                        None,
                    );
                    self.state = State::Installer(Box::new(install));
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
                    let (app, command) = create_app_with_remote_backend(
                        l.settings.clone(),
                        backend_client,
                        wallet,
                        coins,
                        l.datadir.clone(),
                        l.network,
                        config,
                    );

                    self.state = State::App(app);
                    command.map(|msg| Message::Run(Box::new(msg)))
                }
                _ => l.update(*msg).map(|msg| Message::Login(Box::new(msg))),
            },
            (State::Installer(i), Message::Install(msg)) => {
                if let installer::Message::Exit(settings, internal_bitcoind) = *msg {
                    if settings.remote_backend_auth.is_some() {
                        let (login, command) =
                            login::LianaLiteLogin::new(i.datadir.clone(), i.network, *settings);
                        self.state = State::Login(Box::new(login));
                        command.map(|msg| Message::Login(Box::new(msg)))
                    } else {
                        let cfg = app::Config::from_file(
                            &i.datadir
                                .network_directory(i.network)
                                .path()
                                .join(app::config::DEFAULT_FILE_NAME),
                        )
                        .expect("A gui configuration file must be present");

                        // Associate wallet with cube
                        let network_dir = i.datadir.network_directory(i.network);
                        let cube = match app::settings::Cubes::from_file(&network_dir) {
                            Ok(mut cubes) => {
                                // First, check if a cube already has this wallet
                                if let Some(existing_cube) = cubes.cubes.iter().find(|c| {
                                    c.vault_wallet_id.as_ref() == Some(&settings.wallet_id())
                                }) {
                                    existing_cube.clone()
                                }
                                // Second, find a cube without a vault and associate this wallet with it
                                else if let Some(empty_cube) = cubes.cubes.iter_mut().find(|c| c.vault_wallet_id.is_none()) {
                                    empty_cube.vault_wallet_id = Some(settings.wallet_id());
                                    let cube_clone = empty_cube.clone();
                                    // Save the updated cubes
                                    tokio::runtime::Handle::current().block_on(async {
                                        let _ = app::settings::update_cubes_file(&network_dir, |_| cubes.clone()).await;
                                    });
                                    cube_clone
                                }
                                // Third, create a new cube for this wallet
                                else {
                                    let cube = app::settings::CubeSettings::new(
                                        settings.alias.clone().unwrap_or_else(|| format!("My {} Cube", i.network)),
                                        i.network
                                    ).with_vault(settings.wallet_id());
                                    cubes.cubes.push(cube.clone());
                                    // Save the cube
                                    tokio::runtime::Handle::current().block_on(async {
                                        let _ = app::settings::update_cubes_file(&network_dir, |_| cubes.clone()).await;
                                    });
                                    cube
                                }
                            }
                            Err(_) => {
                                // No cubes file yet, create first cube
                                let cube = app::settings::CubeSettings::new(
                                    settings.alias.clone().unwrap_or_else(|| format!("My {} Cube", i.network)),
                                    i.network
                                ).with_vault(settings.wallet_id());
                                tokio::runtime::Handle::current().block_on(async {
                                    let _ = app::settings::update_cubes_file(&network_dir, |mut c| {
                                        c.cubes.push(cube.clone());
                                        c
                                    }).await;
                                });
                                cube
                            }
                        };

                        let (loader, command) = Loader::new(
                            i.datadir.clone(),
                            cfg,
                            i.network,
                            internal_bitcoind,
                            i.context.backup.take(),
                            Some(*settings),
                            cube,
                        );
                        self.state = State::Loader(Box::new(loader));
                        command.map(|msg| Message::Load(Box::new(msg)))
                    }
                } else if let installer::Message::BackToLauncher(network) = *msg {
                    let (launcher, command) = Launcher::new(i.destination_path(), Some(network));
                    self.state = State::Launcher(Box::new(launcher));
                    command.map(|msg| Message::Launch(Box::new(msg)))
                } else if let installer::Message::BackToApp(network) = *msg {
                    // Go back to app without vault using stored cube settings
                    if let Some(cube) = &i.cube_settings {
                        let cfg = app::Config::from_file(
                            &i.datadir
                                .network_directory(network)
                                .path()
                                .join(app::config::DEFAULT_FILE_NAME),
                        )
                        .expect("A gui configuration file must be present");
                        
                        let (app, command) = app::App::new_without_wallet(
                            cfg,
                            i.datadir.clone(),
                            network,
                            cube.clone(),
                        );
                        self.state = State::App(app);
                        command.map(|msg| Message::Run(Box::new(msg)))
                    } else {
                        // No cube settings stored, go to launcher
                        let (launcher, command) = Launcher::new(i.destination_path(), Some(network));
                        self.state = State::Launcher(Box::new(launcher));
                        command.map(|msg| Message::Launch(Box::new(msg)))
                    }
                } else {
                    i.update(*msg).map(|msg| Message::Install(Box::new(msg)))
                }
            }
            (State::Loader(loader), Message::Load(msg)) => match *msg {
                loader::Message::View(loader::ViewMessage::SwitchNetwork) => {
                    let (launcher, command) =
                        Launcher::new(loader.datadir_path.clone(), Some(loader.network));
                    self.state = State::Launcher(Box::new(launcher));
                    command.map(|msg| Message::Launch(Box::new(msg)))
                }
                loader::Message::View(loader::ViewMessage::SetupVault) => {
                    let (install, command) = Installer::new(
                        loader.datadir_path.clone(),
                        loader.network,
                        None,
                        UserFlow::CreateWallet,
                        false,
                        None,
                    );
                    self.state = State::Installer(Box::new(install));
                    command.map(|msg| Message::Install(Box::new(msg)))
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
                        self.state = State::App(app);
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
                    self.state = State::App(app);
                    command.map(|msg| Message::Run(Box::new(msg)))
                }
                loader::Message::App(Err(e), _) => {
                    tracing::error!("Failed to import backup: {e}");
                    Task::none()
                }

                _ => loader.update(*msg).map(|msg| Message::Load(Box::new(msg))),
            },
            (State::App(app), Message::Run(msg)) => {
                let app_msg = *msg;
                match app_msg {
                    app::Message::View(app::view::Message::SetupVault) => {
                        // Launch installer for vault setup from app - should return to app on Previous
                        let (install, command) = Installer::new(
                            app.datadir().clone(),
                            app.cache().network,
                            None,
                            UserFlow::CreateWallet,
                            true,  // launched from app
                            Some(app.cube_settings().clone()),  // pass cube settings for returning
                        );
                        self.state = State::Installer(Box::new(install));
                        command.map(|msg| Message::Install(Box::new(msg)))
                    }
                    _ => app.update(app_msg).map(|msg| Message::Run(Box::new(msg))),
                }
            }
            (State::PinEntry(pin_entry), Message::PinEntry(msg)) => match *msg {
                crate::pin_entry::Message::PinVerified => {
                    // PIN successfully verified, proceed to next state based on on_success
                    match &pin_entry.on_success {
                        crate::pin_entry::PinEntrySuccess::LoadApp { datadir, config, network } => {
                            let cube = pin_entry.cube().clone();
                            let (app, command) = App::new_without_wallet(
                                config.clone(),
                                datadir.clone(),
                                *network,
                                cube,
                            );
                            self.state = State::App(app);
                            command.map(|msg| Message::Run(Box::new(msg)))
                        }
                        crate::pin_entry::PinEntrySuccess::LoadLoader { 
                            datadir, 
                            config, 
                            network, 
                            internal_bitcoind, 
                            backup, 
                            wallet_settings 
                        } => {
                            let cube = pin_entry.cube().clone();
                            if let Some(wallet_settings) = wallet_settings {
                                if wallet_settings.remote_backend_auth.is_some() {
                                    // Go to Login for remote backend
                                    let (login, command) = login::LianaLiteLogin::new(
                                        datadir.clone(),
                                        *network,
                                        wallet_settings.clone(),
                                    );
                                    self.state = State::Login(Box::new(login));
                                    command.map(|msg| Message::Login(Box::new(msg)))
                                } else {
                                    // Go to Loader for local wallet
                                    let (loader, command) = Loader::new(
                                        datadir.clone(),
                                        config.clone(),
                                        *network,
                                        internal_bitcoind.clone(),
                                        backup.clone(),
                                        Some(wallet_settings.clone()),
                                        cube,
                                    );
                                    self.state = State::Loader(Box::new(loader));
                                    command.map(|msg| Message::Load(Box::new(msg)))
                                }
                            } else {
                                // No wallet settings, load app without wallet
                                let (app, command) = App::new_without_wallet(
                                    config.clone(),
                                    datadir.clone(),
                                    *network,
                                    cube,
                                );
                                self.state = State::App(app);
                                command.map(|msg| Message::Run(Box::new(msg)))
                            }
                        }
                    }
                }
                crate::pin_entry::Message::Back => {
                    // Go back to launcher
                    let network = pin_entry.cube().network;
                    let (launcher, command) = Launcher::new(
                        match &pin_entry.on_success {
                            crate::pin_entry::PinEntrySuccess::LoadApp { datadir, .. } => datadir.clone(),
                            crate::pin_entry::PinEntrySuccess::LoadLoader { datadir, .. } => datadir.clone(),
                        },
                        Some(network),
                    );
                    self.state = State::Launcher(Box::new(launcher));
                    command.map(|msg| Message::Launch(Box::new(msg)))
                }
                _ => pin_entry.update(*msg).map(|msg| Message::PinEntry(Box::new(msg))),
            },
            _ => Task::none(),
        }
    }

    pub fn subscription(&self) -> Subscription<Message> {
        match &self.state {
            State::Installer(v) => v.subscription().map(|msg| Message::Install(Box::new(msg))),
            State::Loader(v) => v.subscription().map(|msg| Message::Load(Box::new(msg))),
            State::App(v) => v.subscription().map(|msg| Message::Run(Box::new(msg))),
            State::Launcher(v) => v.subscription().map(|msg| Message::Launch(Box::new(msg))),
            State::Login(_) => Subscription::none(),
            State::PinEntry(_) => Subscription::none(),
        }
    }

    pub fn view(&self) -> Element<Message> {
        match &self.state {
            State::Installer(v) => v.view().map(|msg| Message::Install(Box::new(msg))),
            State::App(v) => v.view().map(|msg| Message::Run(Box::new(msg))),
            State::Launcher(v) => v.view().map(|msg| Message::Launch(Box::new(msg))),
            State::Loader(v) => v.view().map(|msg| Message::Load(Box::new(msg))),
            State::Login(v) => v.view().map(|msg| Message::Login(Box::new(msg))),
            State::PinEntry(v) => v.view().map(|msg| Message::PinEntry(Box::new(msg))),
        }
    }

    pub fn stop(&mut self) {
        match &mut self.state {
            State::Loader(s) => s.stop(),
            State::Launcher(s) => s.stop(),
            State::Installer(s) => s.stop(),
            State::App(s) => s.stop(),
            State::Login(_) => {}
            State::PinEntry(_) => {}
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
            vault_expanded: true,
            active_expanded: false,
            has_vault: true,
        },
        Arc::new(
            Wallet::new(wallet.descriptor)
                .with_name(wallet.name)
                .with_alias(wallet.metadata.wallet_alias)
                .with_pinned_at(wallet_settings.pinned_at)
                .with_key_aliases(aliases)
                .with_provider_keys(provider_keys)
                .with_hardware_wallets(hws)
                .with_fiat_price_setting(wallet_settings.fiat_price)
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
