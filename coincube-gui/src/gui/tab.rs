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
        self, breez_liquid,
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
    Launcher(Launcher),
    Installer(Installer),
    Loader(Loader),
    Login(login::CoincubeLiteLogin),
    PinEntry(crate::pin_entry::PinEntry),
    App(App),
}

impl State {
    pub fn new(
        directory: CoincubeDirectory,
        network: Option<bitcoin::Network>,
    ) -> (Self, Task<Message>) {
        let (launcher, command) = Launcher::new(directory, network);
        (State::Launcher(launcher), command.map(Message::Launch))
    }
}

#[derive(Debug)]
pub enum Message {
    Launch(launcher::Message),
    Install(installer::Message),
    Load(loader::Message),
    Run(app::Message),
    Login(login::Message),
    PinEntry(crate::pin_entry::Message),
    RemoteBackendBreezLoaded {
        wallet_settings: WalletSettings,
        backend_client: BackendWalletClient,
        wallet: api::Wallet,
        coins: ListCoinsResult,
        datadir: CoincubeDirectory,
        network: bitcoin::Network,
        config: app::Config,
        breez_client: Result<Arc<app::breez_liquid::BreezClient>, app::breez_liquid::BreezError>,
    },
    BreezClientLoadedAfterPin {
        breez_client: Result<Arc<app::breez_liquid::BreezClient>, app::breez_liquid::BreezError>,
        /// Spark backend loaded in the same task as the Liquid client.
        /// `None` if the cube has no Spark signer configured; `Some(Err(..))`
        /// if the bridge subprocess failed to spawn or the handshake failed.
        /// A failure here is non-fatal — the gui logs and continues with
        /// `spark_backend = None`, which surfaces as "Spark unavailable" in
        /// the Spark panels.
        spark_backend: Option<Arc<app::wallets::SparkBackend>>,
        config: app::Config,
        datadir: CoincubeDirectory,
        network: bitcoin::Network,
        cube: app::settings::CubeSettings,
        wallet_settings: Option<WalletSettings>,
        internal_bitcoind: Option<crate::node::bitcoind::Bitcoind>,
        backup: Option<crate::backup::Backup>,
    },
    /// Bubbles up to GUI level to toggle the theme
    ToggleTheme,
}

pub struct Tab {
    pub id: usize,
    pub state: State,
    /// Persisted theme mode — carried across state transitions so new App
    /// caches inherit the correct mode immediately.
    pub theme_mode: coincube_ui::theme::palette::ThemeMode,
}

impl Tab {
    pub fn new(id: usize, state: State) -> Self {
        Tab {
            id,
            state,
            theme_mode: coincube_ui::theme::palette::ThemeMode::default(),
        }
    }

    pub fn cache(&self) -> Option<&Cache> {
        if let State::App(ref app) = self.state {
            Some(app.cache())
        } else {
            None
        }
    }

    pub fn set_theme_mode(&mut self, mode: coincube_ui::theme::palette::ThemeMode) {
        self.theme_mode = mode;
        match &mut self.state {
            State::App(app) => app.cache_mut().theme_mode = mode,
            State::Launcher(launcher) => launcher.theme_mode = mode,
            _ => {}
        }
    }

    /// Apply the tab's stored theme_mode to the current state.
    /// Call after any state transition to State::App or State::Launcher.
    fn sync_theme_mode(&mut self) {
        let mode = self.theme_mode;
        match &mut self.state {
            State::App(app) => app.cache_mut().theme_mode = mode,
            State::Launcher(launcher) => launcher.theme_mode = mode,
            _ => {}
        }
    }

    pub fn wallet(&self) -> Option<&Wallet> {
        if let State::App(ref app) = self.state {
            app.wallet()
        } else {
            None
        }
    }

