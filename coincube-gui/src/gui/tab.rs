use std::{collections::HashMap, sync::Arc, time::Instant};

use iced::{Subscription, Task};
use tracing::{error, info, warn};
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
    home::{self, Home},
    hw::HardwareWalletConfig,
    installer::{self, Installer, UserFlow},
    loader::{self, Loader},
    services::connect::{
        client::backend::{api, BackendWalletClient},
        login,
    },
};

pub enum State {
    Home(Home),
    Installer(Installer),
    Loader(Loader),
    Login(login::CoincubeLiteLogin),
    PinEntry(crate::pin_entry::PinEntry),
    App(App),
    /// Cryptic "Duress Mode Activated" dead-end. Entered when a duress PIN is
    /// detected at Cube unlock (after the wipe runs); the device is effectively
    /// retired until duress clears server-side.
    DuressActive(crate::app::view::duress::active_screen::DuressActiveScreen),
}

impl State {
    pub fn new(
        directory: CoincubeDirectory,
        network: Option<bitcoin::Network>,
    ) -> (Self, Task<Message>) {
        // Duress launch-time reconcile (Phase 5 Task 5.2, path 1). If this
        // device is locked into duress — or a wipe was interrupted (journal
        // marker present) — complete the wipe and route straight to the cryptic
        // screen. The user clears from another trusted device; the Sign-in
        // button here only confirms whether that has happened.
        let root = directory.path().to_path_buf();
        let st = crate::services::duress::DuressLocalState::load(&root).unwrap_or_default();
        let journal = crate::services::duress::journal::WipeJournal::new(&root);
        // Phase 4: resume draining any pending activation POSTs left by a prior
        // session (the durable queue survives restarts). Started here so an
        // offline-at-activation device eventually signals Connect.
        let drain = duress_drain_task(&root);
        if st.active || journal.is_pending() {
            complete_pending_wipe(&root, &journal);
            let queue_pending = crate::services::duress::queue::DuressQueue::new(&root)
                .is_empty()
                .map(|empty| !empty)
                .unwrap_or(false);
            let mut screen =
                crate::app::view::duress::active_screen::DuressActiveScreen::with_context(
                    directory, network,
                );
            screen.queue_pending = queue_pending;
            return (State::DuressActive(screen), drain);
        }

        let (home, command) = Home::new(directory, network);
        (
            State::Home(home),
            Task::batch([command.map(Message::Launch), drain]),
        )
    }
}

/// The set of Cube material a duress wipe must obliterate, under EVERY
/// per-network directory below the data root. A duress wipe takes every Cube on
/// the device regardless of which network's Cube triggered it, so activation
/// and the launch-time reconcile must agree on this set.
///
/// Per network directory:
/// - `data/` — wallet databases (BDK, plus breez/spark per-Cube working data
///   under `data/<wallet_id>/`),
/// - `mnemonics/` — the master seed phrases (the crown jewels),
/// - `settings.json` — `security_pin_hash`, `duress_pin_hash`, Cube metadata.
///
/// `connect.json` (the cached Connect auth the cryptic screen needs to check
/// duress state) is deliberately NOT listed, so it survives — as do the
/// root-level duress stores (`duress-*.json`, `duress.key`, the journal), which
/// live outside any network directory. Re-checking existence each call makes
/// this robust to an interrupted wipe: whatever remains is targeted again.
fn duress_wipe_targets(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    const CUBE_MATERIAL: &[&str] = &["data", "mnemonics", "settings.json"];
    let mut targets = Vec::new();
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let net_dir = entry.path();
            if !net_dir.is_dir() {
                continue;
            }
            for name in CUBE_MATERIAL {
                let p = net_dir.join(name);
                if p.exists() {
                    targets.push(p);
                }
            }
        }
    }
    targets
}

/// Completes an interrupted duress wipe on launch. No-op when the journal
/// marker is absent (wipe already finished cleanly).
fn complete_pending_wipe(
    root: &std::path::Path,
    journal: &crate::services::duress::journal::WipeJournal,
) {
    use crate::services::duress::wipe::CubeWiper;
    if !journal.is_pending() {
        return;
    }
    let wiper = CubeWiper::new(duress_wipe_targets(root), journal.clone());
    if let Err(e) = wiper.complete_if_pending() {
        error!("duress: failed to complete interrupted wipe on launch: {e}");
    }
}

/// Builds the Phase 4 activation-queue drainer, or `None` when there's nothing
/// to drain (empty queue) or the device key can't be loaded. The drainer fires
/// queued `trigger-with-code` POSTs and retries them with backoff until they
/// land.
fn build_duress_drainer(
    root: &std::path::Path,
) -> Option<crate::services::duress::drain::DuressDrainer> {
    use crate::services::duress::{
        cipher::DeviceKey, drain::DuressDrainer, orchestrator::DuressTrigger, queue::DuressQueue,
    };
    let queue = DuressQueue::new(root);
    if queue.is_empty().unwrap_or(true) {
        return None;
    }
    let cipher = DeviceKey::load_or_create(root).ok()?;
    let client: std::sync::Arc<dyn DuressTrigger> =
        std::sync::Arc::new(crate::services::coincube::CoincubeClient::new());
    Some(DuressDrainer::new(queue, cipher, client))
}

