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
        // Fail CLOSED: if duress-state.json can't be read (parse/IO error, not a
        // missing file — load() maps that to Ok(default)), assume the device may
        // be locked rather than skipping the lock and opening the normal flow.
        let active = match crate::services::duress::DuressLocalState::load(&root) {
            Ok(st) => st.active,
            Err(e) => {
                error!("duress: reading duress state failed at launch; locking to be safe: {e}");
                true
            }
        };
        let journal = crate::services::duress::journal::WipeJournal::new(&root);
        // Phase 4: resume draining any pending activation POSTs left by a prior
        // session (the durable queue survives restarts). Started here so an
        // offline-at-activation device eventually signals Connect.
        let drain = duress_drain_task(&root);
        if active || journal.is_pending() {
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

    // A duress wipe must never leave seeds or PIN hashes behind because a virus
    // scanner briefly locked a directory (a real transient on Windows). The
    // filesystem probes below therefore retry on transient errors and fail SAFE
    // toward wiping: a path whose existence can't be determined is targeted
    // anyway — CubeWiper deletes idempotently (a NotFound is a no-op), so an
    // extra target is harmless while a missed one is a security failure. If the
    // network enumeration itself can't be read, log loudly instead of silently
    // wiping nothing; the launch-time reconcile (`complete_pending_wipe`) retries.
    match read_dir_paths_with_retry(root) {
        Ok(entries) => {
            for net_dir in entries {
                if !is_dir_or_unknown(&net_dir) {
                    continue;
                }
                for name in CUBE_MATERIAL {
                    let p = net_dir.join(name);
                    if exists_or_target_on_doubt(&p) {
                        targets.push(p);
                    }
                }
            }
        }
        Err(e) => error!(
            "duress: could not list networks under {} to wipe ({e}); Cube material \
             may remain until the launch-time reconcile retries",
            root.display()
        ),
    }
    // Identifying material from inbound-over-Tor: the managed Tor data directory
    // and bitcoind's onion-service key(s). These live under `<root>/bitcoind`,
    // which the loop above deliberately preserves (the blockchain is expensive
    // to re-sync and not sensitive), so they must be added explicitly — the
    // onion key would otherwise survive a wipe and remain a fingerprint of the
    // device. See `PLAN-inbound-tor-connectivity.md` Decision 4.
    targets.extend(crate::node::tor::duress_identifying_targets(root));
    targets
}

/// `read_dir` returning entry paths, retried on transient errors. On Windows a
/// virus scanner can briefly lock a directory; for a duress wipe an unread
/// directory means seeds could be left behind, so a first transient error is
/// not accepted as "nothing here". The whole scan is retried if `read_dir` or
/// any entry surfaces an error, and only a persistent error is returned.
fn read_dir_paths_with_retry(dir: &std::path::Path) -> std::io::Result<Vec<std::path::PathBuf>> {
    let mut attempt = 0u32;
    loop {
        let scan = std::fs::read_dir(dir).and_then(|entries| {
            entries
                .map(|entry| entry.map(|e| e.path()))
                .collect::<std::io::Result<Vec<_>>>()
        });
        match scan {
            Ok(paths) => return Ok(paths),
            Err(_) if attempt < 5 => {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(20 * u64::from(attempt)));
            }
            Err(e) => return Err(e),
        }
    }
}

/// Whether `p` should be descended into as a network directory. A stat error
/// must not silently exclude a directory from the wipe, so treat "unknown" as
/// "descend": a non-directory simply has no Cube-material children to target.
fn is_dir_or_unknown(p: &std::path::Path) -> bool {
    std::fs::metadata(p).map(|m| m.is_dir()).unwrap_or(true)
}

/// Fail-safe existence check for a wipe target: retry transient errors and, if
/// existence still can't be determined, return `true` so the path is wiped
/// anyway. CubeWiper deletes idempotently, so targeting an absent path is a
/// harmless no-op, whereas skipping a present one leaves Cube material behind.
fn exists_or_target_on_doubt(p: &std::path::Path) -> bool {
    let mut attempt = 0u32;
    loop {
        match std::fs::exists(p) {
            Ok(present) => return present,
            Err(_) if attempt < 5 => {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(20 * u64::from(attempt)));
            }
            Err(_) => return true,
        }
    }
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
    // Load-only: never mint a fresh key here (see `build_duress_orchestrator`).
    // No usable key → don't drain now; keep the queued entry for a later launch
    // once the original key is readable, rather than minting a key that would
    // drop the entry as undecryptable.
    let cipher = match DeviceKey::load(root) {
        Ok(Some(cipher)) => cipher,
        Ok(None) => return None,
        Err(e) => {
            tracing::warn!("duress: device key unreadable; deferring activation drain: {e}");
            return None;
        }
    };
    let client: std::sync::Arc<dyn DuressTrigger> =
        std::sync::Arc::new(crate::services::coincube::CoincubeClient::new());
    Some(DuressDrainer::new(queue, cipher, client))
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
            let account = cache.active_account()?;
            let mut client = crate::services::coincube::CoincubeClient::new();
            client.set_token(&account.tokens.access_token);
            client.get_duress_state().await.ok().map(|s| s.active)
        },
        |active: Option<bool>| Message::Duress(DuressMsg::StateChecked(active)),
    )
}

