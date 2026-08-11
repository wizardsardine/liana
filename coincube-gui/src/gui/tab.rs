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

/// The unlock screen a Cube needs, decided in exactly one place.
///
/// Both routes into an unlock screen go through here: opening a Cube from the
/// launcher, and returning to the prompt after the idle timer re-locks one.
///
/// They used to decide separately, and on 2026-08-07 they disagreed. Re-lock
/// sent *every* Cube to PIN entry, so a passkey Cube that had opened correctly a
/// minute earlier became unopenable the moment it locked: a keypad with no PIN
/// to accept and no seed file for one to decrypt, answering every attempt with
/// "this Cube's seed isn't on this device" and offering only Back. A third
/// caller that forgets the branch is that same bug, so this is one function and
/// it is tested.
fn unlock_state(
    cube: crate::app::settings::CubeSettings,
    datadir_root: std::path::PathBuf,
    on_success: crate::pin_entry::PinEntrySuccess,
    duress_account_id: Option<String>,
) -> State {
    if cube.is_passkey_cube() {
        // `duress_account_id` is deliberately dropped: a passkey Cube has no
        // PIN, so it has no duress PIN to enrol and no duress arm to reach.
        State::PasskeyUnlock(crate::passkey_unlock::PasskeyUnlock::new(
            cube,
            datadir_root,
            on_success,
        ))
    } else {
        State::PinEntry(crate::pin_entry::PinEntry::new(
            cube,
            datadir_root,
            on_success,
            duress_account_id,
        ))
    }
}

pub enum State {
    Home(Home),
    Installer(Installer),
    Loader(Loader),
    Login(login::CoincubeLiteLogin),
    PinEntry(crate::pin_entry::PinEntry),
    /// Unlock screen for a passkey Cube. Its own state rather than a mode of
    /// `PinEntry` because it shares nothing with one: no PIN buffer, no
    /// throttle, no duress arm — a passkey Cube has no PIN to guess at and no
    /// duress PIN to enrol.
    PasskeyUnlock(crate::passkey_unlock::PasskeyUnlock),
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
/// - `settings.json` — Cube metadata. (The PIN and duress-PIN hashes that used
///   to live here are gone; the duress *marker* now lives in `mnemonics/`, so
///   it is wiped by that entry.)
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
    PasskeyUnlock(crate::passkey_unlock::Message),
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
    /// A passkey unlock succeeded, but the session it parked its signer in was
    /// closed before the wallets finished loading — a lock, a return to the
    /// launcher, or a duress activation.
    ///
    /// Its own message rather than an `Err` through
    /// [`Message::BreezClientLoadedAfterPin`]: that handler treats every Breez
    /// error as "carry on with a disconnected client", which would drop the
    /// user *inside* a Cube that is supposed to be locked, with no Liquid, no
    /// Spark and no P2P, and the explanation only in the log.
    ///
    /// Carries nothing — the tab is still in [`State::PasskeyUnlock`] at this
    /// point (nothing mutates it between the assertion and the load), so the
    /// screen just returns to its prompt.
    PasskeyRelocked,
    BreezClientLoadedAfterPin {
        /// Set when the post-unlock seed-file migration aborted. The Cube still
        /// opens and its files are untouched, but the upgrade did not happen and
        /// the user is told rather than only the log.
        migration_error: Option<String>,
        /// Set when the Cube's Connect encryption key could not be derived
        /// *despite* a seed file being present (`PLAN-connect-blinding` D2).
        /// Same contract as `migration_error`: the Cube opens fine, but the
        /// consequence is invisible — the Cube never registers with Connect and
        /// its Contacts cannot share keys with it — so it is told, not only
        /// logged.
        enc_key_error: Option<String>,
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
    /// Bubbles up to the pane so it can focus the Home tab on its Connect →
    /// Plan & Billing section — fired when the user clicks "View plans" on a
    /// paid-feature locked card outside the Connect page (Settings → Vault
    /// Recovery Alerts).
    OpenPlanBilling,
    /// Bubbles up to the pane on a Home-tab login edge so it can
    /// broadcast a session re-check to every open Cube tab.
    ConnectSignedIn,
    /// Re-lock the open Cube: drop the `App` (and with it the decrypted signer,
    /// the Liquid client and the Spark bridge subprocess), zeroize the session
    /// PIN, and return to the PIN screen for the same Cube.
    ///
    /// Fired by the idle timer and by the explicit "Lock" control. Before this
    /// existed, the only route out of `App` was back to `Home`, which left the
    /// seed resident until the process exited, and there was no idle re-lock at
    /// all.
    LockCube,
}

pub struct Tab {
    pub id: usize,
    pub state: State,
    /// Persisted theme mode — carried across state transitions so new App
    /// caches inherit the correct mode immediately.
    pub theme_mode: coincube_ui::theme::palette::ThemeMode,
    /// A seed-migration warning waiting for a surface to show it on.
    ///
    /// Held rather than dispatched because only `State::App` handles
    /// `Message::Run`; see [`Tab::flush_migration_warning`].
    /// Non-fatal warnings raised during unlock, parked until an `App` exists
    /// to show them. `Loader` and `Login` cannot toast, so anything raised
    /// before the Cube is up waits here.
    pending_unlock_warnings: Vec<String>,
}

impl Tab {
    pub fn new(id: usize, state: State) -> Self {
        Tab {
            id,
            state,
            theme_mode: coincube_ui::theme::palette::ThemeMode::default(),
            pending_unlock_warnings: Vec::new(),
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
            State::PasskeyUnlock(_) => "Unlock with passkey",
            State::App(a) => a.title(),
            State::DuressActive(_) => "COINCUBE",
        }
    }