/// Spawns the activation drainer as a fire-and-forget background task (used from
/// the update context, which has a Tokio runtime). No-op if the queue is empty
/// or no runtime is available — the launch-time `duress_drain_task` then picks
/// it up.
fn spawn_duress_drainer(root: &std::path::Path) {
    let Some(drainer) = build_duress_drainer(root) else {
        return;
    };
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            handle.spawn(async move { drainer.run_until_empty().await });
        }
        Err(_) => warn!("duress: no runtime to drain activation queue now; queued for next launch"),
    }
}

/// Launch-time drainer as an Iced `Task` (runs in Iced's executor, where
/// `Handle::try_current` may not be available yet). Resumes any pending
/// activation POSTs left by a prior session.
fn duress_drain_task(root: &std::path::Path) -> Task<Message> {
    match build_duress_drainer(root) {
        Some(drainer) => Task::perform(async move { drainer.run_until_empty().await }, |()| {
            Message::DuressDrainComplete
        }),
        None => Task::none(),
    }
}

/// Builds an authenticated `get_duress_state` check from the Connect auth
/// cached at `<network>/connect.json` (preserved through the wipe). Returns a
/// task whose message is `Some(active)` on a successful check, or `None` when
/// the cache/token/network is unavailable or the request fails — `None` is
/// treated as "still locked" so a failed check never opens a sign-in form.
fn duress_state_check_task(datadir: CoincubeDirectory, network: bitcoin::Network) -> Task<Message> {
    use crate::app::view::duress::active_screen::Message as DuressMsg;
    Task::perform(
        async move {
            let network_dir = datadir.network_directory(network);
            let cache =
                crate::services::connect::client::cache::ConnectCache::from_file(&network_dir)
                    .ok()?;
            let account = cache.accounts.into_iter().next()?;
            let mut client = crate::services::coincube::CoincubeClient::new();
            client.set_token(&account.tokens.access_token);
            client.get_duress_state().await.ok().map(|s| s.active)
        },
        |active: Option<bool>| Message::Duress(DuressMsg::StateChecked(active)),
    )
}

/// Activates duress from the local PIN path and returns the cryptic screen to
/// lock into. Follows the orchestrator's sacred ordering so the Connect account
/// lock and offline retry are NOT bypassed on this — the primary — activation
/// path:
///
///   1. journal marker (records the account id for relaunch completion),
///   2. durable queue commit (Connect tiers) — the source of truth that the
///      POST eventually fires, committed BEFORE the wipe,
///   3. fire the unauthenticated `trigger-with-code` POST in the background,
///   4. atomic wipe IN PARALLEL — never gated on the network,
///   5. persist `active` for relaunch reconcile.
///
/// Sovereign devices (no `account_id`) skip steps 2–3 and wipe locally with no
/// server signal. The duress stores at the data-dir root (`duress-state.json`,
/// `duress-queue.json`, `duress.key`, the journal) are outside the wiped tree
/// and survive by construction, so the queued code + key remain available for
/// the POST (and the Phase 4 drain loop) after the wipe.
fn activate_local_duress(
    datadir: CoincubeDirectory,
    network: bitcoin::Network,
) -> crate::app::view::duress::active_screen::DuressActiveScreen {
    use crate::app::view::duress::active_screen::DuressActiveScreen;
    use crate::services::duress::{
        journal::WipeJournal, queue::DuressQueue, wipe::CubeWiper, DuressLocalState,
        PendingActivation,
    };

    let root = datadir.path().to_path_buf();
    let root = root.as_path();

    // Load enrollment state up front: the encrypted duress code + Connect
    // account id drive the server activation POST.
    let mut st = DuressLocalState::load(root).unwrap_or_default();
    let account_id = st.account_id.clone();
    let encrypted_code = st.duress_code.clone();

    // 1. Journal marker FIRST. The recorded account id lets the launch-time
    //    reconcile finish an interrupted wipe.
    let journal = WipeJournal::new(root);
    if let Err(e) = journal.mark_pending_activation(account_id.as_deref().unwrap_or("")) {
        error!("duress: failed to write wipe journal: {e}");
    }

    // 2 + 3. Connect tiers: durably enqueue BEFORE the wipe, then start the
    //    activation drainer in the background, in parallel with the wipe. The
    //    drainer fires the POST immediately and KEEPS retrying with backoff
    //    until it lands (Phase 4), so a coerced account is still locked even if
    //    the first attempt is offline and the user never leaves this screen.
    let queue = DuressQueue::new(root);
    if let (Some(acct), Some(enc)) = (account_id.as_ref(), encrypted_code.as_ref()) {
        let pending = PendingActivation {
            account_id: acct.clone(),
            // Stored encrypted, as everywhere else; the POST decrypts in-flight.
            duress_code: enc.clone(),
            enqueued_at: chrono::Utc::now(),
            attempts: 0,
        };
        // This queue entry is the ONLY thing that drives the server-side lock —
        // the drainer fires trigger-with-code from it and the launch reconcile
        // re-drains it. A dropped enqueue means the account is never locked
        // server-side, so retry the durable commit on a transient IO error.
        // enqueue is atomic and idempotent per account (it replaces any existing
        // entry), so retrying can't duplicate. We never gate the wipe/lock on
        // this: the device locks into the cryptic screen regardless.
        let mut enqueued = false;
        for attempt in 1..=3 {
            match queue.enqueue(pending.clone()) {
                Ok(()) => {
                    enqueued = true;
                    break;
                }
                Err(e) => error!("duress: enqueue activation attempt {attempt}/3 failed: {e}"),
            }
        }
        if !enqueued {
            error!("duress: activation not durably queued; server-side lock may not fire");
        }
        spawn_duress_drainer(root);
    }

    // 4. Wipe (anchor) — runs in parallel with the POST above. Targets EVERY
    //    network's Cube data, matching the launch-time reconcile, so a PIN
    //    unlock on one network can't leave another network's Cubes on disk.
    //    Retry on failure so a transient lock/IO error doesn't leave Cube seeds
    //    or PIN material on disk. CubeWiper does NOT clear the journal on a
    //    failed pass, so even if every attempt here fails, the launch-time
    //    reconcile (complete_pending_wipe) finishes the wipe on next launch.
    //    The device still locks into the cryptic screen regardless — showing a
    //    normal app with Cube data after a duress trigger would be far worse.
    let wiper = CubeWiper::new(duress_wipe_targets(root), journal);
    let mut wiped = false;
    for attempt in 1..=3 {
        match wiper.execute_atomic() {
            Ok(()) => {
                wiped = true;
                break;
            }
            Err(e) => error!("duress: wipe attempt {attempt}/3 failed: {e}"),
        }
    }
    if !wiped {
        error!(
            "duress: Cube data may remain on disk; journal retained, wipe will be \
             retried on next launch"
        );
    }

    // 5. Persist active state so launch-time reconcile re-enters the cryptic
    //    screen.
    st.active = true;
    st.last_activation_attempt = Some(chrono::Utc::now());
    if let Err(e) = st.save(root) {
        error!("duress: failed to persist active state: {e}");
    }

    let queue_pending = queue.is_empty().map(|empty| !empty).unwrap_or(false);
    let mut screen = DuressActiveScreen::with_context(datadir.clone(), Some(network));
    screen.queue_pending = queue_pending;
    screen
}

