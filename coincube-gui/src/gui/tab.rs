use std::{collections::HashMap, sync::Arc, time::Instant};

use iced::{Subscription, Task};
use tracing::{error, info};
extern crate serde;
extern crate serde_json;

use coincube_core::miniscript::bitcoin;
use coincube_ui::widget::Element;
use coincubed::commands::ListCoinsResult;

use crate::{
    app::{
        self, breez,
        cache::{Cache, DaemonCache},
        settings::{update_settings_file, WalletId, WalletSettings},
        wallet::Wallet,
        App,
    },
    dir::{CoincubeDirectory, NetworkDirectory},
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
    Login(Box<login::CoincubeLiteLogin>),
    PinEntry(Box<crate::pin_entry::PinEntry>),
    App(App),
}

impl State {
    pub fn new(
        directory: CoincubeDirectory,
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
    RemoteBackendBreezLoaded {
        wallet_settings: WalletSettings,
        backend_client: BackendWalletClient,
        wallet: api::Wallet,
        coins: ListCoinsResult,
        datadir: CoincubeDirectory,
        network: bitcoin::Network,
        config: app::Config,
        breez_client: Result<Arc<app::breez::BreezClient>, app::breez::BreezError>,
    },
    BreezClientLoadedAfterPin {
        breez_client: Result<Arc<app::breez::BreezClient>, app::breez::BreezError>,
        config: app::Config,
        datadir: CoincubeDirectory,
        network: bitcoin::Network,
        cube: app::settings::CubeSettings,
        wallet_settings: Option<WalletSettings>,
        internal_bitcoind: Option<crate::node::bitcoind::Bitcoind>,
        backup: Option<crate::backup::Backup>,
    },
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
                    let (install, command) =
                        Installer::new(datadir, network, None, init, false, None, None);
                    self.state = State::Installer(Box::new(install));
                    command.map(|msg| Message::Install(Box::new(msg)))
                }
                launcher::Message::Run(datadir_path, cfg, network, cube) => {
                    // PIN is always required - determine what to do after PIN verification
                    // Try to load Vault wallet settings if cube has a vault configured
                    let wallet_settings = cube.vault_wallet_id.as_ref().and_then(|vault_id| {
                        let network_dir = datadir_path.network_directory(network);
                        app::settings::Settings::from_file(&network_dir)
                            .ok()
                            .and_then(|s| {
                                s.wallets
                                    .iter()
                                    .find(|w| w.wallet_id() == *vault_id)
                                    .cloned()
                            })
                    });

                    let on_success = crate::pin_entry::PinEntrySuccess::LoadApp {
                        datadir: datadir_path,
                        config: cfg,
                        network,
                        internal_bitcoind: None,
                        backup: None,
                        wallet_settings,
                    };

                    let pin_entry = crate::pin_entry::PinEntry::new(cube, on_success);
                    self.state = State::PinEntry(Box::new(pin_entry));
                    Task::none()
                }
                launcher::Message::BreezClientLoaded {
                    config,
                    datadir,
                    network,
                    cube,
                    breez_client,
                } => {
                    match breez_client {
                        Ok(breez) => {
                            let (app, command) =
                                App::new_without_wallet(breez, config, datadir, network, cube);
                            self.state = State::App(app);
                            command.map(|msg| Message::Run(Box::new(msg)))
                        }
                        Err(e) => {
                            tracing::error!("Failed to load BreezClient: {}", e);
                            // BreezClient failed to load - return to launcher
                            let (launcher, command) = Launcher::new(datadir.clone(), Some(network));
                            self.state = State::Launcher(Box::new(launcher));
                            command.map(|msg| Message::Launch(Box::new(msg)))
                        }
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
                        None, // No breez_client from login screen
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

                    // Check if BreezClient is already loaded (from PIN entry)
                    if let Some(breez) = l.breez_client.clone() {
                        // Use pre-loaded BreezClient - already has PIN
                        return Task::done(Message::RemoteBackendBreezLoaded {
                            wallet_settings: l.settings.clone(),
                            backend_client,
                            wallet,
                            coins,
                            datadir: l.datadir.clone(),
                            network: l.network,
                            config,
                            breez_client: Ok(breez),
                        });
                    }

                    // ERROR: BreezClient should have been pre-loaded after PIN entry
                    // With mandatory PINs, this path should never execute
                    error!("Login state missing pre-loaded BreezClient - architectural bug");
                    return Task::done(Message::RemoteBackendBreezLoaded {
                        wallet_settings: l.settings.clone(),
                        backend_client,
                        wallet,
                        coins,
                        datadir: l.datadir.clone(),
                        network: l.network,
                        config,
                        breez_client: Err(breez::BreezError::SignerError(
                            "BreezClient missing - should have been pre-loaded after PIN entry. \
                             Active wallet is encrypted and cannot be loaded without PIN."
                                .to_string(),
                        )),
                    });
                }
                _ => l.update(*msg).map(|msg| Message::Login(Box::new(msg))),
            },
            (State::Installer(i), Message::Install(msg)) => {
                if let installer::Message::Exit(settings, internal_bitcoind) = *msg {
                    // Associate wallet with cube
                    let network_dir = i.datadir.network_directory(i.network);

                    let cube_result = find_or_create_cube(
                        &network_dir,
                        &settings.wallet_id(),
                        &settings.alias,
                        i.network,
                    );

                    // Handle cube save failure
                    let cube = match cube_result {
                        Ok(c) => c,
                        Err(_error_msg) => {
                            error!("Aborting loader transition due to cube save failure");
                            if i.launched_from_app {
                                // Return to app state
                                return Task::done(Message::Install(Box::new(
                                    installer::Message::BackToApp(i.network),
                                )));
                            } else {
                                // Return to launcher
                                let (launcher, command) =
                                    Launcher::new(i.datadir.clone(), Some(i.network));
                                self.state = State::Launcher(Box::new(launcher));
                                return command.map(|msg| Message::Launch(Box::new(msg)));
                            }
                        }
                    };

                    if settings.remote_backend_auth.is_some() {
                        let (login, command) = login::CoincubeLiteLogin::new(
                            i.datadir.clone(),
                            i.network,
                            *settings,
                            i.breez_client.clone(),
                        );
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

                        let (loader, command) = Loader::new(
                            i.datadir.clone(),
                            cfg,
                            i.network,
                            internal_bitcoind,
                            i.context.backup.clone(),
                            Some(*settings),
                            cube.clone(),
                            i.breez_client.clone(), // Pass pre-loaded BreezClient from installer
                        );
                        self.state = State::Loader(Box::new(loader));
                        command.map(|msg| Message::Load(Box::new(msg)))
                    }
                } else if let installer::Message::BackToApp(network) = *msg {
                    // Go back to app without vault using stored cube settings and breez_client
                    if let Some(cube) = &i.cube_settings {
                        if let Some(breez) = &i.breez_client {
                            // Use the pre-loaded BreezClient (no PIN re-entry needed)
                            let cfg = app::Config::from_file(
                                &i.datadir
                                    .network_directory(network)
                                    .path()
                                    .join(app::config::DEFAULT_FILE_NAME),
                            )
                            .expect("A gui configuration file must be present");

                            let (app, command) = app::App::new_without_wallet(
                                breez.clone(),
                                cfg,
                                i.datadir.clone(),
                                network,
                                cube.clone(),
                            );
                            self.state = State::App(app);
                            return command.map(|msg| Message::Run(Box::new(msg)));
                        } else {
                            error!(
                                "BackToApp called but no BreezClient stored - should not happen"
                            );
                            // Fallback: go to launcher
                            let (launcher, command) =
                                Launcher::new(i.destination_path(), Some(network));
                            self.state = State::Launcher(Box::new(launcher));
                            return command.map(|msg| Message::Launch(Box::new(msg)));
                        }
                    } else {
                        // No cube settings stored, go to launcher
                        let (launcher, command) =
                            Launcher::new(i.destination_path(), Some(network));
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
                    // Launch installer for vault setup from loader - should return to app on Previous
                    let (install, command) = Installer::new(
                        loader.datadir_path.clone(),
                        loader.network,
                        None,
                        UserFlow::CreateWallet,
                        true, // launched from app (loader is part of app flow)
                        Some(loader.cube_settings.clone()), // pass cube settings for returning
                        loader.breez_client.clone(), // pass breez_client to avoid re-entering PIN
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
                        // Check if BreezClient is already loaded
                        if let Some(breez) = loader.breez_client.clone() {
                            // Use pre-loaded BreezClient (came from PIN entry path)
                            return Task::done(Message::Load(Box::new(
                                loader::Message::BreezLoaded {
                                    breez,
                                    cache,
                                    wallet,
                                    config: loader.gui_config.clone(),
                                    daemon,
                                    datadir: loader.datadir_path.clone(),
                                    bitcoind,
                                    restored_from_backup: false,
                                },
                            )));
                        }

                        // ERROR: BreezClient should have been pre-loaded after PIN entry
                        // With mandatory PINs, this path should never execute
                        error!("Loader Synced missing pre-loaded BreezClient - architectural bug");
                        return Task::done(Message::Load(Box::new(loader::Message::App(
                            Err(loader::Error::Unexpected(
                                "BreezClient missing - should have been pre-loaded after PIN entry. \
                                 Active wallet is encrypted and cannot be loaded without PIN.".to_string()
                            )),
                            false,
                        ))));
                    }
                }
                loader::Message::App(
                    Ok((cache, wallet, config, daemon, datadir, bitcoind)),
                    restored_from_backup,
                ) => {
                    // Check if BreezClient is already loaded
                    if let Some(breez) = loader.breez_client.clone() {
                        // Use pre-loaded BreezClient (came from PIN entry path)
                        return Task::done(Message::Load(Box::new(loader::Message::BreezLoaded {
                            breez,
                            cache,
                            wallet,
                            config,
                            daemon,
                            datadir,
                            bitcoind,
                            restored_from_backup,
                        })));
                    }

                    // ERROR: BreezClient should have been pre-loaded after PIN entry
                    // With mandatory PINs, this path should never execute
                    error!("Loader App missing pre-loaded BreezClient - architectural bug");
                    return Task::done(Message::Load(Box::new(loader::Message::App(
                        Err(loader::Error::Unexpected(
                            "BreezClient missing - should have been pre-loaded after PIN entry. \
                             Active wallet is encrypted and cannot be loaded without PIN."
                                .to_string(),
                        )),
                        restored_from_backup,
                    ))));
                }
                loader::Message::BreezLoaded {
                    breez,
                    cache,
                    wallet,
                    config,
                    daemon,
                    datadir,
                    bitcoind,
                    restored_from_backup,
                } => {
                    let (app, command) = App::new(
                        cache,
                        wallet,
                        breez,
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
                            true,                              // launched from app
                            Some(app.cube_settings().clone()), // pass cube settings for returning
                            Some(app.breez_client()), // pass breez_client to avoid re-entering PIN
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
                        crate::pin_entry::PinEntrySuccess::LoadApp {
                            datadir,
                            config,
                            network,
                            internal_bitcoind,
                            backup,
                            wallet_settings,
                        } => {
                            let cube = pin_entry.cube().clone();
                            let pin = pin_entry.pin();

                            // ALWAYS load BreezClient (Active wallet) with PIN first
                            let config_clone = config.clone();
                            let datadir_clone = datadir.clone();
                            let network_val = *network;
                            let wallet_settings_clone = wallet_settings.clone();
                            let internal_bitcoind_clone = internal_bitcoind.clone();
                            let backup_clone = backup.clone();

                            return Task::perform(
                                async move {
                                    // Load BreezClient for Active wallet with PIN
                                    let breez_result = if let Some(fingerprint) =
                                        cube.active_wallet_signer_fingerprint
                                    {
                                        breez::load_breez_client(
                                            datadir_clone.path(),
                                            network_val,
                                            fingerprint,
                                            Some(&pin),
                                        )
                                        .await
                                    } else {
                                        Err(breez::BreezError::SignerError(
                                            "No Active wallet configured".to_string(),
                                        ))
                                    };

                                    (
                                        config_clone,
                                        datadir_clone,
                                        network_val,
                                        cube,
                                        breez_result,
                                        wallet_settings_clone,
                                        internal_bitcoind_clone,
                                        backup_clone,
                                    )
                                },
                                |(
                                    config,
                                    datadir,
                                    network,
                                    cube,
                                    breez_result,
                                    wallet_settings,
                                    internal_bitcoind,
                                    backup,
                                )| {
                                    Message::BreezClientLoadedAfterPin {
                                        breez_client: breez_result,
                                        config,
                                        datadir,
                                        network,
                                        cube,
                                        wallet_settings,
                                        internal_bitcoind,
                                        backup,
                                    }
                                },
                            );
                        }
                    }
                }
                crate::pin_entry::Message::Back => {
                    // Go back to launcher
                    let network = pin_entry.cube().network;
                    let (launcher, command) = Launcher::new(
                        match &pin_entry.on_success {
                            crate::pin_entry::PinEntrySuccess::LoadApp { datadir, .. } => {
                                datadir.clone()
                            }
                        },
                        Some(network),
                    );
                    self.state = State::Launcher(Box::new(launcher));
                    command.map(|msg| Message::Launch(Box::new(msg)))
                }
                _ => pin_entry
                    .update(*msg)
                    .map(|msg| Message::PinEntry(Box::new(msg))),
            },
            (
                _,
                Message::RemoteBackendBreezLoaded {
                    wallet_settings,
                    backend_client,
                    wallet,
                    coins,
                    datadir,
                    network,
                    config,
                    breez_client,
                },
            ) => {
                match breez_client {
                    Ok(breez) => {
                        let (app, command) = create_app_with_remote_backend(
                            wallet_settings,
                            backend_client,
                            wallet,
                            coins,
                            datadir,
                            network,
                            config,
                            breez,
                        );
                        self.state = State::App(app);
                        command.map(|msg| Message::Run(Box::new(msg)))
                    }
                    Err(e) => {
                        // Failed to load BreezClient - return to launcher with error
                        tracing::error!("Failed to load BreezClient for remote backend: {}", e);
                        let (launcher, command) = Launcher::new(datadir, Some(network));
                        self.state = State::Launcher(Box::new(launcher));
                        command.map(|msg| Message::Launch(Box::new(msg)))
                    }
                }
            }
            (
                _,
                Message::BreezClientLoadedAfterPin {
                    breez_client,
                    config,
                    datadir,
                    network,
                    cube,
                    wallet_settings,
                    internal_bitcoind,
                    backup,
                },
            ) => {
                match breez_client {
                    Ok(breez) => {
                        // BreezClient loaded successfully, now route based on Vault existence
                        if let Some(wallet_settings) = wallet_settings {
                            if wallet_settings.remote_backend_auth.is_some() {
                                // Remote backend: Pass pre-loaded BreezClient to Login
                                let (login, command) = login::CoincubeLiteLogin::new(
                                    datadir.clone(),
                                    network,
                                    wallet_settings.clone(),
                                    Some(breez), // Pass pre-loaded BreezClient
                                );
                                self.state = State::Login(Box::new(login));
                                command.map(|msg| Message::Login(Box::new(msg)))
                            } else {
                                // Local wallet: Pass pre-loaded BreezClient to Loader
                                let (loader, command) = Loader::new(
                                    datadir.clone(),
                                    config.clone(),
                                    network,
                                    internal_bitcoind.clone(),
                                    backup.clone(),
                                    Some(wallet_settings.clone()),
                                    cube,
                                    Some(breez), // Pass pre-loaded BreezClient
                                );
                                self.state = State::Loader(Box::new(loader));
                                command.map(|msg| Message::Load(Box::new(msg)))
                            }
                        } else {
                            // No Vault - create App directly with BreezClient
                            let (app, command) =
                                App::new_without_wallet(breez, config, datadir, network, cube);
                            self.state = State::App(app);
                            command.map(|msg| Message::Run(Box::new(msg)))
                        }
                    }
                    Err(e) => {
                        tracing::error!("Failed to load BreezClient after PIN: {}", e);
                        // BreezClient failed to load - return to launcher
                        let (launcher, command) = Launcher::new(datadir.clone(), Some(network));
                        self.state = State::Launcher(Box::new(launcher));
                        command.map(|msg| Message::Launch(Box::new(msg)))
                    }
                }
            }
            (_, Message::Launch(msg)) => {
                // Handle BreezClientLoaded from any state (e.g., after PIN entry)
                if let launcher::Message::BreezClientLoaded {
                    config,
                    datadir,
                    network,
                    cube,
                    breez_client,
                } = *msg
                {
                    match breez_client {
                        Ok(breez) => {
                            let (app, command) =
                                App::new_without_wallet(breez, config, datadir, network, cube);
                            self.state = State::App(app);
                            command.map(|msg| Message::Run(Box::new(msg)))
                        }
                        Err(e) => {
                            tracing::error!("Failed to load BreezClient: {}", e);
                            // BreezClient failed to load - return to launcher
                            let (launcher, command) = Launcher::new(datadir.clone(), Some(network));
                            self.state = State::Launcher(Box::new(launcher));
                            command.map(|msg| Message::Launch(Box::new(msg)))
                        }
                    }
                } else {
                    Task::none()
                }
            }
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

fn save_cube_settings(
    network_dir: &NetworkDirectory,
    cube: app::settings::CubeSettings,
    network: bitcoin::Network,
    settings_data: app::settings::Settings,
) -> Result<app::settings::CubeSettings, String> {
    let cube_name = cube.name.clone();
    let settings_path = network_dir.path().join("settings.json");

    let save_result = tokio::runtime::Handle::current()
        .block_on(async { update_settings_file(network_dir, |_| Some(settings_data)).await });

    match save_result {
        Ok(_) => {
            info!(
                "Successfully saved cube '{}' on {} network",
                cube_name, network
            );
            Ok(cube)
        }
        Err(e) => {
            error!(
                "Failed to save cube '{}' on {} network to {:?}: {}",
                cube_name, network, settings_path, e
            );
            Err(format!("Failed to save cube configuration: {}", e))
        }
    }
}

fn find_or_create_cube(
    network_dir: &NetworkDirectory,
    wallet_id: &WalletId,
    wallet_alias: &Option<String>,
    network: bitcoin::Network,
) -> Result<app::settings::CubeSettings, String> {
    match app::settings::Settings::from_file(network_dir) {
        Ok(mut settings_data) => {
            // First, check if a cube already has this wallet
            if let Some(existing_cube) = settings_data
                .cubes
                .iter()
                .find(|c| c.vault_wallet_id.as_ref() == Some(wallet_id))
            {
                return Ok(existing_cube.clone());
            }

            // Second, find a cube without a vault and associate this wallet with it
            if let Some(empty_cube) = settings_data
                .cubes
                .iter_mut()
                .find(|c| c.vault_wallet_id.is_none())
            {
                empty_cube.vault_wallet_id = Some(wallet_id.clone());
                let cube_clone = empty_cube.clone();
                let cube_name = empty_cube.name.clone();

                info!(
                    "Associating wallet {} with existing cube '{}' on {} network",
                    wallet_id, cube_name, network
                );

                return save_cube_settings(network_dir, cube_clone, network, settings_data);
            }

            // No existing Cube found to associate Vault with
            // Users must create a Cube first (through launcher) before adding a Vault
            Err(format!(
                "No Cube available to associate Vault wallet with. Please create a Cube first from the launcher."
            ))
        }
        Err(_) => {
            // No settings file exists yet
            // Users must create a Cube first (through launcher) before adding a Vault
            Err(format!(
                "No Cube available to associate Vault wallet with. Please create a Cube first from the launcher."
            ))
        }
    }
}

pub fn create_app_with_remote_backend(
    wallet_settings: WalletSettings,
    remote_backend: BackendWalletClient,
    wallet: api::Wallet,
    coins: ListCoinsResult,
    coincube_dir: CoincubeDirectory,
    network: bitcoin::Network,
    config: app::Config,
    breez_client: Arc<app::breez::BreezClient>,
) -> (app::App, iced::Task<app::Message>) {
    // If someone modified the wallet_alias on Liana-Connect,
    // then the new alias is imported and stored in the settings file.
    if wallet.metadata.wallet_alias != wallet_settings.alias {
        let network_directory = coincube_dir.network_directory(network);
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
                Some(settings)
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
            datadir_path: coincube_dir.clone(),
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
            vault_expanded: false,
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
                .load_hotsigners(&coincube_dir, network)
                .expect("Datadir should be conform"),
        ),
        breez_client,
        config,
        Arc::new(remote_backend),
        coincube_dir,
        None,
        false,
    )
}