    pub fn cube_settings(&self) -> Option<&app::settings::CubeSettings> {
        if let State::App(ref app) = self.state {
            Some(app.cube_settings())
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
            app.on_tick().map(Message::Run)
        } else {
            Task::none()
        }
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        use crate::app::settings::global::GlobalSettings;
        let result = match (&mut self.state, message) {
            (State::Launcher(l), Message::Launch(msg)) => match msg {
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
                    let (install, command) = Installer::new(
                        datadir, network, None, init, false, None, None, None, false,
                    );
                    self.state = State::Installer(install);
                    command.map(Message::Install)
                }
                launcher::Message::Run(datadir_path, cfg, network, cube) => {
                    if cube.is_passkey_cube() {
                        // Passkey Cubes don't have an encrypted mnemonic on
                        // disk — their master seed is re-derived from the
                        // WebAuthn PRF output on every open. That path isn't
                        // wired up yet (blocked on macOS code signing +
                        // associated-domains entitlement), so the only way
                        // to actually open a passkey Cube right now is via
                        // the mnemonic recovery flow.
                        //
                        // Refuse to open, surface a clear error to the user,
                        // and stay on the launcher. This prevents falling
                        // through to the PinEntry state and crashing on the
                        // (missing) mnemonic load.
                        tracing::warn!(
                            "Refusing to open passkey Cube '{}' — passkey auth flow is not \
                             wired up. The user must restore from their mnemonic backup.",
                            cube.name
                        );
                        let msg = if crate::feature_flags::PASSKEY_ENABLED {
                            "This Cube was created with a passkey. Passkey authentication \
                             on Cube open is not yet implemented. Restore from your mnemonic \
                             backup to access this Cube."
                                .to_string()
                        } else {
                            "This Cube was created with a passkey, but the passkey feature \
                             is currently disabled. Restore from your mnemonic backup to \
                             access this Cube, or re-enable COINCUBE_ENABLE_PASSKEY in your \
                             environment."
                                .to_string()
                        };
                        l.set_error(msg);
                        return Task::none();
                    }

                    // PIN entry
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

                    self.state = State::PinEntry(crate::pin_entry::PinEntry::new(cube, on_success));
                    Task::none()
                }
                launcher::Message::View(launcher::ViewMessage::ToggleTheme) => {
                    Task::done(Message::ToggleTheme)
                }
                _ => l.update(msg).map(Message::Launch),
            },
            (State::Login(l), Message::Login(msg)) => match msg {
                login::Message::View(login::ViewMessage::BackToLauncher(network)) => {
                    let (launcher, command) = Launcher::new(l.datadir.clone(), Some(network));
                    self.state = State::Launcher(launcher);
                    command.map(Message::Launch)
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
                        None, // No spark_backend from login screen
                        false,
                    );
                    self.state = State::Installer(install);
                    command.map(Message::Install)
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
                    Task::done(Message::RemoteBackendBreezLoaded {
                        wallet_settings: l.settings.clone(),
                        backend_client,
                        wallet,
                        coins,
                        datadir: l.datadir.clone(),
                        network: l.network,
                        config,
                        breez_client: Err(breez_liquid::BreezError::SignerError(
                            "BreezClient missing - should have been pre-loaded after PIN entry. \
                             Liquid wallet is encrypted and cannot be loaded without PIN."
                                .to_string(),
                        )),
                    })
                }
                _ => l.update(msg).map(Message::Login),
            },
            (State::Installer(i), Message::Install(msg)) => {
                if let installer::Message::Exit(settings, internal_bitcoind) = msg {
                    // Associate wallet with cube
                    let network_dir = i.datadir.network_directory(i.network);
                    let wallet_id = settings.wallet_id();
                    let wallet_alias = settings.alias.clone();
                    let network = i.network;

                    Task::perform(
                        async move {
                            find_or_create_cube(&network_dir, &wallet_id, &wallet_alias, network)
                                .await
                        },
                        move |result| {
                            Message::Install(installer::Message::CubeSaved(
                                result,
                                settings.clone(),
                                internal_bitcoind.clone(),
                            ))
                        },
                    )
                } else if let installer::Message::CubeSaved(result, settings, internal_bitcoind) =
                    msg
                {
                    // Handle cube save failure
                    let cube = match result {
                        Ok(c) => c,
                        Err(err) => {
                            error!("Aborting loader transition due to cube save failure");
                            return i
                                .update(installer::Message::CubeSaveFailed(err))
                                .map(Message::Install);
                        }
                    };

                    if settings.remote_backend_auth.is_some() {
                        let (login, command) = login::CoincubeLiteLogin::new(
                            i.datadir.clone(),
                            i.network,
                            *settings,
                            i.breez_client.clone(),
                        );
                        self.state = State::Login(login);
                        command.map(Message::Login)
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
                            None, // Installer path doesn't plumb Spark yet — follow-up
                        );
                        self.state = State::Loader(loader);
                        command.map(Message::Load)
                    }
                } else if let installer::Message::BackToApp(network) = msg {
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
                                i.spark_backend.clone(),
                                cfg,
                                i.datadir.clone(),
                                network,
                                cube.clone(),
                            );
                            self.state = State::App(app);
                            command.map(Message::Run)
                        } else {
                            error!(
                                "BackToApp called but no BreezClient stored - should not happen"
                            );
                            // Fallback: go to launcher
                            let (launcher, command) =
                                Launcher::new(i.destination_path(), Some(network));
                            self.state = State::Launcher(launcher);
                            command.map(Message::Launch)
                        }
                    } else {
                        // No cube settings stored, go to launcher
                        let (launcher, command) =
                            Launcher::new(i.destination_path(), Some(network));
                        self.state = State::Launcher(launcher);
                        command.map(Message::Launch)
                    }
                } else {
                    i.update(msg).map(Message::Install)
                }
            }
            (State::Loader(loader), Message::Load(msg)) => match msg {
                loader::Message::View(loader::ViewMessage::SwitchNetwork) => {
                    let (launcher, command) =
                        Launcher::new(loader.datadir_path.clone(), Some(loader.network));
                    self.state = State::Launcher(launcher);
                    command.map(Message::Launch)
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
                        None, // spark_backend not available from loader path
                        GlobalSettings::load_developer_mode(&GlobalSettings::path(
                            &loader.datadir_path,
                        )),
                    );
                    self.state = State::Installer(install);
                    command.map(Message::Install)
                }
                loader::Message::Synced(Ok((
                    wallet,
                    cache,
                    daemon,
                    bitcoind,
                    backup,
                    cube_settings,
                ))) => {
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
                                Message::Load(loader::Message::App(
                                    r, /* restored_from_backup */ true,
                                ))
                            },
                        )
                    } else {
                        // Check if BreezClient is already loaded
                        if let Some(breez) = loader.breez_client.clone() {
                            // Use pre-loaded BreezClient (came from PIN entry path)
                            return Task::done(Message::Load(loader::Message::BreezLoaded {
                                breez,
                                spark_backend: loader.spark_backend.clone(),
                                cache,
                                wallet,
                                config: loader.gui_config.clone(),
                                daemon,
                                datadir: loader.datadir_path.clone(),
                                bitcoind,
                                restored_from_backup: false,
                                cube_settings,
                            }));
                        }

                        // ERROR: BreezClient should have been pre-loaded after PIN entry
                        // With mandatory PINs, this path should never execute
                        error!("Loader Synced missing pre-loaded BreezClient - architectural bug");
                        Task::done(Message::Load(loader::Message::App(
                            Err(loader::Error::Unexpected(
                                "BreezClient missing - should have been pre-loaded after PIN entry. \
                                 Liquid wallet is encrypted and cannot be loaded without PIN.".to_string()
                            )),
                            false,
                        )))
                    }
                }
                loader::Message::App(
                    Ok((cache, wallet, config, daemon, datadir, bitcoind)),
                    restored_from_backup,
                ) => {
                    // Check if BreezClient is already loaded
                    if let Some(breez) = loader.breez_client.clone() {
                        // Use pre-loaded BreezClient (came from PIN entry path)
                        return Task::done(Message::Load(loader::Message::BreezLoaded {
                            breez,
                            spark_backend: loader.spark_backend.clone(),
                            cache,
                            wallet,
                            config,
                            daemon,
                            datadir,
                            bitcoind,
                            restored_from_backup,
                            cube_settings: loader.cube_settings.clone(),
                        }));
                    }

                    // ERROR: BreezClient should have been pre-loaded after PIN entry
                    // With mandatory PINs, this path should never execute
                    error!("Loader App missing pre-loaded BreezClient - architectural bug");
                    Task::done(Message::Load(loader::Message::App(
                        Err(loader::Error::Unexpected(
                            "BreezClient missing - should have been pre-loaded after PIN entry. \
                             Liquid wallet is encrypted and cannot be loaded without PIN."
                                .to_string(),
                        )),
                        restored_from_backup,
                    )))
                }
                loader::Message::BreezLoaded {
                    breez,
                    spark_backend,
                    cache,
                    wallet,
                    config,
                    daemon,
                    datadir,
                    bitcoind,
                    restored_from_backup,
                    cube_settings,
                } => {
                    let (app, command) = App::new(
                        cache,
                        wallet,
                        breez,
                        spark_backend,
                        config,
                        daemon,
                        datadir,
                        bitcoind,
                        restored_from_backup,
                        cube_settings,
                    );
                    self.state = State::App(app);
                    command.map(Message::Run)
                }
                loader::Message::App(Err(e), _) => {
                    tracing::error!("Failed to import backup: {e}");
                    Task::none()
                }

                _ => loader.update(msg).map(Message::Load),
            },
            (State::App(app), Message::Run(msg)) => {
                match msg {
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
                            app.spark_backend(),      // preserve Spark bridge across vault setup
                            GlobalSettings::load_developer_mode(&GlobalSettings::path(
                                app.datadir(),
                            )),
                        );
                        self.state = State::Installer(install);
                        command.map(Message::Install)
                    }
                    app::Message::View(app::view::Message::ToggleTheme) => {
                        Task::done(Message::ToggleTheme)
                    }
                    m => app.update(m).map(Message::Run),
                }
            }
            (State::PinEntry(pin_entry), Message::PinEntry(msg)) => match msg {
                crate::pin_entry::Message::PinVerified => {
                    // After PIN verification, load BreezClient before routing to App/Loader/Login
                    match &pin_entry.on_success {
                        crate::pin_entry::PinEntrySuccess::LoadApp {
                            datadir,
                            config,
                            network,
                            wallet_settings,
                            internal_bitcoind,
                            backup,
                        } => {
                            let cube = pin_entry.cube().clone();
                            let pin = pin_entry.pin();

                            // ALWAYS load BreezClient (Liquid wallet) with PIN first
                            let config_clone = config.clone();
                            let datadir_clone = datadir.clone();
                            let network_val = *network;
                            let wallet_settings_clone = wallet_settings.clone();
                            let internal_bitcoind_clone = internal_bitcoind.clone();
                            let backup_clone = backup.clone();

                            Task::perform(
                                async move {
                                    // Both Breez SDKs (Liquid + Spark) load
                                    // from the same master seed fingerprint.
                                    let breez_signer_fingerprint = cube.master_signer_fingerprint;

                                    let breez_result =
                                        if let Some(fingerprint) = breez_signer_fingerprint {
                                            breez_liquid::load_breez_client(
                                                datadir_clone.path(),
                                                network_val,
                                                fingerprint,
                                                &pin,
                                            )
                                            .await
                                        } else {
                                            Err(breez_liquid::BreezError::SignerError(
                                                "No Liquid wallet configured".to_string(),
                                            ))
                                        };

                                    // Load Spark backend alongside Liquid. Failures
                                    // here are non-fatal — we log + return None so
                                    // the gui can continue with Liquid-only and the
                                    // Spark panels surface a placeholder. The load
                                    // path spawns the bridge subprocess
                                    // (coincube-spark-bridge), performs the init
                                    // handshake with the cube's mnemonic, and
                                    // returns an Arc<SparkClient> on success.
                                    let spark_backend =
                                        if let Some(fingerprint) = breez_signer_fingerprint {
                                            match app::breez_spark::load_spark_client(
                                                datadir_clone.path(),
                                                network_val,
                                                fingerprint,
                                                &pin,
                                            )
                                            .await
                                            {
                                                Ok(client) => Some(Arc::new(
                                                    app::wallets::SparkBackend::new(client),
                                                )),
                                                Err(e) => {
                                                    tracing::warn!(
                                                        "Spark bridge unavailable, continuing \
                                                     without Spark: {}",
                                                        e
                                                    );
                                                    None
                                                }
                                            }
                                        } else {
                                            None
                                        };

                                    (
                                        config_clone,
                                        datadir_clone,
                                        network_val,
                                        cube,
                                        breez_result,
                                        spark_backend,
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
                                    spark_backend,
                                    wallet_settings,
                                    internal_bitcoind,
                                    backup,
                                )| {
                                    Message::BreezClientLoadedAfterPin {
                                        breez_client: breez_result,
                                        spark_backend,
                                        config,
                                        datadir,
                                        network,
                                        cube,
                                        wallet_settings,
                                        internal_bitcoind,
                                        backup,
                                    }
                                },
                            )
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
                    self.state = State::Launcher(launcher);
                    command.map(Message::Launch)
                }
                m => pin_entry.update(m).map(Message::PinEntry),
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
                // The Vault is independent of Liquid: any Breez load failure
                // should fall back to a disconnected client so the rest of the
                // app continues to work. The user will see Liquid features
                // surface their own errors on demand.
                let breez = match breez_client {
                    Ok(breez) => breez,
                    Err(e) => {
                        tracing::warn!(
                            "BreezClient unavailable for remote backend, continuing in disconnected mode: {}",
                            e
                        );
                        Arc::new(app::breez_liquid::BreezClient::disconnected(network))
                    }
                };
                match create_app_with_remote_backend(
                    wallet_settings,
                    backend_client,
                    wallet,
                    coins,
                    datadir.clone(),
                    network,
                    config,
                    breez,
                ) {
                    Ok((app, command)) => {
                        self.state = State::App(app);
                        command.map(Message::Run)
                    }
                    Err(e) => {
                        tracing::error!("Failed to create app with remote backend: {}", e);
                        let (launcher, command) = Launcher::new(datadir, Some(network));
                        self.state = State::Launcher(launcher);
                        command.map(Message::Launch)
                    }
                }
            }
            (
                _,
                Message::BreezClientLoadedAfterPin {
                    breez_client,
                    spark_backend,
                    config,
                    datadir,
                    network,
                    cube,
                    wallet_settings,
                    internal_bitcoind,
                    backup,
                },
            ) => {
                // The Vault is independent of Liquid: any Breez load failure
                // (NetworkNotSupported, transient connection errors, SDK
                // throttling, etc.) should fall back to a disconnected client
                // so the user can still access their Vault. Liquid features
                // will surface their own errors on demand.
                let breez = match breez_client {
                    Ok(breez) => breez,
                    Err(app::breez_liquid::BreezError::NetworkNotSupported(_)) => {
                        Arc::new(app::breez_liquid::BreezClient::disconnected(network))
                    }
                    Err(e) => {
                        tracing::warn!(
                            "BreezClient unavailable after PIN, continuing in disconnected mode: {}",
                            e
                        );
                        Arc::new(app::breez_liquid::BreezClient::disconnected(network))
                    }
                };
                if let Some(wallet_settings) = wallet_settings {
                    if wallet_settings.remote_backend_auth.is_some() {
                        // Remote-backend login path doesn't plumb Spark yet —
                        // the remote backend uses `create_app_with_remote_backend`
                        // which takes its own `None` for Spark. Wiring the
                        // remote-backend Spark path is a follow-up.
                        let (login, command) = login::CoincubeLiteLogin::new(
                            datadir.clone(),
                            network,
                            wallet_settings.clone(),
                            Some(breez),
                        );
                        self.state = State::Login(login);
                        command.map(Message::Login)
                    } else {
                        let (loader, command) = Loader::new(
                            datadir.clone(),
                            config.clone(),
                            network,
                            internal_bitcoind.clone(),
                            backup.clone(),
                            Some(wallet_settings.clone()),
                            cube,
                            Some(breez),
                            spark_backend,
                        );
                        self.state = State::Loader(loader);
                        command.map(Message::Load)
                    }
                } else {
                    let (app, command) = App::new_without_wallet(
                        breez,
                        spark_backend,
                        config,
                        datadir,
                        network,
                        cube,
                    );
                    self.state = State::App(app);
                    command.map(Message::Run)
                }
            }
            _ => Task::none(),
        };
        self.sync_theme_mode();
        result
    }

    pub fn subscription(&self) -> Subscription<Message> {
        match &self.state {
            State::Installer(v) => v.subscription().map(Message::Install),
            State::Loader(v) => v.subscription().map(Message::Load),
            State::App(v) => v.subscription().map(Message::Run),
            State::Launcher(v) => v.subscription().map(Message::Launch),
            State::Login(_) => Subscription::none(),
            State::PinEntry(_) => Subscription::none(),
        }
    }

    pub fn view(&self) -> Element<Message> {
        match &self.state {
            State::Installer(v) => v.view().map(Message::Install),
            State::App(v) => v.view().map(Message::Run),
            State::Launcher(v) => v.view().map(Message::Launch),
            State::Loader(v) => v.view().map(Message::Load),
            State::Login(v) => v.view().map(Message::Login),
            State::PinEntry(v) => v.view().map(Message::PinEntry),
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

async fn save_cube_settings(
    network_dir: &NetworkDirectory,
    cube: app::settings::CubeSettings,
    network: bitcoin::Network,
    settings_data: app::settings::Settings,
) -> Result<app::settings::CubeSettings, String> {
    let cube_name = cube.name.clone();
    let settings_path = network_dir.path().join("settings.json");

    let save_result = update_settings_file(network_dir, |_| Some(settings_data)).await;

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
            Err(e.to_string())
        }
    }
}

async fn find_or_create_cube(
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

                return save_cube_settings(network_dir, cube_clone, network, settings_data).await;
            }

            // Third, create a new cube for this wallet
            let cube = app::settings::CubeSettings::new(
                wallet_alias
                    .clone()
                    .unwrap_or_else(|| format!("My {} Cube", network)),
                network,
            )
            .with_vault(wallet_id.clone());
            let cube_name = cube.name.clone();

            info!(
                "Creating new cube '{}' for wallet {} on {} network",
                cube_name, wallet_id, network
            );

            settings_data.cubes.push(cube.clone());
            save_cube_settings(network_dir, cube, network, settings_data).await
        }
        Err(_) => {
            // No settings file yet, create first cube
            let cube = app::settings::CubeSettings::new(
                wallet_alias
                    .clone()
                    .unwrap_or_else(|| format!("My {} Cube", network)),
                network,
            )
            .with_vault(wallet_id.clone());
            let cube_name = cube.name.clone();

            info!(
                "Creating first cube '{}' for wallet {} on {} network",
                cube_name, wallet_id, network
            );

            let mut new_settings = app::settings::Settings::default();
            new_settings.cubes.push(cube.clone());

            save_cube_settings(network_dir, cube, network, new_settings).await
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn create_app_with_remote_backend(
    wallet_settings: WalletSettings,
    remote_backend: BackendWalletClient,
    wallet: api::Wallet,
    coins: ListCoinsResult,
    coincube_dir: CoincubeDirectory,
    network: bitcoin::Network,
    config: app::Config,
    breez_client: Arc<app::breez_liquid::BreezClient>,
) -> Result<(app::App, iced::Task<app::Message>), String> {
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

    // Load cube settings for this wallet
    let network_dir = coincube_dir.network_directory(network);
    let wallet_id = wallet_settings.wallet_id();

    let cube_settings = match app::settings::Settings::from_file(&network_dir) {
        Ok(settings) => {
            if let Some(found_cube) = settings
                .cubes
                .iter()
                .find(|c| c.vault_wallet_id.as_ref() == Some(&wallet_id))
            {
                found_cube.clone()
            } else {
                tracing::error!("No cube found for vault wallet in settings file");
                return Err(
                    "No cube found for this wallet. Please ensure your settings are properly configured."
                        .to_string(),
                );
            }
        }
        Err(_) => {
            tracing::error!("No settings file found for remote backend");
            return Err(
                "No settings file found. Please ensure your wallet is properly set up with a PIN."
                    .to_string(),
            );
        }
    };

    Ok(App::new(
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
            bitcoin_unit: cube_settings.unit_setting.display_unit,
            node_bitcoind_sync_progress: None,
            node_bitcoind_ibd: None,
            node_bitcoind_last_log: None,
            vault_expanded: false,
            spark_expanded: false,
            liquid_expanded: false,
            marketplace_expanded: false,
            marketplace_p2p_expanded: false,
            connect_expanded: false,
            connect_authenticated: false,
            has_vault: true,
            cube_name: cube_settings.name.clone(),
            current_cube_backed_up: cube_settings.backed_up,
            current_cube_is_passkey: cube_settings.is_passkey_cube(),
            has_p2p: false, // Set later by App::new based on mnemonic availability
            theme_mode: coincube_ui::theme::palette::ThemeMode::default(),
            btc_usd_price: None,
            show_direction_badges: true,
            lightning_address: None,
            cube_id: cube_settings.id.clone(),
            default_lightning_backend: cube_settings.default_lightning_backend,
        },
        Arc::new(
            Wallet::new(wallet.descriptor)
                .with_name(wallet.name)
                .with_alias(wallet.metadata.wallet_alias)
                .with_pinned_at(wallet_settings.pinned_at)
                .with_key_aliases(aliases)
                .with_provider_keys(provider_keys)
                .with_border_wallet_fingerprints(wallet_settings.border_wallet_fingerprints())
                .with_hardware_wallets(hws)
                .load_hotsigners(&coincube_dir, network)
                .expect("Datadir should be conform"),
        ),
        breez_client,
        None, // Spark backend — Phase 4 wires the runtime spawn
        config,
        Arc::new(remote_backend),
        coincube_dir,
        None,
        false,
        cube_settings,
    ))
}