#[derive(Debug)]
pub enum Message {
    Launch(home::Message),
    Install(installer::Message),
    Load(loader::Message),
    Run(app::Message),
    Login(login::Message),
    PinEntry(crate::pin_entry::Message),
    /// Messages from the cryptic "Duress Mode Activated" screen.
    Duress(crate::app::view::duress::active_screen::Message),
    /// The background activation-queue drainer finished (queue emptied). No-op
    /// at the UI level — the drainer did its work as a side effect.
    DuressDrainComplete,
    RemoteBackendBreezLoaded {
        wallet_settings: WalletSettings,
        backend_client: BackendWalletClient,
        wallet: api::Wallet,
        coins: ListCoinsResult,
        datadir: CoincubeDirectory,
        network: bitcoin::Network,
        config: app::Config,
        breez_client: Result<Arc<app::breez_liquid::BreezClient>, app::breez_liquid::BreezError>,
        /// Spark backend carried over from the Login state (loaded during
        /// PIN entry alongside the Liquid client). `None` if the cube has
        /// no Spark signer or the bridge failed to spawn.
        spark_backend: Option<Arc<app::wallets::SparkBackend>>,
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
    /// Bubbles up to the pane so it can focus the Home tab on its
    /// Connect section — fired when the user clicks "Sign In" on the
    /// inline prompt rendered by a Connect-requiring feature page
    /// (Spark → Settings → Lightning Address, Cube → Settings →
    /// Avatar / Members).
    OpenConnectSignIn,
    /// Bubbles up to the pane on a Home-tab login edge so it can
    /// broadcast a session re-check to every open Cube tab.
    ConnectSignedIn,
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
            State::Home(home) => home.theme_mode = mode,
            _ => {}
        }
    }

    /// Apply the tab's stored theme_mode to the current state.
    /// Call after any state transition to State::App or State::Home.
    fn sync_theme_mode(&mut self) {
        let mode = self.theme_mode;
        match &mut self.state {
            State::App(app) => app.cache_mut().theme_mode = mode,
            State::Home(home) => home.theme_mode = mode,
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
            State::Home(_) => "Home",
            State::Login(_) => "Login",
            State::PinEntry(_) => "Enter PIN",
            State::App(a) => a.title(),
            State::DuressActive(_) => "COINCUBE",
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
            (State::Home(l), Message::Launch(msg)) => match msg {
                home::Message::Install(datadir, network, init, coincube_client) => {
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
                    // `coincube_client` is populated when the home
                    // already holds an authenticated Connect session (today
                    // the Recovery-Kit restore path forwards it so the
                    // installer step can skip a redundant email+OTP). Other
                    // home entry points pass `None` and the relevant
                    // installer step runs its own auth form as before.
                    let (install, command) = Installer::new(
                        datadir,
                        network,
                        None,
                        init,
                        false,
                        None,
                        None,
                        None,
                        false,
                        coincube_client,
                    );
                    self.state = State::Installer(install);
                    command.map(Message::Install)
                }
                home::Message::Run(datadir_path, cfg, network, cube) => {
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
                        // and stay on the home. This prevents falling
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
                home::Message::View(home::ViewMessage::ToggleTheme) => {
                    Task::done(Message::ToggleTheme)
                }
                home::Message::ConnectSignedInBubble => Task::done(Message::ConnectSignedIn),
                _ => l.update(msg).map(Message::Launch),
            },
            (State::Login(l), Message::Login(msg)) => match msg {
                login::Message::View(login::ViewMessage::BackToHome(network)) => {
                    let (home, command) = Home::new(l.datadir.clone(), Some(network));
                    self.state = State::Home(home);
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
                        None, // No coincube_client from login screen
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
                            spark_backend: l.spark_backend.clone(),
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
                        spark_backend: l.spark_backend.clone(),
                    })
                }
                _ => l.update(msg).map(Message::Login),
            },
            (State::Installer(i), Message::Install(msg)) => {
                if let installer::Message::Exit(settings, internal_bitcoind) = msg {
                    // Associate wallet with cube, and — for the Recovery
                    // Kit restore flow specifically — build the
                    // BreezClient in the same async task so the loader
                    // doesn't hit the "missing pre-loaded BreezClient"
                    // error path and hang on "Starting daemon…".
                    let network_dir = i.datadir.network_directory(i.network);
                    let datadir = i.datadir.clone();
                    let wallet_id = settings.wallet_id();
                    let wallet_alias = settings.alias.clone();
                    let network = i.network;
                    let originating_cube_id = i.cube_settings.as_ref().map(|c| c.id.clone());

                    // Capture restore-flow state up-front. Cloning the
                    // `Zeroizing<String>` here means the PIN copy
                    // carried into the Task is its own heap-zeroing
                    // value — it's dropped (and zeroed) once the task
                    // completes.
                    let restore_seed = match (
                        i.context.restore_pin.clone(),
                        i.context.recovered_signer.as_ref().map(|s| s.fingerprint()),
                    ) {
                        (Some(pin), Some(fp)) => Some(RestoreCubeSeed {
                            pin,
                            master_signer_fingerprint: fp,
                        }),
                        _ => None,
                    };

                    Task::perform(
                        async move {
                            let cube = find_or_create_cube(
                                &network_dir,
                                &wallet_id,
                                &wallet_alias,
                                network,
                                originating_cube_id,
                                restore_seed.as_ref(),
                            )
                            .await?;

                            // Only the restore path needs to build a
                            // BreezClient up-front — fresh-install +
                            // remote-backend flows build it at PIN
                            // entry / login. On `NetworkNotSupported`
                            // (testnet/signet) we mirror the PIN-entry
                            // branch (`BreezClientLoadedAfterPin`
                            // handler) and hand back a disconnected
                            // client: the Loader's Synced/App arms
                            // treat a `None` BreezClient as an
                            // architectural bug and error out, so
                            // pre-loaded-must-exist is the contract.
                            let breez_client = if let Some(seed) = &restore_seed {
                                match breez_liquid::load_breez_client(
                                    datadir.path(),
                                    network,
                                    seed.master_signer_fingerprint,
                                    seed.pin.as_str(),
                                )
                                .await
                                {
                                    Ok(c) => Some(c),
                                    Err(breez_liquid::BreezError::NetworkNotSupported(n)) => {
                                        info!(
                                            "BreezClient not loaded for restored Cube: \
                                             network {} is not supported by Breez SDK; \
                                             using disconnected client",
                                            n
                                        );
                                        Some(Arc::new(breez_liquid::BreezClient::disconnected(
                                            network,
                                        )))
                                    }
                                    Err(e) => {
                                        // A non-network failure here
                                        // means the mnemonic is on disk
                                        // but we can't decrypt/connect.
                                        // Roll the whole post-install
                                        // into an error so the user
                                        // sees something actionable
                                        // rather than silently landing
                                        // on a broken Loader.
                                        return Err(format!(
                                            "Failed to load BreezClient after restore: {}",
                                            e
                                        ));
                                    }
                                }
                            } else {
                                None
                            };

                            // Mirror the PIN-entry path (tab.rs Spark
                            // load near line 781): spawn the bridge
                            // subprocess against the just-encrypted
                            // mnemonic so the Loader can hand a live
                            // SparkBackend to App. Failures here are
                            // non-fatal — without this, the first
                            // boot after restore landed in the app
                            // with `spark_backend = None` and the
                            // Spark panels only populated after the
                            // user closed + re-opened the Cube.
                            let spark_backend = if let Some(seed) = &restore_seed {
                                match app::breez_spark::load_spark_client(
                                    datadir.path(),
                                    network,
                                    seed.master_signer_fingerprint,
                                    seed.pin.as_str(),
                                )
                                .await
                                {
                                    Ok(client) => {
                                        Some(Arc::new(app::wallets::SparkBackend::new(client)))
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            "Spark bridge unavailable after restore, \
                                             continuing without Spark: {}",
                                            e
                                        );
                                        None
                                    }
                                }
                            } else {
                                None
                            };

                            Ok((cube, breez_client, spark_backend))
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
                    let (cube, restored_breez_client, restored_spark_backend) = match result {
                        Ok(triple) => triple,
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
                            // Prefer the just-loaded BreezClient from
                            // the restore path; fall back to whatever
                            // the installer was launched with.
                            restored_breez_client.or_else(|| i.breez_client.clone()),
                            restored_spark_backend.or_else(|| i.spark_backend.clone()),
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
                            // Same preference chain as the Login arm —
                            // the restored BreezClient (built against
                            // the user's new PIN) wins over the
                            // installer-launched one.
                            restored_breez_client.or_else(|| i.breez_client.clone()),
                            // Spark backend built against the user's
                            // new PIN during the restore async block.
                            // Falling back to the installer's existing
                            // handle covers the non-restore flows that
                            // already had Spark wired in before this
                            // Message arm widened.
                            restored_spark_backend.or_else(|| i.spark_backend.clone()),
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
                            // Fallback: go to home
                            let (home, command) = Home::new(i.destination_path(), Some(network));
                            self.state = State::Home(home);
                            command.map(Message::Launch)
                        }
                    } else {
                        // No cube settings stored, go to home
                        let (home, command) = Home::new(i.destination_path(), Some(network));
                        self.state = State::Home(home);
                        command.map(Message::Launch)
                    }
                } else {
                    i.update(msg).map(Message::Install)
                }
            }
            (State::Loader(loader), Message::Load(msg)) => match msg {
                loader::Message::View(loader::ViewMessage::SwitchNetwork) => {
                    let (home, command) =
                        Home::new(loader.datadir_path.clone(), Some(loader.network));
                    self.state = State::Home(home);
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
                        None, // No coincube_client from loader path
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
                        // Local daemon path has no Connect tokens at this
                        // stage; the user may sign in to Connect later
                        // via the Connect tab — that flow handles its own
                        // gRPC bootstrap.
                        None,
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
                            app.authenticated_coincube_client(), // authenticated API client for Keychain keys
                        );
                        self.state = State::Installer(install);
                        command.map(Message::Install)
                    }
                    app::Message::View(app::view::Message::SetupVaultRestoreFromKit) => {
                        // W15 — same installer launch path as SetupVault,
                        // but starts in the Recovery-Kit restore flow
                        // instead of the new-vault descriptor editor.
                        let (install, command) = Installer::new(
                            app.datadir().clone(),
                            app.cache().network,
                            None,
                            UserFlow::RestoreVaultFromRecoveryKit,
                            true,
                            Some(app.cube_settings().clone()),
                            Some(app.breez_client()),
                            app.spark_backend(),
                            GlobalSettings::load_developer_mode(&GlobalSettings::path(
                                app.datadir(),
                            )),
                            app.authenticated_coincube_client(),
                        );
                        self.state = State::Installer(install);
                        command.map(Message::Install)
                    }
                    app::Message::View(app::view::Message::ToggleTheme) => {
                        Task::done(Message::ToggleTheme)
                    }
                    app::Message::View(app::view::Message::DuressLockRemote) => {
                        // Phase 7b: remote duress activation. Lock the running
                        // app into the cryptic screen immediately — WITHOUT
                        // wiping (remote activation can be accidental; only a
                        // local duress PIN wipes). The App's gRPC handler already
                        // attempts to persist DuressLocalState.active, but a
                        // failed write there would let the relaunch reconcile
                        // (which keys off st.active) drop back to the normal Home
                        // flow with Cube data intact. So re-persist here as a
                        // durable backstop tied to the UI lock, before showing
                        // the cryptic screen.
                        let datadir = app.datadir().clone();
                        let network = app.cache().network;
                        let root = datadir.path();
                        let mut st = crate::services::duress::DuressLocalState::load(root)
                            .unwrap_or_default();
                        if !st.active {
                            st.active = true;
                            let mut saved = false;
                            for attempt in 1..=3 {
                                match st.save(root) {
                                    Ok(()) => {
                                        saved = true;
                                        break;
                                    }
                                    Err(e) => error!(
                                        "duress: persist remote active state on UI lock \
                                         attempt {attempt}/3 failed: {e}"
                                    ),
                                }
                            }
                            if !saved {
                                error!(
                                    "duress: remote active state not persisted; a relaunch \
                                     may not stay locked"
                                );
                            }
                        }
                        let screen =
                            crate::app::view::duress::active_screen::DuressActiveScreen::with_context(
                                datadir,
                                Some(network),
                            );
                        self.state = State::DuressActive(screen);
                        Task::none()
                    }
                    app::Message::View(app::view::Message::OpenConnectSignIn) => {
                        // Re-check this tab's ConnectAccountPanel against
                        // the keyring before deciding whether to bubble
                        // up. When the user already signed in on another
                        // tab the session is in the shared keyring entry
                        // and Init can refresh this tab's panel in place;
                        // jumping to the Home tab in that case would be
                        // an unnecessary context switch. We only bubble
                        // when the panel has no path to authenticating
                        // itself.
                        let needs_home_handoff = !app.can_restore_connect_session();
                        let init_task = app
                            .update(app::Message::View(app::view::Message::ConnectAccount(
                                app::view::ConnectAccountMessage::Init,
                            )))
                            .map(Message::Run);
                        if needs_home_handoff {
                            let bubble = Task::done(Message::OpenConnectSignIn);
                            Task::batch([init_task, bubble])
                        } else {
                            init_task
                        }
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
                                    let mut cube = cube;
                                    // Backfill `master_signer_fingerprint` for
                                    // Cubes minted before the field existed —
                                    // without it, the Liquid + Spark loaders
                                    // below silently skip and the Connect
                                    // Lightning Address claim flow / Spark
                                    // panels stay disabled. Only the cube's
                                    // own master seed will decrypt with this
                                    // PIN, so a successful match is sound.
                                    if cube.master_signer_fingerprint.is_none() {
                                        if let Some(fp) =
                                            app::settings::derive_master_signer_fingerprint(
                                                datadir_clone.path(),
                                                network_val,
                                                &pin,
                                                cube.created_at,
                                            )
                                        {
                                            cube.master_signer_fingerprint = Some(fp);
                                            let cube_id = cube.id.clone();
                                            let network_dir =
                                                datadir_clone.network_directory(network_val);
                                            if let Err(e) = app::settings::update_settings_file(
                                                &network_dir,
                                                |mut s| {
                                                    if let Some(c) =
                                                        s.cubes.iter_mut().find(|c| c.id == cube_id)
                                                    {
                                                        c.master_signer_fingerprint = Some(fp);
                                                    }
                                                    Some(s)
                                                },
                                            )
                                            .await
                                            {
                                                tracing::warn!(
                                                    "Failed to persist backfilled \
                                                     master_signer_fingerprint for cube {}: {}",
                                                    cube.id,
                                                    e
                                                );
                                            } else {
                                                tracing::info!(
                                                    "Backfilled master_signer_fingerprint {} \
                                                     for legacy cube {}",
                                                    fp,
                                                    cube.id
                                                );
                                            }
                                        }
                                    }

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
                    // Go back to home
                    let network = pin_entry.cube().network;
                    let (home, command) = Home::new(
                        match &pin_entry.on_success {
                            crate::pin_entry::PinEntrySuccess::LoadApp { datadir, .. } => {
                                datadir.clone()
                            }
                        },
                        Some(network),
                    );
                    self.state = State::Home(home);
                    command.map(Message::Launch)
                }
                crate::pin_entry::Message::DuressDetected => {
                    // Duress PIN entered at Cube unlock. The on-device trust
                    // anchor is the atomic local wipe — run it now, BEFORE
                    // rendering anything, then lock into the cryptic screen.
                    // (The activation POST/queue is driven by the duress
                    // orchestrator engine; threading the Connect account context
                    // into this surface so it can fire here is a follow-up.)
                    let network = pin_entry.cube().network;
                    let datadir = match &pin_entry.on_success {
                        crate::pin_entry::PinEntrySuccess::LoadApp { datadir, .. } => {
                            datadir.clone()
                        }
                    };
                    let screen = activate_local_duress(datadir, network);
                    self.state = State::DuressActive(screen);
                    Task::none()
                }
                m => pin_entry.update(m).map(Message::PinEntry),
            },
            (State::DuressActive(screen), Message::Duress(msg)) => match msg {
                crate::app::view::duress::active_screen::Message::SignInPressed => {
                    // Gated entirely on server-side duress state. Read cached
                    // Connect auth (preserved through the wipe) and check
                    // get_duress_state BEFORE rendering any sign-in surface. No
                    // credential prompt ever appears here.
                    match (screen.datadir().cloned(), screen.network()) {
                        (Some(datadir), Some(network)) => {
                            screen.checking = true;
                            screen.error = None;
                            duress_state_check_task(datadir, network)
                        }
                        _ => {
                            // No way to reach the server (no network resolved) —
                            // safe default is to stay locked.
                            screen.error =
                                Some("Duress mode is active. Try again later.".to_string());
                            Task::none()
                        }
                    }
                }
                crate::app::view::duress::active_screen::Message::StateChecked(active) => {
                    match active {
                        Some(false) => {
                            // Server reports duress cleared from another device.
                            // Update local state and exit into the normal flow.
                            if let Some(datadir) = screen.datadir().cloned() {
                                let root = datadir.path();
                                let mut st = crate::services::duress::DuressLocalState::load(root)
                                    .unwrap_or_default();
                                st.active = false;
                                st.unlock_at = None;
                                if let Err(e) = st.save(root) {
                                    error!("duress: failed to clear local state: {e}");
                                }
                                let network = screen.network();
                                let (home, command) = Home::new(datadir, network);
                                self.state = State::Home(home);
                                return command.map(Message::Launch);
                            }
                            screen.checking = false;
                            Task::none()
                        }
                        // Still active, or the check failed/was unreachable —
                        // never reveal more than the cryptic message already does.
                        _ => {
                            screen.checking = false;
                            screen.error =
                                Some("Duress mode is active. Try again later.".to_string());
                            Task::none()
                        }
                    }
                }
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
                    spark_backend,
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
                    spark_backend,
                ) {
                    Ok((app, command)) => {
                        self.state = State::App(app);
                        command.map(Message::Run)
                    }
                    Err(e) => {
                        tracing::error!("Failed to create app with remote backend: {}", e);
                        let (home, command) = Home::new(datadir, Some(network));
                        self.state = State::Home(home);
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
                        let (login, command) = login::CoincubeLiteLogin::new(
                            datadir.clone(),
                            network,
                            wallet_settings.clone(),
                            Some(breez),
                            spark_backend,
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
            State::Home(v) => v.subscription().map(Message::Launch),
            State::Login(_) => Subscription::none(),
            State::PinEntry(_) => Subscription::none(),
            State::DuressActive(_) => Subscription::none(),
        }
    }

    pub fn view(&self) -> Element<Message> {
        match &self.state {
            State::Installer(v) => v.view().map(Message::Install),
            State::App(v) => v.view().map(Message::Run),
            State::Home(v) => v.view().map(Message::Launch),
            State::Loader(v) => v.view().map(Message::Load),
            State::Login(v) => v.view().map(Message::Login),
            State::PinEntry(v) => v.view().map(Message::PinEntry),
            State::DuressActive(v) => v.view().map(Message::Duress),
        }
    }

    pub fn stop(&mut self) {
        match &mut self.state {
            State::Loader(s) => s.stop(),
            State::Home(s) => s.stop(),
            State::Installer(s) => s.stop(),
            State::App(s) => s.stop(),
            State::Login(_) => {}
            State::PinEntry(_) => {}
            State::DuressActive(_) => {}
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

/// Bundle of restore-flow context that lets `find_or_create_cube`
/// mint a `CubeSettings` with the same shape a fresh-install Cube
/// produces: a PIN hash + master-signer fingerprint. Populated only
/// for `UserFlow::RestoreFromRecoveryKit` after `RestorePinSetupStep`;
/// `None` for every other flow preserves the previous behaviour.
struct RestoreCubeSeed {
    pin: zeroize::Zeroizing<String>,
    master_signer_fingerprint: bitcoin::bip32::Fingerprint,
}

async fn find_or_create_cube(
    network_dir: &NetworkDirectory,
    wallet_id: &WalletId,
    wallet_alias: &Option<String>,
    network: bitcoin::Network,
    originating_cube_id: Option<String>,
    restore_seed: Option<&RestoreCubeSeed>,
) -> Result<app::settings::CubeSettings, String> {
    // Helper: decorate a freshly-minted CubeSettings with
    // PIN + master-signer-fingerprint when we're on the restore path.
    // Pulled out so the three "new cube" branches share one code path.
    let decorate_new =
        |mut cube: app::settings::CubeSettings| -> Result<app::settings::CubeSettings, String> {
            if let Some(seed) = restore_seed {
                cube = cube.with_master_signer(seed.master_signer_fingerprint);
                cube = cube
                    .with_pin(seed.pin.as_str())
                    .map_err(|e| format!("Failed to set PIN on restored cube: {}", e))?;
            }
            Ok(cube)
        };

    match app::settings::Settings::from_file(network_dir) {
        Ok(mut settings_data) => {
            // First, check if a cube already has this wallet.
            // We don't decorate existing cubes with the restore PIN —
            // if the cube already has a PIN hash / fingerprint those
            // are its source of truth. The restore flow only overwrites
            // Cube-level credentials when we're actually minting a new
            // Cube for the restored wallet.
            if let Some(existing_cube) = settings_data
                .cubes
                .iter()
                .find(|c| c.vault_wallet_id.as_ref() == Some(wallet_id))
            {
                return Ok(existing_cube.clone());
            }

            // Second, if we have an originating cube ID, validate and use it
            if let Some(target_cube_id) = originating_cube_id {
                if let Some(target_cube) = settings_data
                    .cubes
                    .iter_mut()
                    .find(|c| c.id == target_cube_id)
                {
                    if target_cube.vault_wallet_id.is_some() {
                        return Err(format!(
                            "Cube '{}' already has a vault. Remove the existing vault before creating a new one.",
                            target_cube.name
                        ));
                    }
                    target_cube.vault_wallet_id = Some(wallet_id.clone());
                    // Apply restore-flow credentials (PIN hash + fingerprint) if
                    // restoring to this cube — same rationale as the empty-cube
                    // fallback: the hash must match the newly-encrypted mnemonic.
                    let cube_clone = decorate_new(target_cube.clone())?;
                    *target_cube = cube_clone.clone();
                    let cube_name = target_cube.name.clone();

                    info!(
                        "Associating wallet {} with originating cube '{}' on {} network",
                        wallet_id, cube_name, network
                    );

                    return save_cube_settings(network_dir, cube_clone, network, settings_data)
                        .await;
                } else {
                    return Err(format!(
                        "Cannot find originating cube with ID '{}'. Please restart the app and try again.",
                        target_cube_id
                    ));
                }
            }

            // Third, find a cube without a vault and associate this wallet with it
            // Find by index so we can overwrite with a decorated clone without
            // fighting the borrow checker over a mutable reference that would
            // otherwise need `mem::take` (and `CubeSettings` doesn't implement
            // `Default`).
            if let Some(empty_idx) = settings_data
                .cubes
                .iter()
                .position(|c| c.vault_wallet_id.is_none())
            {
                let mut empty_cube = settings_data.cubes[empty_idx].clone();
                empty_cube.vault_wallet_id = Some(wallet_id.clone());
                // Reuse `decorate_new` so the fingerprint + PIN-hash
                // path matches the brand-new-Cube branches below. If
                // the Cube had its own `security_pin_hash`, `with_pin`
                // replaces it with one derived from the PIN the user
                // just chose — consistent with the newly-encrypted
                // mnemonic on disk (otherwise PIN entry against the
                // old hash would silently succeed but fail to decrypt
                // the mnemonic).
                let empty_cube = decorate_new(empty_cube)?;
                settings_data.cubes[empty_idx] = empty_cube.clone();
                let cube_name = empty_cube.name.clone();

                info!(
                    "Associating wallet {} with existing cube '{}' on {} network",
                    wallet_id, cube_name, network
                );

                return save_cube_settings(network_dir, empty_cube, network, settings_data).await;
            }

            // Third, create a new cube for this wallet
            let cube = decorate_new(
                app::settings::CubeSettings::new(
                    wallet_alias
                        .clone()
                        .unwrap_or_else(|| format!("My {} Cube", network)),
                    network,
                )
                .with_vault(wallet_id.clone()),
            )?;
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
            let cube = decorate_new(
                app::settings::CubeSettings::new(
                    wallet_alias
                        .clone()
                        .unwrap_or_else(|| format!("My {} Cube", network)),
                    network,
                )
                .with_vault(wallet_id.clone()),
            )?;
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
    spark_backend: Option<Arc<app::wallets::SparkBackend>>,
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

    // Reuse the existing `Arc<RwLock<AccessTokenResponse>>` from the
    // remote backend so the gRPC interceptor and the REST client share
    // a single source of truth — token refreshes propagate to both
    // without manual fan-out.
    let connect_auth = Some((
        remote_backend.inner_client().auth.clone(),
        remote_backend.user_email().to_string(),
    ));

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
            display_mode: crate::app::settings::Settings::from_file(
                &coincube_dir.network_directory(network),
            )
            .ok()
            .map(|s| s.display_mode)
            .unwrap_or_default(),
            node_bitcoind_sync_progress: None,
            node_bitcoind_ibd: None,
            node_bitcoind_last_log: None,
            connect_authenticated: false,
            has_vault: true,
            cube_name: cube_settings.name.clone(),
            current_cube_backed_up: cube_settings.backed_up,
            backup_warning_dismissed: false,
            current_cube_is_passkey: cube_settings.is_passkey_cube(),
            has_p2p: false, // Set later by App::new based on mnemonic availability
            theme_mode: coincube_ui::theme::palette::ThemeMode::default(),
            btc_usd_price: None,
            show_direction_badges: true,
            lightning_address: None,
            avatar_handle: None,
            cube_id: cube_settings.id.clone(),
            current_cube_server_id: None,
            current_descriptor_fingerprint: None,
            recovery_kit_last_backed_up_descriptor_fingerprint: cube_settings
                .recovery_kit_last_backed_up_descriptor_fingerprint
                .clone(),
            // grpc_url isn't known yet — `Message::ConnectStreamReady`
            // backfills both fields once `get_service_config` returns.
            // Tokens we have right now (shared Arc with the REST client)
            // so populate them eagerly.
            connect_grpc_url: None,
            connect_tokens: Some(remote_backend.inner_client().auth.clone()),
            connect_stream_status: crate::app::ConnectionStatus::default(),
            connect_device_id: None,
            connect_email: Some(remote_backend.user_email().to_string()),
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
        spark_backend,
        config,
        Arc::new(remote_backend),
        coincube_dir,
        None,
        false,
        cube_settings,
        connect_auth,
    ))
}

#[cfg(test)]
mod duress_wipe_target_tests {
    use super::duress_wipe_targets;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    fn touch(path: &std::path::Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, b"x").unwrap();
    }

    #[test]
    fn wipes_all_cube_material_and_preserves_connect_auth() {
        let seq = SEQ.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "coincube-wipe-targets-{}-{}",
            std::process::id(),
            seq
        ));
        let _ = std::fs::remove_dir_all(&root);

        let net = root.join("bitcoin");
        touch(&net.join("data").join("cube_a").join("wallet.db"));
        touch(&net.join("mnemonics").join("aabbccdd-master"));
        touch(&net.join("settings.json"));
        touch(&net.join("connect.json"));
        // A second network is covered too.
        touch(&root.join("testnet").join("mnemonics").join("seed"));
        // bitcoind (root-level, not a network dir) carries no cube material.
        touch(&root.join("bitcoind").join("blocks").join("blk0.dat"));

        let targets = duress_wipe_targets(&root);

        assert!(targets.contains(&net.join("data")), "data/ must be wiped");
        assert!(
            targets.contains(&net.join("mnemonics")),
            "mnemonics/ (seeds) must be wiped"
        );
        assert!(
            targets.contains(&net.join("settings.json")),
            "settings.json (PIN hashes) must be wiped"
        );
        assert!(
            targets.contains(&root.join("testnet").join("mnemonics")),
            "every network's seeds must be wiped"
        );
        // Cached Connect auth and the bitcoind blockchain are preserved.
        assert!(
            !targets.iter().any(|t| t.ends_with("connect.json")),
            "connect.json (cached auth) must survive"
        );
        assert!(
            !targets.iter().any(|t| t.starts_with(root.join("bitcoind"))),
            "bitcoind blockchain must not be wiped"
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}