/// Builds the [`DuressOrchestrator`] for this data directory — the single
/// production trust anchor for local duress activation (journal → enqueue →
/// spawn POST → wipe → persist; see
/// `services/duress/orchestrator.rs::activate`). Do NOT re-inline that sequence
/// here: keeping it in one place is the whole point of this consolidation.
///
/// Infallible by design: if the device key can't be read the orchestrator is
/// still built (with no cipher) so the wipe — the on-device trust anchor —
/// always runs; only the server POST is skipped. The wipe target set is every
/// network's Cube material, matching the launch-time reconcile, so a PIN unlock
/// on one network can't leave another network's Cubes on disk.
fn build_duress_orchestrator(
    root: &std::path::Path,
) -> crate::services::duress::orchestrator::DuressOrchestrator {
    use crate::services::duress::{
        cipher::DeviceKey,
        journal::WipeJournal,
        orchestrator::{DuressOrchestrator, DuressTrigger},
        queue::DuressQueue,
        wipe::CubeWiper,
        DuressLocalState,
    };

    // load() maps a missing file to default; a real read error is logged and we
    // proceed with a default so the wipe + lock still happen (the local lock
    // takes priority over preserving an already-unreadable file).
    let local_state = DuressLocalState::load(root).unwrap_or_else(|e| {
        error!(
            "duress: reading duress state failed during activation; the server lock \
             may be skipped, but wiping and locking locally anyway: {e}"
        );
        DuressLocalState::default()
    });
    let journal = WipeJournal::new(root);
    let queue = DuressQueue::new(root);
    let wipe = CubeWiper::new(duress_wipe_targets(root), journal.clone());
    // Load-only: NEVER mint a fresh key on the activation path. A fresh key
    // can't decrypt a `duress_code` sealed under the original, and minting one
    // would let the drainer drop the queued POST as undecryptable (and clobber
    // the key slot, defeating recovery if the original key later returns).
    // Absent or unreadable key → cipher = None → the POST is left for the
    // launch-time drainer once the key is back; the wipe never depends on it.
    let cipher = match DeviceKey::load(root) {
        Ok(cipher) => cipher,
        Err(e) => {
            error!("duress: device key unreadable at activation; server POST deferred: {e}");
            None
        }
    };
    let client: std::sync::Arc<dyn DuressTrigger> =
        std::sync::Arc::new(crate::services::coincube::CoincubeClient::new());

    DuressOrchestrator::new(
        local_state,
        root.to_path_buf(),
        journal,
        queue,
        wipe,
        cipher,
        client,
        // No event channel: the caller transitions to the cryptic screen when
        // the activation task completes, not via DuressEvent.
        None,
    )
}