    /// How long an open Cube may sit idle before it re-locks.
    ///
    /// Long enough not to interrupt someone reading their transaction history,
    /// short enough that a laptop left on a café table doesn't stay unlocked
    /// for the afternoon.
    const IDLE_LOCK_AFTER: std::time::Duration = std::time::Duration::from_secs(15 * 60);

    /// Emit the pending migration warning once the tab is actually in `App`.
    ///
    /// The toast rides on `Message::Run`, and the only arm that handles that is
    /// `(State::App(app), Message::Run(..))` — everything else falls through to
    /// `_ => Task::none()`. Queuing it behind the transition to `Loader` or
    /// `Login` therefore did not delay the warning, it discarded it: those
    /// states run for as long as a daemon start or a sign-in takes, and the
    /// message is consumed and dropped long before `App` exists. "Your seed
    /// files were not upgraded" going silent is the one outcome this warning
    /// exists to prevent, so hold it on the tab and flush it from the state
    /// that can render it.
    fn flush_migration_warning(&mut self) -> Task<Message> {
        if !matches!(self.state, State::App(_)) {
            return Task::none();
        }
        let queued = std::mem::take(&mut self.pending_unlock_warnings);
        if queued.is_empty() {
            return Task::none();
        }
        Task::batch(queued.into_iter().map(|msg| {
            Task::done(Message::Run(app::Message::View(
                app::view::Message::ShowToast(log::Level::Warn, msg),
            )))
        }))
    }