/// Drives a local duress activation to completion through the orchestrator.
/// Errors are logged, never propagated — the caller locks into the cryptic
/// screen regardless (a wipe that failed every retry leaves the journal for the
/// launch-time reconcile to finish).
async fn run_local_duress_activation(root: &std::path::Path, account_id: Option<String>) {
    let mut orchestrator = build_duress_orchestrator(root);
    if let Err(e) = orchestrator.activate(account_id).await {
        error!(
            "duress: activation reported an error (device still locks into the cryptic \
             screen; the wipe is retried on next launch): {e}"
        );
    }
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
    /// Local duress activation finished (orchestrator returned): lock into the
    /// cryptic screen. The wipe has already run inside the activation task, so
    /// no Cube data reaches this transition.
    DuressActivated {
        datadir: CoincubeDirectory,
        network: bitcoin::Network,
        queue_pending: bool,
    },
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

                    // Carry this device's enrolled Connect duress account id into
                    // the PIN-entry path so a duress trigger hands it to the
                    // orchestrator explicitly (Task A.1). `None` for sovereign
                    // enrollment or an unreadable state file — the orchestrator
                    // falls back to its own persisted copy, so the server lock is
                    // never silently dropped.
                    let duress_account_id =
                        crate::services::duress::DuressLocalState::load(datadir_path.path())
                            .map(|st| st.account_id)
                            .unwrap_or(None);

                    let on_success = crate::pin_entry::PinEntrySuccess::LoadApp {
                        datadir: datadir_path,
                        config: cfg,
                        network,
                        internal_bitcoind: None,
                        backup: None,
                        wallet_settings,
                    };

                    self.state = State::PinEntry(crate::pin_entry::PinEntry::new(
                        cube,
                        on_success,
                        duress_account_id,
                    ));
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
                if let installer::Message::Exit(settings_opt, internal_bitcoind) = msg {
                    // Associate wallet with cube, and — for the Recovery
                    // Kit restore flow specifically — build the
                    // BreezClient in the same async task so the loader
                    // doesn't hit the "missing pre-loaded BreezClient"
                    // error path and hang on "Starting daemon…".
                    let network_dir = i.datadir.network_directory(i.network);
                    let datadir = i.datadir.clone();
                    let wallet_id = settings_opt.as_ref().map(|s| s.wallet_id());
                    let wallet_alias = settings_opt.as_ref().and_then(|s| s.alias.clone());
                    let network = i.network;
                    let originating_cube_id = i.cube_settings.as_ref().map(|c| c.id.clone());

                    // Recovery-Kit *seed* restore: the deleted Cube's original
                    // UUID + name, threaded through the context by
                    // `RecoveryKitRestoreStep` from the decrypted kit.
                    // `find_or_create_cube` re-mints the Cube with this UUID so
                    // Connect re-registration reactivates the deleted Cube
                    // instead of creating a duplicate. Gated on
                    // `cube_settings.is_none()`: that's the wiped-install
                    // recovery flow (launched from Home with no local Cube to
                    // attach to). When a Cube shell already exists locally
                    // (`cube_settings.is_some()`, e.g. AddWallet inside a Cube)
                    // the `originating_cube_id` path owns that association, and
                    // `context.cube_id` there is that existing Cube's identity —
                    // not a restore target — so we deliberately skip it.
                    let restored_cube = if i.cube_settings.is_none() {
                        i.context
                            .cube_id
                            .clone()
                            .zip(i.context.cube_name.clone())
                            .map(|(uuid, name)| RestoreCubeIdentity { uuid, name })
                    } else {
                        None
                    };

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
                                wallet_id.as_ref(),
                                &wallet_alias,
                                network,
                                originating_cube_id,
                                restored_cube,
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
                                    // Restore-from-seed: there is no persisted
                                    // grant yet (the cube is being created right
                                    // now), so let the on-chain scan decide.
                                    // That's the whole point of the probe — a
                                    // restore with real L-BTC keeps its wallet
                                    // even though the flag is off, and an empty
                                    // one is discarded.
                                    false,
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
                                settings_opt.clone(),
                                internal_bitcoind.clone(),
                            ))
                        },
                    )
                } else if let installer::Message::CubeSaved(
                    result,
                    settings_opt,
                    internal_bitcoind,
                ) = msg
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

                    let remote_backend_auth = settings_opt
                        .as_ref()
                        .and_then(|s| s.remote_backend_auth.clone());
                    if remote_backend_auth.is_some() {
                        let settings = settings_opt.expect("Remote backend auth requires settings");
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
                    } else if settings_opt.is_none() {
                        if let Some(bitcoind) = internal_bitcoind {
                            tracing::info!("Stopping internal bitcoind as it is not needed for Seed-Only cubes");
                            bitcoind.stop();
                        }

                        // Seed-only restore: the installer now writes
                        // `gui.toml` (see `ensure_gui_config`), but this is the
                        // one load site that historically ran with no config on
                        // disk and panicked. A missing-or-corrupt config here
                        // must degrade to defaults, not abort a restore that
                        // already persisted the seed. The wallet-path load sites
                        // keep `.expect(...)` — their installers guarantee the
                        // file.
                        let cfg = app::Config::from_file(
                            &i.datadir
                                .network_directory(i.network)
                                .path()
                                .join(app::config::DEFAULT_FILE_NAME),
                        )
                        .unwrap_or_else(|_| app::Config::new(false));

                        let breez = restored_breez_client
                            .or_else(|| i.breez_client.clone())
                            .expect("BreezClient must exist for Seed-Only cube");
                        let spark = restored_spark_backend.or_else(|| i.spark_backend.clone());

                        let (app, command) = app::App::new_without_wallet(
                            breez,
                            spark,
                            cfg,
                            i.datadir.clone(),
                            i.network,
                            cube.clone(),
                        );
                        self.state = State::App(app);
                        command.map(Message::Run)
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
                            settings_opt.map(|s| *s),
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
                    // Restore Connect auth cached at `<network>/connect.json`
                    // by a prior sign-in, mirroring the remote-backend path
                    // (which threads its live tokens in). Without this, every
                    // local-node launch discards persisted Connect auth and
                    // `connect_stream_ready_task` never runs, leaving
                    // Connect-dependent features — Sign via Keychain in
                    // particular — unavailable until the user re-signs in via
                    // the Connect tab. We read the same file
                    // `duress_state_check_task` already consults at launch.
                    // (The stream bootstrap still no-ops until a `device_id`
                    // is registered for the account; the Connect-tab sign-in
                    // flow handles that registration.)
                    let connect_auth =
                        crate::services::connect::client::cache::ConnectCache::from_file(
                            &datadir.network_directory(cache.network),
                        )
                        .ok()
                        .and_then(|c| c.active_account().cloned())
                        .map(|account| {
                            (
                                Arc::new(tokio::sync::RwLock::new(account.tokens)),
                                account.email,
                            )
                        });

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
                        connect_auth,
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
                        // Skip the persist on a real read error (vs a missing
                        // file) rather than clobbering valid state with a default.
                        // The UI still locks below; the cryptic screen's own
                        // server poll re-syncs durability.
                        match crate::services::duress::DuressLocalState::load(root) {
                            Ok(mut st) if !st.active => {
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
                            Ok(_) => {}
                            Err(e) => {
                                error!("duress: reading duress state failed; not overwriting: {e}")
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
                                                // Last-seen `liquidEnabled` grant.
                                                // Connect hasn't signed in yet at
                                                // this point (and may never), so
                                                // the persisted copy is the only
                                                // one available — see
                                                // `CubeSettings::liquid_granted`.
                                                cube.liquid_granted.unwrap_or(false),
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
                crate::pin_entry::Message::DuressDetected { account_id } => {
                    // Duress PIN entered at Cube unlock. Delegate to the single
                    // trust anchor — `DuressOrchestrator::activate`
                    // (services/duress/orchestrator.rs) — which journals,
                    // enqueues the server POST, drives it in the background, and
                    // runs the atomic wipe in parallel (never gated on the
                    // network). It is async (spawns the POST), so drive it from a
                    // Task rather than re-inlining the sequence here. `account_id`
                    // is the explicitly-threaded enrolled Connect account, `None`
                    // for sovereign (Task A.1). The PinEntry is already showing
                    // its neutral loading screen, so no Cube data is visible
                    // during the brief gap; we lock into the cryptic screen the
                    // instant activation returns (the wipe completes within it).
                    let network = pin_entry.cube().network;
                    let datadir = match &pin_entry.on_success {
                        crate::pin_entry::PinEntrySuccess::LoadApp { datadir, .. } => {
                            datadir.clone()
                        }
                    };
                    Task::perform(
                        async move {
                            let root = datadir.path().to_path_buf();
                            run_local_duress_activation(&root, account_id).await;
                            let queue_pending =
                                crate::services::duress::queue::DuressQueue::new(&root)
                                    .is_empty()
                                    .map(|empty| !empty)
                                    .unwrap_or(false);
                            (datadir, network, queue_pending)
                        },
                        |(datadir, network, queue_pending)| Message::DuressActivated {
                            datadir,
                            network,
                            queue_pending,
                        },
                    )
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
                                // A server clear must NEVER drop us into the normal
                                // app with un-wiped Cube data. If the activation
                                // wipe failed all its retries (or was interrupted),
                                // the journal is still pending — finish it first,
                                // and if it STILL can't complete, stay locked (the
                                // launch-time reconcile retries on next start).
                                // This is the same invariant `State::new` enforces.
                                let journal =
                                    crate::services::duress::journal::WipeJournal::new(root);
                                if journal.is_pending() {
                                    complete_pending_wipe(root, &journal);
                                    if journal.is_pending() {
                                        screen.checking = false;
                                        screen.error = Some(
                                            "Duress mode is active. Try again later.".to_string(),
                                        );
                                        return Task::none();
                                    }
                                }
                                // Skip the write on a real read error rather than
                                // clobbering valid state with a default; the next
                                // poll re-clears once the file is readable again.
                                match crate::services::duress::DuressLocalState::load(root) {
                                    Ok(mut st) => {
                                        st.active = false;
                                        st.unlock_at = None;
                                        if let Err(e) = st.save(root) {
                                            error!("duress: failed to clear local state: {e}");
                                        }
                                    }
                                    Err(e) => error!(
                                        "duress: reading duress state failed; not overwriting: {e}"
                                    ),
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
                Message::DuressActivated {
                    datadir,
                    network,
                    queue_pending,
                },
            ) => {
                // Local activation finished in the background task — the wipe
                // has run. Lock into the cryptic "Duress Mode Activated" screen.
                let mut screen =
                    crate::app::view::duress::active_screen::DuressActiveScreen::with_context(
                        datadir,
                        Some(network),
                    );
                screen.queue_pending = queue_pending;
                self.state = State::DuressActive(screen);
                Task::none()
            }
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

/// The deleted Cube's original identity, carried out of the decrypted
/// kit/envelope by the restore steps (via `ctx.cube_id` / `ctx.cube_name`).
/// Both fields are always present together: a restore either knows the full
/// original identity or it isn't a seed restore at all — which is why
/// `find_or_create_cube` takes `Option<RestoreCubeIdentity>` rather than a
/// struct of `Option`s.
struct RestoreCubeIdentity {
    /// Original UUID, preserved verbatim (see `CubeSettings::new_with_raw_id`).
    uuid: String,
    /// Original display name, so the revived Cube doesn't inherit the
    /// wallet-alias default.
    name: String,
}

async fn find_or_create_cube(
    network_dir: &NetworkDirectory,
    wallet_id: Option<&WalletId>,
    wallet_alias: &Option<String>,
    network: bitcoin::Network,
    originating_cube_id: Option<String>,
    // Original Cube identity for a Recovery-Kit *seed* restore. When present,
    // the restored Cube reuses the deleted Cube's UUID so the Connect
    // `register_cube` call (idempotent on UUID) reactivates it rather than
    // creating a duplicate. `None` for every non-restore flow.
    restored_cube: Option<RestoreCubeIdentity>,
    restore_seed: Option<&RestoreCubeSeed>,
) -> Result<app::settings::CubeSettings, String> {
    // Helper: decorate a freshly-minted CubeSettings with
    // PIN + master-signer-fingerprint when we're on the restore path.
    // Pulled out so the "new cube" branches share one code path.
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

    // Base CubeSettings for a *brand-new* Cube. On the Recovery-Kit restore
    // path we reuse the deleted Cube's original UUID + name (verbatim — see
    // `new_with_raw_id`); otherwise fall back to a fresh UUID + the wallet
    // alias.
    let new_cube_base = || -> app::settings::CubeSettings {
        match &restored_cube {
            Some(identity) => app::settings::CubeSettings::new_with_raw_id(
                identity.uuid.clone(),
                identity.name.clone(),
                network,
            ),
            None => app::settings::CubeSettings::new(
                wallet_alias
                    .clone()
                    .unwrap_or_else(|| format!("My {} Cube", network)),
                network,
            ),
        }
    };

    match app::settings::Settings::from_file(network_dir) {
        Ok(mut settings_data) => {
            // First, check if a cube already has this wallet.
            // We don't decorate existing cubes with the restore PIN —
            // if the cube already has a PIN hash / fingerprint those
            // are its source of truth. The restore flow only overwrites
            // Cube-level credentials when we're actually minting a new
            // Cube for the restored wallet.
            if let Some(w_id) = wallet_id {
                if let Some(existing_idx) = settings_data
                    .cubes
                    .iter()
                    .position(|c| c.vault_wallet_id.as_ref() == Some(w_id))
                {
                    // On a Recovery-Kit restore we must reconcile identity *before*
                    // returning this match: if the wallet is attached to a cube with
                    // a **different** UUID than the one being restored, that's the
                    // old-bug duplicate (a prior buggy recovery minted a new Cube).
                    // Returning it here would leave the duplicate attached and the
                    // original still recoverable — the very bug this flow fixes. So
                    // drop the spurious duplicate entirely and fall through, letting
                    // the restore reconciliation below re-attach the wallet to the
                    // restored UUID (reused or minted). Identities agree (or no
                    // restore) → normal return.
                    match &restored_cube {
                        Some(identity) if settings_data.cubes[existing_idx].id != identity.uuid => {
                            info!(
                                "Wallet {} was attached to duplicate cube '{}' ({}); removing it to \
                                 reconcile with restored UUID {}",
                                w_id,
                                settings_data.cubes[existing_idx].name,
                                settings_data.cubes[existing_idx].id,
                                identity.uuid,
                            );
                            settings_data.cubes.remove(existing_idx);
                        }
                        _ => return Ok(settings_data.cubes[existing_idx].clone()),
                    }
                }
            }

            // Recovery-Kit restore: the restored Cube must carry the deleted
            // Cube's *original* UUID so the Connect `register_cube` call
            // (idempotent on UUID) reactivates it instead of minting a
            // duplicate — the reported bug where recovery produced a new Cube
            // and left the original still listed as recoverable. This is
            // checked before the originating / empty-cube reuse below on
            // purpose: attaching the wallet to a *different* local Cube (with
            // its own UUID) is exactly the duplicate we're trying to avoid. If
            // a local Cube already carries the original UUID (a re-run),
            // reuse it; otherwise mint one with that UUID.
            if let Some(RestoreCubeIdentity { uuid, .. }) = &restored_cube {
                if let Some(idx) = settings_data.cubes.iter().position(|c| &c.id == uuid) {
                    if settings_data.cubes[idx].vault_wallet_id.is_some() {
                        return Err(format!(
                            "Cube '{}' has already been recovered on this device.",
                            settings_data.cubes[idx].name
                        ));
                    }
                    let mut cube = settings_data.cubes[idx].clone();
                    cube.vault_wallet_id = wallet_id.cloned();
                    let cube = decorate_new(cube)?;
                    settings_data.cubes[idx] = cube.clone();

                    info!(
                        "Reactivating recovered cube '{}' ({}) with wallet {:?} on {} network",
                        cube.name, uuid, wallet_id, network
                    );

                    return save_cube_settings(network_dir, cube, network, settings_data).await;
                }

                let mut base_cube = new_cube_base();
                base_cube.vault_wallet_id = wallet_id.cloned();
                let cube = decorate_new(base_cube)?;

                info!(
                    "Re-minting recovered cube '{}' ({}) for wallet {:?} on {} network",
                    cube.name, uuid, wallet_id, network
                );

                settings_data.cubes.push(cube.clone());
                return save_cube_settings(network_dir, cube, network, settings_data).await;
            }

            // Second, if we have an originating cube ID, validate and use it
            if let Some(target_cube_id) = originating_cube_id {
                if let Some(target_cube) = settings_data
                    .cubes
                    .iter_mut()
                    .find(|c| c.id == target_cube_id)
                {
                    if let Some(w_id) = wallet_id {
                        if target_cube.vault_wallet_id.is_some() {
                            return Err(format!(
                                "Cube '{}' already has a vault. Remove the existing vault before creating a new one.",
                                target_cube.name
                            ));
                        }
                        target_cube.vault_wallet_id = Some(w_id.clone());
                    }
                    // Apply restore-flow credentials (PIN hash + fingerprint) if
                    // restoring to this cube — same rationale as the empty-cube
                    // fallback: the hash must match the newly-encrypted mnemonic.
                    let cube_clone = decorate_new(target_cube.clone())?;
                    *target_cube = cube_clone.clone();
                    let cube_name = target_cube.name.clone();

                    info!(
                        "Associating wallet {:?} with originating cube '{}' on {} network",
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
                empty_cube.vault_wallet_id = wallet_id.cloned();
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
                    "Associating wallet {:?} with existing cube '{}' on {} network",
                    wallet_id, cube_name, network
                );

                return save_cube_settings(network_dir, empty_cube, network, settings_data).await;
            }

            // Finally, create a new cube for this wallet. `restored_cube` is
            // `None` here (the restore branch above returns early), so
            // `new_cube_base` yields the alias-based fresh-UUID cube.
            let mut base_cube = new_cube_base();
            base_cube.vault_wallet_id = wallet_id.cloned();
            let cube = decorate_new(base_cube)?;
            let cube_name = cube.name.clone();

            info!(
                "Creating new cube '{}' for wallet {:?} on {} network",
                cube_name, wallet_id, network
            );

            settings_data.cubes.push(cube.clone());
            save_cube_settings(network_dir, cube, network, settings_data).await
        }
        Err(_) => {
            // No settings file yet, create first cube. On the restore path
            // `new_cube_base` reuses the deleted Cube's original UUID + name.
            let mut base_cube = new_cube_base();
            base_cube.vault_wallet_id = wallet_id.cloned();
            let cube = decorate_new(base_cube)?;
            let cube_name = cube.name.clone();

            info!(
                "Creating first cube '{}' for wallet {:?} on {} network",
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
            // Recomputed from the P2P panel's Mostro config once panels are built.
            p2p_test_coordinator: false,
            // Fail-closed until `/connect/features` loads and the account panel
            // mirrors the real flags in (see `App::update`'s ConnectAccount arm).
            marketplace_flags: crate::app::features::MarketplaceServerFlags::OFF,
            // Liquid sunset gate. Both halves are filled in later: the local
            // half in `App::new` (from whether the Liquid SDK actually
            // connected), the server half when `/connect/features` loads.
            liquid_gate: crate::app::features::LiquidGate::HIDDEN,
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
            daemon_switch_in_progress: false,
            node_bitcoind_last_log: None,
            node_net_stats: None,
            connect_authenticated: false,
            // Remote backend implies an authenticated Connect session from the
            // start, even before the Connect panel reaches its Dashboard step.
            has_connect_session: true,
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
            recovery_kit_last_backed_up_keychain_descriptor_fingerprint: cube_settings
                .recovery_kit_last_backed_up_keychain_descriptor_fingerprint
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
        // bitcoind (root-level, not a network dir): the blockchain is preserved,
        // but inbound-over-Tor identifying material (Tor data + onion key) is
        // obliterated — see Decision 4.
        touch(
            &root
                .join("bitcoind")
                .join("datadir")
                .join("blocks")
                .join("blk0.dat"),
        );
        touch(&root.join("bitcoind").join("tor-data").join("state"));
        touch(
            &root
                .join("bitcoind")
                .join("datadir")
                .join("onion_v3_private_key"),
        );

        // duress_wipe_targets retries transient read_dir/exists failures
        // internally (see its helpers), so a single call is reliable even on
        // Windows where a virus scanner can briefly hide a just-created dir.
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
        // Cached Connect auth is preserved.
        assert!(
            !targets.iter().any(|t| t.ends_with("connect.json")),
            "connect.json (cached auth) must survive"
        );
        // The blockchain is preserved (expensive to re-sync, not sensitive)...
        assert!(
            !targets.iter().any(|t| t.ends_with("blk0.dat")),
            "bitcoind blockchain must not be wiped"
        );
        // ...but the Tor state and onion-service key (identifying) are wiped.
        assert!(
            targets.contains(&root.join("bitcoind").join("tor-data")),
            "managed Tor data dir must be wiped"
        );
        assert!(
            targets.contains(
                &root
                    .join("bitcoind")
                    .join("datadir")
                    .join("onion_v3_private_key")
            ),
            "onion-service key must be wiped"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn scan_targets_only_material_that_exists() {
        // Fail-safe wiping must not over-target in the normal case: material
        // that genuinely isn't present is excluded (existence returns a definite
        // "no", not the "on doubt, wipe it" fallback).
        let seq = SEQ.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "coincube-wipe-partial-{}-{}",
            std::process::id(),
            seq
        ));
        let net = root.join("bitcoin");
        touch(&net.join("data").join("wallet.db")); // only data/ exists

        let targets = duress_wipe_targets(&root);

        assert!(
            targets.contains(&net.join("data")),
            "present data/ is targeted"
        );
        assert!(
            !targets.contains(&net.join("mnemonics")),
            "absent mnemonics/ must not be targeted"
        );
        assert!(
            !targets.contains(&net.join("settings.json")),
            "absent settings.json must not be targeted"
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod find_or_create_cube_tests {
    //! Regression tests for the Recovery-Kit restore bug: recovering a
    //! deleted Cube must reuse the deleted Cube's *original* UUID so the
    //! Connect `register_cube` call (idempotent on UUID) reactivates it
    //! rather than minting a brand-new Cube (which left the original still
    //! listed as recoverable and let the flow be repeated indefinitely).
    use super::*;

    const ORIG_UUID: &str = "11111111-2222-3333-4444-555555555555";

    fn temp_network_dir(tag: &str) -> NetworkDirectory {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "coincube-foc-{}-{}-{}",
            tag,
            std::process::id(),
            seq
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        NetworkDirectory::new(path)
    }

    fn wallet_id() -> WalletId {
        WalletId::new("abcd1234".to_string(), Some(1_700_000_000))
    }

    /// Reload settings written earlier in the test, tolerating the transient
    /// misses Windows raises when a virus scanner or search indexer briefly
    /// holds a just-written settings.json (surfacing as NotFound or a
    /// permission error). The file has always been written by this point, so a
    /// miss is transient — retry briefly before giving up.
    fn reload(nd: &NetworkDirectory) -> app::settings::Settings {
        for _ in 0..20 {
            if let Ok(settings) = app::settings::Settings::from_file(nd) {
                return settings;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        app::settings::Settings::from_file(nd).expect("reload settings after retries")
    }

    /// The reported scenario: recovery on a wiped install (no settings
    /// file). The restored Cube must carry the original UUID + name, not a
    /// freshly generated one.
    #[tokio::test]
    async fn restore_on_wiped_install_reuses_original_uuid() {
        let nd = temp_network_dir("wiped");
        let wid = wallet_id();
        let cube = find_or_create_cube(
            &nd,
            Some(&wid),
            &Some("Ignored Alias".to_string()),
            bitcoin::Network::Bitcoin,
            None, // originating_cube_id
            Some(RestoreCubeIdentity {
                uuid: ORIG_UUID.to_string(),
                name: "My Vault".to_string(),
            }),
            None, // restore_seed
        )
        .await
        .expect("restore should succeed");

        assert_eq!(
            cube.id, ORIG_UUID,
            "restored Cube must keep the original UUID"
        );
        assert_eq!(
            cube.name, "My Vault",
            "restored Cube keeps the original name"
        );
        assert_eq!(cube.vault_wallet_id.as_ref(), Some(&wid));
    }

    /// Recovery on an install that already holds other, unrelated Cubes:
    /// the restore must mint a *new* Cube carrying the original UUID, never
    /// attach the restored wallet to an unrelated local Cube.
    #[tokio::test]
    async fn restore_with_other_cubes_mints_cube_with_original_uuid() {
        let nd = temp_network_dir("others");
        let mut settings = app::settings::Settings::default();
        let other =
            app::settings::CubeSettings::new("Other".to_string(), bitcoin::Network::Bitcoin)
                .with_vault(WalletId::new("otherchk".to_string(), Some(1)));
        let other_id = other.id.clone();
        settings.cubes.push(other);
        update_settings_file(&nd, |_| Some(settings)).await.unwrap();

        let wid = wallet_id();
        let cube = find_or_create_cube(
            &nd,
            Some(&wid),
            &None,
            bitcoin::Network::Bitcoin,
            None,
            Some(RestoreCubeIdentity {
                uuid: ORIG_UUID.to_string(),
                name: "My Vault".to_string(),
            }),
            None,
        )
        .await
        .expect("restore should succeed");

        assert_eq!(cube.id, ORIG_UUID);
        assert_ne!(cube.id, other_id, "must not reuse the unrelated Cube");
        let reloaded = reload(&nd);
        assert_eq!(reloaded.cubes.len(), 2, "unrelated Cube is preserved");
    }

    /// If a vault-less local Cube already carries the original UUID (e.g. a
    /// partial earlier run), reactivate it in place — don't duplicate it.
    #[tokio::test]
    async fn restore_reactivates_existing_cube_with_original_uuid() {
        let nd = temp_network_dir("reactivate");
        let mut settings = app::settings::Settings::default();
        let mut shell =
            app::settings::CubeSettings::new("Shell".to_string(), bitcoin::Network::Bitcoin);
        shell.id = ORIG_UUID.to_string();
        settings.cubes.push(shell);
        update_settings_file(&nd, |_| Some(settings)).await.unwrap();

        let wid = wallet_id();
        let cube = find_or_create_cube(
            &nd,
            Some(&wid),
            &None,
            bitcoin::Network::Bitcoin,
            None,
            Some(RestoreCubeIdentity {
                uuid: ORIG_UUID.to_string(),
                name: "My Vault".to_string(),
            }),
            None,
        )
        .await
        .expect("restore should succeed");

        assert_eq!(cube.id, ORIG_UUID);
        assert_eq!(cube.vault_wallet_id.as_ref(), Some(&wid));
        let reloaded = reload(&nd);
        assert_eq!(
            reloaded.cubes.len(),
            1,
            "must reactivate in place, not create a duplicate"
        );
    }

    /// Non-restore install is unchanged: a fresh UUID and the wallet alias.
    #[tokio::test]
    async fn non_restore_install_mints_fresh_uuid() {
        let nd = temp_network_dir("fresh");
        let wid = wallet_id();
        let cube = find_or_create_cube(
            &nd,
            Some(&wid),
            &Some("My Alias".to_string()),
            bitcoin::Network::Bitcoin,
            None,
            None, // no restored_cube
            None,
        )
        .await
        .expect("install should succeed");

        assert_ne!(cube.id, ORIG_UUID);
        assert_eq!(cube.name, "My Alias");
        assert_eq!(cube.vault_wallet_id.as_ref(), Some(&wid));
    }

    #[tokio::test]
    async fn existing_wallet_match_returns_its_cube_without_rewriting_settings() {
        let nd = temp_network_dir("existing-wallet");
        let wid = wallet_id();
        let mut settings = app::settings::Settings::default();
        let existing =
            app::settings::CubeSettings::new("Existing".to_string(), bitcoin::Network::Bitcoin)
                .with_vault(wid.clone());
        let existing_id = existing.id.clone();
        settings.cubes.push(existing);
        update_settings_file(&nd, |_| Some(settings)).await.unwrap();

        let cube = find_or_create_cube(
            &nd,
            Some(&wid),
            &Some("Ignored Alias".to_string()),
            bitcoin::Network::Bitcoin,
            None,
            None,
            None,
        )
        .await
        .expect("existing wallet should be found");

        assert_eq!(cube.id, existing_id);
        assert_eq!(cube.name, "Existing");
        let reloaded = reload(&nd);
        assert_eq!(reloaded.cubes.len(), 1);
    }

    #[tokio::test]
    async fn restore_fails_when_original_cube_already_has_a_vault() {
        let nd = temp_network_dir("restore-conflict");
        let wid = wallet_id();
        let mut settings = app::settings::Settings::default();
        let mut restored = app::settings::CubeSettings::new(
            "Already Restored".to_string(),
            bitcoin::Network::Bitcoin,
        )
        .with_vault(WalletId::new("otherwallet".to_string(), Some(9)));
        restored.id = ORIG_UUID.to_string();
        settings.cubes.push(restored);
        update_settings_file(&nd, |_| Some(settings)).await.unwrap();

        let err = find_or_create_cube(
            &nd,
            Some(&wid),
            &None,
            bitcoin::Network::Bitcoin,
            None,
            Some(RestoreCubeIdentity {
                uuid: ORIG_UUID.to_string(),
                name: "My Vault".to_string(),
            }),
            None,
        )
        .await
        .expect_err("restore should reject an already recovered cube");

        assert!(err.contains("already been recovered"));
    }

    #[tokio::test]
    async fn originating_cube_attaches_wallet_and_restore_credentials() {
        let nd = temp_network_dir("originating");
        let wid = wallet_id();
        let fp = bitcoin::bip32::Fingerprint::from([0xaa, 0xbb, 0xcc, 0xdd]);
        let seed = RestoreCubeSeed {
            pin: zeroize::Zeroizing::new("135790".to_string()),
            master_signer_fingerprint: fp,
        };
        let mut settings = app::settings::Settings::default();
        let shell =
            app::settings::CubeSettings::new("Shell".to_string(), bitcoin::Network::Bitcoin);
        let shell_id = shell.id.clone();
        settings.cubes.push(shell);
        update_settings_file(&nd, |_| Some(settings)).await.unwrap();

        let cube = find_or_create_cube(
            &nd,
            Some(&wid),
            &None,
            bitcoin::Network::Bitcoin,
            Some(shell_id.clone()),
            None,
            Some(&seed),
        )
        .await
        .expect("originating cube should be updated");

        assert_eq!(cube.id, shell_id);
        assert_eq!(cube.vault_wallet_id.as_ref(), Some(&wid));
        assert_eq!(cube.master_signer_fingerprint, Some(fp));
        assert!(cube.verify_pin("135790"));
    }

    #[tokio::test]
    async fn originating_cube_errors_when_missing_or_already_vaulted() {
        let nd = temp_network_dir("originating-errors");
        let wid = wallet_id();
        let mut settings = app::settings::Settings::default();
        let occupied =
            app::settings::CubeSettings::new("Occupied".to_string(), bitcoin::Network::Bitcoin)
                .with_vault(WalletId::new("otherwallet".to_string(), Some(2)));
        let occupied_id = occupied.id.clone();
        settings.cubes.push(occupied);
        update_settings_file(&nd, |_| Some(settings)).await.unwrap();

        let occupied_err = find_or_create_cube(
            &nd,
            Some(&wid),
            &None,
            bitcoin::Network::Bitcoin,
            Some(occupied_id),
            None,
            None,
        )
        .await
        .expect_err("originating cube with an existing vault should fail");
        assert!(occupied_err.contains("already has a vault"));

        let missing_err = find_or_create_cube(
            &nd,
            Some(&wid),
            &None,
            bitcoin::Network::Bitcoin,
            Some("missing-cube".to_string()),
            None,
            None,
        )
        .await
        .expect_err("missing originating cube should fail");
        assert!(missing_err.contains("Cannot find originating cube"));
    }

    #[tokio::test]
    async fn empty_cube_fallback_attaches_wallet_before_minting_new_cube() {
        let nd = temp_network_dir("empty-cube");
        let wid = wallet_id();
        let mut settings = app::settings::Settings::default();
        let empty =
            app::settings::CubeSettings::new("Empty".to_string(), bitcoin::Network::Bitcoin);
        let empty_id = empty.id.clone();
        settings.cubes.push(empty);
        update_settings_file(&nd, |_| Some(settings)).await.unwrap();

        let cube = find_or_create_cube(
            &nd,
            Some(&wid),
            &Some("Should Not Mint".to_string()),
            bitcoin::Network::Bitcoin,
            None,
            None,
            None,
        )
        .await
        .expect("empty cube should be reused");

        assert_eq!(cube.id, empty_id);
        assert_eq!(cube.vault_wallet_id.as_ref(), Some(&wid));
        let reloaded = reload(&nd);
        assert_eq!(reloaded.cubes.len(), 1, "must reuse instead of minting");
    }

    #[tokio::test]
    async fn first_non_restore_cube_uses_default_alias_when_none_is_given() {
        let nd = temp_network_dir("first-default");
        let wid = wallet_id();

        let cube = find_or_create_cube(
            &nd,
            Some(&wid),
            &None,
            bitcoin::Network::Signet,
            None,
            None,
            None,
        )
        .await
        .expect("first cube should be created");

        assert_eq!(cube.name, "My signet Cube");
        assert_eq!(cube.network, bitcoin::Network::Signet);
        assert_eq!(cube.vault_wallet_id.as_ref(), Some(&wid));
        let reloaded = reload(&nd);
        assert_eq!(reloaded.cubes.len(), 1);
    }

    /// Upgrade path: a previous (buggy) recovery left the wallet attached to a
    /// *duplicate* Cube with a different UUID, while the original Cube is still
    /// recoverable. Re-running recovery must reconcile — move the wallet onto
    /// the restored (original) UUID and remove the duplicate — rather than
    /// returning the stale duplicate match and leaving the original recoverable.
    #[tokio::test]
    async fn restore_reconciles_wallet_off_a_duplicate_uuid() {
        let nd = temp_network_dir("dup-uuid");
        let wid = wallet_id();

        // The exact state the old bug produced: a duplicate Cube (its own,
        // different UUID) already holds the wallet.
        let mut settings = app::settings::Settings::default();
        let dup =
            app::settings::CubeSettings::new("Duplicate".to_string(), bitcoin::Network::Bitcoin)
                .with_vault(wid.clone());
        let dup_id = dup.id.clone();
        assert_ne!(
            dup_id, ORIG_UUID,
            "duplicate must not already carry the original UUID"
        );
        settings.cubes.push(dup);
        update_settings_file(&nd, |_| Some(settings)).await.unwrap();

        let cube = find_or_create_cube(
            &nd,
            Some(&wid),
            &None,
            bitcoin::Network::Bitcoin,
            None,
            Some(RestoreCubeIdentity {
                uuid: ORIG_UUID.to_string(),
                name: "My Vault".to_string(),
            }),
            None,
        )
        .await
        .expect("restore should succeed");

        // The wallet now lives on the restored (original) UUID, not the duplicate.
        assert_eq!(cube.id, ORIG_UUID, "wallet must move to the restored UUID");
        assert_eq!(cube.vault_wallet_id.as_ref(), Some(&wid));

        let reloaded = reload(&nd);
        assert!(
            reloaded.cubes.iter().all(|c| c.id != dup_id),
            "the spurious duplicate Cube must be removed outright"
        );
        assert_eq!(
            reloaded.cubes.len(),
            1,
            "only the restored Cube should remain"
        );
        let restored = reloaded
            .cubes
            .iter()
            .find(|c| c.id == ORIG_UUID)
            .expect("restored cube exists");
        assert_eq!(restored.vault_wallet_id.as_ref(), Some(&wid));
    }

    /// Seed-only (Vault-less) Recovery-Kit restore: `wallet_id` is `None`
    /// because a seed-only Cube has no Vault descriptor to attach. The restored
    /// Cube must still reuse the deleted Cube's original UUID + name, leave
    /// `vault_wallet_id` empty, and carry the restore credentials (PIN hash +
    /// master-signer fingerprint) so PIN entry decrypts the just-persisted
    /// mnemonic.
    #[tokio::test]
    async fn restore_seed_only_reuses_uuid_and_applies_credentials() {
        let nd = temp_network_dir("seed-only");
        let fp = bitcoin::bip32::Fingerprint::from([0xde, 0xad, 0xbe, 0xef]);
        let seed = RestoreCubeSeed {
            pin: zeroize::Zeroizing::new("246810".to_string()),
            master_signer_fingerprint: fp,
        };

        let cube = find_or_create_cube(
            &nd,
            None, // seed-only: no Vault wallet to attach
            &Some("Ignored Alias".to_string()),
            bitcoin::Network::Bitcoin,
            None, // originating_cube_id
            Some(RestoreCubeIdentity {
                uuid: ORIG_UUID.to_string(),
                name: "My Seed Cube".to_string(),
            }),
            Some(&seed),
        )
        .await
        .expect("seed-only restore should succeed");

        assert_eq!(
            cube.id, ORIG_UUID,
            "restored seed-only Cube keeps the original UUID"
        );
        assert_eq!(cube.name, "My Seed Cube", "restored Cube keeps its name");
        assert_eq!(
            cube.vault_wallet_id, None,
            "seed-only Cube has no Vault wallet"
        );
        assert_eq!(
            cube.master_signer_fingerprint,
            Some(fp),
            "restore fingerprint applied"
        );
        assert!(
            cube.verify_pin("246810"),
            "restore PIN hash applied and verifies"
        );
    }

    /// Non-restore install with `wallet_id: None` and no originating cube and no
    /// restored identity: this must not error, and — critically — must not
    /// steal an unrelated existing vault-less Cube's identity in a way that
    /// clobbers its credentials. With `restore_seed = None`, `decorate_new` is a
    /// no-op, so the reused empty Cube keeps whatever PIN hash / fingerprint it
    /// already had.
    #[tokio::test]
    async fn seed_only_non_restore_does_not_clobber_existing_cube_credentials() {
        let nd = temp_network_dir("seed-only-guard");

        // An existing vault-less Cube that already carries its own credentials.
        let mut settings = app::settings::Settings::default();
        let existing =
            app::settings::CubeSettings::new("Existing".to_string(), bitcoin::Network::Bitcoin)
                .with_master_signer(bitcoin::bip32::Fingerprint::from([1, 2, 3, 4]))
                .with_pin("111111")
                .expect("hash pin");
        let existing_id = existing.id.clone();
        settings.cubes.push(existing);
        update_settings_file(&nd, |_| Some(settings)).await.unwrap();

        let cube = find_or_create_cube(
            &nd,
            None, // no Vault wallet
            &None,
            bitcoin::Network::Bitcoin,
            None, // no originating cube
            None, // no restored identity
            None, // no restore seed
        )
        .await
        .expect("non-restore seed-only path should not error");

        // The vault-less Cube is reused (empty-cube branch) but its credentials
        // are left intact — no restore seed means no re-hash.
        assert_eq!(cube.id, existing_id, "reuses the existing vault-less Cube");
        assert_eq!(
            cube.master_signer_fingerprint,
            Some(bitcoin::bip32::Fingerprint::from([1, 2, 3, 4])),
            "existing fingerprint is preserved, not clobbered"
        );
        assert!(
            cube.verify_pin("111111"),
            "existing PIN hash is preserved, not clobbered"
        );
        assert_eq!(cube.vault_wallet_id, None);
    }
}