    pub fn on_tick(&mut self) -> Task<Message> {
        // Idle auto-lock.
        //
        // Activity is recorded from the real keyboard/mouse/touch listener in
        // `gui::subscription`, **not** from message traffic. Treating any
        // non-`Tick` message as activity did not work: this very tick spawns
        // `UpdateDaemonCache` and `BitcoindNetStats` every second, and their
        // results arrive back as non-`Tick` messages, so the timer reset once a
        // second and the lock never fired at all.
        let idle_expired = crate::app::session::idle_for()
            .map(|d| d >= Self::IDLE_LOCK_AFTER)
            .unwrap_or(false);

        match &mut self.state {
            State::App(app) => {
                if idle_expired {
                    return Task::done(Message::LockCube);
                }
                app.on_tick().map(Message::Run)
            }
            // `Loader` and `Login` hold an open session too — the PIN and the
            // decrypted signer — and a hung daemon start can leave the app
            // sitting in `Loader` indefinitely.
            //
            // Drop the secrets rather than tearing the state down. Dropping a
            // `Loader` would kill an in-progress daemon start or full chain
            // scan that the user may be deliberately waiting on, and it is the
            // in-memory secret that matters, not which screen is showing. The
            // cost is that a Vault set up later in this session has no PIN to
            // encrypt with and fails with a clear "re-open the Cube" message.
            //
            // Once closed, `idle_for()` is `None`, so this does not repeat.
            State::Loader(_) | State::Login(_) => {
                if idle_expired {
                    tracing::info!("idle: dropping session secrets held during load / sign-in");
                    crate::app::session::close();
                }
                Task::none()
            }
            _ => Task::none(),
        }
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        use crate::app::settings::global::GlobalSettings;

        let result = match (&mut self.state, message) {
            (State::App(app), Message::LockCube) => {
                // Order matters. Zeroize the session PIN first, then drop the
                // `App`: dropping it takes the decrypted `MasterSigner` (which
                // now scrubs itself), the Liquid client, and the Spark backend —
                // whose `Drop` sends `Method::Shutdown` to the bridge
                // subprocess, so the child process holding the plaintext
                // mnemonic exits too.
                let cube = app.cube_settings().clone();
                let datadir = app.datadir().clone();
                let network = app.cache().network;
                crate::app::session::close();

                let config = app::Config::from_file(
                    &datadir
                        .network_directory(network)
                        .path()
                        .join(app::config::DEFAULT_FILE_NAME),
                );

                let Ok(config) = config else {
                    // No readable gui config to hand the PIN screen. Fall all
                    // the way back to the launcher rather than staying open.
                    let (home, command) = Home::new(datadir, Some(network));
                    self.state = State::Home(home);
                    return command.map(Message::Launch);
                };

                let wallet_settings = cube.vault_wallet_id.as_ref().and_then(|vault_id| {
                    let network_dir = datadir.network_directory(network);
                    app::settings::Settings::from_file(&network_dir)
                        .ok()
                        .and_then(|s| {
                            s.wallets
                                .iter()
                                .find(|w| w.wallet_id() == *vault_id)
                                .cloned()
                        })
                });
                let duress_account_id =
                    crate::services::duress::DuressLocalState::load(datadir.path())
                        .map(|st| st.account_id)
                        .unwrap_or(None);
                let datadir_root = datadir.path().to_path_buf();
                let on_success = crate::pin_entry::PinEntrySuccess::LoadApp {
                    datadir,
                    config,
                    network,
                    internal_bitcoind: None,
                    backup: None,
                    wallet_settings,
                };
                // The `PASSKEY_ENABLED` guard the open path carries is
                // deliberately not repeated here: this Cube is already open in
                // this process, so the build plainly has the feature on.
                self.state = unlock_state(cube, datadir_root, on_success, duress_account_id);
                Task::none()
            }
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
                    // Mandatory-backup gate (PLAN-cube-unlock-hardening PR 7).
                    // A Cube created under the gate is not usable until its
                    // backup is demonstrated or explicitly bypassed: its seed is
                    // sealed to this machine's keystore, so losing the machine
                    // without a backup loses the funds outright. Cubes that
                    // predate the gate are never blocked — see
                    // `CubeSettings::creation_backup_required`.
                    //
                    // # Why `None` for the kit halves (X4)
                    //
                    // Passing `None` means `cube_backup_completeness` answers
                    // `Unknown`, which `evaluate` fails **closed**. Read alone
                    // that says a Cube backed up only by a server Recovery Kit
                    // would be blocked here. It cannot be, and fetching the
                    // halves would be worse than not:
                    //
                    // 1. **The shape is unreachable.** `home.rs` arms the flag
                    //    and writes `backed_up` (or a `CreationBackupBypass`)
                    //    in the *same* settings update — see
                    //    `finalize_cube_creation` and the recovery path beside
                    //    it. An armed Cube with neither piece of local evidence
                    //    is never persisted, so `evaluate` returns
                    //    `Satisfied`/`Bypassed` before it ever consults the
                    //    kit. Pinned by
                    //    `creation_never_persists_an_armed_cube_without_evidence`.
                    // 2. **`backed_up` is monotone.** Nothing in the codebase
                    //    clears it; the only writers set it to `true`. A Cube
                    //    that satisfies the gate once satisfies it forever.
                    // 3. **Fetching would put the network on the unlock path.**
                    //    Failing closed on `Unknown` is right for a *creation*
                    //    decision the user can retry. At open it would mean an
                    //    offline user, an expired Connect session or a
                    //    five-second API timeout locks a wallet whose seed is
                    //    sitting decryptable on this machine. That converts an
                    //    outage into a lost Cube — the exact class of failure
                    //    this gate exists to prevent.
                    //
                    // Local evidence is therefore not a weaker check here, it
                    // is the only one that can be made without introducing a
                    // worse failure. The gate's own doc makes the same call for
                    // creation: a written seed phrase alone satisfies it,
                    // because "demanding a Connect account to create a local
                    // wallet would be wrong".
                    if let crate::services::unlock::creation_gate::CreationGate::Blocked(reason) =
                        crate::services::unlock::creation_gate::evaluate_for_cube(&cube, None)
                    {
                        l.set_error(format!(
                            "{reason}\n\n{}",
                            crate::services::unlock::creation_gate::not_a_backup_copy(
                                cube.is_passkey_cube()
                            )
                        ));
                        return Task::none();
                    }

                    // A passkey Cube has no PIN and no encrypted seed file —
                    // its master seed is re-derived from a WebAuthn PRF
                    // assertion on every open. Route it to its own unlock
                    // screen rather than the PIN keypad, which would ask for a
                    // PIN that does not exist and then fail on a seed file that
                    // is not there.
                    //
                    // The flag still gates it: `PASSKEY_ENABLED` off means no
                    // build can *create* one of these, but a datadir carried
                    // from a build where it was on can still contain one. Say
                    // what is true and stop, rather than open a Cube whose
                    // unlock path this build has deliberately turned off.
                    let passkey_cube = cube.is_passkey_cube();
                    if passkey_cube && !crate::feature_flags::PASSKEY_ENABLED {
                        l.set_error(
                            "This Cube is unlocked with a passkey, and passkey unlock is \
                             turned off in this build. Restore it from your Recovery Kit \
                             and its recovery password, or use a build with \
                             COINCUBE_ENABLE_PASSKEY set.",
                        );
                        return Task::none();
                    }

                    // Shared by both unlock paths from here down: the Vault's
                    // wallet settings, the duress account id (PIN path only —
                    // a passkey Cube has no duress PIN), and where to go on
                    // success.
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

                    // Captured before `datadir_path` is moved into `on_success`.
                    // PIN entry needs it to reach the seed file it verifies against.
                    let datadir_root = datadir_path.path().to_path_buf();

                    let on_success = crate::pin_entry::PinEntrySuccess::LoadApp {
                        datadir: datadir_path,
                        config: cfg,
                        network,
                        internal_bitcoind: None,
                        backup: None,
                        wallet_settings,
                    };

                    self.state = unlock_state(cube, datadir_root, on_success, duress_account_id);
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
                                    app::seed_source::SeedSource::encrypted_file(
                                        seed.master_signer_fingerprint,
                                        seed.pin.as_str(),
                                    ),
                                    &cube.id,
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
                                    app::seed_source::SeedSource::encrypted_file(
                                        seed.master_signer_fingerprint,
                                        seed.pin.as_str(),
                                    ),
                                    &cube.id,
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
                    app::Message::View(app::view::Message::OpenPlanBilling) => {
                        // Pure navigation: bubble to the pane so it focuses the
                        // Home tab on Connect → Plan & Billing (the "View plans"
                        // CTA on a paid-feature locked card outside Connect).
                        Task::done(Message::OpenPlanBilling)
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
                                    // Carried out of the migration block so the
                                    // failure reaches the user rather than only
                                    // the log file.
                                    let mut migration_error: Option<String> = None;
                                    // Set only by the CEK::Unreadable arm below.
                                    let mut enc_key_error: Option<String> = None;

                                    // The PIN stays available for the lifetime
                                    // of the open Cube. The Vault installer
                                    // (launched later, from inside the app) has
                                    // to encrypt the hot signer it generates,
                                    // and there is no plaintext branch left for
                                    // it to fall back on. See `app::session`.
                                    app::session::open(cube.id.clone(), pin.clone());

                                    // Bring any legacy seed files up to the
                                    // current wire version now that the PIN is
                                    // in hand: plaintext files written by the
                                    // pre-hardening installer, `ENCRYPTED_V1`
                                    // files with unauthenticated headers, and
                                    // `ENCRYPTED_V2` files on a machine that now
                                    // has a device secret. Never eager — this
                                    // only runs after an unlock has already
                                    // succeeded. Logs a count, never content.
                                    {
                                        let root = datadir_clone.path().to_path_buf();
                                        let cube_for_migration = cube.clone();
                                        let pin_for_migration = pin.clone();
                                        let joined = tokio::task::spawn_blocking(move || {
                                            let loc = crate::services::unlock::CubeLocation::new(
                                                &root,
                                                &cube_for_migration,
                                            );
                                            let outcome =
                                                crate::services::unlock::migrate_seed_files(
                                                    &loc,
                                                    &pin_for_migration,
                                                )?;
                                            // Give this Cube its second slot if
                                            // it arrived without one — restored
                                            // from a Recovery Kit, or minted
                                            // before unit 6b. Same blocking
                                            // task: it costs one Argon2 pass and
                                            // must not run on the UI thread.
                                            let slot =
                                                crate::services::unlock::ensure_second_slot(&loc)?;
                                            Ok::<_, crate::services::unlock::UnlockError>((
                                                outcome, slot,
                                            ))
                                        })
                                        .await;

                                        // Do not discard this. `let _ = …` threw
                                        // away the count *and* the `JoinError`,
                                        // so a panic inside migration — in code
                                        // that rewrites seed files — left no
                                        // trace at all.
                                        match joined {
                                            Ok(Ok((outcome, new_slot))) => {
                                                // Persist the backfilled slot
                                                // name. Without this the decoy
                                                // just written is unreachable
                                                // and the next unlock mints
                                                // another one.
                                                if let Some(name) = new_slot {
                                                    cube.duress_slot_file = Some(name.clone());
                                                    let cube_id = cube.id.clone();
                                                    let network_dir = datadir_clone
                                                        .network_directory(network_val);
                                                    if let Err(e) =
                                                        app::settings::update_settings_file(
                                                            &network_dir,
                                                            |mut s| {
                                                                if let Some(c) = s
                                                                    .cubes
                                                                    .iter_mut()
                                                                    .find(|c| c.id == cube_id)
                                                                {
                                                                    c.duress_slot_file = Some(name);
                                                                }
                                                                Some(s)
                                                            },
                                                        )
                                                        .await
                                                    {
                                                        error!(
                                                            "could not record the second slot for \
                                                             cube {}: {e}",
                                                            cube.id
                                                        );
                                                    }
                                                }
                                                if outcome.did_work() {
                                                    info!(
                                                        "migration: {} seed file(s) upgraded for cube {}",
                                                        outcome.migrated, cube.id
                                                    );
                                                }
                                                if outcome.skipped_no_backup {
                                                    info!(
                                                        "migration: cube {} stays at v2 until it \
                                                         has a backup",
                                                        cube.id
                                                    );
                                                }
                                                // Migration cannot re-seal a
                                                // duress marker — that needs
                                                // the duress PIN, and this
                                                // path holds the regular one —
                                                // so it replaces the slot with
                                                // a decoy and the enrolment is
                                                // gone. Say so, and make the
                                                // device's own state agree
                                                // rather than keep claiming an
                                                // enrolment whose trigger no
                                                // longer exists.
                                                //
                                                // Acceptable only because
                                                // duress is feature-gated and
                                                // unreleased. Revisit before
                                                // it ships.
                                                if outcome.duress_was_cleared() {
                                                    log::warn!(
                                                        "migration: duress enrolment on cube {} was \
                                                         cleared — its marker could not be carried \
                                                         across the upgrade. Re-enroll to arm it \
                                                         again.",
                                                        cube.id
                                                    );
                                                    let root = datadir_clone.path().to_path_buf();
                                                    if let Ok(mut st) =
                                                        crate::services::duress::DuressLocalState::load(
                                                            &root,
                                                        )
                                                    {
                                                        if st.enrolled || st.arming {
                                                            st.disarm();
                                                            let _ = st.save(&root);
                                                            // Only here, inside the
                                                            // enrolled check: the slot
                                                            // is rewritten whenever its
                                                            // wire version is stale, so
                                                            // `duress_was_cleared` is
                                                            // true for most Cubes and
                                                            // says nothing on its own
                                                            // about whether there was an
                                                            // enrolment to lose. This
                                                            // branch is where one
                                                            // demonstrably was.
                                                            //
                                                            // A log line is not telling
                                                            // the user: they believe a
                                                            // PIN still wipes this
                                                            // device, and it no longer
                                                            // does.
                                                            migration_error = Some(
                                                                "Upgrading this Cube's files \
                                                                 turned duress mode off — your \
                                                                 duress PIN no longer erases \
                                                                 this device. Turn duress mode \
                                                                 on again to set it up."
                                                                    .to_string(),
                                                            );
                                                        }
                                                    }
                                                }
                                            }
                                            // The keystore was reachable-but-broken.
                                            // The Cube still opens — the seed files
                                            // were left exactly as they were — but
                                            // the user has to know the upgrade did
                                            // not happen.
                                            Ok(Err(e)) => {
                                                error!(
                                                    "migration aborted for cube {}: {e}",
                                                    cube.id
                                                );
                                                migration_error = Some(e.to_string());
                                            }
                                            Err(e) => {
                                                error!(
                                                    "migration task failed for cube {}: {e}",
                                                    cube.id
                                                );
                                                migration_error = Some(
                                                    "Coincube couldn't finish upgrading this \
                                                     Cube's files. Your Cube is unchanged and \
                                                     still opens."
                                                        .to_string(),
                                                );
                                            }
                                        }
                                    }

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
                                                &cube.id,
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

                                    // Connect blinding (PLAN-connect-blinding
                                    // PR D2): the Cube's encryption *public*
                                    // key is seed-derived, so unlock is the
                                    // only moment it can be computed — the
                                    // registration wave that publishes it runs
                                    // later, after Connect sign-in, with no PIN
                                    // in hand. Derive once and persist; the
                                    // private half is never stored and is
                                    // re-derived on demand at decrypt time.
                                    if cube.connect_encryption_pubkey.is_none() {
                                        if let Some(fp) = breez_signer_fingerprint {
                                            use app::settings::ConnectEncryptionKey as CEK;
                                            match app::settings::derive_connect_encryption_pubkey(
                                                datadir_clone.path(),
                                                network_val,
                                                fp,
                                                &pin,
                                                &cube.id,
                                            ) {
                                                CEK::Derived(pubkey) => {
                                                    cube.connect_encryption_pubkey =
                                                        Some(pubkey.clone());
                                                    let cube_id = cube.id.clone();
                                                    let network_dir = datadir_clone
                                                        .network_directory(network_val);
                                                    if let Err(e) =
                                                        app::settings::update_settings_file(
                                                            &network_dir,
                                                            |mut s| {
                                                                if let Some(c) = s
                                                                    .cubes
                                                                    .iter_mut()
                                                                    .find(|c| c.id == cube_id)
                                                                {
                                                                    c.connect_encryption_pubkey =
                                                                        Some(pubkey.clone());
                                                                }
                                                                Some(s)
                                                            },
                                                        )
                                                        .await
                                                    {
                                                        tracing::warn!(
                                                            "Failed to persist Connect encryption \
                                                             pubkey for cube {}: {}",
                                                            cube.id,
                                                            e
                                                        );
                                                    }
                                                }
                                                // No seed on this device — expected for a
                                                // watch-only restore or a passkey Cube. Nothing
                                                // to derive and nothing wrong.
                                                CEK::NoSeed => {
                                                    tracing::debug!(
                                                        "Cube {} has no master seed on this \
                                                         device; skipping Connect encryption-key \
                                                         derivation",
                                                        cube.id
                                                    );
                                                }
                                                // The Cube just unlocked and a seed file IS
                                                // there, but it would not open for us — the
                                                // credentials disagree with the unlock path
                                                // (seed_crypt cube_id binding, or a v3 file
                                                // reached without the device secret). Never
                                                // skip this quietly: the Cube would silently
                                                // never register an encryption pubkey, its
                                                // Contacts could not enrol enveloped keys, and
                                                // it would sit in the A5 coverage report as a
                                                // straggler with no visible cause.
                                                CEK::Unreadable(why) => {
                                                    tracing::error!(
                                                        cube_id = %cube.id,
                                                        fingerprint = %fp,
                                                        error = %why,
                                                        "Connect blinding: the master seed for \
                                                         this Cube exists but could not be \
                                                         opened to derive its encryption key. \
                                                         This Cube will not register with \
                                                         Connect and its Contacts cannot share \
                                                         keys with it until this is fixed."
                                                    );
                                                    enc_key_error = Some(format!(
                                                        "This Cube couldn't prepare its Connect \
                                                         encryption key, so contacts can't share \
                                                         keys with it yet. Details in the logs \
                                                         ({why})."
                                                    ));
                                                }
                                            }
                                        }
                                    }

                                    let breez_result =
                                        if let Some(fingerprint) = breez_signer_fingerprint {
                                            breez_liquid::load_breez_client(
                                                datadir_clone.path(),
                                                network_val,
                                                app::seed_source::SeedSource::encrypted_file(
                                                    fingerprint,
                                                    &pin,
                                                ),
                                                &cube.id,
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
                                                app::seed_source::SeedSource::encrypted_file(
                                                    fingerprint,
                                                    &pin,
                                                ),
                                                &cube.id,
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
                                        migration_error,
                                        enc_key_error,
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
                                    migration_error,
                                    enc_key_error,
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
                                        migration_error,
                                        enc_key_error,
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
                    // Drop every in-memory secret before the wipe runs. A duress
                    // wipe destroys the seed on disk; leaving a decrypted copy
                    // resident would undo that for the rest of the process's
                    // life. This matters more than it looks: the session holds
                    // the *decrypted master signer* now, not just a 4-digit PIN.
                    //
                    // No Cube was ever opened on this path, so the only thing
                    // being cleared is whatever a previous unlock left behind.
                    crate::app::session::close();
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
            (State::PasskeyUnlock(unlock), Message::PasskeyUnlock(msg)) => match msg {
                crate::passkey_unlock::Message::Unlocked { fingerprint } => {
                    // The assertion succeeded and `passkey_unlock` has already
                    // parked the derived signer in `app::session`. What follows
                    // is the PIN path's post-unlock work minus everything that
                    // needs a PIN, which for a passkey Cube is everything that
                    // touches a seed file:
                    //
                    // * **No seed-file migration.** A passkey Cube has no seed
                    //   file to bring up to `ENCRYPTED_V3`. `migrate_seed_files`
                    //   would find nothing and needs a PIN it cannot be given.
                    // * **No second (duress) slot.** Duress is triggered by
                    //   entering a duress *PIN*; a Cube with no PIN has no
                    //   trigger, so minting a decoy slot would be theatre.
                    // * **No fingerprint backfill by trial decryption.** The
                    //   fingerprint is not guessed from disk — the assertion
                    //   just derived it. It is written through below for the
                    //   Cubes minted before it was recorded.
                    let crate::pin_entry::PinEntrySuccess::LoadApp {
                        datadir,
                        config,
                        network,
                        wallet_settings,
                        internal_bitcoind,
                        backup,
                    } = &unlock.on_success;

                    let cube = unlock.cube().clone();
                    let config_clone = config.clone();
                    let datadir_clone = datadir.clone();
                    let network_val = *network;
                    let wallet_settings_clone = wallet_settings.clone();
                    let internal_bitcoind_clone = internal_bitcoind.clone();
                    let backup_clone = backup.clone();

                    Task::perform(
                        async move {
                            let mut cube = cube;

                            // The seed the assertion derived, from the session
                            // it was parked in. It is not carried through the
                            // message because iced clones messages freely and
                            // every clone would be another master seed on the
                            // heap.
                            let signer =
                                crate::app::session::unlocked_signer(&cube.id, fingerprint);
                            let Some(signer) = signer else {
                                // The session was closed between the assertion
                                // and here. Send the user back to the unlock
                                // prompt rather than into a Cube that is
                                // supposed to be locked — see
                                // `Message::PasskeyRelocked`.
                                return Message::PasskeyRelocked;
                            };
                            let signer = std::sync::Arc::new(signer);

                            // Record the fingerprint for a passkey Cube minted
                            // before the field existed. Unlike the PIN path
                            // this is not a trial decryption over the mnemonics
                            // folder — the assertion is the authority.
                            if cube.master_signer_fingerprint != Some(fingerprint) {
                                cube.master_signer_fingerprint = Some(fingerprint);
                                let cube_id = cube.id.clone();
                                let network_dir = datadir_clone.network_directory(network_val);
                                if let Err(e) =
                                    app::settings::update_settings_file(&network_dir, |mut s| {
                                        if let Some(c) =
                                            s.cubes.iter_mut().find(|c| c.id == cube_id)
                                        {
                                            c.master_signer_fingerprint = Some(fingerprint);
                                        }
                                        Some(s)
                                    })
                                    .await
                                {
                                    tracing::warn!(
                                        "Failed to persist master_signer_fingerprint for \
                                         passkey cube {}: {}",
                                        cube.id,
                                        e
                                    );
                                }
                            }

                            // Connect blinding (PLAN-connect-blinding D2).
                            //
                            // Derived from the signer this task is already
                            // holding, **not** through
                            // `derive_connect_encryption_pubkey`. That helper
                            // takes a fingerprint and a password and goes
                            // looking for the seed again — session cache first,
                            // seed file second — which is right for the PIN
                            // path and wrong here twice over:
                            //
                            // 1. There is an `.await` above (the fingerprint
                            //    backfill), so its session lookup is a *second*
                            //    one and can miss where the first hit. A
                            //    passkey Cube has no seed file for the fallback
                            //    to read, so a miss became `NoSeed` — logged at
                            //    debug and skipped. The Cube would then never
                            //    register an encryption pubkey, its Contacts
                            //    could never share keys with it, and it would
                            //    sit in the coverage report as a straggler with
                            //    no visible cause.
                            // 2. The password it was handed was `""`, which is
                            //    not a credential for anything.
                            //
                            // `CubeEncryptionKey::derive` is infallible given a
                            // signer, so there is no failure to report and the
                            // `NoSeed` / `Unreadable` arms this used to match on
                            // are gone rather than merely unlikely.
                            if cube.connect_encryption_pubkey.is_none() {
                                let pubkey =
                                    crate::services::connect::crypto::CubeEncryptionKey::derive(
                                        &signer,
                                        network_val,
                                    )
                                    .public_key_hex();
                                cube.connect_encryption_pubkey = Some(pubkey.clone());
                                let cube_id = cube.id.clone();
                                let network_dir = datadir_clone.network_directory(network_val);
                                if let Err(e) =
                                    app::settings::update_settings_file(&network_dir, |mut s| {
                                        if let Some(c) =
                                            s.cubes.iter_mut().find(|c| c.id == cube_id)
                                        {
                                            c.connect_encryption_pubkey = Some(pubkey.clone());
                                        }
                                        Some(s)
                                    })
                                    .await
                                {
                                    tracing::warn!(
                                        "Failed to persist Connect encryption pubkey \
                                         for passkey cube {}: {}",
                                        cube.id,
                                        e
                                    );
                                }
                            }

                            // The whole point of PR A: both loaders take the
                            // signer the assertion produced, with no seed file
                            // anywhere in the picture.
                            let breez_result = breez_liquid::load_breez_client(
                                datadir_clone.path(),
                                network_val,
                                app::seed_source::SeedSource::in_memory(signer.clone()),
                                &cube.id,
                                cube.liquid_granted.unwrap_or(false),
                            )
                            .await;

                            let spark_backend = match app::breez_spark::load_spark_client(
                                datadir_clone.path(),
                                network_val,
                                app::seed_source::SeedSource::in_memory(signer),
                                &cube.id,
                            )
                            .await
                            {
                                Ok(client) => {
                                    Some(Arc::new(app::wallets::SparkBackend::new(client)))
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        "Spark bridge unavailable, continuing without \
                                         Spark: {}",
                                        e
                                    );
                                    None
                                }
                            };

                            Message::BreezClientLoadedAfterPin {
                                breez_client: breez_result,
                                spark_backend,
                                config: config_clone,
                                datadir: datadir_clone,
                                network: network_val,
                                cube,
                                wallet_settings: wallet_settings_clone,
                                internal_bitcoind: internal_bitcoind_clone,
                                backup: backup_clone,
                                // A passkey Cube has no seed file, so there is
                                // no migration that could have failed — and the
                                // encryption key is derived from a signer this
                                // task already holds, which cannot fail either.
                                migration_error: None,
                                enc_key_error: None,
                            }
                        },
                        // The block builds its own message: the relock path
                        // above needs to pick a *different* one, which a
                        // fixed-shape tuple could not express.
                        |msg| msg,
                    )
                }
                crate::passkey_unlock::Message::Back => {
                    // Dropping the screen drops any in-flight ceremony, which
                    // cancels the system prompt.
                    let network = unlock.cube().network;
                    let crate::pin_entry::PinEntrySuccess::LoadApp { datadir, .. } =
                        &unlock.on_success;
                    let (home, command) = Home::new(datadir.clone(), Some(network));
                    self.state = State::Home(home);
                    command.map(Message::Launch)
                }
                m => unlock.update(m).map(Message::PasskeyUnlock),
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
            // Still in `PasskeyUnlock` — nothing mutates the tab state between
            // the assertion and the wallet load — so the screen is reused in
            // place rather than rebuilt. If the user has since navigated away,
            // this falls through to the catch-all and is correctly ignored
            // rather than yanking them back.
            (State::PasskeyUnlock(unlock), Message::PasskeyRelocked) => {
                unlock.relocked();
                Task::none()
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
                    migration_error,
                    enc_key_error,
                },
            ) => {
                // Surfaced through the app's normal toast path once the Cube is
                // up. Deliberately not fatal: the seed files were left exactly
                // as they were, so the Cube opens — but a silent "your files
                // were not upgraded" is what this whole PR exists to stop.
                //
                // Two of the three branches below land in `Loader` or `Login`,
                // which cannot show a toast, so park it and let
                // `flush_migration_warning` deliver it when `App` arrives.
                self.pending_unlock_warnings =
                    migration_error.into_iter().chain(enc_key_error).collect();
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
        // After the transition, not before: this is the point where the arm
        // above has already decided which state the tab is in, so a warning
        // parked on the way to `Loader` is released by whichever later update
        // finally reaches `App`.
        Task::batch([result, self.flush_migration_warning()])
    }

    pub fn subscription(&self) -> Subscription<Message> {
        match &self.state {
            State::Installer(v) => v.subscription().map(Message::Install),
            State::Loader(v) => v.subscription().map(Message::Load),
            State::App(v) => v.subscription().map(Message::Run),
            State::Home(v) => v.subscription().map(Message::Launch),
            State::Login(_) => Subscription::none(),
            State::PinEntry(_) => Subscription::none(),
            // Unlike `PinEntry`, this one needs a subscription: the macOS
            // authorization delegate answers on a channel with no waker to
            // hook, so the ceremony has to be polled.
            State::PasskeyUnlock(v) => v.subscription().map(Message::PasskeyUnlock),
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
            State::PasskeyUnlock(v) => v.view().map(Message::PasskeyUnlock),
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
            State::PasskeyUnlock(_) => {}
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
                // No PIN hash is recorded. The restored Cube's seed file was
                // just written encrypted under `seed.pin`, and decrypting it is
                // what verifies that PIN from now on (I1).
                cube = cube.with_master_signer(seed.master_signer_fingerprint);
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
            connect_transport_key: None,
            cube_encryption_key: None,
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
            node_bitcoind_subversion: None,
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
            recovery_kit_password_backed_up: cube_settings.recovery_kit_password_backed_up,
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
mod migration_warning_tests {
    use super::*;

    /// A state that is not `App` — the shape the warning has to survive.
    ///
    /// The datadir gets its own directory rather than the shared temp root:
    /// `Home::new` only reads, but handing any code a datadir that *is*
    /// `$TMPDIR` is how a later change starts writing into every other test's
    /// scratch space.
    fn non_app_tab() -> Tab {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "coincube-tab-migration-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).expect("create test datadir");
        let (home, _) = Home::new(CoincubeDirectory::new(dir), None);
        Tab::new(1, State::Home(home))
    }

    /// Flushing while the tab is still loading would hand the toast to a state
    /// that drops `Message::Run` on the floor, which is how "your seed files
    /// were not upgraded" went silent for every Cube that routed through
    /// `Loader` or `Login`.
    #[test]
    fn the_warning_is_held_while_no_state_can_show_it() {
        let mut tab = non_app_tab();
        tab.pending_unlock_warnings = vec!["seed files were not upgraded".to_string()];

        let _ = tab.flush_migration_warning();

        assert_eq!(
            tab.pending_unlock_warnings,
            vec!["seed files were not upgraded".to_string()],
            "the warning was consumed by a state that cannot display it"
        );
    }

    /// Nothing pending must not manufacture a toast.
    #[test]
    fn flushing_without_a_warning_keeps_it_empty() {
        let mut tab = non_app_tab();
        let _ = tab.flush_migration_warning();
        assert!(tab.pending_unlock_warnings.is_empty());
    }

    /// Two independent things can fail in one unlock — a seed-file migration
    /// and the Connect encryption-key derivation. Neither may swallow the
    /// other: the queue was widened from a single slot precisely so the
    /// second one cannot go silent.
    #[test]
    fn both_unlock_warnings_are_held_together() {
        let mut tab = non_app_tab();
        tab.pending_unlock_warnings = vec![
            "seed files were not upgraded".to_string(),
            "couldn't prepare its Connect encryption key".to_string(),
        ];

        let _ = tab.flush_migration_warning();

        assert_eq!(
            tab.pending_unlock_warnings.len(),
            2,
            "a non-App state must hold every warning, not just the first"
        );
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
        // No PIN hash is recorded any more — the restored Cube's PIN is
        // whatever its (already-written) seed file decrypts under.
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
        // No `security_pin_hash` to assert on: the restore PIN is proved by
        // the seed file it encrypted, not by a second stored verifier.
    }

    /// Non-restore install with `wallet_id: None` and no originating cube and no
    /// restored identity: this must not error, and — critically — must not
    /// steal an unrelated existing vault-less Cube's identity in a way that
    /// clobbers its credentials. With `restore_seed = None`, `decorate_new` is a
    /// no-op, so the reused empty Cube keeps whatever fingerprint it already had.
    #[tokio::test]
    async fn seed_only_non_restore_does_not_clobber_existing_cube_credentials() {
        let nd = temp_network_dir("seed-only-guard");

        // An existing vault-less Cube that already carries its own credentials.
        let mut settings = app::settings::Settings::default();
        let existing =
            app::settings::CubeSettings::new("Existing".to_string(), bitcoin::Network::Bitcoin)
                .with_master_signer(bitcoin::bip32::Fingerprint::from([1, 2, 3, 4]));
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
        assert_eq!(cube.vault_wallet_id, None);
    }
}

#[cfg(test)]
mod unlock_routing_tests {
    use super::{unlock_state, State};
    use crate::app::settings::{CubeSettings, PasskeyMetadata};
    use coincube_core::miniscript::bitcoin::Network;

    fn on_success() -> crate::pin_entry::PinEntrySuccess {
        crate::pin_entry::PinEntrySuccess::LoadApp {
            datadir: crate::dir::CoincubeDirectory::new(std::path::PathBuf::from("/tmp/unused")),
            config: crate::app::Config::new(false),
            network: Network::Bitcoin,
            internal_bitcoind: None,
            backup: None,
            wallet_settings: None,
        }
    }

    /// The 2026-08-07 regression, stated as a test.
    ///
    /// A passkey Cube must never be shown PIN entry. There is no PIN to accept
    /// and no seed file for one to decrypt, so the screen is a dead end: it
    /// rejects every attempt and offers only Back. This is reachable from the
    /// idle re-lock path, which is how a Cube that opened correctly became
    /// unopenable a minute later.
    #[test]
    fn a_passkey_cube_never_routes_to_pin_entry() {
        let cube = CubeSettings::new("Passkey Cube".to_string(), Network::Bitcoin).with_passkey(
            PasskeyMetadata {
                credential_id: "Y3JlZC1pZA==".to_string(),
                rp_id: "coincube.io".to_string(),
                created_at: 1_786_122_245,
                label: None,
            },
        );
        assert!(
            cube.is_passkey_cube(),
            "test fixture must be a passkey Cube"
        );

        let state = unlock_state(
            cube,
            std::path::PathBuf::from("/tmp/unused"),
            on_success(),
            // Non-`None` on purpose: a duress id must not drag a passkey Cube
            // into the PIN path, which is the only place duress is honoured.
            Some("acct-123".to_string()),
        );

        assert!(
            matches!(state, State::PasskeyUnlock(_)),
            "a passkey Cube must unlock with its passkey, not a PIN keypad"
        );
    }

    /// The other half: the PIN path is untouched by the fix. Without this, a
    /// future "just always use PasskeyUnlock" simplification passes the test
    /// above and breaks every PIN Cube.
    #[test]
    fn a_pin_cube_still_routes_to_pin_entry() {
        let cube = CubeSettings::new("PIN Cube".to_string(), Network::Bitcoin);
        assert!(!cube.is_passkey_cube());

        let state = unlock_state(
            cube,
            std::path::PathBuf::from("/tmp/unused"),
            on_success(),
            None,
        );

        assert!(
            matches!(state, State::PinEntry(_)),
            "a PIN Cube must still get PIN entry"
        );
    }
}
