use coincube_ui::locale::copy as localized_copy;
use std::{collections::HashMap, sync::Arc, time::Instant};

use iced::{Subscription, Task};
use tracing::{error, info};
extern crate serde;
extern crate serde_json;

use coincube_core::miniscript::bitcoin;
use coincube_ui::widget::Element;

use crate::{
    app::{
        self, breez_liquid,
        cache::Cache,
        settings::{update_settings_file, VaultIdentity, WalletSettings},
        wallet::Wallet,
        App,
    },
    chain::ChainIdExt,
    dir::{CoincubeDirectory, NetworkDirectory},
    export::import_backup_at_launch,
    home::{self, Home},
    installer::{self, Installer, UserFlow},
    loader::{self, Loader},
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
    Installer(Box<Installer>),
    Loader(Loader),
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
/// - `imported-local-keys/` — encrypted independent imported roots, public
///   account inventory and operation tombstones,
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
    const CUBE_MATERIAL: &[&str] = &[
        "data",
        "mnemonics",
        "imported-local-keys",
        "settings.json",
        "mcp-pairings",
    ];
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
    StartClaimInstaller,
    /// The copy installer this tab hosts is done: cancelled, or `created` its
    /// Bitcoin Blake2b copy. The pane closes this tab and returns to the
    /// source App (`owner`); after a creation, to its Tools › Split coins.
    CopyInstallFinished {
        owner: app::cache::AppGeneration,
        created: bool,
    },
    /// An App callback is valid only for the App generation that emitted it.
    /// A retained Launcher tab can otherwise reuse this tab ID after Lock.
    AppAsync(u64, Box<Message>),
    /// A verified settings duress marker survives an App generation change.
    /// The proof is one-use and bound to the original data root/Cube.
    LocalDuress(crate::services::unlock::throttle::VerifiedSeedExportDuress),
    ForkAsync(u64, Box<Message>),
    ForkTaskFinished(u64, u64),
    Launch(home::Message),
    Install(installer::Message),
    Load(loader::Message),
    Run(app::Message),
    PinEntry(crate::pin_entry::Message),
    PasskeyUnlock(crate::passkey_unlock::Message),
    /// Frame clock only while a selected Launcher details face turns toward
    /// the existing PIN/passkey screen. It never changes unlock authority.
    UnlockMotionTick(Instant),
    /// Clock for the opening on the Cube: every frame while the Cube moves,
    /// slower while only its edge light breathes. Presentation only.
    OpeningTick(Instant),
    /// Cancel on a stalled opening: back to the Launcher, the Cube locked.
    CancelOpening,
    /// Go on into the Cube while its Spark wallet is still loading.
    OpenWithoutSpark,
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
        connect_client: Option<crate::services::coincube::CoincubeClient>,
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
    GlobalSettingsChanged,
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
    /// Show the Launcher without discarding this App session. The pane owns
    /// the tab switch, so this is bubbled up rather than handled here.
    OpenLauncher,
    OpenAppPreferences(PreferencesSource),
    /// The Launcher dock's network activity source. The GUI saves it only
    /// while the sending tab is still showing the Launcher.
    HomeDevicePreference(crate::app::settings::preferences::PreferenceChange),
    /// Explicitly lock the Cube and return to the Launcher in this tab.
    LockCubeToLauncher,
    /// [`Self::LockCubeToLauncher`], then open the Launcher's delete
    /// confirmation for that Cube (an import abandoned before it finished).
    LockCubeToDelete,
    /// Completion of an authenticated fork installer generation.
    ForkInstallCompleted(u64, installer::Message),
}

/// The screen that emitted a queued App Preferences request. App requests
/// retain their generation so a later Cube session cannot reuse the intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreferencesSource {
    Home,
    App(u64),
}

/// A closed loader can still have queued worker results with transferred
/// resource ownership. Reject the result without ever admitting a new App.
pub(crate) fn discard_closed_loader_result(message: Message) {
    fn stop_embedded(daemon: &Arc<dyn crate::daemon::Daemon + Sync + Send>) {
        if daemon.backend().is_embedded() {
            if let Err(error) = tokio::runtime::Handle::current().block_on(daemon.stop()) {
                tracing::error!("Closed loader daemon stop failed: {error}");
            }
        }
    }

    match message {
        Message::AppAsync(_, nested) | Message::ForkAsync(_, nested) => {
            discard_closed_loader_result(*nested);
        }
        Message::Load(loader::Message::Started(Ok((daemon, pending, _)))) => {
            stop_embedded(&daemon);
            // PendingForkNode owns its node until admitted by a live Loader.
            drop(pending);
        }
        Message::Load(loader::Message::Synced(Ok((_, _, daemon, node, _, _))))
        | Message::Load(loader::Message::App(Ok((_, _, _, daemon, _, node)))) => {
            stop_embedded(&daemon);
            if let Some(node) = node {
                node.stop();
            }
        }
        Message::Load(loader::Message::BreezLoaded {
            daemon, bitcoind, ..
        }) => {
            stop_embedded(&daemon);
            if let Some(node) = bitcoind {
                node.stop();
            }
        }
        Message::Load(loader::Message::Loaded(Ok((daemon, _)))) => {
            stop_embedded(&daemon);
        }
        Message::BreezClientLoadedAfterPin {
            internal_bitcoind: Some(node),
            ..
        } => node.stop(),
        _ => {}
    }
}

#[cfg(test)]
mod admitted_app_startup_tests {
    use super::*;
    use iced_runtime::futures::futures::StreamExt;

    fn recovery_route(message: &Message) -> bool {
        matches!(message,
            Message::AppAsync(_, nested)
                if matches!(nested.as_ref(), Message::Run(app::Message::View(
                    app::view::Message::Menu(app::menu::Menu::Cube(
                        app::menu::CubeSubMenu::Settings(app::menu::CubeSettingsOption::Recovery)
                    ))
                )))
        )
    }

    async fn held_recovery_route(task: Task<Message>) -> Message {
        let mut stream = iced_runtime::task::into_stream(task).expect("admitted App startup work");
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while let Some(action) = stream.next().await {
                if let iced_runtime::Action::Output(message) = action {
                    if recovery_route(&message) {
                        return message;
                    }
                }
            }
            panic!("admitted App startup lost its Recovery route");
        })
        .await
        .expect("Recovery route completed within the fixture bound")
    }

    #[test]
    fn newly_admitted_app_startup_and_recovery_route_reject_stale_generation() {
        std::thread::Builder::new()
            .name("admitted-app-startup".into())
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let _guard = crate::app::session::test_guard();
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let _enter = runtime.enter();
                let path = std::env::temp_dir()
                    .join(format!("admitted-app-startup-{}", uuid::Uuid::new_v4()));
                let root = CoincubeDirectory::new(path.clone());
                let (fixture, wallet) = crate::app::shell_tests::bitcoin_app(&path);
                let settings = fixture.cube_settings().clone();
                let cache = fixture.cache().clone();
                let daemon = crate::app::shell_tests::fixture_daemon(&fixture);
                drop(fixture);
                let breez = Arc::new(app::breez_liquid::BreezClient::disconnected(
                    settings.network.bitcoin_network(),
                ));
                let make_loader = || {
                    let (loader, startup) = Loader::new(
                        root.clone(),
                        app::Config::new(false),
                        settings.network.bitcoin_network(),
                        None,
                        None,
                        None,
                        settings.clone(),
                        Some(breez.clone()),
                        None,
                    );
                    drop(startup);
                    loader
                };
                let admit = |tab: &mut Tab| {
                    tab.pending_recovery_route = Some((
                        root.clone(),
                        settings.network,
                        settings.id.clone(),
                    ));
                    tab.update(Message::Load(loader::Message::BreezLoaded {
                        breez: breez.clone(),
                        spark_backend: None,
                        cache: cache.clone(),
                        wallet: wallet.clone(),
                        config: app::Config::new(false),
                        daemon: daemon.clone(),
                        datadir: root.clone(),
                        bitcoind: None,
                        cube_settings: settings.clone(),
                    }))
                };

                let mut tab = Tab::new(1, State::Loader(make_loader()));
                let first = admit(&mut tab);
                assert!(matches!(tab.state, State::App(_)));
                let first_generation = tab.app_generation_token().unwrap();
                let stale = runtime.block_on(held_recovery_route(first));
                assert!(matches!(stale, Message::AppAsync(generation, _) if generation == first_generation));

                drop(tab.update(Message::LockCubeToLauncher));
                assert!(matches!(tab.state, State::Home(_)));
                tab.state = State::Loader(make_loader());
                let second = admit(&mut tab);
                let second_generation = tab.app_generation_token().unwrap();
                assert_ne!(first_generation, second_generation);
                let current = runtime.block_on(held_recovery_route(second));
                assert!(matches!(tab.state, State::App(_)));
                assert_eq!(tab.update(stale).units(), 0);
                let State::App(app) = &tab.state else { unreachable!() };
                assert!(!matches!(app.current_menu_for_test(),
                    app::menu::Menu::Cube(app::menu::CubeSubMenu::Settings(
                        app::menu::CubeSettingsOption::Recovery
                    ))));
                drop(tab.update(current));
                let State::App(app) = &tab.state else { unreachable!() };
                assert!(matches!(app.current_menu_for_test(),
                    app::menu::Menu::Cube(app::menu::CubeSubMenu::Settings(
                        app::menu::CubeSettingsOption::Recovery
                    ))));
                drop(tab);
                std::fs::remove_dir_all(path).ok();
            })
            .unwrap()
            .join()
            .unwrap();
    }
}

/// Resolve the Vault from its authoritative chain, never its address encoding.
fn vault_settings_for_cube(
    root: &CoincubeDirectory,
    chain: crate::chain::ChainId,
    cube: &app::settings::CubeSettings,
) -> Option<WalletSettings> {
    if cube.network != chain {
        return None;
    }
    let vault_id = cube.vault_wallet_id.as_ref()?;
    app::settings::Settings::from_file(&root.network_directory(chain))
        .ok()?
        .wallets
        .into_iter()
        .find(|wallet| wallet.wallet_id() == *vault_id)
}

/// What the unlock's seed-file step did, once it has finished.
struct UnlockMigration {
    outcome: crate::services::unlock::MigrationOutcome,
    /// The second slot this unlock wrote, already recorded in `settings.json`.
    new_slot: Option<String>,
}

/// Told to the user once migration has turned duress off on this device. A
/// log line is not telling the user: they believe a PIN still wipes this
/// device, and it no longer does.
const DURESS_DISARM_NOTICE: &str = "Upgrading this Cube's files turned duress mode off — your \
     duress PIN no longer erases this device. Turn duress mode on again to set it up.";

/// The duress-off warning, if a migration disarmed this device and the user
/// has not seen it yet.
fn duress_disarm_notice(root: &std::path::Path) -> Option<String> {
    crate::services::duress::DuressLocalState::load(root)
        .ok()
        .filter(|st| st.migration_disarm_notice)
        .map(|_| DURESS_DISARM_NOTICE.to_string())
}

/// The duress-off warning is on screen: stop repeating it.
fn acknowledge_duress_disarm_notice(root: &std::path::Path) {
    if let Ok(mut st) = crate::services::duress::DuressLocalState::load(root) {
        if st.migration_disarm_notice {
            st.migration_disarm_notice = false;
            if let Err(e) = st.save(root) {
                error!("could not clear the duress-off notice: {e}");
            }
        }
    }
}

type UnlockMigrationResult =
    Result<Result<UnlockMigration, crate::services::unlock::UnlockError>, tokio::task::JoinError>;

/// Bring the Cube's seed files up to the current wire version, give it its
/// second slot if it has none, and make the device's settings and duress
/// state agree with what was written.
///
/// Each file write and the record that makes it reachable are one step, so
/// the work runs in a task of its own, spawned at the call. Cancel may drop
/// the returned future mid-way: a decoy written without its name recorded is
/// unreachable, and the next unlock would mint another; a marker replaced
/// without the disarm leaves an enrolment that nothing triggers. The spawned
/// task runs to the end regardless.
fn migrate_seed_files_on_unlock(
    root: std::path::PathBuf,
    network_dir: NetworkDirectory,
    cube: app::settings::CubeSettings,
    pin: zeroize::Zeroizing<String>,
) -> impl std::future::Future<Output = UnlockMigrationResult> {
    let task = tokio::spawn(migrate_seed_files_on_unlock_now(
        root,
        network_dir,
        cube,
        pin,
    ));
    async move { task.await.unwrap_or_else(Err) }
}

async fn migrate_seed_files_on_unlock_now(
    root: std::path::PathBuf,
    network_dir: NetworkDirectory,
    cube: app::settings::CubeSettings,
    pin: zeroize::Zeroizing<String>,
) -> UnlockMigrationResult {
    let cube_id = cube.id.clone();
    let blocking_root = root.clone();
    let joined = tokio::task::spawn_blocking(move || {
        let loc = crate::services::unlock::CubeLocation::new(&blocking_root, &cube);
        let outcome = crate::services::unlock::migrate_seed_files(&loc, &pin)?;
        // Give this Cube its second slot if it arrived without one — restored
        // from a Recovery Kit, or minted before unit 6b. Same blocking task:
        // it costs one Argon2 pass and must not run on the UI thread.
        let slot = crate::services::unlock::ensure_second_slot(&loc)?;
        Ok::<_, crate::services::unlock::UnlockError>((outcome, slot))
    })
    .await;
    let (outcome, new_slot) = match joined {
        Ok(Ok(done)) => done,
        Ok(Err(e)) => return Ok(Err(e)),
        Err(e) => return Err(e),
    };
    // Persist the backfilled slot name. Without this the decoy just written
    // is unreachable and the next unlock mints another one.
    if let Some(name) = new_slot.clone() {
        if let Err(e) = app::settings::update_settings_file(&network_dir, |mut s| {
            if let Some(c) = s.cubes.iter_mut().find(|c| c.id == cube_id) {
                c.duress_slot_file = Some(name);
            }
            Some(s)
        })
        .await
        {
            error!("could not record the second slot for cube {cube_id}: {e}");
        }
    }
    // Migration cannot re-seal a duress marker — that needs the duress PIN,
    // and this path holds the regular one — so it replaces the slot with a
    // decoy and the enrolment is gone. Make the device's own state agree
    // rather than keep claiming an enrolment whose trigger no longer exists.
    //
    // Acceptable only because duress is feature-gated and unreleased.
    // Revisit before it ships.
    if outcome.duress_was_cleared() {
        log::warn!(
            "migration: duress enrolment on cube {cube_id} was cleared — its marker could not \
             be carried across the upgrade. Re-enroll to arm it again."
        );
        if let Ok(mut st) = crate::services::duress::DuressLocalState::load(&root) {
            // Only here, inside the enrolled check: the slot is rewritten
            // whenever its wire version is stale, so `duress_was_cleared` is
            // true for most Cubes and says nothing on its own about whether
            // there was an enrolment to lose. This branch is where one
            // demonstrably was.
            if st.enrolled || st.arming {
                st.disarm();
                // Saved with the disarm, so the warning survives a Cancel
                // that drops the unlock before it can show one.
                st.migration_disarm_notice = true;
                let _ = st.save(&root);
            }
        }
    }
    Ok(Ok(UnlockMigration { outcome, new_slot }))
}

/// Resolve either local Bitcoin wallet attached to this exact Cube. Imported
/// wallets keep the same WalletSettings/daemon directory lifecycle as Vaults,
/// while their descriptor and signing capabilities remain distinct.
fn bitcoin_wallet_settings_for_cube(
    root: &CoincubeDirectory,
    chain: crate::chain::ChainId,
    cube: &app::settings::CubeSettings,
) -> Option<WalletSettings> {
    if cube.network != chain {
        return None;
    }
    let Some(import) = &cube.bitcoin_import else {
        return vault_settings_for_cube(root, chain, cube);
    };
    if cube.vault_wallet_id.is_some() {
        return None;
    }
    let attachment = import.active_attachment()?;
    attachment.validate(chain).ok()?;
    app::settings::Settings::from_file(&root.network_directory(chain))
        .ok()?
        .wallets
        .into_iter()
        .find(|wallet| {
            wallet.wallet_id() == attachment.wallet_id && wallet.remote_backend_auth.is_none()
        })
}

/// A Connect authentication change `GUI::update` broadcasts to every tab.
#[derive(Clone)]
pub enum AuthChange {
    /// A log-out from any tab.
    LogOut,
    /// A login or refresh result (`SetSession`) from any tab, for this
    /// Connect user.
    SignIn {
        user_id: u32,
        token: zeroize::Zeroizing<String>,
    },
}

impl std::fmt::Debug for AuthChange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LogOut => f.write_str("LogOut"),
            Self::SignIn { user_id, .. } => f
                .debug_struct("SignIn")
                .field("user_id", user_id)
                .field("token", &"<redacted>")
                .finish(),
        }
    }
}

/// The opening on the Cube, from a verified unlock to the App.
struct CubeOpening {
    scene: home::launcher_opening::Opening,
    /// Starts with the unlock screen's quote, then cycles through others.
    quotes: home::launcher_opening::QuoteCycle,
    /// The unlock's wallet loads, aborted by Cancel before the Loader exists.
    loads: Vec<iced::task::Handle>,
    /// The user chose to go on while Spark was still loading.
    without_spark: bool,
}

pub struct Tab {
    pub id: usize,
    pub state: State,
    /// Persisted theme mode — carried across state transitions so new App
    /// caches inherit the correct mode immediately.
    pub theme_mode: coincube_ui::theme::palette::ThemeMode,
    /// The effective reduce-motion preference, carried into new App caches.
    pub reduce_motion: bool,
    /// Connect, not the user, signed this computer out: the GUI relays it
    /// to every tab, and new App caches inherit it (L3).
    pub connect_signed_out_by: Option<crate::app::SignedOutBy>,
    pub balance_privacy: Option<crate::app::state::preferences::BalancePrivacyChrome>,
    unlock_motion: Option<Instant>,
    /// Safe, owned Launcher geometry retained only through this unlock.
    unlock_floor: Option<home::LauncherSceneContext>,
    /// The system's reduce-motion setting, as last observed.
    system_reduces_motion: Option<bool>,
    /// The verified Cube opening, until the App shows.
    opening: Option<CubeOpening>,
    /// An opening was cancelled before the Loader: its wallet load may still
    /// answer, and must not open the Cube.
    opening_cancelled: bool,
    /// A seed-migration warning waiting for a surface to show it on.
    ///
    /// Held rather than dispatched because only `State::App` handles
    /// `Message::Run`; see [`Tab::flush_migration_warning`].
    /// Non-fatal warnings raised during unlock, parked until an `App` exists
    /// to show them. `Loader` and `Login` cannot toast, so anything raised
    /// before the Cube is up waits here.
    pending_unlock_warnings: Vec<String>,
    /// The datadir whose duress-off notice is among
    /// `pending_unlock_warnings`; cleared on disk once `App` shows it.
    pending_duress_notice: Option<std::path::PathBuf>,
    app_generation: u64,
    seed_export_origins: Vec<(
        crate::app::cache::AppGeneration,
        std::path::PathBuf,
        String,
        crate::chain::ChainId,
    )>,
    pending_seed_export_trials: usize,
    closed_seed_export_origin: Option<usize>,
    /// The source App that launched this tab's Bitcoin Blake2b copy
    /// installer. The pane keeps that App open in its own tab; cancelling
    /// returns to it and closing it closes this installer. Cleared once the
    /// copy is saved, so the copy then opens as an ordinary Cube.
    copy_owner: Option<app::cache::AppGeneration>,
    fork_session_generation: u64,
    fork_save_task: Option<iced::task::Handle>,
    fork_tasks: HashMap<u64, iced::task::Handle>,
    next_fork_task: u64,
    /// An Account Recovery row may choose a destination before this Cube is
    /// unlocked. It applies only to the exact Cube that completes admission.
    pending_recovery_route: Option<(CoincubeDirectory, crate::chain::ChainId, String)>,
}

impl Tab {
    /// Exact on-disk Cube identity for an unlocked or still-loading owner.
    /// An installer without a known Cube ID is deliberately not matched.
    pub(crate) fn matches_cube_identity(
        &self,
        root: &CoincubeDirectory,
        chain: crate::chain::ChainId,
        cube_id: &str,
    ) -> bool {
        let same = |path: &CoincubeDirectory, settings: &app::settings::CubeSettings| {
            crate::chain::same_path(root.path(), path.path())
                && settings.network == chain
                && settings.id == cube_id
        };
        match &self.state {
            State::App(app) => same(app.datadir(), app.cube_settings()),
            State::Loader(loader) => same(&loader.datadir_path, &loader.cube_settings),
            State::Installer(installer) => {
                installer
                    .cube_settings
                    .as_ref()
                    .is_some_and(|settings| same(&installer.datadir, settings))
                    || installer
                        .source_cube
                        .as_ref()
                        .is_some_and(|source| same(&installer.datadir, &source.settings))
                    || (crate::chain::same_path(root.path(), installer.datadir.path())
                        && installer.context.bitcoin_config.chain == chain
                        && installer.context.cube_id.as_deref() == Some(cube_id))
            }
            _ => false,
        }
    }
    pub fn new(id: usize, state: State) -> Self {
        Tab {
            id,
            state,
            theme_mode: coincube_ui::theme::palette::ThemeMode::default(),
            reduce_motion: false,
            connect_signed_out_by: None,
            balance_privacy: None,
            unlock_motion: None,
            unlock_floor: None,
            system_reduces_motion: None,
            opening: None,
            opening_cancelled: false,
            pending_unlock_warnings: Vec::new(),
            pending_duress_notice: None,
            app_generation: 0,
            seed_export_origins: Vec::new(),
            pending_seed_export_trials: 0,
            closed_seed_export_origin: None,
            copy_owner: None,
            fork_session_generation: 0,
            fork_save_task: None,
            fork_tasks: HashMap::new(),
            next_fork_task: 0,
            pending_recovery_route: None,
        }
    }

    pub(crate) fn set_copy_owner(&mut self, owner: app::cache::AppGeneration) {
        self.copy_owner = Some(owner);
    }

    pub(crate) fn copy_owner(&self) -> Option<app::cache::AppGeneration> {
        self.copy_owner
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

    /// The effective reduce-motion preference for an open Cube's chrome (the
    /// Connect light's pulse).
    pub fn set_reduce_motion(&mut self, reduce: bool) {
        self.reduce_motion = reduce;
        match &mut self.state {
            State::App(app) => app.cache_mut().reduce_motion = reduce,
            State::Home(home) => home.reduce_motion = reduce,
            _ => {}
        }
    }

    /// Show or clear the signed-out-by-Connect notice in this tab.
    pub fn set_connect_signed_out_by(&mut self, reason: Option<crate::app::SignedOutBy>) {
        self.connect_signed_out_by = reason;
        match &mut self.state {
            State::App(app) => app.set_connect_signed_out_by(reason),
            State::Home(home) => home.connect_account.signed_out_by = reason,
            _ => {}
        }
    }

    /// Feed the current native motion observation to a retained Launcher.
    pub fn set_system_reduces_motion(&mut self, observed: Option<bool>) {
        self.system_reduces_motion = observed;
        if let State::Home(home) = &mut self.state {
            home.set_system_reduces_motion(observed);
        }
    }

    /// Apply the tab's stored privacy display to the current state.
    pub fn set_balance_privacy(
        &mut self,
        chrome: Option<crate::app::state::preferences::BalancePrivacyChrome>,
    ) {
        self.balance_privacy = chrome;
        if let State::App(app) = &mut self.state {
            app.cache_mut().balance_privacy = chrome;
        }
    }

    /// Apply the tab's stored theme_mode (and balance-privacy display) to the
    /// current state. Call after any state transition to State::App or State::Home.
    fn sync_theme_mode(&mut self) {
        let mode = self.theme_mode;
        match &mut self.state {
            State::App(app) => {
                app.cache_mut().theme_mode = mode;
                app.cache_mut().reduce_motion = self.reduce_motion;
                app.cache_mut().balance_privacy = self.balance_privacy;
                app.set_connect_signed_out_by(self.connect_signed_out_by);
            }
            State::Home(home) => {
                home.theme_mode = mode;
                home.reduce_motion = self.reduce_motion;
                home.connect_account.signed_out_by = self.connect_signed_out_by;
            }
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
            State::Home(_) => localized_copy("Home"),
            State::PinEntry(_) => localized_copy("Enter PIN"),
            State::PasskeyUnlock(_) => localized_copy("Unlock with passkey"),
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

    /// Admit the proposal or discard the rejected builder and show a locked launcher.
    #[cfg(feature = "mcp-proposals")]
    fn open_mcp_proposal_handoff(
        &mut self,
        mut install: Installer,
        settings: app::local_mcp::Settings,
        proposal: installer::proposal::Session,
    ) -> Task<Message> {
        match install.use_mcp_proposal(settings, proposal) {
            Ok(task) => {
                self.state = State::Installer(Box::new(install));
                task.map(Message::Install)
            }
            Err(error) => {
                let root = install.datadir.clone();
                let chain = install.context.bitcoin_config.chain;
                install.stop();
                drop(install);
                let (home, task) = Home::after_native_refusal(root, chain, error);
                self.state = State::Home(home);
                task.map(Message::Launch)
            }
        }
    }

    /// Emit the pending migration warning once the tab is actually in `App`.
    ///
    /// Open a Bitcoin Blake2b Cube that has no Vault yet. It lands where a
    /// Bitcoin Cube without a Vault lands, with Create Vault next, and never
    /// starts Liquid or Spark. The caller has already opened the session; a
    /// refusal closes it and returns the reason for the unlock screen.
    fn open_fork_without_vault(
        &mut self,
        cek: Arc<crate::services::connect::crypto::CubeEncryptionKey>,
        client: Option<crate::services::coincube::CoincubeClient>,
        config: app::config::Config,
        datadir: CoincubeDirectory,
        cube: app::settings::CubeSettings,
    ) -> Result<Task<Message>, String> {
        let cube_id = cube.id.clone();
        match App::new_fork_without_wallet(cek, client, config, datadir, cube) {
            Ok((app, command)) => {
                self.state = State::App(app);
                Ok(command.map(Message::Run))
            }
            Err(error) => {
                app::session::close_cube(&cube_id);
                Err(crate::user_error::report(&error))
            }
        }
    }

    /// Open the Vault a Bitcoin Blake2b Cube just created from inside itself,
    /// through the fork loader the unlock screen would use, without asking
    /// for the PIN again. `None` when the session no longer holds this Cube's
    /// signer, or its backend needs a Connect session that is gone: the
    /// caller then falls back to the Cube's own unlock screen.
    fn open_fork_vault_in_session(
        &mut self,
        datadir: CoincubeDirectory,
        config: app::config::Config,
        cube: app::settings::CubeSettings,
        wallet_settings: Option<app::settings::WalletSettings>,
        client: Option<crate::services::coincube::CoincubeClient>,
    ) -> Option<Task<Message>> {
        let signer = app::session::unlocked_signer(&cube.id, cube.master_signer_fingerprint?)?;
        let wallet_settings = wallet_settings.filter(|w| w.remote_backend_auth.is_none())?;
        let requires_connect =
            !crate::chain::has_managed_local_fork(&datadir, cube.network, Some(&wallet_settings));
        let client = client.filter(|c| c.token().is_some());
        if requires_connect && client.is_none() {
            return None;
        }
        let cek = Arc::new(crate::services::connect::crypto::CubeEncryptionKey::derive(
            &signer,
            cube.network.bitcoin_network(),
        ));
        let (loader, command) = match client.filter(|_| requires_connect) {
            Some(client) => {
                Loader::new_for_chain(datadir, config, wallet_settings, cube, client, cek)
            }
            None => Loader::new_local_fork(datadir, config, wallet_settings, cube, cek),
        };
        self.state = State::Loader(loader);
        Some(command.map(Message::Load))
    }

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
        if let Some(root) = self.pending_duress_notice.take() {
            acknowledge_duress_disarm_notice(&root);
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

    pub(crate) fn reload_global_settings(&mut self, root: &CoincubeDirectory) -> Task<Message> {
        let enabled = crate::app::settings::global::GlobalSettings::load_bitcoin_blake2b_beta(
            &crate::app::settings::global::GlobalSettings::path(root),
        );
        let fork = match &self.state {
            State::App(app) => app.cube_settings().network.is_blake2b(),
            State::Loader(loader) => loader.cube_settings.network.is_blake2b(),
            State::PinEntry(pin) => pin.cube().network.is_blake2b(),
            State::PasskeyUnlock(unlock) => unlock.cube().network.is_blake2b(),
            State::Installer(installer) => installer.context.bitcoin_config.chain.is_blake2b(),
            _ => false,
        };
        let revoke = if fork && !enabled {
            self.invalidate_fork_state(AuthChange::LogOut, true, true)
        } else {
            Task::none()
        };
        let refresh = match &mut self.state {
            State::Home(home) => home.reload_global_settings().map(Message::Launch),
            State::App(app) => app.reload_global_settings().map(Message::Run),
            _ => Task::none(),
        };
        Task::batch([revoke, refresh])
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

        let task = match &mut self.state {
            State::App(app) => {
                if idle_expired {
                    return if app.cube_settings().network.is_blake2b() {
                        self.guard_fork_task(Task::done(Message::LockCube))
                    } else {
                        Task::done(Message::AppAsync(
                            self.app_generation,
                            Box::new(Message::LockCube),
                        ))
                    };
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
            #[cfg(feature = "mcp-proposals")]
            State::Installer(installer) if installer.context.mcp_proposal.is_some() => {
                if idle_expired
                    || installer
                        .context
                        .mcp_proposal
                        .as_ref()
                        .is_some_and(|p| !p.unlocked())
                {
                    installer.expire_mcp_proposal();
                    if !installer.proposal_install_dispatched || installer.proposal_finished {
                        let root = installer.datadir.clone();
                        let chain = installer.context.bitcoin_config.chain;
                        let failure = installer.proposal_failure.clone();
                        installer.stop();
                        let (mut home, task) = Home::new_for_chain(root, Some(chain));
                        home.set_error(failure.unwrap_or_else(|| "Proposal setup locked. Reopen the Cube and approve a new client to continue.".into()));
                        self.state = State::Home(home);
                        return task.map(Message::Launch);
                    }
                }
                Task::none()
            }
            State::Loader(_) => {
                if idle_expired {
                    tracing::info!("idle: dropping session secrets held during load / sign-in");
                    crate::app::session::close();
                }
                Task::none()
            }
            _ => Task::none(),
        };
        if matches!(&self.state, State::App(app) if app.cube_settings().network.is_blake2b()) {
            self.guard_fork_task(task)
        } else {
            if matches!(self.state, State::App(_)) {
                let generation = self.app_generation;
                task.map(move |message| Message::AppAsync(generation, Box::new(message)))
            } else {
                task
            }
        }
    }

    /// The global Connect auth boundary, broadcast by `GUI::update` to every
    /// tab for a log-out or a sign-in (`SetSession`) from any tab. The fork
    /// arms tear down or refuse regardless of the kind; the Bitcoin App arm
    /// decides from it — hold on a log-out, revoke for a re-read on a sibling
    /// same-account sign-in, hold on another account — and leaves its own
    /// sign-in to its own hook (`originated`).
    pub fn invalidate_fork_session(
        &mut self,
        change: AuthChange,
        originated: bool,
    ) -> Task<Message> {
        self.invalidate_fork_state(change, originated, false)
    }

    fn invalidate_fork_state(
        &mut self,
        change: AuthChange,
        originated: bool,
        capability_revoked: bool,
    ) -> Task<Message> {
        if !capability_revoked {
            match &mut self.state {
                State::App(app)
                    if app.cube_settings().network.is_blake2b() && !app.requires_connect() =>
                {
                    app.on_local_auth_change(originated);
                    return Task::none();
                }
                State::Loader(loader)
                    if loader.cube_settings.network.is_blake2b() && !loader.requires_connect() =>
                {
                    loader.connect_client = None;
                    return Task::none();
                }
                State::PinEntry(pin)
                    if pin.cube().network.is_blake2b() && !pin.requires_connect() =>
                {
                    let crate::pin_entry::PinEntrySuccess::LoadApp { connect_client, .. } =
                        &mut pin.on_success;
                    *connect_client = None;
                    return Task::none();
                }
                State::PasskeyUnlock(unlock)
                    if unlock.cube().network.is_blake2b() && !unlock.requires_connect() =>
                {
                    let crate::pin_entry::PinEntrySuccess::LoadApp { connect_client, .. } =
                        &mut unlock.on_success;
                    *connect_client = None;
                    return Task::none();
                }
                State::Installer(installer)
                    if installer.context.fork_local_vault_allowed()
                        && !installer.context.use_coincube_connect =>
                {
                    installer.context.coincube_client = None;
                    installer.context.connect_jwt = None;
                    return Task::none();
                }
                _ => {}
            }
        }
        self.fork_session_generation = self.fork_session_generation.wrapping_add(1);
        self.fork_save_task.take();
        self.fork_tasks.clear();
        let mut command = Task::none();
        let mut replacement = None;
        match &mut self.state {
            State::App(app) if app.cube_settings().network.is_blake2b() => {
                let cube = app.cube_settings().clone();
                let datadir = app.datadir().clone();
                app.invalidate_fork_session();
                app::session::close_cube(&cube.id);
                // Dropping the fork App drops signing/export panels and its
                // wallet. EmbeddedDaemon Drop uses fork-scoped safe cleanup;
                // App::stop would also stop unrelated globally managed Tor.
                let (mut home, startup) = Home::new_for_chain(datadir, Some(cube.network));
                let adopted = match &change {
                    AuthChange::SignIn { token, .. } if !capability_revoked && !originated => {
                        home.adopt_connect_session(token)
                    }
                    _ => Task::none(),
                };
                home.set_error(if capability_revoked {
                    "Bitcoin Blake2b is disabled or its availability could not be verified. Check Global Settings before reopening this Cube."
                } else { "Connect session changed. Reopen this Cube after the account check finishes." });
                command = Task::batch([startup, adopted]).map(Message::Launch);
                replacement = Some(State::Home(home));
            }
            // A Bitcoin App stays open across a Connect change — its wallet
            // does not depend on the session — but a claim in flight does.
            // The App decides from the kind of change and who made it: see
            // `App::on_global_auth_change`.
            State::App(app) => app.on_global_auth_change(change.clone(), originated),
            State::Home(home) => match &change {
                AuthChange::SignIn { token, .. } if !originated => {
                    command = home.adopt_connect_session(token).map(Message::Launch);
                }
                AuthChange::LogOut if !originated => {
                    command = home.on_global_connect_logout().map(Message::Launch);
                }
                AuthChange::LogOut => {
                    home.connect_account.invalidate_auth();
                }
                _ => {}
            },
            State::Loader(loader) if loader.cube_settings.network.is_blake2b() => {
                loader.invalidate_fork_session();
                if capability_revoked {
                    loader.fail(loader::Error::Unexpected("Bitcoin Blake2b is disabled. Check Global Settings before reopening this Cube.".into()));
                }
            }
            State::PinEntry(pin) if pin.cube().network.is_blake2b() => {
                let crate::pin_entry::PinEntrySuccess::LoadApp { connect_client, .. } =
                    &mut pin.on_success;
                *connect_client = None;
                pin.fail_open(if capability_revoked { "Bitcoin Blake2b is disabled. Check Global Settings before reopening this Cube." } else { "Connect session changed. Return Home and sign in again." }.into());
            }
            // The same for a passkey copy: its assertion must not open the
            // copy with the previous account's client.
            State::PasskeyUnlock(unlock) if unlock.cube().network.is_blake2b() => {
                let crate::pin_entry::PinEntrySuccess::LoadApp { connect_client, .. } =
                    &mut unlock.on_success;
                *connect_client = None;
                unlock.fail_open(if capability_revoked { "Bitcoin Blake2b is disabled. Check Global Settings before reopening this Cube." } else { "Connect session changed. Return Home and sign in again." }.into());
            }
            State::Installer(installer) if installer.context.bitcoin_config.chain.is_blake2b() => {
                installer.stop();
                // A Vault being built inside an open Cube leaves that Cube
                // here: its session must not outlive it on Home.
                if let Some(cube) = &installer.cube_settings {
                    app::session::close_cube(&cube.id);
                }
                let (mut home, startup) = Home::new_for_chain(
                    installer.datadir.clone(),
                    Some(installer.context.bitcoin_config.chain),
                );
                home.set_error(
                    if capability_revoked { "Bitcoin Blake2b is disabled. Check Global Settings before restarting this installation." } else { "Connect session changed; restart this Cube's installation after signing in" },
                );
                command = startup.map(Message::Launch);
                replacement = Some(State::Home(home));
            }
            _ => {}
        }
        if let Some(state) = replacement {
            if matches!(state, State::Home(_)) {
                self.copy_owner = None;
            }
            self.state = state;
        }
        command
    }

    pub(crate) fn accepts_fork_generation(&self, generation: u64) -> bool {
        generation == self.fork_session_generation
    }

    pub(crate) fn accepts_app_generation(&self, generation: u64) -> bool {
        generation == self.app_generation && matches!(self.state, State::App(_))
    }

    pub(crate) fn app_generation_token(&self) -> Option<u64> {
        matches!(self.state, State::App(_)).then_some(self.app_generation)
    }

    pub(crate) fn pending_seed_export_trials(&self) -> usize {
        self.pending_seed_export_trials
    }

    pub(crate) fn finish_stale_seed_export_trial(&mut self) {
        self.pending_seed_export_trials = self.pending_seed_export_trials.saturating_sub(1);
    }

    pub(crate) fn hold_closed_seed_export_trials(&mut self, origin: usize, count: usize) {
        self.closed_seed_export_origin = Some(origin);
        self.pending_seed_export_trials = count;
    }

    pub(crate) fn finish_closed_seed_export_trial(&mut self, origin: usize) -> Option<bool> {
        if self.closed_seed_export_origin != Some(origin) || self.pending_seed_export_trials == 0 {
            return None;
        }
        self.pending_seed_export_trials -= 1;
        Some(self.pending_seed_export_trials == 0)
    }

    #[cfg(test)]
    pub(crate) fn mark_seed_export_trial_pending_for_test(&mut self) {
        self.pending_seed_export_trials += 1;
    }

    pub(crate) fn preferences_source(&self) -> Option<PreferencesSource> {
        match &self.state {
            State::Home(_) => Some(PreferencesSource::Home),
            State::App(_) => Some(PreferencesSource::App(self.app_generation)),
            _ => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn reviewer_fork_session_generation(&self) -> u64 {
        self.fork_session_generation
    }

    /// The sole emission boundary for App work started outside Tab::update.
    /// GUI/Pane may ask the current App to produce a Task, but its eventual
    /// result must re-enter through the generation checked by GUI::update.
    /// Synchronous user events still enter Tab::update directly.
    pub(crate) fn emit_app_task(&self, task: Task<app::Message>) -> Task<Message> {
        let Some(generation) = self.app_generation_token() else {
            drop(task);
            return Task::none();
        };
        task.map(move |message| Message::AppAsync(generation, Box::new(Message::Run(message))))
    }

    /// Pure admission checks shared by GUI's teardown barrier and this Tab's
    /// Home transition. A refused destination must leave the current public
    /// Cube and its process session untouched.
    pub(crate) fn validate_home_exit(&self, message: &home::Message) -> Result<(), String> {
        let State::Home(home) = &self.state else {
            return Err("The Launcher destination is no longer open.".into());
        };
        if !home.accepts_launcher_message(message) {
            return Err("Acknowledge the native refusal before opening another Cube.".into());
        }
        match message {
            home::Message::Run(root, _, chain, cube)
            | home::Message::RunFromFloor(root, _, chain, cube, _)
            | home::Message::RunToRecovery(root, _, chain, cube) => {
                if let home::Message::RunFromFloor(_, _, _, _, generation) = message {
                    if !home.floor_open_matches_queued(*generation, cube) {
                        return Err("This Cube changed while opening. Select it again to use the current details.".into());
                    }
                }
                if !crate::chain::same_path(root.path(), home.datadir_path.path()) {
                    return Err("The selected Cube belongs to another data directory.".into());
                }
                if cube.network != *chain {
                    return Err(format!(
                        "This Cube's settings say it lives on {}, but it was found in the {} folder. Tenshu won't open it until the two agree.",
                        cube.network.label(),
                        chain.label(),
                    ));
                }
                if let Some(reason) = home.chain_availability(cube.network).reason() {
                    return Err(reason.to_string());
                }
                if let crate::services::unlock::creation_gate::CreationGate::Blocked(reason) =
                    crate::services::unlock::creation_gate::evaluate_for_cube(cube, None)
                {
                    return Err(format!(
                        "{reason}\n\n{}",
                        crate::services::unlock::creation_gate::not_a_backup_copy(
                            cube.is_passkey_cube()
                        )
                    ));
                }
                if cube.is_passkey_cube() && !crate::feature_flags::PASSKEY_ENABLED {
                    return Err("This Cube is unlocked with a passkey, and passkey unlock is turned off in this build. Restore it from your Recovery Kit and its recovery password, or use a build with COINCUBE_ENABLE_PASSKEY set.".into());
                }
                if cube
                    .bitcoin_import
                    .as_deref()
                    .and_then(app::settings::BitcoinImport::active_attachment)
                    .is_some()
                    && bitcoin_wallet_settings_for_cube(root, *chain, cube).is_none()
                {
                    return Err("This Cube's imported wallet registration is missing or inconsistent. Restore the wallet from its Recovery Kit before opening it.".into());
                }
                Ok(())
            }
            home::Message::Install(root, chain, flow, _) => {
                if !crate::chain::same_path(root.path(), home.datadir_path.path()) {
                    return Err("The installation belongs to another data directory.".into());
                }
                // Home has no Cube master to supply to Vault creation on any
                // chain. Refuse before GUI retires the unrelated open Cube/session.
                if matches!(flow, UserFlow::CreateWallet) {
                    return Err("Create or unlock a Cube before creating its Vault".into());
                }
                home.chain_availability(*chain)
                    .reason()
                    .map_or(Ok(()), |reason| Err(reason.to_string()))
            }
            _ => Err("This is not a Cube admission.".into()),
        }
    }

    fn guard_fork_task(&mut self, task: Task<Message>) -> Task<Message> {
        if task.units() == 0 {
            return task;
        }
        let generation = self.fork_session_generation;
        let id = self.next_fork_task;
        self.next_fork_task = self.next_fork_task.wrapping_add(1);
        let (task, handle) = task
            .map(move |message| Message::ForkAsync(generation, Box::new(message)))
            .chain(Task::done(Message::ForkTaskFinished(generation, id)))
            .abortable();
        self.fork_tasks.insert(id, handle.abort_on_drop());
        task
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        if let (State::Home(home), Message::Launch(launch)) = (&self.state, &message) {
            if !home.accepts_launcher_message(launch) {
                return Task::none();
            }
        }
        if let State::App(app) = &self.state {
            let origin = app.cache().app_generation;
            if !self
                .seed_export_origins
                .iter()
                .any(|(known, _, _, _)| *known == origin)
            {
                self.seed_export_origins.push((
                    origin,
                    app.datadir().path().to_path_buf(),
                    app.cube_settings().id.clone(),
                    app.cube_settings().network,
                ));
            }
        }
        let message = match message {
            Message::LocalDuress(proof) => return self.activate_verified_seed_export_duress(proof),
            Message::Run(app::Message::View(app::view::Message::DuressActivateLocal(proof))) => {
                return self.activate_verified_seed_export_duress(proof);
            }
            Message::OpeningTick(now) => {
                self.sync_opening(now);
                return Task::none();
            }
            Message::CancelOpening => return self.cancel_opening(),
            Message::OpenWithoutSpark => {
                if let Some(opening) = self.opening.as_mut() {
                    opening.without_spark = true;
                }
                self.sync_opening(Instant::now());
                return Task::none();
            }
            message @ Message::BreezClientLoadedAfterPin { .. } if self.opening_cancelled => {
                // A load the user cancelled finished anyway: drop it and
                // stop anything it started, as for a closed loader.
                discard_closed_loader_result(message);
                return Task::none();
            }
            Message::UnlockMotionTick(now) => {
                if self.unlock_motion.is_some_and(|started| {
                    home::launcher_presenter::unlock_turn_progress(started, now) >= 1.0
                }) {
                    self.unlock_motion = None;
                }
                return Task::none();
            }
            Message::AppAsync(generation, message) => {
                if !self.accepts_app_generation(generation) {
                    discard_closed_loader_result(*message);
                    return Task::none();
                }
                *message
            }
            Message::ForkAsync(generation, message) => {
                if generation != self.fork_session_generation {
                    discard_closed_loader_result(*message);
                    return Task::none();
                }
                *message
            }
            Message::ForkTaskFinished(generation, id) => {
                if generation == self.fork_session_generation {
                    self.fork_tasks.remove(&id);
                }
                return Task::none();
            }
            message => message,
        };
        if matches!(
            message,
            Message::StartClaimInstaller | Message::CopyInstallFinished { .. }
        ) {
            return Task::done(message);
        }
        let preserve_seed_export_trial = matches!(
            &message,
            Message::Run(app::Message::View(app::view::Message::Settings(
                app::view::SettingsMessage::BackupMasterSeed(
                    app::view::BackupWalletMessage::VerifyPin
                ) | app::view::SettingsMessage::RecoveryKit(
                    app::view::RecoveryKitMessage::VerifyPin
                ) | app::view::SettingsMessage::RecoveryAlerts(
                    app::view::RecoveryAlertsMessage::ConfirmFullCube
                )
            )))
        );
        if matches!(
            &message,
            Message::Run(app::Message::View(app::view::Message::Settings(
                app::view::SettingsMessage::BackupMasterSeed(
                    app::view::BackupWalletMessage::PinVerifiedAttempt(..)
                ) | app::view::SettingsMessage::RecoveryKit(
                    app::view::RecoveryKitMessage::PinVerifiedAttempt(..)
                ) | app::view::SettingsMessage::RecoveryAlerts(
                    app::view::RecoveryAlertsMessage::FullCubePinReady(..)
                        | app::view::RecoveryAlertsMessage::FullCubePinFailed(..)
                )
            )))
        ) {
            self.pending_seed_export_trials = self.pending_seed_export_trials.saturating_sub(1);
        }
        let was_app = matches!(&self.state, State::App(_));
        let was_fork_app =
            matches!(&self.state, State::App(app) if app.cube_settings().network.is_blake2b());
        // The PIN screen's loading view also covers classification, which a
        // wrong or duress PIN ends; the opening starts only once it passed.
        let pin_verified = matches!(
            message,
            Message::PinEntry(crate::pin_entry::Message::PinVerified)
        );
        let mut result = self.update_inner(message);
        if preserve_seed_export_trial && result.units() > 0 {
            self.pending_seed_export_trials += 1;
        }
        result = self.track_opening(pin_verified, result);
        if !matches!(self.state, State::PinEntry(_) | State::PasskeyUnlock(_))
            || matches!(&self.state, State::PasskeyUnlock(passkey) if passkey.is_opening())
            || self.opening.is_some()
        {
            self.unlock_motion = None;
            self.unlock_floor = None;
        }
        let is_app = matches!(&self.state, State::App(_));
        if matches!(self.state, State::Home(_) | State::DuressActive(_)) {
            self.pending_recovery_route = None;
        }
        if was_app != is_app {
            self.app_generation = self.app_generation.wrapping_add(1);
        }
        if matches!(&self.state, State::App(app) if app.local_feature_revoked()) {
            drop(result);
            return self.invalidate_fork_state(AuthChange::LogOut, true, true);
        }
        if matches!(&self.state, State::App(app) if app.requires_connect() && app.authenticated_coincube_client().is_none())
        {
            drop(result);
            // A fork App that lost its client: its own arm tears it down; the
            // kind and origin only matter to the Bitcoin arm, not reached here.
            return self.invalidate_fork_session(AuthChange::LogOut, true);
        }
        if !was_app && is_app {
            if let (Some((root, chain, cube_id)), State::App(app)) =
                (self.pending_recovery_route.take(), &self.state)
            {
                if app.datadir() == &root
                    && app.cube_settings().network == chain
                    && app.cube_settings().id == cube_id
                {
                    result = Task::batch([
                        result,
                        Task::done(Message::Run(app::Message::View(app::view::Message::Menu(
                            app::menu::Menu::Cube(app::menu::CubeSubMenu::Settings(
                                app::menu::CubeSettingsOption::Recovery,
                            )),
                        )))),
                    ]);
                }
            }
        }
        if matches!(&self.state, State::App(app) if app.cube_settings().network.is_blake2b()) {
            if preserve_seed_export_trial {
                // A confirmed duress marker is an emergency even when this
                // fork App is cancelled while Argon2 is running. Ordinary
                // words/JSON still pass through ForkAsync's stale fence.
                let generation = self.fork_session_generation;
                result.map(move |message| Message::ForkAsync(generation, Box::new(message)))
            } else {
                self.guard_fork_task(result)
            }
        } else {
            if was_fork_app {
                self.fork_session_generation = self.fork_session_generation.wrapping_add(1);
                self.fork_tasks.clear();
            }
            // A Loader -> App transition also returns App::new startup work.
            // Its callbacks must belong to the generation just admitted,
            // including the deferred Recovery destination.
            if is_app {
                let generation = self.app_generation;
                result.map(move |message| Message::AppAsync(generation, Box::new(message)))
            } else {
                result
            }
        }
    }

    fn activate_verified_seed_export_duress(
        &mut self,
        proof: crate::services::unlock::throttle::VerifiedSeedExportDuress,
    ) -> Task<Message> {
        let (origin_root, origin_cube, origin_chain, origin_app) = proof.origin();
        if !self
            .seed_export_origins
            .iter()
            .any(|(app, root, cube, chain)| {
                *app == origin_app
                    && crate::chain::same_path(root, origin_root)
                    && cube == origin_cube
                    && *chain == origin_chain
            })
        {
            return Task::none();
        }
        self.activate_trusted_seed_export_duress(proof)
    }

    /// The originating tab can be closed while its PIN task finishes. GUI
    /// checks the private proof against its own data directory before handing
    /// it to a surviving tab; that tab has no historical App origin to match.
    pub(super) fn activate_unhosted_seed_export_duress(
        &mut self,
        proof: crate::services::unlock::throttle::VerifiedSeedExportDuress,
    ) -> Task<Message> {
        self.activate_trusted_seed_export_duress(proof)
    }

    fn activate_trusted_seed_export_duress(
        &mut self,
        proof: crate::services::unlock::throttle::VerifiedSeedExportDuress,
    ) -> Task<Message> {
        let Some((root, cube_id, chain, _app, account_id)) = proof.claim() else {
            return Task::none();
        };
        if !root.is_absolute() || cube_id.is_empty() {
            return Task::none();
        }
        self.activate_local_duress(root, chain, account_id)
    }

    /// The unlock classifier itself minted this one-use proof. Its captured
    /// Cube survives a Back, closed tab, or another PinEntry at the same id.
    pub(super) fn activate_verified_unlock_duress(
        &mut self,
        proof: crate::pin_entry::VerifiedUnlockDuress,
    ) -> Task<Message> {
        let Some((root, cube_id, chain, account_id)) = proof.claim() else {
            return Task::none();
        };
        if !root.is_absolute() || cube_id.is_empty() {
            return Task::none();
        }
        self.activate_local_duress(root, chain, account_id)
    }

    fn activate_local_duress(
        &mut self,
        root: std::path::PathBuf,
        chain: crate::chain::ChainId,
        account_id: Option<String>,
    ) -> Task<Message> {
        let datadir = CoincubeDirectory::new(root.clone());
        let network = chain.bitcoin_network();
        // Drop the current App, any in-flight seed reveal, and any different
        // Cube session before the local wipe. A restored Cube can have no
        // process session; verified marker evidence is sufficient.
        self.stop();
        self.state = State::DuressActive(
            crate::app::view::duress::active_screen::DuressActiveScreen::with_context(
                datadir.clone(),
                Some(network),
            ),
        );
        self.seed_export_origins.clear();
        self.pending_seed_export_trials = 0;
        crate::app::session::close();
        Task::perform(
            async move {
                run_local_duress_activation(&root, account_id).await;
                let queue_pending = crate::services::duress::queue::DuressQueue::new(&root)
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

    fn update_inner(&mut self, message: Message) -> Task<Message> {
        use crate::app::settings::global::GlobalSettings;
        let message = match message {
            Message::Launch(floor @ home::Message::RunFromFloor(..)) => {
                if let Err(error) = self.validate_home_exit(&floor) {
                    if let State::Home(home) = &mut self.state {
                        home.set_error(error);
                    }
                    return Task::none();
                }
                let home::Message::RunFromFloor(root, config, chain, cube, _) = floor else {
                    unreachable!();
                };
                self.pending_recovery_route = None;
                Message::Launch(home::Message::Run(root, config, chain, cube))
            }
            Message::Launch(home::Message::RunToRecovery(root, config, chain, cube)) => {
                self.pending_recovery_route = Some((root.clone(), chain, cube.id.clone()));
                Message::Launch(home::Message::Run(root, config, chain, cube))
            }
            Message::Launch(home::Message::Run(root, config, chain, cube)) => {
                self.pending_recovery_route = None;
                Message::Launch(home::Message::Run(root, config, chain, cube))
            }
            Message::ForkInstallCompleted(generation, msg) => {
                if generation != self.fork_session_generation {
                    return Task::none();
                }
                self.fork_save_task.take();
                Message::Install(msg)
            }
            msg => msg,
        };

        if let Message::Launch(ref launch) = message {
            if matches!(
                launch,
                home::Message::Run(..)
                    | home::Message::RunToRecovery(..)
                    | home::Message::Install(..)
            ) {
                if let Err(error) = self.validate_home_exit(launch) {
                    if let State::Home(home) = &mut self.state {
                        home.set_error(error);
                    }
                    return Task::none();
                }
            }
        }

        // A Spark nickname this App still owes is written when the tab
        // replaces the App (Lock Cube, Set up Vault), as leaving its tab
        // would. Taken before the App goes; started after, without waiting.
        let nickname_flush = match (&self.state, &message) {
            (
                State::App(app),
                Message::LockCube | Message::LockCubeToLauncher | Message::LockCubeToDelete,
            )
            | (
                State::App(app),
                Message::Run(app::Message::View(
                    app::view::Message::SetupVault | app::view::Message::SetupVaultRestoreFromKit,
                )),
            ) => app.spark_nickname_flush(),
            _ => None,
        };

        let result = match (&mut self.state, message) {
            #[cfg(feature = "mcp-proposals")]
            (State::Installer(installer), Message::LockCube)
                if installer.context.mcp_proposal.is_some() =>
            {
                installer.expire_mcp_proposal();
                if installer.proposal_install_dispatched && !installer.proposal_finished {
                    return Task::none();
                }
                let root = installer.datadir.clone();
                let chain = installer.context.bitcoin_config.chain;
                let failure = installer.proposal_failure.clone();
                installer.stop();
                let (mut home, task) = Home::new_for_chain(root, Some(chain));
                if let Some(failure) = failure {
                    home.set_error(failure);
                }
                self.state = State::Home(home);
                task.map(Message::Launch)
            }
            (
                State::App(app),
                message @ (Message::LockCubeToLauncher | Message::LockCubeToDelete),
            ) => {
                let datadir = app.datadir().clone();
                let chain = app.cube_settings().network;
                let delete = matches!(message, Message::LockCubeToDelete)
                    .then(|| app.cube_settings().id.clone());
                // Stop owned services as well as revoking Claim before this
                // App is dropped. GUI dispatch already closed same-Cube
                // siblings across panes.
                crate::app::session::close();
                app.stop_in_background();
                let (home, command) = Home::new_for_chain(datadir, Some(chain));
                self.state = State::Home(home);
                let command = command.map(Message::Launch);
                match delete {
                    // Not chained: the Launcher's startup includes Connect
                    // requests, and the confirmation waits only for its list.
                    Some(cube_id) => Task::batch([
                        command,
                        Task::done(Message::Launch(home::Message::View(
                            home::ViewMessage::DeleteCube(home::DeleteCubeMessage::ShowModalFor {
                                cube_id,
                                chain,
                            }),
                        ))),
                    ]),
                    None => command,
                }
            }
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
                let chain = cube.network;
                // A claim in flight is revoked before the Cube's context goes:
                // the `App` is dropped a few lines down, and a queued
                // submission must not outlive the session it was confirmed in.
                // Unlocking again waits for the daemon this leaves stopping.
                app.stop_in_background();
                crate::app::session::close();

                let config = app::Config::from_file(
                    &datadir
                        .network_directory(chain)
                        .path()
                        .join(app::config::DEFAULT_FILE_NAME),
                );

                let Ok(config) = config else {
                    // No readable gui config to hand the PIN screen. Fall all
                    // the way back to the launcher rather than staying open.
                    let (home, command) = Home::new_for_chain(datadir, Some(chain));
                    self.state = State::Home(home);
                    return command.map(Message::Launch);
                };

                let wallet_settings = bitcoin_wallet_settings_for_cube(&datadir, chain, &cube);
                if cube
                    .bitcoin_import
                    .as_deref()
                    .and_then(app::settings::BitcoinImport::active_attachment)
                    .is_some()
                    && wallet_settings.is_none()
                {
                    let (mut home, command) = Home::new_for_chain(datadir, Some(chain));
                    home.set_error("This Cube's imported wallet registration is missing or inconsistent. Restore the wallet from its Recovery Kit before opening it.");
                    self.state = State::Home(home);
                    return command.map(Message::Launch);
                }
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
                    connect_client: app.authenticated_coincube_client(),
                };
                // The `PASSKEY_ENABLED` guard the open path carries is
                // deliberately not repeated here: this Cube is already open in
                // this process, so the build plainly has the feature on.
                self.state = unlock_state(cube, datadir_root, on_success, duress_account_id);
                Task::none()
            }
            (State::Home(l), Message::Launch(msg)) => match msg {
                home::Message::Install(datadir, network, init, coincube_client) => {
                    if let Some(reason) = l.chain_availability(network).reason() {
                        l.set_error(reason.to_string());
                        return Task::none();
                    }

                    // `coincube_client` is populated when the home
                    // already holds an authenticated Connect session (today
                    // the Recovery-Kit restore path forwards it so the
                    // installer step can skip a redundant email+OTP). Other
                    // home entry points pass `None` and the relevant
                    // installer step runs its own auth form as before.
                    let (install, command) = match Installer::try_new_for_chain(
                        datadir.clone(),
                        network,
                        init,
                        false,
                        None,
                        None,
                        None,
                        false,
                        coincube_client,
                    ) {
                        Ok(result) => result,
                        Err(error) => {
                            l.set_error(error.to_string());
                            return Task::none();
                        }
                    };
                    if !network.is_blake2b() && !datadir.exists() {
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
                    self.state = State::Installer(Box::new(install));
                    command.map(Message::Install)
                }
                home::Message::Run(datadir_path, cfg, chain, cube) => {
                    self.unlock_floor = None;
                    let opened_on_floor = l.launcher_floor_open_armed();
                    let spin_from_details = l.take_launcher_details_face(&cube);
                    if opened_on_floor && !l.floor_open_matches_current_cube(&cube) {
                        l.set_error(
                            "This Cube changed while opening. Select it again to use the current details.",
                        );
                        self.unlock_motion = None;
                        return Task::none();
                    }
                    // Every refusal below abandons this open: drop an armed Split
                    // handoff with it rather than let a later open inherit it.
                    // Chain gate — first, before anything below can touch a
                    // seed file, an SDK, the daemon or a node. Two checks:
                    //
                    // 1. The directory this Cube was read from must be the one
                    //    its own settings name. `Home` loaded it from
                    //    `<datadir>/<chain.dir_name()>/settings.json`; a record
                    //    claiming another chain is either a hand-edited file
                    //    or a copy that landed in the wrong directory, and
                    //    either way its seed folder, Connect string and
                    //    provider would disagree with each other from here on.
                    // 2. This build must be able to *run* the chain. Both
                    //    Bitcoin Blake2b identities are dormant: their settings
                    //    are kept intact and distinct, and that is all this
                    //    version does with them.
                    //
                    // Only after both pass is the identity projected onto the
                    // `bitcoin::Network` the unlock and load paths encode with.
                    if cube.network != chain {
                        l.set_error(format!(
                            "This Cube's settings say it lives on {}, but it was found in \
                             the {} folder. Tenshu won't open it until the two agree.",
                            cube.network.label(),
                            chain.label(),
                        ));
                        return Task::none();
                    }
                    if let Some(reason) = l.chain_availability(cube.network).reason() {
                        l.set_error(reason.to_string());
                        return Task::none();
                    }
                    let network = chain.bitcoin_network();

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
                    let wallet_settings =
                        bitcoin_wallet_settings_for_cube(&datadir_path, chain, &cube);
                    if cube
                        .bitcoin_import
                        .as_deref()
                        .and_then(app::settings::BitcoinImport::active_attachment)
                        .is_some()
                        && wallet_settings.is_none()
                    {
                        l.set_error("This Cube's imported wallet registration is missing or inconsistent. Restore the wallet from its Recovery Kit before opening it.");
                        return Task::none();
                    }

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
                        connect_client: l.connect_account.authenticated_client(),
                    };

                    let unlock_floor = spin_from_details
                        .then(|| l.launcher_scene_context(&cube))
                        .flatten();
                    let mut state = unlock_state(cube, datadir_root, on_success, duress_account_id);
                    if let State::PinEntry(pin) = &mut state {
                        pin.set_connect_provider(&l.connect_account.client.base_url);
                        if pin.requires_connect() {
                            // The GUI supplies a scoped completed-account candidate;
                            // a raw Home token may still be behind its safety gate.
                            let crate::pin_entry::PinEntrySuccess::LoadApp {
                                connect_client, ..
                            } = &mut pin.on_success;
                            *connect_client = None;
                        }
                    }
                    self.state = state;
                    self.unlock_motion = unlock_floor.as_ref().map(|_| Instant::now());
                    self.unlock_floor = unlock_floor;
                    match &self.state {
                        State::PinEntry(_) => {
                            iced::widget::operation::focus(crate::pin_input::first_digit_id())
                        }
                        // Opening a passkey Cube is the request to unlock it,
                        // so raise the system prompt now rather than wait for a
                        // second press. Only this route does: a re-lock must
                        // not answer itself with a prompt, so it shows the
                        // button and waits.
                        State::PasskeyUnlock(_) => Task::batch([
                            iced::widget::operation::focus(crate::passkey_unlock::begin_button_id()),
                            Task::done(Message::PasskeyUnlock(
                                crate::passkey_unlock::Message::Begin,
                            )),
                        ]),
                        _ => Task::none(),
                    }
                }
                home::Message::View(home::ViewMessage::ToggleTheme) => {
                    Task::done(Message::ToggleTheme)
                }
                home::Message::View(home::ViewMessage::OpenAppPreferences) => {
                    Task::done(Message::OpenAppPreferences(PreferencesSource::Home))
                }
                home::Message::View(home::ViewMessage::ChangeDevicePreference(change)) => {
                    Task::done(Message::HomeDevicePreference(change))
                }
                home::Message::GlobalSettingsChanged => Task::done(Message::GlobalSettingsChanged),
                home::Message::ConnectSignedInBubble => Task::done(Message::ConnectSignedIn),
                _ => l.update(msg).map(Message::Launch),
            },
            (State::Installer(i), Message::Install(msg)) => {
                #[cfg(feature = "mcp-proposals")]
                if i.proposal_failure.is_some() && !matches!(msg, installer::Message::BackToApp(..))
                {
                    // No automatic retry or late callback may reopen setup
                    // after a classified partial failure.
                    return Task::none();
                }
                #[cfg(feature = "mcp-proposals")]
                if i.context
                    .mcp_proposal
                    .as_ref()
                    .is_some_and(|p| !p.unlocked())
                {
                    i.expire_mcp_proposal();
                }
                #[cfg(feature = "mcp-proposals")]
                if i.context.mcp_proposal.is_some()
                    && matches!(msg, installer::Message::BackToApp(..))
                {
                    // This message is intercepted here, not by Installer.update.
                    // Never reconstruct an unlocked App from retained clients.
                    if i.proposal_install_dispatched && !i.proposal_finished {
                        return Task::none();
                    }
                    let root = i.datadir.clone();
                    let chain = i.context.bitcoin_config.chain;
                    let failure = i.proposal_failure.clone();
                    i.stop();
                    let (mut home, task) = Home::new_for_chain(root, Some(chain));
                    if let Some(failure) = failure {
                        home.set_error(failure);
                    }
                    self.state = State::Home(home);
                    return task.map(Message::Launch);
                }
                if let installer::Message::Exit(settings_opt, internal_bitcoind) = msg {
                    // Associate wallet with cube, and — for the Recovery
                    // Kit restore flow specifically — build the
                    // BreezClient in the same async task so the loader
                    // doesn't hit the "missing pre-loaded BreezClient"
                    // error path and hang on "Starting daemon…".
                    let chain = i.context.bitcoin_config.chain;
                    let network_dir = i.datadir.network_directory(chain);
                    let datadir = i.datadir.clone();
                    // The Vault's identity, both halves. The installer context
                    // is the only place the descriptor is in hand at exit —
                    // `WalletSettings` carries the checksum but not the
                    // descriptor. Persisting the fingerprint
                    // now is what lets the home cube list render a vault id
                    // without parsing a descriptor per row
                    // (`plans/PLAN-vault-identity-unification.md` D1).
                    let vault = settings_opt.as_ref().map(|s| {
                        app::settings::VaultIdentity::new(
                            s.wallet_id(),
                            i.context.descriptor.as_ref(),
                        )
                    });
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
                    let restored_cube = installer_exit_identity(i);

                    // Capture restore-flow state up-front. Cloning the
                    // `Zeroizing<String>` here means the PIN copy
                    // carried into the Task is its own heap-zeroing
                    // value — it's dropped (and zeroed) once the task
                    // completes.
                    // Only a *password* Recovery Kit evidences the flag the
                    // Home badge reads. The keychain seal keeps its own record
                    // and an heir has no kit of their own, yet both reach the
                    // same commit seam — so this asks which restore it was, not
                    // merely whether one happened.
                    let from_password_kit = matches!(
                        i.context.restore_source,
                        Some(installer::RestoreSource::PasswordKit)
                    );
                    let restore_seed = installer_exit_seed(i);
                    let imported_restore = i.context.restored_imported_wallet.clone();
                    let imported_kit_source = match i.context.restore_source {
                        Some(installer::RestoreSource::PasswordKit) => {
                            Some(crate::services::bitcoin_import::RestoredKitSource::Password)
                        }
                        Some(installer::RestoreSource::KeychainSeal) => {
                            Some(crate::services::bitcoin_import::RestoredKitSource::OwnerKeychain)
                        }
                        _ => None,
                    };
                    // Kit restore uses the authenticated Connect Esplora proxy;
                    // imported wallets never enter the remote Vault backend.
                    let imported_backend = i
                        .context
                        .connect_jwt
                        .as_ref()
                        .map(|jwt| {
                            coincubed::config::BitcoinBackend::Esplora(
                                installer::connect_esplora_config(chain, jwt.as_str()),
                            )
                        })
                        .or_else(|| i.context.bitcoin_backend.clone());

                    let generation = self.fork_session_generation;
                    let task = Task::perform(
                        async move {
                            let mut cube = if let Some(blob) = imported_restore.as_ref() {
                                find_or_create_cube_for_imported_restore(
                                    &network_dir,
                                    vault.as_ref(),
                                    &wallet_alias,
                                    chain,
                                    originating_cube_id,
                                    restored_cube,
                                    restore_seed.as_ref(),
                                    blob,
                                )
                                .await?
                            } else {
                                find_or_create_cube(
                                    &network_dir,
                                    vault.as_ref(),
                                    &wallet_alias,
                                    chain,
                                    originating_cube_id,
                                    restored_cube,
                                    restore_seed.as_ref(),
                                )
                                .await?
                            };
                            if let Some(blob) = &imported_restore {
                                let backend = imported_backend.ok_or_else(||
                                    "Sign in to Connect to restore this imported Bitcoin wallet.".to_string())?;
                                cube = crate::services::bitcoin_import::restore_from_kit(
                                    &network_dir,
                                    &cube.id,
                                    blob,
                                    backend,
                                    imported_kit_source.ok_or_else(|| {
                                        "Imported wallets require owner recovery.".to_string()
                                    })?,
                                )
                                .await?;
                            }

                            // Hand the freshly minted Cube's PIN to the session.
                            //
                            // `session::open` is otherwise called from exactly
                            // one place — the PIN-entry unlock screen — and a
                            // restore never passes through it: it goes installer
                            // → app directly. So the session held no PIN for the
                            // Cube that had just been created, `pin_for` answered
                            // `None`, and `Wallet::load_hotsigners` fell to its
                            // no-credential branch. The Vault then could not open
                            // the hot signer whose seed the installer had written
                            // moments earlier, under this very PIN, and the
                            // signing picker reported the key as unreachable
                            // until the app was restarted and unlocked normally.
                            //
                            // Only the restore flows reach here with a PIN of
                            // their own; a Vault installed inside an already-open
                            // Cube inherits that Cube's live session.
                            // Fork installs always return to the PIN screen.
                            // An async metadata save must never reopen their
                            // session after logout or account replacement.
                            if !chain.is_blake2b() {
                                if let Some(seed) = &restore_seed {
                                    app::session::open(cube.id.clone(), seed.pin.clone());
                                }
                            }

                            // Getting here through a kit *is* proof the Cube has
                            // one: we just fetched and decrypted it. But
                            // `has_recovery_kit()` reads three local flags, all
                            // written only when a backup is made *from this
                            // device* — so a Cube restored from a kit made
                            // elsewhere had none of them set and the Home card
                            // announced "Registered to Connect — no recovery
                            // kit" about the very kit it had just been built
                            // from. Recording it here is what makes the card
                            // agree with what the user just did; Settings would
                            // otherwise only correct it once its status loaded.
                            if from_password_kit {
                                if let Err(e) =
                                    app::settings::update_settings_file(&network_dir, |settings| {
                                        Some(marked_kit_backed_up(settings, &cube.id))
                                    })
                                    .await
                                {
                                    tracing::warn!(
                                        "Could not record that this Cube was restored from a \
                                         Recovery Kit: {}. The Home card may show it as having \
                                         no kit until Settings reconciles with the server.",
                                        e
                                    );
                                }
                            }

                            // Only the restore path needs to build a
                            // BreezClient up-front — the fresh-install
                            // flow builds it at PIN entry. On `NetworkNotSupported`
                            // (testnet/signet) we mirror the PIN-entry
                            // branch (`BreezClientLoadedAfterPin`
                            // handler) and hand back a disconnected
                            // client: the Loader's Synced/App arms
                            // treat a `None` BreezClient as an
                            // architectural bug and error out, so
                            // pre-loaded-must-exist is the contract.
                            if chain.is_blake2b() {
                                return Ok((cube, None, None));
                            }
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
                            let msg = installer::Message::CubeSaved(
                                result,
                                settings_opt.clone(),
                                internal_bitcoind.clone(),
                            );
                            if chain.is_blake2b() {
                                Message::ForkInstallCompleted(generation, msg)
                            } else {
                                Message::Install(msg)
                            }
                        },
                    );
                    if chain.is_blake2b() {
                        let (task, handle) = task.abortable();
                        self.fork_save_task = Some(handle.abort_on_drop());
                        task
                    } else {
                        task
                    }
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
                            #[cfg(feature = "mcp-proposals")]
                            i.finish_proposal_failure("The wallet/Cube setup could not be saved completely. Partial local data is retained. Return to the launcher and inspect the outcome before retrying.");
                            return i
                                .update(installer::Message::CubeSaveFailed(err))
                                .map(Message::Install);
                        }
                    };

                    // An imported Recovery Kit attaches its wallet inside the
                    // durable import journal after the installer handed us a
                    // seed-only `settings_opt`. Resolve the newly persisted
                    // wallet row now so this first open takes the same Loader
                    // route as every later open.
                    let settings_opt = if cube
                        .bitcoin_import
                        .as_deref()
                        .and_then(app::settings::BitcoinImport::active_attachment)
                        .is_some()
                    {
                        match bitcoin_wallet_settings_for_cube(&i.datadir, cube.network, &cube) {
                            Some(wallet) => Some(Box::new(wallet)),
                            None => {
                                return i
                                    .update(installer::Message::CubeSaveFailed(
                                        "Imported wallet registration is missing or inconsistent after recovery. Retry this Recovery Kit before opening the Cube.".into(),
                                    ))
                                    .map(Message::Install);
                            }
                        }
                    } else {
                        settings_opt
                    };

                    #[cfg(feature = "mcp-proposals")]
                    if i.context.mcp_proposal.is_some() {
                        // Complete association, then drop all installer/client
                        // handles. Even a still-open original session must unlock
                        // afresh; old tokens/proposal IDs never become read grants.
                        let root = i.datadir.clone();
                        i.stop();
                        let (mut home, task) = Home::new_for_chain(root, Some(cube.network));
                        home.set_error("Vault created. Reopen this Cube to verify keys/backups and approve fresh read access.");
                        self.state = State::Home(home);
                        return task.map(Message::Launch);
                    }

                    if cube.network.is_blake2b() {
                        // A copy (Tools › Split coins → Create copy) is
                        // finished once saved. Its source App is still open in
                        // the tab that launched it, so hand control back
                        // there; the copy opens later from the Launcher like
                        // any other Cube, which keeps one public Cube session.
                        if i.source_cube.is_some() {
                            i.claim_target_saved = true;
                            if let Some(owner) = self.copy_owner.take() {
                                return Task::done(Message::CopyInstallFinished {
                                    owner,
                                    created: true,
                                });
                            }
                        }
                        let config = match app::Config::from_file(
                            &i.datadir
                                .network_directory(cube.network)
                                .path()
                                .join(app::config::DEFAULT_FILE_NAME),
                        ) {
                            Ok(config) => config,
                            Err(e) => {
                                return i
                                    .update(installer::Message::CubeSaveFailed(e.to_string()))
                                    .map(Message::Install);
                            }
                        };
                        // A Vault created inside an open Cube reopens it from
                        // that Cube's session, as on Bitcoin.
                        if i.launched_from_app && i.source_cube.is_none() {
                            let datadir = i.datadir.clone();
                            let client = i.context.coincube_client.clone();
                            let wallet_settings = settings_opt.as_deref().cloned();
                            if let Some(task) = self.open_fork_vault_in_session(
                                datadir,
                                config.clone(),
                                cube.clone(),
                                wallet_settings,
                                client,
                            ) {
                                return task;
                            }
                            // The Cube's own unlock screen opens a fresh
                            // session; the one this installer ran under ends.
                            app::session::close_cube(&cube.id);
                            let State::Installer(i) = &mut self.state else {
                                return Task::none();
                            };
                            let on_success = crate::pin_entry::PinEntrySuccess::LoadApp {
                                datadir: i.datadir.clone(),
                                config,
                                network: cube.network.bitcoin_network(),
                                internal_bitcoind: None,
                                backup: None,
                                wallet_settings: settings_opt.map(|s| *s),
                                connect_client: i.context.coincube_client.clone(),
                            };
                            self.state = unlock_state(
                                cube,
                                i.datadir.path().to_path_buf(),
                                on_success,
                                None,
                            );
                            return Task::none();
                        }
                        let on_success = crate::pin_entry::PinEntrySuccess::LoadApp {
                            datadir: i.datadir.clone(),
                            config,
                            network: cube.network.bitcoin_network(),
                            internal_bitcoind: None,
                            backup: None,
                            wallet_settings: settings_opt.map(|s| *s),
                            connect_client: i.context.coincube_client.clone(),
                        };
                        // A Cube this installer minted (a claim target or a
                        // restore) opens its own PIN screen.
                        self.state =
                            unlock_state(cube, i.datadir.path().to_path_buf(), on_success, None);
                        return Task::none();
                    }

                    if let Some(settings) = settings_opt {
                        let cfg = app::Config::from_file(
                            &i.datadir
                                .network_directory(i.network)
                                .path()
                                .join(app::config::DEFAULT_FILE_NAME),
                        )
                        .expect("A gui configuration file must be present");

                        let imported = cube
                            .bitcoin_import
                            .as_ref()
                            .is_some_and(|i| i.active_attachment().is_some());
                        let (loader, command) = if imported {
                            Loader::new_imported(
                                i.datadir.clone(),
                                cfg,
                                i.network,
                                internal_bitcoind,
                                i.context.backup.clone(),
                                *settings,
                                cube.clone(),
                                restored_breez_client.or_else(|| i.breez_client.clone()),
                                restored_spark_backend.or_else(|| i.spark_backend.clone()),
                                i.context.coincube_client.clone(),
                            )
                        } else {
                            Loader::new(
                                i.datadir.clone(),
                                cfg,
                                i.network,
                                internal_bitcoind,
                                i.context.backup.clone(),
                                Some(*settings),
                                cube.clone(),
                                // The restored BreezClient (built against
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
                            )
                        };
                        self.state = State::Loader(loader);
                        command.map(Message::Load)
                    } else {
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

                        let (app, command) = match app::App::new_without_wallet(
                            breez,
                            spark,
                            cfg,
                            i.datadir.clone(),
                            i.network,
                            cube.clone(),
                        ) {
                            Ok(app) => app,
                            Err(error) => {
                                let error = match error {
                                    app::error::Error::Daemon(error) => {
                                        loader::Error::Daemon(error)
                                    }
                                    error => {
                                        loader::Error::Unexpected(crate::user_error::report(&error))
                                    }
                                };
                                return Task::done(Message::Load(loader::Message::App(Err(error))));
                            }
                        };
                        self.state = State::App(app);
                        command.map(Message::Run)
                    }
                } else if let installer::Message::BackToApp(network) = msg {
                    if let Some(owner) = self.copy_owner.take() {
                        return Task::done(Message::CopyInstallFinished {
                            owner,
                            created: false,
                        });
                    }
                    // A claim was launched from a *different* Cube than the one
                    // being built, so there is no `cube_settings` to go back to
                    // — without this the user who backs out of a claim lands on
                    // Home instead of the Cube they started in. Routed through
                    // the Loader rather than `new_without_wallet` because the
                    // source Cube has a Vault (that is what a claim reuses) and
                    // rebuilding it Vault-less would hide it until relaunch.
                    if let Some(source) = i.source_cube.take() {
                        match reopen_claim_source(&i.datadir, &source) {
                            Ok((state, task)) => {
                                self.state = state;
                                return task;
                            }
                            Err(error) => {
                                i.source_cube = Some(source);
                                i.navigation_error = Some(error);
                                return Task::none();
                            }
                        }
                    }
                    // A Bitcoin Blake2b Cube has no Breez client: go back to it
                    // from its session, which still holds its master signer.
                    if let Some(cube) = i
                        .cube_settings
                        .clone()
                        .filter(|cube| cube.network.is_blake2b())
                    {
                        let datadir = i.datadir.clone();
                        let client = i.context.coincube_client.clone();
                        let destination = i.destination_path();
                        let reopened = cube
                            .master_signer_fingerprint
                            .and_then(|fp| app::session::unlocked_signer(&cube.id, fp))
                            .zip(
                                app::Config::from_file(
                                    &datadir
                                        .network_directory(cube.network)
                                        .path()
                                        .join(app::config::DEFAULT_FILE_NAME),
                                )
                                .ok(),
                            );
                        if let Some((signer, cfg)) = reopened {
                            let cek = Arc::new(
                                crate::services::connect::crypto::CubeEncryptionKey::derive(
                                    &signer,
                                    cube.network.bitcoin_network(),
                                ),
                            );
                            let chain = cube.network;
                            match self.open_fork_without_vault(cek, client, cfg, datadir, cube) {
                                Ok(task) => return task,
                                Err(reason) => {
                                    let (mut home, command) =
                                        Home::new_for_chain(destination, Some(chain));
                                    home.set_error(reason);
                                    self.state = State::Home(home);
                                    return command.map(Message::Launch);
                                }
                            }
                        }
                        app::session::close_cube(&cube.id);
                        let (home, command) = Home::new_for_chain(destination, Some(cube.network));
                        self.state = State::Home(home);
                        return command.map(Message::Launch);
                    }
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

                            let (app, command) = match app::App::new_without_wallet(
                                breez.clone(),
                                i.spark_backend.clone(),
                                cfg,
                                i.datadir.clone(),
                                network,
                                cube.clone(),
                            ) {
                                Ok(app) => app,
                                Err(error) => {
                                    let error = match error {
                                        app::error::Error::Daemon(error) => {
                                            loader::Error::Daemon(error)
                                        }
                                        error => loader::Error::Unexpected(
                                            crate::user_error::report(&error),
                                        ),
                                    };
                                    return Task::done(Message::Load(loader::Message::App(Err(
                                        error,
                                    ))));
                                }
                            };
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
                        // No cube settings stored, go to home, on the Import
                        // or restore path a restore's path list left for.
                        let (mut home, command) = Home::new(i.destination_path(), Some(network));
                        if let Some(destination) = i.take_restore_exit() {
                            home.open_restore_destination(destination);
                        }
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
                    loader.stop();
                    self.state = State::Home(home);
                    command.map(Message::Launch)
                }
                loader::Message::View(loader::ViewMessage::SetupVault) => {
                    // A cached PIN outlives Loader auth revocation. Refuse the
                    // stale intent before the installer can read the fork seed.
                    if loader.cube_settings.network.is_blake2b() && loader.fork_revoked() {
                        loader.fail(loader::Error::Unexpected(
                            "Connect session changed; reopen this Cube".into(),
                        ));
                        return Task::none();
                    }
                    if loader.cube_settings.bitcoin_import.is_some() {
                        loader.fail(loader::Error::Unexpected(
                            "This Cube is reserved for an imported Bitcoin wallet. Resume its import from the Cube overview."
                                .into(),
                        ));
                        return Task::none();
                    }
                    // Launch installer for vault setup from loader - should return to app on Previous
                    // Opened on the Cube's own chain through the admission gate,
                    // like the App paths: `loader.network` cannot name a fork.
                    match Installer::try_new_for_chain(
                        loader.datadir_path.clone(),
                        loader.cube_settings.network,
                        UserFlow::CreateWallet,
                        true, // launched from app (loader is part of app flow)
                        Some(loader.cube_settings.clone()), // pass cube settings for returning
                        loader.breez_client.clone(), // pass breez_client to avoid re-entering PIN
                        None, // spark_backend not available from loader path
                        GlobalSettings::load_developer_mode(&GlobalSettings::path(
                            &loader.datadir_path,
                        )),
                        loader.connect_client.clone(), // the fork gate needs the session it was admitted with
                    ) {
                        Ok((install, command)) => {
                            self.state = State::Installer(Box::new(install));
                            command.map(Message::Install)
                        }
                        Err(error) => {
                            loader.fail(loader::Error::Unexpected(error.to_string()));
                            Task::none()
                        }
                    }
                }
                loader::Message::Synced(Ok((
                    wallet,
                    cache,
                    daemon,
                    bitcoind,
                    backup,
                    cube_settings,
                ))) => {
                    if loader.fork_revoked() {
                        return Task::none();
                    }
                    if cube_settings.network.is_blake2b() {
                        let client = if loader.requires_connect() {
                            let Some(client) = loader.connect_client.clone() else {
                                return Task::done(Message::Load(loader::Message::Failure(
                                    crate::daemon::DaemonError::Unexpected(
                                        "Connect session lost; reopen this Cube".into(),
                                    ),
                                )));
                            };
                            Some(client)
                        } else {
                            None
                        };
                        let opened = if let Some(client) = client {
                            App::new_for_chain_with_node(
                                cache,
                                wallet,
                                loader.cube_encryption_key.clone(),
                                client,
                                loader.gui_config.clone(),
                                daemon,
                                loader.datadir_path.clone(),
                                cube_settings,
                                bitcoind,
                            )
                        } else {
                            App::new_local_fork(
                                cache,
                                wallet,
                                loader.cube_encryption_key.clone(),
                                loader.gui_config.clone(),
                                daemon,
                                loader.datadir_path.clone(),
                                cube_settings,
                                bitcoind,
                            )
                        };
                        match opened {
                            Ok((app, task)) => {
                                self.state = State::App(app);
                                return task.map(Message::Run);
                            }
                            Err(error) => {
                                return Task::done(Message::Load(loader::Message::Failure(
                                    crate::daemon::DaemonError::Unexpected(error.to_string()),
                                )));
                            }
                        }
                    }
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
                                Message::Load(loader::Message::App(r))
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
                        )))
                    }
                }
                loader::Message::App(Ok((cache, wallet, config, daemon, datadir, bitcoind))) => {
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
                            cube_settings: loader.cube_settings.clone(),
                        }));
                    }

                    // ERROR: BreezClient should have been pre-loaded after PIN entry
                    // With mandatory PINs, this path should never execute
                    error!("Loader App missing pre-loaded BreezClient - architectural bug");
                    Task::done(Message::Load(loader::Message::App(Err(
                        loader::Error::Unexpected(
                            "BreezClient missing - should have been pre-loaded after PIN entry. \
                             Liquid wallet is encrypted and cannot be loaded without PIN."
                                .to_string(),
                        ),
                    ))))
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
                    cube_settings,
                } => {
                    // Restore Connect auth cached at `<network>/connect.json`
                    // by a prior sign-in. Without this, every
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

                    let admitted_app = App::new(
                        cache,
                        wallet,
                        breez,
                        spark_backend,
                        config,
                        daemon,
                        datadir,
                        bitcoind,
                        cube_settings,
                        connect_auth,
                    );
                    // App::new consumes only its exact target. Never leave a
                    // staged operation globally armed after an error.
                    let (app, command) = match admitted_app {
                        Ok(app) => app,
                        Err(error) => {
                            let error = match error {
                                app::error::Error::Daemon(error) => loader::Error::Daemon(error),
                                error => {
                                    loader::Error::Unexpected(crate::user_error::report(&error))
                                }
                            };
                            return Task::done(Message::Load(loader::Message::App(Err(error))));
                        }
                    };
                    self.state = State::App(app);
                    command.map(Message::Run)
                }
                loader::Message::App(Err(e)) => {
                    tracing::error!("Failed to import backup: {e}");
                    Task::none()
                }

                _ => loader.update(msg).map(Message::Load),
            },
            (State::App(app), Message::Run(msg)) => {
                match msg {
                    #[cfg(feature = "mcp-proposals")]
                    app::Message::View(app::view::Message::LocalMcp(ref review))
                        if matches!(
                            review,
                            app::local_mcp::Message::ReviewProposal
                                | app::local_mcp::Message::ReviewPairedProposal(_)
                        ) =>
                    {
                        if !app.mcp_proposals_supported() {
                            return Task::none();
                        }
                        if app.fork_feature_unconfirmed() {
                            return Task::done(Message::Run(app::Message::View(
                                app::view::Message::ShowError(
                                    "Checking Bitcoin Blake2b availability. Try again in a moment."
                                        .into(),
                                ),
                            )));
                        }
                        // Construct the normal builder only in response to the
                        // user's native review click, never from an MCP worker.
                        match Installer::try_new_for_chain(
                            app.datadir().clone(),
                            app.cube_settings().network,
                            UserFlow::CreateWallet,
                            true,
                            Some(app.cube_settings().clone()),
                            app.breez_client(),
                            app.spark_backend(),
                            false,
                            app.authenticated_coincube_client(),
                        ) {
                            Ok((install, _)) => {
                                let client = match review {
                                    app::local_mcp::Message::ReviewPairedProposal(id) => {
                                        Some(id.as_str())
                                    }
                                    _ => None,
                                };
                                let Some((settings, session)) = app.take_mcp_proposal(client)
                                else {
                                    return Task::none();
                                };
                                self.open_mcp_proposal_handoff(install, settings, session)
                            }
                            Err(error) => Task::done(Message::Run(app::Message::View(
                                app::view::Message::ShowError(error.to_string()),
                            ))),
                        }
                    }
                    app::Message::View(app::view::Message::SetupVault) => {
                        // The installer would drop a revocation that lands
                        // on it, so it waits for the Cube's first check.
                        if app.fork_feature_unconfirmed() {
                            return Task::done(Message::Run(app::Message::View(
                                app::view::Message::ShowError(
                                    "Checking Bitcoin Blake2b availability. Try again in a moment."
                                        .into(),
                                ),
                            )));
                        }
                        if app.cube_settings().bitcoin_import.is_some() {
                            return Task::done(Message::Run(app::Message::View(
                                app::view::Message::ShowError(
                                    "This Cube is reserved for an imported Bitcoin wallet. Resume its import from the Cube overview."
                                        .into(),
                                ),
                            )));
                        }
                        // Launch installer for vault setup from app - should return to app on Previous.
                        // The installer is opened on the Cube's own chain through the
                        // admission gate: `app.cache().network` is a `bitcoin::Network`
                        // and cannot name a fork, so deriving the chain from it would
                        // build a Bitcoin installer for a Bitcoin Blake2b Cube.
                        match Installer::try_new_for_chain(
                            app.datadir().clone(),
                            app.cube_settings().network,
                            UserFlow::CreateWallet,
                            true,                              // launched from app
                            Some(app.cube_settings().clone()), // pass cube settings for returning
                            app.breez_client(), // pass breez_client to avoid re-entering PIN
                            app.spark_backend(), // preserve Spark bridge across vault setup
                            GlobalSettings::load_developer_mode(&GlobalSettings::path(
                                app.datadir(),
                            )),
                            app.authenticated_coincube_client(), // authenticated API client for Keychain keys
                        ) {
                            Ok((install, command)) => {
                                self.state = State::Installer(Box::new(install));
                                command.map(Message::Install)
                            }
                            Err(error) => Task::done(Message::Run(app::Message::View(
                                app::view::Message::ShowError(error.to_string()),
                            ))),
                        }
                    }
                    app::Message::View(app::view::Message::StartClaimBlake2b) => {
                        // Tools › Split coins → Create copy. Re-checked here
                        // rather than trusted from the button: the account
                        // grant can lapse and a copy can appear meanwhile.
                        app.refresh_copy_exists();
                        let source = app.claim_source_cube();
                        let refusal = app::features::claim_blake2b(source)
                            .reason()
                            .map(str::to_string)
                            .or_else(|| {
                                source.already_claimed.then(|| {
                                    "This Cube already has a Bitcoin Blake2b copy.".to_string()
                                })
                            });
                        match refusal {
                            Some(error) => Task::done(Message::Run(app::Message::View(
                                app::view::Message::ShowError(error),
                            ))),
                            // The pane retains this source App while a
                            // temporary installer builds its copy.
                            None => Task::done(Message::StartClaimInstaller),
                        }
                    }
                    app::Message::View(app::view::Message::SetupVaultRestoreFromKit) => {
                        // W15 — same installer launch path as SetupVault,
                        // but starts in the Recovery-Kit restore flow
                        // instead of the new-vault descriptor editor.
                        // Same chain derivation and admission gate as `SetupVault`;
                        // a Bitcoin Blake2b Cube is refused here because the fork
                        // admits only the Connect-backed creation flow.
                        match Installer::try_new_for_chain(
                            app.datadir().clone(),
                            app.cube_settings().network,
                            UserFlow::RestoreVaultFromRecoveryKit,
                            true,
                            Some(app.cube_settings().clone()),
                            app.breez_client(),
                            app.spark_backend(),
                            GlobalSettings::load_developer_mode(&GlobalSettings::path(
                                app.datadir(),
                            )),
                            app.authenticated_coincube_client(),
                        ) {
                            Ok((install, command)) => {
                                self.state = State::Installer(Box::new(install));
                                command.map(Message::Install)
                            }
                            Err(error) => Task::done(Message::Run(app::Message::View(
                                app::view::Message::ShowError(error.to_string()),
                            ))),
                        }
                    }
                    app::Message::View(app::view::Message::ToggleTheme) => {
                        Task::done(Message::ToggleTheme)
                    }
                    app::Message::View(app::view::Message::OpenLauncher) => {
                        Task::done(Message::OpenLauncher)
                    }
                    app::Message::View(app::view::Message::OpenAppPreferences) => Task::done(
                        Message::OpenAppPreferences(PreferencesSource::App(self.app_generation)),
                    ),
                    app::Message::View(app::view::Message::LockCubeToLauncher) => {
                        Task::done(Message::LockCubeToLauncher)
                    }
                    app::Message::View(app::view::Message::DeleteUnfinishedCube) => {
                        // Deleting the Cube would leave its Spark or Liquid
                        // funds without it. A balance still loading does not
                        // stop it; the Launcher still asks for the PIN.
                        if app.holds_loaded_wallet_funds() {
                            Task::done(Message::Run(app::Message::View(
                                app::view::Message::ShowError(
                                    UNFINISHED_CUBE_HOLDS_FUNDS.to_string(),
                                ),
                            )))
                        } else {
                            Task::done(Message::LockCubeToDelete)
                        }
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
                    app::Message::View(app::view::Message::OpenConnectSwitchAccount) => {
                        // Unlike OpenConnectSignIn there is no session to
                        // restore: the user is already authenticated, just as
                        // the wrong account. Always hand off to Home so they
                        // can log out and sign back in.
                        Task::done(Message::OpenConnectSignIn)
                    }
                    app::Message::View(
                        app::view::Message::OpenPlanBilling
                        | app::view::Message::OpenPlanBillingFrom(_),
                    ) => {
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
                            connect_client,
                        } => {
                            let cube = pin_entry.cube().clone();
                            let pin = pin_entry.pin();

                            if cube.network.is_blake2b() && wallet_settings.is_none() {
                                let Some(signer) = pin_entry.take_bitcoin_signer() else {
                                    pin_entry.fail_open(
                                        "Unlock this Cube again to recover its chain-bound signer"
                                            .into(),
                                    );
                                    return Task::none();
                                };
                                app::session::open(cube.id.clone(), pin.clone());
                                // Kept for the Vault this Cube creates next:
                                // its installer exit reopens the Cube from the
                                // session instead of asking for the PIN again.
                                let secp = coincube_core::miniscript::bitcoin::secp256k1::Secp256k1::signing_only();
                                let fingerprint = signer.fingerprint(&secp);
                                let key =
                                    crate::services::connect::crypto::CubeEncryptionKey::derive(
                                        &signer,
                                        cube.network.bitcoin_network(),
                                    );
                                app::session::store_unlocked_signer(&cube.id, fingerprint, signer);
                                let client = connect_client.clone();
                                let (datadir, config) = (datadir.clone(), config.clone());
                                return match self.open_fork_without_vault(
                                    Arc::new(key),
                                    client,
                                    config,
                                    datadir,
                                    cube,
                                ) {
                                    Ok(task) => task,
                                    Err(reason) => {
                                        if let State::PinEntry(pin_entry) = &mut self.state {
                                            pin_entry.fail_open(reason);
                                        }
                                        Task::none()
                                    }
                                };
                            }
                            if cube.network.is_blake2b() {
                                let requires_connect = pin_entry.requires_connect();
                                let client = connect_client.clone().filter(|c| c.token().is_some());
                                if requires_connect && client.is_none() {
                                    pin_entry.fail_open("Sign in to Connect from Home before opening this Bitcoin Blake2b Cube".into());
                                    return Task::none();
                                }
                                let Some(wallet_settings) = wallet_settings
                                    .clone()
                                    .filter(|w| w.remote_backend_auth.is_none())
                                else {
                                    pin_entry.fail_open(
                                        "Bitcoin Blake2b requires a local Connect-backed Vault"
                                            .into(),
                                    );
                                    return Task::none();
                                };
                                let Some(signer) = pin_entry.take_fork_signer() else {
                                    pin_entry.fail_open(
                                        "Unlock this Cube again to recover its chain-bound signer"
                                            .into(),
                                    );
                                    return Task::none();
                                };
                                let cek = Arc::new(
                                    crate::services::connect::crypto::CubeEncryptionKey::derive(
                                        &signer,
                                        cube.network.bitcoin_network(),
                                    ),
                                );
                                app::session::open(cube.id.clone(), pin.clone());
                                let (loader, command) = if requires_connect {
                                    Loader::new_for_chain(
                                        datadir.clone(),
                                        config.clone(),
                                        wallet_settings,
                                        cube,
                                        client.expect("authenticated backend checked"),
                                        cek,
                                    )
                                } else {
                                    Loader::new_local_fork(
                                        datadir.clone(),
                                        config.clone(),
                                        wallet_settings,
                                        cube,
                                        cek,
                                    )
                                };
                                self.state = State::Loader(loader);
                                return command.map(Message::Load);
                            }

                            let Some(signer) = pin_entry.take_bitcoin_signer() else {
                                pin_entry.fail_open(
                                    "Unlock this Cube again to recover its verified signer".into(),
                                );
                                return Task::none();
                            };
                            let secp = coincube_core::miniscript::bitcoin::secp256k1::Secp256k1::signing_only();
                            let fingerprint = signer.fingerprint(&secp);
                            app::session::store_unlocked_signer(&cube.id, fingerprint, signer);

                            // ALWAYS load BreezClient (Liquid wallet) with PIN first
                            let config_clone = config.clone();
                            let datadir_clone = datadir.clone();
                            let network_val = *network;
                            let wallet_settings_clone = wallet_settings.clone();
                            let internal_bitcoind_clone = internal_bitcoind.clone();
                            let backup_clone = backup.clone();
                            let connect_client_clone = connect_client.clone();

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
                                        // Its own task: a Cancel that drops
                                        // this future cannot separate a file
                                        // write from the record that makes
                                        // the device agree with it.
                                        let joined = migrate_seed_files_on_unlock(
                                            datadir_clone.path().to_path_buf(),
                                            datadir_clone.network_directory(network_val),
                                            cube.clone(),
                                            pin.clone(),
                                        )
                                        .await;

                                        // Do not discard this. `let _ = …` threw
                                        // away the count *and* the `JoinError`,
                                        // so a panic inside migration — in code
                                        // that rewrites seed files — left no
                                        // trace at all.
                                        match joined {
                                            Ok(Ok(UnlockMigration { outcome, new_slot })) => {
                                                if let Some(name) = new_slot {
                                                    cube.duress_slot_file = Some(name);
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
                                        // Read from disk rather than from this
                                        // unlock's migration: a Cancel may have
                                        // dropped the unlock that disarmed. A
                                        // failure above takes the one slot; the
                                        // notice waits for the next unlock.
                                        if migration_error.is_none() {
                                            migration_error =
                                                duress_disarm_notice(datadir_clone.path());
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
                                                cube.network,
                                                &pin,
                                                &cube.id,
                                                cube.created_at,
                                            )
                                        {
                                            cube.master_signer_fingerprint = Some(fp);
                                            let cube_id = cube.id.clone();
                                            let network_dir =
                                                datadir_clone.network_directory(cube.network);
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
                                                cube.network,
                                                fp,
                                                &pin,
                                                &cube.id,
                                            ) {
                                                CEK::Derived(pubkey) => {
                                                    cube.connect_encryption_pubkey =
                                                        Some(pubkey.clone());
                                                    let cube_id = cube.id.clone();
                                                    let network_dir = datadir_clone
                                                        .network_directory(cube.network);
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
                                        connect_client_clone,
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
                                    connect_client,
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
                                        connect_client,
                                        migration_error,
                                        enc_key_error,
                                    }
                                },
                            )
                        }
                    }
                }
                crate::pin_entry::Message::Back => {
                    // Go back to home. `Home` speaks `bitcoin::Network` (it
                    // only ever lists the Bitcoin family), and a Cube on the
                    // PIN screen is one that passed the open gate, so the
                    // projection is lossless here.
                    let network = pin_entry.cube().network.bitcoin_network();
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
                    // for sovereign (Task A.1). Enter the cryptic screen now;
                    // GUI seals sibling surfaces before this returned Task can
                    // be polled, even if activation later stalls.
                    let network = pin_entry.cube().network.bitcoin_network();
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
                    self.state = State::DuressActive(
                        crate::app::view::duress::active_screen::DuressActiveScreen::with_context(
                            datadir.clone(),
                            Some(network),
                        ),
                    );
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
                    if unlock.cube().network.is_blake2b() {
                        // A Bitcoin Blake2b copy of a passkey Cube unlocks with
                        // the same passkey (decision 2026-10-29). It has a
                        // Vault only: open it through the fork loader, as the
                        // PIN path does, and never start Liquid or Spark.
                        let crate::pin_entry::PinEntrySuccess::LoadApp {
                            datadir,
                            config,
                            wallet_settings,
                            connect_client,
                            ..
                        } = &unlock.on_success;
                        let cube = unlock.cube().clone();
                        if wallet_settings.is_none() {
                            // No Vault yet: open the Cube itself, as a Bitcoin
                            // Cube without one opens, minus Liquid and Spark.
                            let Some(signer) =
                                crate::app::session::unlocked_signer(&cube.id, fingerprint)
                            else {
                                crate::app::session::close_cube(&cube.id);
                                unlock.relocked();
                                return Task::none();
                            };
                            let client = connect_client.clone();
                            let (datadir, config) = (datadir.clone(), config.clone());
                            let key = Arc::new(
                                crate::services::connect::crypto::CubeEncryptionKey::derive(
                                    &signer,
                                    cube.network.bitcoin_network(),
                                ),
                            );
                            return match self
                                .open_fork_without_vault(key, client, config, datadir, cube)
                            {
                                Ok(task) => task,
                                Err(reason) => {
                                    if let State::PasskeyUnlock(unlock) = &mut self.state {
                                        unlock.fail_open(reason);
                                    }
                                    Task::none()
                                }
                            };
                        }
                        // Every way out below that does not open the Vault
                        // closes the session the assertion just opened, so a
                        // copy that looks locked holds no seed.
                        let requires_connect = !crate::chain::has_managed_local_fork(
                            datadir,
                            cube.network,
                            wallet_settings.as_ref(),
                        );
                        let client = connect_client.clone().filter(|c| c.token().is_some());
                        if requires_connect && client.is_none() {
                            crate::app::session::close_cube(&cube.id);
                            unlock.fail_open(
                                "Sign in to Connect from Home before opening this Bitcoin Blake2b Cube"
                                    .into(),
                            );
                            return Task::none();
                        }
                        let Some(wallet_settings) = wallet_settings
                            .clone()
                            .filter(|w| w.remote_backend_auth.is_none())
                        else {
                            crate::app::session::close_cube(&cube.id);
                            unlock.fail_open(
                                "Bitcoin Blake2b requires a local Connect-backed Vault".into(),
                            );
                            return Task::none();
                        };
                        // The assertion parked the derived master seed in the
                        // session under this Cube; the Vault's seed file opens
                        // with the password derived from it
                        // (`app::session::seed_file_password`).
                        let Some(signer) =
                            crate::app::session::unlocked_signer(&cube.id, fingerprint)
                        else {
                            crate::app::session::close_cube(&cube.id);
                            unlock.relocked();
                            return Task::none();
                        };
                        let cek =
                            Arc::new(crate::services::connect::crypto::CubeEncryptionKey::derive(
                                &signer,
                                cube.network.bitcoin_network(),
                            ));
                        let (loader, command) = if requires_connect {
                            Loader::new_for_chain(
                                datadir.clone(),
                                config.clone(),
                                wallet_settings,
                                cube,
                                client.expect("authenticated backend checked"),
                                cek,
                            )
                        } else {
                            Loader::new_local_fork(
                                datadir.clone(),
                                config.clone(),
                                wallet_settings,
                                cube,
                                cek,
                            )
                        };
                        self.state = State::Loader(loader);
                        return command.map(Message::Load);
                    }
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
                        connect_client,
                    } = &unlock.on_success;

                    let cube = unlock.cube().clone();
                    let config_clone = config.clone();
                    let datadir_clone = datadir.clone();
                    let network_val = *network;
                    let wallet_settings_clone = wallet_settings.clone();
                    let internal_bitcoind_clone = internal_bitcoind.clone();
                    let backup_clone = backup.clone();
                    let connect_client_clone = connect_client.clone();

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
                                connect_client: connect_client_clone,
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
                    // A failed open stays on this screen with the session the
                    // assertion opened; leaving closes it.
                    crate::app::session::close_cube(&unlock.cube().id);
                    // Dropping the screen drops any in-flight ceremony, which
                    // cancels the system prompt.
                    let network = unlock.cube().network.bitcoin_network();
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
                    connect_client,
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
                // The duress-off notice stays on disk until `App` shows it.
                self.pending_duress_notice = (migration_error.as_deref()
                    == Some(DURESS_DISARM_NOTICE))
                .then(|| datadir.path().to_path_buf());
                self.pending_unlock_warnings =
                    migration_error.into_iter().chain(enc_key_error).collect();
                // The Vault is independent of Liquid: any Breez load failure
                // (NetworkNotSupported, transient connection errors, SDK
                // throttling, etc.) should fall back to a disconnected client
                // so the user can still access their Vault. Liquid features
                // will surface their own errors on demand.
                let breez = match breez_client {
                    Ok(breez) => breez,
                    Err(e) => {
                        tracing::warn!(
                            "BreezClient unavailable after PIN, continuing in disconnected mode: {}",
                            e
                        );
                        disconnected_breez_client_after_unlock(&cube, network)
                    }
                };
                if let Some(wallet_settings) = wallet_settings {
                    if let Some(auth) = &wallet_settings.remote_backend_auth {
                        let (mut home, command) = Home::new(datadir.clone(), Some(network));
                        home.set_error(auth.retired_backend_notice());
                        self.state = State::Home(home);
                        command.map(Message::Launch)
                    } else {
                        let (loader, command) = if cube
                            .bitcoin_import
                            .as_ref()
                            .is_some_and(|i| i.active_attachment().is_some())
                        {
                            Loader::new_imported(
                                datadir.clone(),
                                config.clone(),
                                network,
                                internal_bitcoind.clone(),
                                backup.clone(),
                                wallet_settings.clone(),
                                cube,
                                Some(breez),
                                spark_backend,
                                connect_client,
                            )
                        } else {
                            Loader::new(
                                datadir.clone(),
                                config.clone(),
                                network,
                                internal_bitcoind.clone(),
                                backup.clone(),
                                Some(wallet_settings.clone()),
                                cube,
                                Some(breez),
                                spark_backend,
                            )
                        };
                        self.state = State::Loader(loader);
                        command.map(Message::Load)
                    }
                } else {
                    let (app, command) = match App::new_without_wallet(
                        breez,
                        spark_backend,
                        config,
                        datadir,
                        network,
                        cube,
                    ) {
                        Ok(app) => app,
                        Err(error) => {
                            let error = match error {
                                app::error::Error::Daemon(error) => loader::Error::Daemon(error),
                                error => {
                                    loader::Error::Unexpected(crate::user_error::report(&error))
                                }
                            };
                            return Task::done(Message::Load(loader::Message::App(Err(error))));
                        }
                    };
                    self.state = State::App(app);
                    command.map(Message::Run)
                }
            }
            _ => Task::none(),
        };
        self.sync_theme_mode();
        // Only if the App is gone: a refused Set up Vault keeps it, and its
        // field still saves as usual.
        let nickname_flush = nickname_flush
            .filter(|_| !matches!(self.state, State::App(_)))
            .map(|write| {
                Task::perform(write.run(), |result| {
                    if let Err(error) = result {
                        log::error!("Couldn't save the Spark nickname: {}", error);
                    }
                })
                .discard()
            })
            .unwrap_or_else(Task::none);
        // After the transition, not before: this is the point where the arm
        // above has already decided which state the tab is in, so a warning
        // parked on the way to `Loader` is released by whichever later update
        // finally reaches `App`.
        Task::batch([result, self.flush_migration_warning(), nickname_flush])
    }

    pub fn subscription(&self, foreground: bool) -> Subscription<Message> {
        let base = match &self.state {
            State::Installer(v) => v.subscription().map(Message::Install),
            State::Loader(v) => v.subscription().map(Message::Load),
            // `Subscription::map` requires a non-capturing closure, so the
            // generation travels through `with` instead of being captured.
            // `with` folds the value into the subscription's identity, which is
            // safe here: the generation only ever changes as this arm stops
            // being selected — `invalidate_fork_session` swaps the state to
            // `Home`, and `update` bumps it only once the state is no longer a
            // fork `App`. So it cannot restart a live subscription mid-session.
            State::App(v) if v.cube_settings().network.is_blake2b() => v
                .subscription()
                .with(self.fork_session_generation)
                .map(|(generation, message)| {
                    Message::ForkAsync(generation, Box::new(Message::Run(message)))
                }),
            State::App(v) => {
                v.subscription()
                    .with(self.app_generation)
                    .map(|(generation, message)| {
                        Message::AppAsync(generation, Box::new(Message::Run(message)))
                    })
            }
            State::Home(v) => v.subscription(foreground).map(Message::Launch),
            State::PinEntry(_) => Subscription::none(),
            // Unlike `PinEntry`, this one needs a subscription: the macOS
            // authorization delegate answers on a channel with no waker to
            // hook, so the ceremony has to be polled.
            State::PasskeyUnlock(v) => v.subscription().map(Message::PasskeyUnlock),
            State::DuressActive(_) => Subscription::none(),
        };
        if foreground
            && self.unlock_motion.is_some()
            && matches!(self.state, State::PinEntry(_) | State::PasskeyUnlock(_))
        {
            Subscription::batch([base, iced::window::frames().map(Message::UnlockMotionTick)])
        } else if let (true, Some(opening)) = (foreground, self.opening.as_ref()) {
            let now = Instant::now();
            let clock = if opening.scene.needs_frames(now) || opening.quotes.needs_frames(now) {
                iced::window::frames()
            } else if opening.scene.breathes() {
                iced::time::every(std::time::Duration::from_millis(50))
            } else {
                // Still the stall wait to count.
                iced::time::every(std::time::Duration::from_secs(1))
            };
            Subscription::batch([base, clock.map(Message::OpeningTick)])
        } else {
            base
        }
    }

    pub(crate) fn view(
        &self,
        worldview: crate::app::settings::preferences::WorldviewMode,
    ) -> Element<Message> {
        let Some(opening) = self.opening.as_ref() else {
            return self.state_view(worldview);
        };
        let now = Instant::now();
        let (cube, app) = match &self.state {
            State::PinEntry(pin) if pin.is_loading() => (pin.cube(), false),
            State::PasskeyUnlock(passkey) if passkey.is_opening() => (passkey.cube(), false),
            State::Loader(loader) if loader.opening_stage().is_some() => {
                (&loader.cube_settings, false)
            }
            State::App(app) => (app.cube_settings(), opening.scene.shows_app(now)),
            _ => return self.state_view(worldview),
        };
        let base = if app {
            self.state_view(worldview)
        } else {
            let face = home::launcher_presenter::OpeningFace {
                cube,
                quote: opening.quotes.current(),
                outgoing: opening.quotes.outgoing(now),
                stage: opening.scene.stage(),
                stalled: opening.scene.stalled(now),
                cancel: || Message::CancelOpening,
                without_spark: || Message::OpenWithoutSpark,
            };
            match opening.scene.floor() {
                Some(floor) => home::launcher_presenter::opening_on_floor(
                    floor,
                    face,
                    self.theme_mode,
                    &opening.scene,
                    now,
                ),
                None => home::launcher_presenter::opening_full_screen(
                    face,
                    self.theme_mode,
                    &opening.scene,
                    now,
                ),
            }
        };
        let wash = opening.scene.wash(now);
        if wash > 0.0 {
            iced::widget::Stack::new()
                .push(base)
                .push(home::launcher_presenter::opening_wash(cube.network, wash))
                .into()
        } else {
            base
        }
    }

    fn state_view(
        &self,
        worldview: crate::app::settings::preferences::WorldviewMode,
    ) -> Element<Message> {
        match &self.state {
            State::Installer(v) => v.view().map(Message::Install),
            State::App(v) if v.cube_settings().network.is_blake2b() => {
                let generation = self.fork_session_generation;
                v.view().map(move |message| {
                    Message::ForkAsync(generation, Box::new(Message::Run(message)))
                })
            }
            State::App(v) => {
                let generation = self.app_generation;
                v.view().map(move |message| {
                    Message::AppAsync(generation, Box::new(Message::Run(message)))
                })
            }
            State::Home(v) => v.view_with_worldview(worldview).map(Message::Launch),
            State::Loader(v) => v.view().map(Message::Load),
            State::PinEntry(v) => {
                let progress = self
                    .unlock_motion
                    .map(|started| {
                        home::launcher_presenter::unlock_turn_progress(started, Instant::now())
                    })
                    .filter(|progress| *progress < 1.0);
                if let Some(floor) = self.unlock_floor.as_ref() {
                    home::launcher_presenter::unlock_turn_with_floor_controls(
                        v.cube(),
                        self.theme_mode,
                        floor,
                        progress,
                        v.view_with_theme_and_floor(self.theme_mode, Some(floor))
                            .map(Message::PinEntry),
                    )
                } else {
                    home::launcher_presenter::unlock_turn_with_controls(
                        v.cube(),
                        self.theme_mode,
                        progress,
                        v.view_with_theme(self.theme_mode).map(Message::PinEntry),
                    )
                }
            }
            State::PasskeyUnlock(v) => {
                let progress = self
                    .unlock_motion
                    .map(|started| {
                        home::launcher_presenter::unlock_turn_progress(started, Instant::now())
                    })
                    .filter(|progress| *progress < 1.0);
                if let Some(floor) = self.unlock_floor.as_ref() {
                    home::launcher_presenter::unlock_turn_with_floor_controls(
                        v.cube(),
                        self.theme_mode,
                        floor,
                        progress,
                        v.view_with_theme_and_floor(self.theme_mode, Some(floor))
                            .map(Message::PasskeyUnlock),
                    )
                } else {
                    home::launcher_presenter::unlock_turn_with_controls(
                        v.cube(),
                        self.theme_mode,
                        progress,
                        v.view_with_theme(self.theme_mode)
                            .map(Message::PasskeyUnlock),
                    )
                }
            }
            State::DuressActive(v) => v.view().map(Message::Duress),
        }
    }

    /// A tab still on its way into a Cube (unlock or loading) may be the
    /// open an armed Split handoff is waiting for.
    pub fn is_opening_cube(&self) -> bool {
        matches!(
            self.state,
            State::Loader(_) | State::PinEntry(_) | State::PasskeyUnlock(_)
        )
    }

    /// The opening's stage for the current state. `None` ends the opening;
    /// `Some(None)` keeps it but has no stage (a loader error page).
    fn opening_stage(&self) -> Option<Option<home::launcher_opening::Stage>> {
        use home::launcher_opening::Stage;
        // The unlock routes past its screen (to the Loader, or back to the
        // Launcher for a legacy Liana Connect Vault) exactly when it carries Bitcoin wallet settings: a
        // Vault or an imported wallet. Without them there is no daemon to
        // start: the wallets load and the Cube is almost there.
        let unlocked = |on_success: &crate::pin_entry::PinEntrySuccess| {
            let crate::pin_entry::PinEntrySuccess::LoadApp {
                wallet_settings, ..
            } = on_success;
            Some(if wallet_settings.is_some() {
                Stage::Connecting
            } else {
                Stage::AlmostThere
            })
        };
        match &self.state {
            State::PinEntry(pin) if pin.is_loading() => Some(unlocked(&pin.on_success)),
            State::PasskeyUnlock(passkey) if passkey.is_opening() => {
                Some(unlocked(&passkey.on_success))
            }
            State::Loader(loader) => Some(loader.opening_stage()),
            // The App is up, but its Spark wallet may still be loading:
            // the face waits for it unless the user goes on without it.
            State::App(app)
                if !app.spark_settled()
                    && !self.opening.as_ref().is_some_and(|o| o.without_spark) =>
            {
                Some(Some(Stage::Spark))
            }
            State::App(_) => Some(Some(Stage::Ready)),
            _ => None,
        }
    }

    /// Start the opening once an unlock is verified, keep its abort handles
    /// while the unlock screen still owns the load, and follow its stage.
    fn track_opening(&mut self, pin_verified: bool, result: Task<Message>) -> Task<Message> {
        let now = Instant::now();
        // Every unlock reads the motion preference afresh, whichever route
        // (Launcher, auto-lock, a new Cube) brought it here.
        let datadir = |on_success: &crate::pin_entry::PinEntrySuccess| {
            let crate::pin_entry::PinEntrySuccess::LoadApp { datadir, .. } = on_success;
            datadir.clone()
        };
        let verified = match &self.state {
            State::PinEntry(pin) if pin_verified && pin.is_loading() => {
                Some((pin.loading_quote().clone(), datadir(&pin.on_success)))
            }
            State::PasskeyUnlock(passkey) if passkey.is_opening() => Some((
                passkey.loading_quote().clone(),
                datadir(&passkey.on_success),
            )),
            _ => None,
        };
        if let (None, Some((quote, root))) = (self.opening.as_ref(), verified) {
            if let Some(Some(stage)) = self.opening_stage() {
                self.opening_cancelled = false;
                let reduced = home::motion_reduced(&root, self.system_reduces_motion);
                self.opening = Some(CubeOpening {
                    scene: home::launcher_opening::Opening::new(
                        self.unlock_floor.take(),
                        reduced,
                        stage,
                        now,
                    ),
                    quotes: home::launcher_opening::QuoteCycle::new(quote, reduced, now),
                    loads: Vec::new(),
                    without_spark: false,
                });
            }
        }
        let preloader = matches!(&self.state, State::PinEntry(pin) if pin.is_loading())
            || matches!(&self.state, State::PasskeyUnlock(passkey) if passkey.is_opening());
        let result = match self.opening.as_mut() {
            Some(opening) if preloader && result.units() > 0 => {
                let (task, handle) = result.abortable();
                opening.loads.push(handle);
                task
            }
            _ => result,
        };
        self.sync_opening(now);
        result
    }

    fn sync_opening(&mut self, now: Instant) {
        let stage = self.opening_stage();
        let Some(opening) = self.opening.as_mut() else {
            return;
        };
        match stage {
            None => self.opening = None,
            Some(stage) => {
                match stage {
                    Some(stage) => opening.scene.set_stage(stage, now),
                    // The error page owns the screen; a Retry starts the
                    // stall wait from when it leaves.
                    None => opening.scene.hold(now),
                }
                opening.quotes.advance(now);
                if !matches!(self.state, State::PinEntry(_) | State::PasskeyUnlock(_)) {
                    // The Loader or App owns its work now; Cancel uses its routes.
                    opening.loads.clear();
                }
                if opening.scene.done(now) {
                    self.opening = None;
                }
            }
        }
    }

    /// Cancel from a stalled loading face. Before the Loader, the unlock's
    /// own loads are aborted and its Back route taken; in the Loader, its
    /// Back. Either way the session for this Cube is closed: it is locked.
    fn cancel_opening(&mut self) -> Task<Message> {
        // A press that lands after the Cube is ready changes nothing: the
        // opening carries on to its finale.
        let cube_id = match &self.state {
            State::PinEntry(pin) => pin.cube().id.clone(),
            State::PasskeyUnlock(passkey) => passkey.cube().id.clone(),
            State::Loader(loader) => loader.cube_settings.id.clone(),
            _ => return Task::none(),
        };
        let Some(opening) = self.opening.take() else {
            return Task::none();
        };
        for load in opening.loads {
            load.abort();
        }
        let back = match &self.state {
            State::PinEntry(_) => {
                self.opening_cancelled = true;
                Message::PinEntry(crate::pin_entry::Message::Back)
            }
            State::PasskeyUnlock(_) => {
                self.opening_cancelled = true;
                Message::PasskeyUnlock(crate::passkey_unlock::Message::Back)
            }
            _ => Message::Load(loader::Message::View(loader::ViewMessage::SwitchNetwork)),
        };
        let task = self.update_inner(back);
        app::session::close_cube(&cube_id);
        task
    }

    pub fn stop(&mut self) {
        self.stop_with(false);
    }

    /// [`Self::stop`] for locking: an App's or Loader's daemon stops on its
    /// own thread instead of blocking the UI.
    pub fn stop_in_background(&mut self) {
        self.stop_with(true);
    }

    fn stop_with(&mut self, in_background: bool) {
        self.copy_owner = None;
        self.unlock_motion = None;
        self.unlock_floor = None;
        self.opening = None;
        match &mut self.state {
            State::Loader(s) if in_background => s.stop_in_background(),
            State::Loader(s) => s.stop(),
            State::Home(s) => s.stop(),
            State::Installer(s) => s.stop(),
            State::App(s) if in_background => s.stop_in_background(),
            State::App(s) => s.stop(),
            State::PinEntry(_) => {}
            State::PasskeyUnlock(_) => {}
            State::DuressActive(_) => {}
        }
    }
}

/// Apply only the Cube changes selected by the installer. Each touched record
/// must still match its read snapshot (or already match the desired result).
/// Unrelated concurrent updates, wallet records and preferences remain intact.
fn merge_fork_cube_changes(
    mut latest: app::settings::Settings,
    expected: &app::settings::Settings,
    desired: &app::settings::Settings,
) -> Result<app::settings::Settings, app::settings::SettingsError> {
    let ids: std::collections::HashSet<_> = expected
        .cubes
        .iter()
        .chain(&desired.cubes)
        .map(|cube| cube.id.as_str())
        .collect();
    for id in ids {
        let before = expected.cubes.iter().find(|cube| cube.id == id);
        let after = desired.cubes.iter().find(|cube| cube.id == id);
        let current = latest.cubes.iter().find(|cube| cube.id == id);
        let encode = |cube: Option<&app::settings::CubeSettings>| {
            serde_json::to_value(cube)
                .map_err(|e| app::settings::SettingsError::WritingFile(e.to_string()))
        };
        let before_value = encode(before)?;
        let after_value = encode(after)?;
        if before_value == after_value {
            continue;
        }
        let current_value = encode(current)?;
        if current_value == after_value {
            continue;
        }
        if current_value != before_value {
            return Err(app::settings::SettingsError::WritingFile(
                "Cube settings changed during installation; reopen and retry without overwriting them".into()));
        }
        match (latest.cubes.iter().position(|cube| cube.id == id), after) {
            (Some(index), Some(cube)) => latest.cubes[index] = cube.clone(),
            (Some(index), None) => {
                latest.cubes.remove(index);
            }
            (None, Some(cube)) => latest.cubes.push(cube.clone()),
            (None, None) => (),
        }
    }
    Ok(latest)
}

async fn save_cube_settings(
    network_dir: &NetworkDirectory,
    cube: app::settings::CubeSettings,
    network: crate::chain::ChainId,
    settings_data: app::settings::Settings,
    expected: app::settings::Settings,
) -> Result<app::settings::CubeSettings, String> {
    let cube_name = cube.name.clone();
    let settings_path = network_dir.path().join("settings.json");

    let save_result = if network.is_blake2b() {
        app::settings::update_fork_settings_checked(network_dir, move |latest| {
            merge_fork_cube_changes(latest, &expected, &settings_data).map(Some)
        })
        .await
    } else {
        update_settings_file(network_dir, |_| Some(settings_data)).await
    };

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
pub(crate) struct RestoreCubeSeed {
    pub(crate) pin: zeroize::Zeroizing<String>,
    pub(crate) master_signer_fingerprint: bitcoin::bip32::Fingerprint,
    pub(crate) seed_backed_up: bool,
    /// For a Bitcoin Blake2b copy of a passkey Cube: the source's passkey,
    /// which the copy unlocks with. Its seed file was written under the
    /// password derived from that passkey's seed, and `pin` is empty and
    /// never read: a copy is a Blake2b Cube, and every use of `pin` at the
    /// exit seam is on a Bitcoin-family Cube.
    pub(crate) passkey: Option<app::settings::PasskeyMetadata>,
}

/// The deleted Cube's original identity, carried out of the decrypted
/// kit/envelope by the restore steps (via `ctx.cube_id` / `ctx.cube_name`).
/// Both fields are always present together: a restore either knows the full
/// original identity or it isn't a seed restore at all — which is why
/// `find_or_create_cube` takes `Option<RestoreCubeIdentity>` rather than a
/// struct of `Option`s.
/// The Cube identity the installer's exit seam mints from.
///
/// `find_or_create_cube` falls back to a **fresh UUID** when this is `None`, so
/// any flow that minted its own `cube_id` — and bound a seed file to it — must
/// be recognised here or the Cube that lands is one nothing can open.
///
/// `None` when a Cube shell already exists locally (`cube_settings.is_some()`,
/// e.g. AddWallet inside a Cube): there the `originating_cube_id` path owns the
/// association and `context.cube_id` is that existing Cube's identity.
pub(crate) fn installer_exit_identity(i: &Installer) -> Option<RestoreCubeIdentity> {
    if i.cube_settings.is_some() {
        return None;
    }
    i.context
        .cube_id
        .clone()
        .zip(i.context.cube_name.clone())
        .map(|(uuid, name)| RestoreCubeIdentity { uuid, name })
}

/// Reopen the source through its normal Vault loader.
/// This preserves the source's wallet identity and parked SDK handles.
fn reopen_claim_source(
    datadir: &CoincubeDirectory,
    source: &installer::SourceCube,
) -> Result<(State, Task<Message>), String> {
    let cfg = app::Config::from_file(
        &datadir
            .network_directory(source.settings.network)
            .path()
            .join(app::config::DEFAULT_FILE_NAME),
    )
    .map_err(|error| format!("Couldn’t reopen your Bitcoin Cube: {error}. Restore its gui.toml configuration, then try returning again."))?;
    // The same lookup the ordinary open path uses (`:1043`),
    // and for the same reason: `WalletId` is the checksum
    // *and* the timestamp, so matching on the checksum alone
    // can select a different Vault of the same descriptor —
    // a re-created one, or a pinned sibling. The return must
    // restore the Vault the Cube actually points at.
    let wallet_settings =
        vault_settings_for_cube(datadir, source.settings.network, &source.settings);
    // A legacy Liana Connect source has no local daemon to start, and
    // handing it to the Loader would try to bring up a daemon for a Vault
    // that lives on that retired backend: go back to the Launcher instead.
    if let Some(auth) = wallet_settings
        .as_ref()
        .and_then(|w| w.remote_backend_auth.as_ref())
    {
        let (mut home, command) =
            Home::new_for_chain(datadir.clone(), Some(source.settings.network));
        home.set_error(auth.retired_backend_notice());
        return Ok((State::Home(home), command.map(Message::Launch)));
    }
    let (loader, command) = Loader::new(
        datadir.clone(),
        cfg,
        source.settings.network.bitcoin_network(),
        None,
        None,
        wallet_settings,
        source.settings.clone(),
        source.breez_client.clone(),
        source.spark_backend.clone(),
    );
    Ok((State::Loader(loader), command.map(Message::Load)))
}

/// The seed credentials the freshly minted Cube records: which master signer
/// its seed file belongs to, the PIN that file was written under, and whether
/// that seed is backed up.
///
/// Two flows reach this seam and each answers from a different place — a
/// Recovery-Kit restore from `restore_pin` + `recovered_signer`, and a Bitcoin
/// Blake2b **claim** from the source Cube it reuses. Vault creation never
/// mints a Cube here: it runs inside an existing one. A flow missing here
/// mints a Cube with no master signer recorded.
pub(crate) fn installer_exit_seed(i: &Installer) -> Option<RestoreCubeSeed> {
    if let Some(source) = &i.context.claim_source {
        // The claim target's seed file is the source Cube's mnemonic, written
        // under the source Cube's PIN (`Context::cube_pin`, resolved from the
        // source session at admission) — not a `restore_pin`, because nothing
        // was restored and no PIN was chosen.
        if let Some(passkey) = source.cube.passkey_metadata.clone() {
            return i
                .context
                .passkey_seed_password
                .as_ref()
                .map(|_| RestoreCubeSeed {
                    pin: zeroize::Zeroizing::new(String::new()),
                    master_signer_fingerprint: source.master_signer_fingerprint(),
                    seed_backed_up: source.seed_backed_up(),
                    passkey: Some(passkey),
                });
        }
        return i.context.cube_pin.clone().map(|pin| RestoreCubeSeed {
            pin,
            master_signer_fingerprint: source.master_signer_fingerprint(),
            seed_backed_up: source.seed_backed_up(),
            passkey: None,
        });
    }
    match (
        i.context.restore_pin.clone(),
        i.context.recovered_signer.as_ref().map(|s| s.fingerprint()),
    ) {
        (Some(pin), Some(fp)) => Some(RestoreCubeSeed {
            pin,
            master_signer_fingerprint: fp,
            seed_backed_up: false,
            passkey: None,
        }),
        _ => None,
    }
}

pub(crate) struct RestoreCubeIdentity {
    /// Original UUID, preserved verbatim (see `CubeSettings::new_with_raw_id`).
    pub(crate) uuid: String,
    /// Original display name, so the revived Cube doesn't inherit the
    /// wallet-alias default.
    pub(crate) name: String,
}

/// Attach `vault` to `cube`, or leave the Cube vaultless when the installer
/// exited without one. The `Option` mirrors the old `vault_wallet_id =
/// wallet_id.cloned()` assignment, minus the chance of writing one half of the
/// identity without the other.
fn attach_vault(cube: &mut app::settings::CubeSettings, vault: Option<&VaultIdentity>) {
    match vault {
        Some(v) => cube.set_vault(v.clone()),
        None => cube.clear_vault(),
    }
}

/// `settings` with this Cube recorded as having a Recovery Kit, and everything
/// else untouched.
///
/// Returns `Settings`, not `Option<Settings>`, on purpose — see
/// [`crate::app::cleared_pending_rescan`] for the hazard. An updater that
/// returns `None` makes [`app::settings::update_settings_file`] **delete**
/// `settings.json`, so a Cube id matching nothing must still yield the settings
/// unchanged rather than a `None` that wipes every Cube on this network.
fn marked_kit_backed_up(
    mut settings: app::settings::Settings,
    cube_id: &str,
) -> app::settings::Settings {
    if let Some(cube) = settings.cubes.iter_mut().find(|c| c.id == cube_id) {
        cube.recovery_kit_password_backed_up = true;
    }
    settings
}

/// Resume an imported Kit restore after its wallet was already attached but a
/// later installer step failed. The generic restore path deliberately refuses
/// every Cube with an import; only this exact wallet/Cube/seed match may reuse
/// that attachment and retry the journal's idempotent completion.
#[allow(clippy::too_many_arguments)]
async fn find_or_create_cube_for_imported_restore(
    network_dir: &NetworkDirectory,
    vault: Option<&VaultIdentity>,
    wallet_alias: &Option<String>,
    chain: crate::chain::ChainId,
    originating_cube_id: Option<String>,
    restored_cube: Option<RestoreCubeIdentity>,
    restore_seed: Option<&RestoreCubeSeed>,
    blob: &crate::services::recovery::ImportedDescriptorBlob,
) -> Result<app::settings::CubeSettings, String> {
    blob.validate(chain)?;
    if vault.is_some() {
        return Err("An imported Bitcoin restore cannot also attach a Vault.".into());
    }
    let expected = app::settings::ImportedWalletAttachment::new(
        blob.wallet.descriptor.clone(),
        chain,
        blob.wallet.birthday,
        blob.wallet.initial_lookahead,
    )?;
    let target_id = restored_cube
        .as_ref()
        .map(|identity| identity.uuid.as_str())
        .or(originating_cube_id.as_deref())
        .ok_or("The imported wallet restore has no original Cube identity.")?;
    if target_id != blob.cube.uuid {
        return Err("This imported wallet backup belongs to a different Cube.".into());
    }
    if let Ok(settings) = app::settings::Settings::from_file(network_dir) {
        if let Some(cube) = settings.cubes.iter().find(|cube| cube.id == target_id) {
            if let Some(intent) = &cube.bitcoin_import {
                if cube.network != chain
                    || cube.vault_wallet_id.is_some()
                    || intent.active_attachment() != Some(&expected)
                    || (restored_cube.is_some()
                        && !restore_seed.is_some_and(|seed| {
                            cube.master_signer_fingerprint == Some(seed.master_signer_fingerprint)
                        }))
                {
                    return Err("This Cube already has a different or unfinished Bitcoin wallet import. Resume that import before retrying recovery.".into());
                }
                return Ok(cube.clone());
            }
        }
    }
    find_or_create_cube(
        network_dir,
        vault,
        wallet_alias,
        chain,
        originating_cube_id,
        restored_cube,
        restore_seed,
    )
    .await
}

pub(crate) async fn find_or_create_cube<C: Into<crate::chain::ChainId>>(
    network_dir: &NetworkDirectory,
    // Both halves of the Vault's identity, or `None` when the installer exited
    // without one. Deliberately not a bare `WalletId`: every attach site below
    // has to write the fingerprint too, and a two-argument signature is how
    // one of them ends up not doing so
    // (`plans/PLAN-vault-identity-unification.md` D1).
    vault: Option<&VaultIdentity>,
    wallet_alias: &Option<String>,
    network: C,
    originating_cube_id: Option<String>,
    // Original Cube identity for a Recovery-Kit *seed* restore. When present,
    // the restored Cube reuses the deleted Cube's UUID so the Connect
    // `register_cube` call (idempotent on UUID) reactivates it rather than
    // creating a duplicate. `None` for every non-restore flow.
    restored_cube: Option<RestoreCubeIdentity>,
    restore_seed: Option<&RestoreCubeSeed>,
) -> Result<app::settings::CubeSettings, String> {
    let network = network.into();
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
                cube.backed_up |= seed.seed_backed_up;
                if let Some(passkey) = &seed.passkey {
                    cube = cube.with_passkey(passkey.clone());
                }
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
            let expected = settings_data.clone();
            if settings_data
                .cubes
                .iter()
                .any(|cube| cube.network != network)
            {
                return Err("Cube settings contain a different chain identity".into());
            }
            // First, check if a cube already has this wallet.
            // We don't decorate existing cubes with the restore PIN —
            // if the cube already has a PIN hash / fingerprint those
            // are its source of truth. The restore flow only overwrites
            // Cube-level credentials when we're actually minting a new
            // Cube for the restored wallet.
            if let Some(w_id) = vault.map(|v| &v.wallet_id) {
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
                    if settings_data.cubes[idx].bitcoin_import.is_some() {
                        return Err(
                            "The restored Cube is reserved for a Bitcoin wallet import.".into()
                        );
                    }
                    if settings_data.cubes[idx].vault_wallet_id.is_some() {
                        return Err(format!(
                            "Cube '{}' has already been recovered on this device.",
                            settings_data.cubes[idx].name
                        ));
                    }
                    let mut cube = settings_data.cubes[idx].clone();
                    attach_vault(&mut cube, vault);
                    let cube = decorate_new(cube)?;
                    settings_data.cubes[idx] = cube.clone();

                    info!(
                        "Reactivating recovered cube '{}' ({}) with wallet {:?} on {} network",
                        cube.name, uuid, cube.vault_wallet_id, network
                    );

                    return save_cube_settings(network_dir, cube, network, settings_data, expected)
                        .await;
                }

                let mut base_cube = new_cube_base();
                attach_vault(&mut base_cube, vault);
                let cube = decorate_new(base_cube)?;

                info!(
                    "Re-minting recovered cube '{}' ({}) for wallet {:?} on {} network",
                    cube.name, uuid, cube.vault_wallet_id, network
                );

                settings_data.cubes.push(cube.clone());
                return save_cube_settings(network_dir, cube, network, settings_data, expected)
                    .await;
            }

            // Second, if we have an originating cube ID, validate and use it
            if let Some(target_cube_id) = originating_cube_id {
                if let Some(target_cube) = settings_data
                    .cubes
                    .iter_mut()
                    .find(|c| c.id == target_cube_id)
                {
                    if let Some(v) = vault {
                        if target_cube.bitcoin_import.is_some() {
                            return Err(format!(
                                "Cube '{}' is reserved for an imported Bitcoin wallet. Resume or cancel that import before creating a Vault.",
                                target_cube.name
                            ));
                        }
                        if target_cube.vault_wallet_id.is_some() {
                            return Err(format!(
                                "Cube '{}' already has a vault. Remove the existing vault before creating a new one.",
                                target_cube.name
                            ));
                        }
                        target_cube.set_vault(v.clone());
                    }
                    // Apply restore-flow credentials (PIN hash + fingerprint) if
                    // restoring to this cube — same rationale as the empty-cube
                    // fallback: the hash must match the newly-encrypted mnemonic.
                    let cube_clone = decorate_new(target_cube.clone())?;
                    *target_cube = cube_clone.clone();
                    let cube_name = target_cube.name.clone();

                    info!(
                        "Associating wallet {:?} with originating cube '{}' on {} network",
                        cube_clone.vault_wallet_id, cube_name, network
                    );

                    return save_cube_settings(
                        network_dir,
                        cube_clone,
                        network,
                        settings_data,
                        expected,
                    )
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
                .position(|c| c.vault_wallet_id.is_none() && c.bitcoin_import.is_none())
            {
                let mut empty_cube = settings_data.cubes[empty_idx].clone();
                attach_vault(&mut empty_cube, vault);
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
                    empty_cube.vault_wallet_id, cube_name, network
                );

                return save_cube_settings(
                    network_dir,
                    empty_cube,
                    network,
                    settings_data,
                    expected,
                )
                .await;
            }

            // Finally, create a new cube for this wallet. `restored_cube` is
            // `None` here (the restore branch above returns early), so
            // `new_cube_base` yields the alias-based fresh-UUID cube.
            let mut base_cube = new_cube_base();
            attach_vault(&mut base_cube, vault);
            let cube = decorate_new(base_cube)?;
            let cube_name = cube.name.clone();

            info!(
                "Creating new cube '{}' for wallet {:?} on {} network",
                cube_name, cube.vault_wallet_id, network
            );

            settings_data.cubes.push(cube.clone());
            save_cube_settings(network_dir, cube, network, settings_data, expected).await
        }
        Err(_) => {
            // No settings file yet, create first cube. On the restore path
            // `new_cube_base` reuses the deleted Cube's original UUID + name.
            let mut base_cube = new_cube_base();
            attach_vault(&mut base_cube, vault);
            let cube = decorate_new(base_cube)?;
            let cube_name = cube.name.clone();

            info!(
                "Creating first cube '{}' for wallet {:?} on {} network",
                cube_name, cube.vault_wallet_id, network
            );

            let mut new_settings = app::settings::Settings::default();
            new_settings.cubes.push(cube.clone());

            save_cube_settings(
                network_dir,
                cube,
                network,
                new_settings,
                app::settings::Settings::default(),
            )
            .await
        }
    }
}

/// The client the unlock handler falls back to when the Liquid load fails.
///
/// Every feature that hangs off the master seed — the Connect encryption key
/// ([`app::App::new`] derives it from the client's signer), Spark, P2P — only
/// needs the *signer*, not a connected SDK. A load that resolved the seed and
/// then failed to reach Liquid must therefore not degrade to a signer-less
/// client: that would leave `Cache::cube_encryption_key` unset and every
/// blinded Connect key (the phone recovery key included) failing as `Locked`
/// with advice to "restore from seed" on a Cube that has its seed on disk.
///
/// The unlock parked its decrypted signer in the session (both the PIN and the
/// passkey paths do), so it is recovered from there. A signer-less client is
/// only produced when the session holds none — a seed-less Cube, or a session
/// closed since the unlock — which is exactly the case it was meant for.
fn disconnected_breez_client_after_unlock(
    cube: &app::settings::CubeSettings,
    network: bitcoin::Network,
) -> Arc<app::breez_liquid::BreezClient> {
    let signer = cube
        .master_signer_fingerprint
        .and_then(|fp| crate::app::session::unlocked_signer(&cube.id, fp));
    match signer {
        Some(signer) => Arc::new(app::breez_liquid::BreezClient::disconnected_with_signer(
            network,
            Arc::new(std::sync::Mutex::new(signer)),
        )),
        None => {
            tracing::warn!(
                "No unlocked master signer for Cube {} in the session; Liquid fallback client \
                 carries no signer",
                cube.id
            );
            Arc::new(app::breez_liquid::BreezClient::disconnected(network))
        }
    }
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
        touch(
            &net.join("imported-local-keys")
                .join("operation-a")
                .join("record.key"),
        );
        touch(&net.join("settings.json"));
        touch(&net.join("connect.json"));
        touch(&root.join("duress-state.json"));
        // A second network is covered too.
        touch(&root.join("testnet").join("mnemonics").join("seed"));
        touch(
            &root
                .join("testnet")
                .join("imported-local-keys")
                .join("operation-b")
                .join("manifest.json"),
        );
        touch(&net.join("mcp-pairings").join("fixture").join("clients.enc"));
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
            targets.contains(&net.join("mcp-pairings")),
            "durable client authorization must be wiped"
        );
        assert!(
            targets.contains(&net.join("mnemonics")),
            "mnemonics/ (seeds) must be wiped"
        );
        assert!(
            targets.contains(&net.join("imported-local-keys")),
            "imported independent keys and inventory must be wiped"
        );
        assert!(
            targets.contains(&net.join("settings.json")),
            "settings.json (PIN hashes) must be wiped"
        );
        assert!(
            targets.contains(&root.join("testnet").join("mnemonics")),
            "every network's seeds must be wiped"
        );
        assert!(
            targets.contains(&root.join("testnet").join("imported-local-keys")),
            "every network's imported keys must be wiped"
        );
        // Cached Connect auth is preserved.
        assert!(
            !targets.iter().any(|t| t.ends_with("connect.json")),
            "connect.json (cached auth) must survive"
        );
        assert!(
            !targets.iter().any(|t| t.ends_with("duress-state.json")),
            "root-level duress state must survive for activation recovery"
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
            !targets.contains(&net.join("imported-local-keys")),
            "absent imported-local-keys/ must not be targeted"
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

    // ── The Cube open gate (BTCB2 plan PR 2, dormant slice) ─────────────────

    /// Drive `home::Message::Run` for `cube` found under `found_in` and report
    /// where the tab ended up plus the home error, if it stayed home.
    fn open_from_home(
        cube: app::settings::CubeSettings,
        found_in: crate::chain::ChainId,
    ) -> (&'static str, Option<String>) {
        let mut tab = non_app_tab();
        let datadir = match &tab.state {
            State::Home(home) => home.datadir_path.clone(),
            _ => unreachable!(),
        };
        let _ = tab.update(Message::Launch(home::Message::Run(
            datadir,
            app::config::Config::new(false),
            found_in,
            cube,
        )));
        match &tab.state {
            State::Home(home) => ("Home", home.error().map(|e| e.to_string())),
            State::PinEntry(_) => ("PinEntry", None),
            State::PasskeyUnlock(_) => ("PasskeyUnlock", None),
            State::Loader(_) => ("Loader", None),
            State::App(_) => ("App", None),
            _ => ("other", None),
        }
    }

    #[test]
    fn refused_home_open_discards_any_prior_floor_snapshot() {
        let mut tab = non_app_tab();
        tab.unlock_floor = Some(home::LauncherSceneContext {
            selected: home::CubeCatalogKey {
                chain: crate::chain::ChainId::Bitcoin,
                cube_id: "prior".into(),
            },
            page: 0,
            selected_slot: 0,
            backup: "Status unknown",
            connect: String::new(),
            wallets: "Not yet known".into(),
            peers: Vec::new(),
            pages: 1,
            chrome: Default::default(),
        });
        let datadir = match &tab.state {
            State::Home(home) => home.datadir_path.clone(),
            _ => unreachable!(),
        };
        let cube = cube_on(crate::chain::ChainId::Bitcoin);
        drop(tab.update(Message::Launch(home::Message::Run(
            datadir,
            app::config::Config::new(false),
            crate::chain::ChainId::Signet,
            cube,
        ))));
        assert!(matches!(tab.state, State::Home(_)));
        assert!(tab.unlock_floor.is_none());
        assert!(tab.unlock_motion.is_none());
    }

    fn cube_on(chain: crate::chain::ChainId) -> app::settings::CubeSettings {
        let mut cube = app::settings::CubeSettings::new("Cube".to_string(), chain);
        // Predates the creation-backup gate, so that gate never interferes
        // with what these tests are about.
        cube.created_at = 0;
        cube
    }

    /// A persisted Bitcoin Blake2b Cube is refused at the open gate: the tab
    /// stays on Home, and no unlock screen — the step that loads the Liquid
    /// SDK and reads seed files — is ever built. The fork route is admitted
    /// only through an authenticated Connect account whose feature flag is
    /// on; this fixture has neither, so the refusal names the account gate
    /// (the same reason `home::a_dormant_chain_record_in_its_own_directory_is_refused_for_the_runtime`
    /// pins), not the generic dormant-runtime copy.
    #[test]
    fn a_blake2b_cube_is_refused_before_any_unlock_screen() {
        for chain in [
            crate::chain::ChainId::BitcoinBlake2b,
            crate::chain::ChainId::BitcoinBlake2bTestnet4,
        ] {
            let (state, error) = open_from_home(cube_on(chain), chain);
            assert_eq!(state, "Home", "{:?}", chain);
            assert_eq!(
                error.as_deref(),
                Some("Bitcoin Blake2b is unavailable. Enable its beta preference in Global Settings and verify feature availability."),
                "{:?}",
                chain
            );
        }
    }

    /// The gate is not "is this Bitcoin": a Bitcoin-family Cube still goes to
    /// its unlock screen (the PIN keypad here — the fixture has no seed file,
    /// which is a later, different refusal).
    #[test]
    fn a_bitcoin_family_cube_still_reaches_its_unlock_screen() {
        let (state, error) = open_from_home(
            cube_on(crate::chain::ChainId::Signet),
            crate::chain::ChainId::Signet,
        );
        assert_eq!(state, "PinEntry");
        assert_eq!(error, None);
    }

    /// Opening a passkey Cube from the Launcher raises the system prompt by
    /// itself: the open task carries `Begin`, so the user is not left on a
    /// face whose only job is to be pressed again. The task is inspected, not
    /// run, so no real prompt is raised here.
    #[test]
    fn a_launcher_opened_passkey_cube_raises_its_prompt_at_once() {
        use iced_runtime::futures::futures::StreamExt;

        let cube =
            cube_on(crate::chain::ChainId::Signet).with_passkey(app::settings::PasskeyMetadata {
                credential_id: "Y3JlZC1pZA==".to_string(),
                rp_id: "coincube.io".to_string(),
                created_at: 1_786_122_245,
                label: None,
            });
        let mut tab = non_app_tab();
        let datadir = match &tab.state {
            State::Home(home) => home.datadir_path.clone(),
            _ => unreachable!(),
        };
        let task = tab.update(Message::Launch(home::Message::Run(
            datadir,
            app::config::Config::new(false),
            crate::chain::ChainId::Signet,
            cube,
        )));

        if !crate::feature_flags::PASSKEY_ENABLED {
            // The open path refuses passkey Cubes in this build; that refusal
            // has its own copy and is not what this test is about.
            assert!(matches!(tab.state, State::Home(_)));
            return;
        }
        assert!(matches!(tab.state, State::PasskeyUnlock(_)));
        let actions = iced_runtime::futures::futures::executor::block_on(
            iced_runtime::task::into_stream(task)
                .expect("the open returns a task")
                .collect::<Vec<_>>(),
        );
        assert!(
            actions.iter().any(|action| matches!(
                action,
                iced_runtime::Action::Output(Message::PasskeyUnlock(
                    crate::passkey_unlock::Message::Begin
                ))
            )),
            "the passkey prompt must start without a second press"
        );
    }

    /// Directory identity must agree with the persisted settings before any
    /// projection to `bitcoin::Network`: a Cube whose file says BTCB2 but was
    /// read from `bitcoin/` (or the reverse) is refused — not opened as
    /// whichever of the two the caller happened to hold.
    #[test]
    fn a_cube_found_in_the_wrong_chain_directory_is_refused() {
        let cases = [
            (
                crate::chain::ChainId::BitcoinBlake2b,
                crate::chain::ChainId::Bitcoin,
            ),
            (
                crate::chain::ChainId::Bitcoin,
                crate::chain::ChainId::BitcoinBlake2b,
            ),
            (
                crate::chain::ChainId::Testnet4,
                crate::chain::ChainId::BitcoinBlake2bTestnet4,
            ),
            (
                crate::chain::ChainId::Signet,
                crate::chain::ChainId::Testnet,
            ),
        ];
        for (persisted, found_in) in cases {
            let (state, error) = open_from_home(cube_on(persisted), found_in);
            assert_eq!(state, "Home", "{:?} in {:?}", persisted, found_in);
            let error = error.expect("an error is shown");
            assert!(error.contains(persisted.label()), "{}", error);
            assert!(error.contains(found_in.label()), "{}", error);
            assert!(!error.contains(crate::chain::BTCB2_DORMANT_REASON));
        }
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

    /// The duress-off notice is cleared on disk only by the `App` that shows
    /// it. A tab still loading holds the toast in memory, and a Cancel or
    /// close from there must leave the next unlock to say it.
    #[test]
    fn the_duress_notice_stays_on_disk_until_the_app_shows_it() {
        let root = std::env::temp_dir().join(format!("duress-notice-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        crate::services::duress::DuressLocalState {
            migration_disarm_notice: true,
            ..Default::default()
        }
        .save(&root)
        .unwrap();
        let mut tab = non_app_tab();
        tab.pending_unlock_warnings = vec![DURESS_DISARM_NOTICE.to_string()];
        tab.pending_duress_notice = Some(root.clone());

        let _ = tab.flush_migration_warning();

        assert_eq!(tab.pending_duress_notice.as_ref(), Some(&root));
        assert_eq!(
            duress_disarm_notice(&root).as_deref(),
            Some(DURESS_DISARM_NOTICE)
        );
        std::fs::remove_dir_all(root).unwrap();
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
    use app::settings::WalletId;
    use coincube_core::miniscript::bitcoin::bip32::{DerivationPath, Xpriv, Xpub};
    use coincube_core::miniscript::bitcoin::secp256k1::Secp256k1;
    use coincube_core::wallet_descriptor::ImportedWalletDescriptor;
    use std::str::FromStr;

    fn imported_restore_blob(seed_byte: u8) -> crate::services::recovery::ImportedDescriptorBlob {
        let secp = Secp256k1::new();
        let master = Xpriv::new_master(bitcoin::Network::Bitcoin, &[seed_byte; 32]).unwrap();
        let path = DerivationPath::from_str("m/84h/0h/0h").unwrap();
        let account = master.derive_priv(&secp, &path).unwrap();
        let xpub = Xpub::from_priv(&secp, &account);
        let key = format!("[{}/84h/0h/0h]{xpub}", master.fingerprint(&secp));
        let descriptor = ImportedWalletDescriptor::parse(
            &format!("wpkh({key}/0/*)"),
            Some(&format!("wpkh({key}/1/*)")),
            crate::chain::ChainId::Bitcoin,
        )
        .unwrap();
        crate::services::recovery::ImportedDescriptorBlob::new(
            ORIG_UUID.to_string(),
            crate::chain::ChainId::Bitcoin,
            crate::services::recovery::ImportedDescriptorBlobWallet {
                name: "My imported wallet".into(),
                descriptor,
                birthday: 0,
                initial_lookahead: 20,
                receive_index: 7,
                change_index: 3,
            },
        )
        .unwrap()
    }

    #[tokio::test]
    async fn imported_cube_resolves_its_normal_local_wallet_row() {
        let root_path =
            std::env::temp_dir().join(format!("imported-wallet-open-{}", uuid::Uuid::new_v4()));
        let root = CoincubeDirectory::new(root_path.clone());
        let network = root.network_directory(crate::chain::ChainId::Bitcoin);
        let blob = imported_restore_blob(31);
        let attachment = app::settings::ImportedWalletAttachment::new(
            blob.wallet.descriptor.clone(),
            crate::chain::ChainId::Bitcoin,
            blob.wallet.birthday,
            blob.wallet.initial_lookahead,
        )
        .unwrap();
        let mut cube =
            app::settings::CubeSettings::new("Imported".into(), crate::chain::ChainId::Bitcoin);
        cube.id = ORIG_UUID.into();
        cube.bitcoin_import = Some(Box::new(app::settings::BitcoinImport {
            operation_id: uuid::Uuid::new_v4(),
            stage: app::settings::BitcoinImportStage::Syncing {
                wallet: attachment.clone(),
            },
            last_error: None,
        }));
        let wallet = app::settings::WalletSettings {
            name: "Imported".into(),
            alias: None,
            descriptor_checksum: attachment.wallet_id.descriptor_checksum.clone(),
            pinned_at: None,
            keys: Vec::new(),
            hardware_wallets: Vec::new(),
            remote_backend_auth: None,
            start_internal_bitcoind: Some(false),
            pending_rescan: None,
            keychain_keys_recorded: false,
        };
        update_settings_file(&network, |_| {
            Some(app::settings::Settings {
                cubes: vec![cube.clone()],
                wallets: vec![wallet.clone()],
                ..Default::default()
            })
        })
        .await
        .unwrap();

        assert_eq!(
            bitcoin_wallet_settings_for_cube(&root, crate::chain::ChainId::Bitcoin, &cube)
                .unwrap()
                .wallet_id(),
            attachment.wallet_id
        );
        assert!(
            bitcoin_wallet_settings_for_cube(&root, crate::chain::ChainId::Signet, &cube).is_none()
        );
        let mut pending = cube.clone();
        pending.bitcoin_import.as_mut().unwrap().stage =
            app::settings::BitcoinImportStage::PendingSource;
        assert!(
            bitcoin_wallet_settings_for_cube(&root, crate::chain::ChainId::Bitcoin, &pending)
                .is_none()
        );
        std::fs::remove_dir_all(root_path).unwrap();
    }

    #[tokio::test]
    async fn imported_restore_retries_exact_attached_cube_without_replacing_it() {
        let nd = temp_network_dir("imported-retry");
        let blob = imported_restore_blob(19);
        let attachment = app::settings::ImportedWalletAttachment::new(
            blob.wallet.descriptor.clone(),
            crate::chain::ChainId::Bitcoin,
            blob.wallet.birthday,
            blob.wallet.initial_lookahead,
        )
        .unwrap();
        let fp = bitcoin::bip32::Fingerprint::from([1, 2, 3, 4]);
        let seed = RestoreCubeSeed {
            pin: zeroize::Zeroizing::new("246810".to_string()),
            master_signer_fingerprint: fp,
            passkey: None,
            seed_backed_up: false,
        };
        let mut cube = app::settings::CubeSettings::new(
            "Restored".to_string(),
            crate::chain::ChainId::Bitcoin,
        )
        .with_master_signer(fp);
        cube.id = ORIG_UUID.to_string();
        cube.bitcoin_import = Some(Box::new(app::settings::BitcoinImport {
            operation_id: uuid::Uuid::new_v4(),
            stage: app::settings::BitcoinImportStage::Syncing { wallet: attachment },
            last_error: None,
        }));
        update_settings_file(&nd, |_| {
            Some(app::settings::Settings {
                cubes: vec![cube.clone()],
                ..Default::default()
            })
        })
        .await
        .unwrap();

        let returned = find_or_create_cube_for_imported_restore(
            &nd,
            None,
            &None,
            crate::chain::ChainId::Bitcoin,
            None,
            Some(RestoreCubeIdentity {
                uuid: ORIG_UUID.to_string(),
                name: "Restored".into(),
            }),
            Some(&seed),
            &blob,
        )
        .await
        .expect("exact published attachment may resume");
        assert_eq!(returned.id, ORIG_UUID);
        assert_eq!(reload(&nd).cubes.len(), 1);

        let different_blob = imported_restore_blob(20);
        let mismatch = find_or_create_cube_for_imported_restore(
            &nd,
            None,
            &None,
            crate::chain::ChainId::Bitcoin,
            None,
            Some(RestoreCubeIdentity {
                uuid: ORIG_UUID.to_string(),
                name: "Restored".into(),
            }),
            Some(&seed),
            &different_blob,
        )
        .await
        .expect_err("another imported wallet must not reuse the Cube");
        assert!(mismatch.contains("different or unfinished"));

        let wrong_seed = RestoreCubeSeed {
            pin: zeroize::Zeroizing::new("246810".to_string()),
            master_signer_fingerprint: bitcoin::bip32::Fingerprint::from([9, 9, 9, 9]),
            passkey: None,
            seed_backed_up: false,
        };
        let mismatch = find_or_create_cube_for_imported_restore(
            &nd,
            None,
            &None,
            crate::chain::ChainId::Bitcoin,
            None,
            Some(RestoreCubeIdentity {
                uuid: ORIG_UUID.to_string(),
                name: "Restored".into(),
            }),
            Some(&wrong_seed),
            &blob,
        )
        .await
        .expect_err("another Cube seed must not reuse the attachment");
        assert!(mismatch.contains("different or unfinished"));

        let returned = find_or_create_cube_for_imported_restore(
            &nd,
            None,
            &None,
            crate::chain::ChainId::Bitcoin,
            Some(ORIG_UUID.to_string()),
            None,
            None,
            &blob,
        )
        .await
        .expect("descriptor-only restore may resume its exact Cube");
        assert_eq!(returned.id, ORIG_UUID);
    }

    #[tokio::test]
    async fn fork_install_save_preserves_updates_after_its_snapshot() {
        let root = std::env::temp_dir().join(format!("fork-save-{}", uuid::Uuid::new_v4()));
        let dir = CoincubeDirectory::new(root.clone())
            .network_directory(crate::chain::ChainId::BitcoinBlake2b);
        let existing =
            app::settings::CubeSettings::new("other".into(), crate::chain::ChainId::BitcoinBlake2b);
        update_settings_file(&dir, |mut settings| {
            settings.cubes.push(existing);
            Some(settings)
        })
        .await
        .unwrap();
        let new_cube = app::settings::CubeSettings::new(
            "installed".into(),
            crate::chain::ChainId::BitcoinBlake2b,
        );
        save_cube_settings(
            &dir,
            new_cube.clone(),
            crate::chain::ChainId::BitcoinBlake2b,
            app::settings::Settings {
                cubes: vec![new_cube.clone()],
                ..Default::default()
            },
            app::settings::Settings::default(),
        )
        .await
        .unwrap();
        let saved = reload(&dir);
        assert_eq!(saved.cubes.len(), 2);
        assert_eq!(saved.cubes[0].name, "other");
        assert_eq!(saved.cubes[1].name, "installed");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn fork_existing_attachment_empty_reuse_and_duplicate_reconciliation_persist() {
        let chain = crate::chain::ChainId::BitcoinBlake2b;
        for mode in ["originating", "empty", "restore_duplicate"] {
            let root = std::env::temp_dir().join(format!("fork-attach-{}", uuid::Uuid::new_v4()));
            let dir = CoincubeDirectory::new(root.clone()).network_directory(chain);
            let target = app::settings::CubeSettings::new("target".into(), chain);
            let target_id = target.id.clone();
            let mut snapshot = app::settings::Settings {
                cubes: vec![target],
                ..Default::default()
            };
            if mode == "restore_duplicate" {
                snapshot.cubes.push(
                    app::settings::CubeSettings::new("duplicate".into(), chain)
                        .with_vault(vault_identity()),
                );
            }
            update_settings_file(&dir, |_| Some(snapshot))
                .await
                .unwrap();
            let result = find_or_create_cube(
                &dir,
                Some(&vault_identity()),
                &None,
                chain,
                (mode == "originating").then(|| target_id.clone()),
                (mode == "restore_duplicate").then(|| RestoreCubeIdentity {
                    uuid: target_id.clone(),
                    name: "target".into(),
                }),
                None,
            )
            .await
            .unwrap();
            assert_eq!(result.id, target_id);
            let saved = reload(&dir);
            assert_eq!(saved.cubes.len(), 1);
            assert_eq!(saved.cubes[0].id, target_id);
            assert_eq!(
                saved.cubes[0].vault_wallet_id,
                Some(vault_identity().wallet_id)
            );
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[tokio::test]
    async fn fork_delta_preserves_unrelated_edits_and_refuses_changed_target_without_write() {
        let chain = crate::chain::ChainId::BitcoinBlake2b;
        for conflicting in [false, true] {
            let root = std::env::temp_dir().join(format!("fork-conflict-{}", uuid::Uuid::new_v4()));
            let dir = CoincubeDirectory::new(root.clone()).network_directory(chain);
            let target = app::settings::CubeSettings::new("target".into(), chain);
            let other = app::settings::CubeSettings::new("other".into(), chain);
            let expected = app::settings::Settings {
                cubes: vec![target.clone(), other],
                ..Default::default()
            };
            let mut desired = expected.clone();
            desired.cubes[0].set_vault(vault_identity());
            let mut latest = expected.clone();
            latest.cubes[usize::from(!conflicting)].name = "concurrent edit".into();
            update_settings_file(&dir, |_| Some(latest)).await.unwrap();
            let before = std::fs::read(dir.path().join("settings.json")).unwrap();
            let result =
                save_cube_settings(&dir, desired.cubes[0].clone(), chain, desired, expected).await;
            if conflicting {
                assert!(result.is_err());
                assert_eq!(
                    std::fs::read(dir.path().join("settings.json")).unwrap(),
                    before
                );
            } else {
                result.unwrap();
                let saved = reload(&dir);
                assert_eq!(
                    saved.cubes[0].vault_wallet_id,
                    Some(vault_identity().wallet_id)
                );
                assert_eq!(saved.cubes[1].name, "concurrent edit");
            }
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    /// Same hazard as `cleared_pending_rescan`: `update_settings_file` deletes
    /// `settings.json` when its updater returns `None`, so a Cube id that
    /// matches nothing must leave the settings alone rather than wipe every
    /// Cube on the network.
    #[test]
    fn recording_a_kit_for_an_unknown_cube_changes_nothing() {
        let cube = |id: &str, backed_up: bool| {
            let mut c = app::settings::CubeSettings::new_with_raw_id(
                id.to_string(),
                format!("Cube {}", id),
                bitcoin::Network::Bitcoin,
            );
            c.recovery_kit_password_backed_up = backed_up;
            c
        };

        let before = app::settings::Settings {
            cubes: vec![cube("aaa", false), cube("bbb", false)],
            ..Default::default()
        };

        let after = marked_kit_backed_up(before.clone(), "nosuchcube");
        assert_eq!(after.cubes.len(), 2, "no Cube may be dropped");
        assert!(after
            .cubes
            .iter()
            .all(|c| !c.recovery_kit_password_backed_up));

        let after = marked_kit_backed_up(before, "bbb");
        assert_eq!(after.cubes.len(), 2);
        assert!(!after.cubes[0].recovery_kit_password_backed_up);
        assert!(
            after.cubes[1].recovery_kit_password_backed_up,
            "the restored Cube is the only one marked"
        );
    }

    #[tokio::test]
    async fn new_fork_cube_retains_identity_and_confirmed_seed_backup() {
        let nd = temp_network_dir("fork-create");
        let chain = crate::chain::ChainId::BitcoinBlake2b;
        let fp = bitcoin::bip32::Fingerprint::from([1, 2, 3, 4]);
        let seed = RestoreCubeSeed {
            pin: zeroize::Zeroizing::new("synthetic-pin".into()),
            master_signer_fingerprint: fp,
            passkey: None,
            seed_backed_up: true,
        };
        let cube = find_or_create_cube(
            &nd,
            Some(&vault_identity()),
            &Some("Fork Vault".into()),
            chain,
            None,
            Some(RestoreCubeIdentity {
                uuid: "synthetic-fork-cube".into(),
                name: "Fork Cube".into(),
            }),
            Some(&seed),
        )
        .await
        .unwrap();
        assert_eq!(cube.network, chain);
        assert_eq!(cube.id, "synthetic-fork-cube");
        assert_eq!(cube.master_signer_fingerprint, Some(fp));
        assert!(cube.backed_up);
        let before = std::fs::read(nd.path().join("settings.json")).unwrap();
        let wrong = find_or_create_cube(
            &nd,
            Some(&vault_identity()),
            &None,
            bitcoin::Network::Bitcoin,
            None,
            None,
            None,
        )
        .await;
        assert!(wrong.is_err());
        assert_eq!(
            before,
            std::fs::read(nd.path().join("settings.json")).unwrap()
        );
        std::fs::remove_dir_all(nd.path()).unwrap();
    }

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

    /// The fingerprint the installer derives from the descriptor it just
    /// sealed. Distinct from the wallet id's checksum on purpose: the two are
    /// different things, and a test that reused one for the other would not
    /// catch them being confused again.
    const VAULT_FP: &str = "8099ee80";

    fn vault_identity() -> VaultIdentity {
        VaultIdentity {
            wallet_id: wallet_id(),
            fingerprint: Some(VAULT_FP.to_string()),
        }
    }

    /// An unrelated Cube's Vault — identified only by its storage key, as a
    /// Cube attached before `vault_fingerprint` existed would be.
    fn other_vault(checksum: &str, timestamp: i64) -> VaultIdentity {
        VaultIdentity::new(WalletId::new(checksum.to_string(), Some(timestamp)), None)
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
        let v = vault_identity();
        let wid = v.wallet_id.clone();
        let cube = find_or_create_cube(
            &nd,
            Some(&v),
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
                .with_vault(other_vault("otherchk", 1));
        let other_id = other.id.clone();
        settings.cubes.push(other);
        update_settings_file(&nd, |_| Some(settings)).await.unwrap();

        let v = vault_identity();
        let cube = find_or_create_cube(
            &nd,
            Some(&v),
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

        let v = vault_identity();
        let wid = v.wallet_id.clone();
        let cube = find_or_create_cube(
            &nd,
            Some(&v),
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

    /// D1 (`plans/PLAN-vault-identity-unification.md`): every branch that
    /// attaches a Vault must persist *both* halves of its identity. The home
    /// cube list reads the fingerprint back from `settings.json` and has no
    /// descriptor in scope to recompute it, so a branch that wrote only the
    /// wallet id would leave that Cube identity-less until the next open.
    ///
    /// Exercises the three attach branches that mint or reuse a Cube — fresh
    /// install, originating cube, empty-cube reuse — and re-reads the file
    /// rather than trusting the returned value.
    #[tokio::test]
    async fn every_attach_branch_persists_the_vault_fingerprint() {
        // Fresh install: mint a new Cube.
        let nd = temp_network_dir("fp-fresh");
        let v = vault_identity();
        let cube = find_or_create_cube(
            &nd,
            Some(&v),
            &None,
            bitcoin::Network::Bitcoin,
            None,
            None,
            None,
        )
        .await
        .expect("install should succeed");
        assert_eq!(cube.vault_fingerprint.as_deref(), Some(VAULT_FP));
        assert_eq!(
            reload(&nd).cubes[0].vault_fingerprint.as_deref(),
            Some(VAULT_FP),
            "a minted Cube's identity must reach settings.json"
        );

        // Originating cube: attach to a Cube the caller named.
        let nd = temp_network_dir("fp-originating");
        let mut settings = app::settings::Settings::default();
        let shell =
            app::settings::CubeSettings::new("Shell".to_string(), bitcoin::Network::Bitcoin);
        let shell_id = shell.id.clone();
        settings.cubes.push(shell);
        update_settings_file(&nd, |_| Some(settings)).await.unwrap();
        let cube = find_or_create_cube(
            &nd,
            Some(&v),
            &None,
            bitcoin::Network::Bitcoin,
            Some(shell_id),
            None,
            None,
        )
        .await
        .expect("originating cube should be updated");
        assert_eq!(cube.vault_fingerprint.as_deref(), Some(VAULT_FP));
        assert_eq!(
            reload(&nd).cubes[0].vault_fingerprint.as_deref(),
            Some(VAULT_FP)
        );

        // Empty-cube reuse: adopt the first vault-less Cube.
        let nd = temp_network_dir("fp-empty");
        let mut settings = app::settings::Settings::default();
        settings.cubes.push(app::settings::CubeSettings::new(
            "Empty".to_string(),
            bitcoin::Network::Bitcoin,
        ));
        update_settings_file(&nd, |_| Some(settings)).await.unwrap();
        let cube = find_or_create_cube(
            &nd,
            Some(&v),
            &None,
            bitcoin::Network::Bitcoin,
            None,
            None,
            None,
        )
        .await
        .expect("empty cube should be reused");
        assert_eq!(cube.vault_fingerprint.as_deref(), Some(VAULT_FP));
        assert_eq!(
            reload(&nd).cubes[0].vault_fingerprint.as_deref(),
            Some(VAULT_FP)
        );
    }

    /// A seed-only restore attaches no Vault, so it must leave both halves
    /// empty — not just the wallet id.
    #[tokio::test]
    async fn a_vault_less_attach_leaves_no_stale_fingerprint() {
        let nd = temp_network_dir("fp-none");
        let cube = find_or_create_cube(
            &nd,
            None,
            &None,
            bitcoin::Network::Bitcoin,
            None,
            None,
            None,
        )
        .await
        .expect("vault-less install should succeed");
        assert_eq!(cube.vault_wallet_id, None);
        assert_eq!(cube.vault_fingerprint, None);
    }

    /// Non-restore install is unchanged: a fresh UUID and the wallet alias.
    #[tokio::test]
    async fn non_restore_install_mints_fresh_uuid() {
        let nd = temp_network_dir("fresh");
        let v = vault_identity();
        let wid = v.wallet_id.clone();
        let cube = find_or_create_cube(
            &nd,
            Some(&v),
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
        let v = vault_identity();
        let mut settings = app::settings::Settings::default();
        let existing =
            app::settings::CubeSettings::new("Existing".to_string(), bitcoin::Network::Bitcoin)
                .with_vault(v.clone());
        let existing_id = existing.id.clone();
        settings.cubes.push(existing);
        update_settings_file(&nd, |_| Some(settings)).await.unwrap();

        let cube = find_or_create_cube(
            &nd,
            Some(&v),
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
        let v = vault_identity();
        let mut settings = app::settings::Settings::default();
        let mut restored = app::settings::CubeSettings::new(
            "Already Restored".to_string(),
            bitcoin::Network::Bitcoin,
        )
        .with_vault(other_vault("otherwallet", 9));
        restored.id = ORIG_UUID.to_string();
        settings.cubes.push(restored);
        update_settings_file(&nd, |_| Some(settings)).await.unwrap();

        let err = find_or_create_cube(
            &nd,
            Some(&v),
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
        let v = vault_identity();
        let wid = v.wallet_id.clone();
        let fp = bitcoin::bip32::Fingerprint::from([0xaa, 0xbb, 0xcc, 0xdd]);
        let seed = RestoreCubeSeed {
            pin: zeroize::Zeroizing::new("135790".to_string()),
            master_signer_fingerprint: fp,
            passkey: None,
            seed_backed_up: false,
        };
        let mut settings = app::settings::Settings::default();
        let shell =
            app::settings::CubeSettings::new("Shell".to_string(), bitcoin::Network::Bitcoin);
        let shell_id = shell.id.clone();
        settings.cubes.push(shell);
        update_settings_file(&nd, |_| Some(settings)).await.unwrap();

        let cube = find_or_create_cube(
            &nd,
            Some(&v),
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
        let v = vault_identity();
        let mut settings = app::settings::Settings::default();
        let occupied =
            app::settings::CubeSettings::new("Occupied".to_string(), bitcoin::Network::Bitcoin)
                .with_vault(other_vault("otherwallet", 2));
        let occupied_id = occupied.id.clone();
        settings.cubes.push(occupied);
        update_settings_file(&nd, |_| Some(settings)).await.unwrap();

        let occupied_err = find_or_create_cube(
            &nd,
            Some(&v),
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
            Some(&v),
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
        let v = vault_identity();
        let wid = v.wallet_id.clone();
        let mut settings = app::settings::Settings::default();
        let empty =
            app::settings::CubeSettings::new("Empty".to_string(), bitcoin::Network::Bitcoin);
        let empty_id = empty.id.clone();
        settings.cubes.push(empty);
        update_settings_file(&nd, |_| Some(settings)).await.unwrap();

        let cube = find_or_create_cube(
            &nd,
            Some(&v),
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
        let v = vault_identity();
        let wid = v.wallet_id.clone();

        let cube = find_or_create_cube(
            &nd,
            Some(&v),
            &None,
            bitcoin::Network::Signet,
            None,
            None,
            None,
        )
        .await
        .expect("first cube should be created");

        assert_eq!(cube.name, "My signet Cube");
        assert_eq!(cube.network, crate::chain::ChainId::Signet);
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
        let v = vault_identity();
        let wid = v.wallet_id.clone();

        // The exact state the old bug produced: a duplicate Cube (its own,
        // different UUID) already holds the wallet.
        let mut settings = app::settings::Settings::default();
        let dup =
            app::settings::CubeSettings::new("Duplicate".to_string(), bitcoin::Network::Bitcoin)
                .with_vault(v.clone());
        let dup_id = dup.id.clone();
        assert_ne!(
            dup_id, ORIG_UUID,
            "duplicate must not already carry the original UUID"
        );
        settings.cubes.push(dup);
        update_settings_file(&nd, |_| Some(settings)).await.unwrap();

        let cube = find_or_create_cube(
            &nd,
            Some(&v),
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
            passkey: None,
            seed_backed_up: false,
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
mod breez_fallback_tests {
    use super::disconnected_breez_client_after_unlock;
    use crate::app::session;
    use crate::app::settings::CubeSettings;
    use coincube_core::miniscript::bitcoin::{secp256k1::Secp256k1, Network};
    use coincube_core::signer::MasterSigner;

    /// A Liquid load that failed *after* the unlock decrypted the seed must not
    /// cost the Cube its signer: `App::new` derives the Connect encryption key
    /// from the fallback client, and without it every blinded key (the phone
    /// recovery key included) reports `Locked` on a Cube whose seed is right
    /// there on disk.
    #[test]
    fn fallback_client_keeps_the_signer_the_unlock_parked() {
        let _g = session::test_guard();
        session::close();
        let signer = MasterSigner::generate(Network::Bitcoin).unwrap();
        let fingerprint = signer.fingerprint(&Secp256k1::signing_only());
        let cube = CubeSettings::new("Fallback".to_string(), Network::Bitcoin)
            .with_master_signer(fingerprint);
        session::store_unlocked_signer(&cube.id, fingerprint, signer);

        let client = disconnected_breez_client_after_unlock(&cube, Network::Bitcoin);

        assert!(
            !client.is_connected(),
            "fallback client never carries an SDK"
        );
        assert!(
            client.liquid_signer().is_some(),
            "fallback client must carry the session's signer"
        );
        session::close();
    }

    /// A seed-less Cube (or a session closed since the unlock) has nothing to
    /// preserve, and the fallback stays the signer-less client it always was.
    #[test]
    fn fallback_client_is_signerless_without_a_parked_signer() {
        let _g = session::test_guard();
        session::close();
        let cube = CubeSettings::new("NoSeed".to_string(), Network::Bitcoin).with_master_signer(
            coincube_core::miniscript::bitcoin::bip32::Fingerprint::from([9, 9, 9, 9]),
        );

        let client = disconnected_breez_client_after_unlock(&cube, Network::Bitcoin);

        assert!(!client.is_connected());
        assert!(client.liquid_signer().is_none());
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
            connect_client: None,
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

    /// #947 review: after the assertion parked the seed, a refused open
    /// (here: no Connect sign-in) must close the session, and so must Back.
    #[test]
    fn a_refused_passkey_copy_open_closes_the_session() {
        use crate::chain::ChainId;
        use coincube_core::{miniscript::bitcoin::secp256k1::Secp256k1, signer::MasterSigner};
        let _guard = crate::app::session::test_guard();
        crate::app::session::close();
        let signer = MasterSigner::generate(Network::Bitcoin).unwrap();
        let fingerprint = signer.fingerprint(&Secp256k1::signing_only());
        let cube = CubeSettings::new("Copy".to_string(), ChainId::BitcoinBlake2b).with_passkey(
            PasskeyMetadata {
                credential_id: "Y3JlZC1pZA==".to_string(),
                rp_id: "coincube.io".to_string(),
                created_at: 1_786_122_245,
                label: None,
            },
        );
        let id = cube.id.clone();
        // The copy has a Vault: a Vault-less fork Cube opens without Connect.
        let mut with_vault = on_success();
        let crate::pin_entry::PinEntrySuccess::LoadApp {
            wallet_settings, ..
        } = &mut with_vault;
        *wallet_settings = Some(
            serde_json::from_value(serde_json::json!({
                "name":"synthetic", "alias":"copy", "descriptor_checksum":"copy-vault",
                "pinned_at":1700000000
            }))
            .unwrap(),
        );
        let mut tab = super::Tab::new(
            1,
            unlock_state(
                cube,
                std::path::PathBuf::from("/tmp/unused"),
                with_vault,
                None,
            ),
        );
        crate::app::session::open_without_pin(id.clone());
        crate::app::session::store_unlocked_signer(&id, fingerprint, signer);
        let _ = tab.update(super::Message::PasskeyUnlock(
            crate::passkey_unlock::Message::Unlocked { fingerprint },
        ));
        assert!(
            matches!(tab.state, State::PasskeyUnlock(_)),
            "the open was refused"
        );
        assert!(crate::app::session::unlocked_signer(&id, fingerprint).is_none());
        assert!(!crate::app::session::is_open());

        // Back from a failed open closes it too.
        let signer = MasterSigner::generate(Network::Bitcoin).unwrap();
        let fingerprint = signer.fingerprint(&Secp256k1::signing_only());
        crate::app::session::open_without_pin(id.clone());
        crate::app::session::store_unlocked_signer(&id, fingerprint, signer);
        let _ = tab.update(super::Message::PasskeyUnlock(
            crate::passkey_unlock::Message::Back,
        ));
        assert!(!crate::app::session::is_open());
    }

    /// A passkey Blake2b Cube with no Vault has nothing for Connect to serve:
    /// a sign-in or sign-out elsewhere drops its client and leaves the prompt
    /// waiting, as the PIN screen does.
    #[test]
    fn a_connect_change_spares_a_vault_less_passkey_open() {
        use crate::chain::ChainId;
        let cube = CubeSettings::new("Fork".to_string(), ChainId::BitcoinBlake2b).with_passkey(
            PasskeyMetadata {
                credential_id: "Y3JlZC1pZA==".to_string(),
                rp_id: "coincube.io".to_string(),
                created_at: 1_786_122_245,
                label: None,
            },
        );
        let mut success = on_success();
        let crate::pin_entry::PinEntrySuccess::LoadApp { connect_client, .. } = &mut success;
        *connect_client = Some(crate::services::coincube::CoincubeClient::for_test(
            "https://connect.example/",
        ));
        let mut tab = super::Tab::new(
            1,
            unlock_state(cube, std::path::PathBuf::from("/tmp/unused"), success, None),
        );
        drop(tab.invalidate_fork_state(super::AuthChange::LogOut, false, false));
        let State::PasskeyUnlock(unlock) = &tab.state else {
            panic!("still at the prompt");
        };
        assert!(!unlock.requires_connect());
        assert!(
            unlock.error_for_test().is_none(),
            "the prompt is not failed"
        );
        let crate::pin_entry::PinEntrySuccess::LoadApp { connect_client, .. } = &unlock.on_success;
        assert!(connect_client.is_none(), "the old client is dropped");
    }

    /// Signed in, a Vault-less passkey Blake2b Cube is held to its account
    /// as the PIN screen holds it: an account change fails the prompt.
    #[test]
    fn a_connect_change_fails_a_signed_in_vault_less_passkey_open() {
        use crate::chain::ChainId;
        let cube = CubeSettings::new("Fork".to_string(), ChainId::BitcoinBlake2b).with_passkey(
            PasskeyMetadata {
                credential_id: "Y3JlZC1pZA==".to_string(),
                rp_id: "coincube.io".to_string(),
                created_at: 1_786_122_245,
                label: None,
            },
        );
        let mut success = on_success();
        let crate::pin_entry::PinEntrySuccess::LoadApp { connect_client, .. } = &mut success;
        let mut client =
            crate::services::coincube::CoincubeClient::for_test("https://connect.example/");
        client.set_token("account-a");
        *connect_client = Some(client);
        let mut tab = super::Tab::new(
            1,
            unlock_state(cube, std::path::PathBuf::from("/tmp/unused"), success, None),
        );
        drop(tab.invalidate_fork_state(super::AuthChange::LogOut, false, false));
        let State::PasskeyUnlock(unlock) = &tab.state else {
            panic!("still at the prompt");
        };
        assert!(unlock.error_for_test().is_some(), "the prompt is failed");
        let crate::pin_entry::PinEntrySuccess::LoadApp { connect_client, .. } = &unlock.on_success;
        assert!(connect_client.is_none(), "the old client is dropped");
    }

    /// #947 review: a Connect change while a Connect-backed passkey copy is
    /// at its prompt drops the old client and fails the open, as PIN entry
    /// does, so the assertion cannot open it under the previous account.
    #[test]
    fn a_connect_change_fails_a_waiting_passkey_copy_open() {
        use crate::chain::ChainId;
        let cube = CubeSettings::new("Copy".to_string(), ChainId::BitcoinBlake2b).with_passkey(
            PasskeyMetadata {
                credential_id: "Y3JlZC1pZA==".to_string(),
                rp_id: "coincube.io".to_string(),
                created_at: 1_786_122_245,
                label: None,
            },
        );
        let mut success = on_success();
        let crate::pin_entry::PinEntrySuccess::LoadApp {
            connect_client,
            wallet_settings,
            ..
        } = &mut success;
        *connect_client = Some(crate::services::coincube::CoincubeClient::for_test(
            "https://connect.example/",
        ));
        // A Connect-backed copy has a Vault; a Vault-less one ignores the
        // change (`a_connect_change_spares_a_vault_less_passkey_open`).
        *wallet_settings = Some(
            serde_json::from_value(serde_json::json!({
                "name":"synthetic", "alias":"copy", "descriptor_checksum":"copy-vault",
                "pinned_at":1700000000
            }))
            .unwrap(),
        );
        let mut tab = super::Tab::new(
            1,
            unlock_state(cube, std::path::PathBuf::from("/tmp/unused"), success, None),
        );
        drop(tab.invalidate_fork_state(super::AuthChange::LogOut, false, false));
        let State::PasskeyUnlock(unlock) = &tab.state else {
            panic!("still at the prompt");
        };
        assert!(unlock.requires_connect());
        let crate::pin_entry::PinEntrySuccess::LoadApp { connect_client, .. } = &unlock.on_success;
        assert!(connect_client.is_none(), "the old client is dropped");
        assert_eq!(
            unlock.error_for_test(),
            Some("Connect session changed. Return Home and sign in again.")
        );
    }

    fn blake2b_error_loader() -> super::State {
        use crate::chain::ChainId;
        let (loader, _task) = crate::loader::Loader::new(
            crate::dir::CoincubeDirectory::new(std::path::PathBuf::from("/nonexistent")),
            crate::app::config::Config::new(false),
            ChainId::BitcoinBlake2b.bitcoin_network(),
            None,
            None,
            None,
            CubeSettings::new_with_raw_id(
                "split-loader".to_string(),
                "Fork".to_string(),
                ChainId::BitcoinBlake2b,
            ),
            None,
            None,
        );
        super::State::Loader(loader)
    }

    #[test]
    fn revoked_fork_loader_cannot_create_a_vault_with_a_leftover_session_pin() {
        use crate::dir::CoincubeDirectory;
        use coincube_core::miniscript::bitcoin;
        let _guard = crate::app::session::test_guard();
        let root =
            std::env::temp_dir().join(format!("revoked-installer-761-{}", uuid::Uuid::new_v4()));
        let master = crate::signer::Signer::generate(bitcoin::Network::Bitcoin).unwrap();
        master
            .store_encrypted_seed_only_for_chain(
                &CoincubeDirectory::new(root.clone()),
                crate::chain::ChainId::BitcoinBlake2b,
                "2468",
                "split-loader",
                None,
            )
            .unwrap();
        crate::app::session::open("split-loader", zeroize::Zeroizing::new("2468".into()));
        let mut tab = super::Tab::new(1, blake2b_error_loader());
        let State::Loader(loader) = &mut tab.state else {
            unreachable!()
        };
        loader.datadir_path = CoincubeDirectory::new(root.clone());
        loader.cube_settings.master_signer_fingerprint = Some(master.fingerprint());
        loader.invalidate_fork_session();
        // Even a stale auth completion must not reopen this revoked generation.
        let mut client = crate::services::coincube::CoincubeClient::new();
        client.set_token("synthetic-stale-token");
        loader.connect_client = Some(client);
        assert!(crate::app::session::pin_for("split-loader").is_some());
        drop(
            tab.update(super::Message::Load(crate::loader::Message::View(
                crate::loader::ViewMessage::SetupVault,
            ))),
        );
        let State::Loader(loader) = &tab.state else {
            panic!("a revoked Loader must never release a root to an installer");
        };
        assert!(loader.fork_revoked());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn global_beta_off_invalidates_a_fork_loader_and_rejects_late_results() {
        let _guard = crate::app::session::test_guard();
        let root_path =
            std::env::temp_dir().join(format!("coincube-beta-tab-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root_path).unwrap();
        let root = crate::dir::CoincubeDirectory::new(root_path.clone());
        let mut tab = super::Tab::new(1, blake2b_error_loader());
        let before = tab.fork_session_generation;
        drop(tab.reload_global_settings(&root));
        assert_ne!(tab.fork_session_generation, before);
        assert!(!tab.accepts_fork_generation(before));
        assert_eq!(
            tab.update(super::Message::ForkAsync(
                before,
                Box::new(super::Message::LockCube)
            ))
            .units(),
            0
        );
        assert!(
            matches!(tab.state, super::State::Loader(_)),
            "stale results must not replace the refused loader"
        );
        std::fs::remove_dir_all(root_path).unwrap();
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

#[cfg(test)]
mod fork_completion_tests {
    use super::*;
    use crate::chain::ChainId;

    #[test]
    fn sibling_connect_refresh_is_adopted_without_another_keyring_refresh() {
        let root_path =
            std::env::temp_dir().join(format!("coincube-sibling-refresh-{}", uuid::Uuid::new_v4()));
        let root = CoincubeDirectory::new(root_path.clone());
        let (home, startup) = Home::new_for_chain(root, Some(ChainId::BitcoinBlake2b));
        drop(startup);
        let mut tab = Tab::new(2, State::Home(home));
        let previous_epoch = match &tab.state {
            State::Home(home) => home.connect_account.auth_epoch(),
            _ => unreachable!(),
        };
        drop(tab.invalidate_fork_session(
            AuthChange::SignIn {
                user_id: 7,
                token: zeroize::Zeroizing::new("fresh-bearer".into()),
            },
            false,
        ));
        let State::Home(home) = &tab.state else {
            panic!("the sibling Home should stay open");
        };
        assert!(home.connect_account.auth_epoch() > previous_epoch);
        assert_eq!(
            home.connect_account
                .authenticated_client()
                .and_then(|client| client.token().map(str::to_owned)),
            Some("fresh-bearer".into())
        );
        let _ = std::fs::remove_dir_all(root_path);
    }

    async fn outputs(task: Task<Message>) -> Vec<Message> {
        use iced_runtime::futures::futures::StreamExt;
        match iced_runtime::task::into_stream(task) {
            Some(stream) => {
                stream
                    .filter_map(|action| async move {
                        match action {
                            iced_runtime::Action::Output(msg) => Some(msg),
                            _ => None,
                        }
                    })
                    .collect()
                    .await
            }
            None => Vec::new(),
        }
    }

    fn wallet(alias: &str) -> WalletSettings {
        serde_json::from_value(serde_json::json!({
            "name":"synthetic", "alias":alias, "descriptor_checksum":"same-vault", "pinned_at":1700000000
        })).unwrap()
    }

    #[tokio::test]
    async fn production_reopen_lookup_selects_fork_wallet_over_bitcoin_encoding_twin() {
        let root_path =
            std::env::temp_dir().join(format!("fork-vault-lookup-{}", uuid::Uuid::new_v4()));
        let root = CoincubeDirectory::new(root_path.clone());
        for (chain, alias) in [
            (ChainId::Bitcoin, "bitcoin-twin"),
            (ChainId::BitcoinBlake2b, "fork-vault"),
        ] {
            let dir = root.network_directory(chain);
            dir.init().unwrap();
            app::settings::update_settings_file(&dir, |_| {
                Some(app::settings::Settings {
                    wallets: vec![wallet(alias)],
                    ..Default::default()
                })
            })
            .await
            .unwrap();
        }
        let cube = app::settings::CubeSettings::new("Synthetic".into(), ChainId::BitcoinBlake2b)
            .with_vault(VaultIdentity::new(wallet("fork-vault").wallet_id(), None));
        assert_eq!(
            vault_settings_for_cube(&root, cube.network, &cube)
                .unwrap()
                .alias
                .as_deref(),
            Some("fork-vault")
        );
        assert!(vault_settings_for_cube(&root, ChainId::Bitcoin, &cube).is_none());
        std::fs::remove_dir_all(root.network_directory(ChainId::BitcoinBlake2b).path()).unwrap();
        assert!(vault_settings_for_cube(&root, cube.network, &cube).is_none());
        std::fs::remove_dir_all(root_path).unwrap();
    }

    #[test]
    fn invalidation_cancels_or_discards_real_post_install_completion_without_reopening_pin() {
        let _guard = app::session::test_guard();
        // This finite debug fixture moves several inline 31 KiB Tab/State
        // values through the real update path. Measured by the independent
        // reviewer: 2 MiB aborts, 2176 KiB passes (not recursive growth).
        // Give only each fixture ordering 8 MiB, with coverage headroom;
        // production State layout and the global test runner stay unchanged.
        for (completed_before_logout, local_capability) in
            [(false, false), (true, false), (false, true), (true, true)]
        {
            let result = std::thread::Builder::new()
                .name(format!("fork-save-ordering-{completed_before_logout}"))
                .stack_size(8 * 1024 * 1024)
                .spawn(move || {
                    tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .unwrap()
                        .block_on(post_install_completion_ordering(
                            completed_before_logout,
                            local_capability,
                        ));
                })
                .unwrap()
                .join();
            if let Err(panic) = result {
                std::panic::resume_unwind(panic);
            }
        }
        app::session::close();
    }

    async fn post_install_completion_ordering(
        completed_before_logout: bool,
        local_capability: bool,
    ) {
        app::session::close();
        let root_path =
            std::env::temp_dir().join(format!("fork-save-cancel-{}", uuid::Uuid::new_v4()));
        let root = CoincubeDirectory::new(root_path.clone());
        root.network_directory(ChainId::BitcoinBlake2b)
            .init()
            .unwrap();
        // Stage an already-installed synthetic fork at the real Exit seam.
        // No daemon or unlock/runtime gate is used to make this test pass.
        let (mut installer, _) = Installer::new(
            root.clone(),
            bitcoin::Network::Bitcoin,
            installer::UserFlow::RestoreFromRecoveryKit { cube_uuid: None },
            false,
            None,
            None,
            None,
            false,
            None,
        )
        .expect("a Bitcoin fixture installer");
        installer.context.bitcoin_config.chain = ChainId::BitcoinBlake2b;
        installer.context.use_coincube_connect = !local_capability;
        // A restored fork Cube: the exit seam mints it from the recovered
        // signer and its chosen PIN, under the original identity.
        installer.context.recovered_signer = Some(std::sync::Arc::new(
            crate::signer::Signer::generate(bitcoin::Network::Bitcoin).unwrap(),
        ));
        installer.context.cube_id = Some("synthetic-save-cube".into());
        installer.context.cube_name = Some("fork-vault".into());
        installer.context.restore_pin = Some(zeroize::Zeroizing::new("2468".to_string()));
        let mut tab = Box::new(Tab::new(1, State::Installer(Box::new(installer))));
        let mut save = Some(tab.update(Message::Install(installer::Message::Exit(
            Some(Box::new(wallet("fork-vault"))),
            None,
        ))));
        let mut completed = if completed_before_logout {
            outputs(save.take().unwrap()).await
        } else {
            Vec::new()
        };
        if completed_before_logout {
            assert!(matches!(
                completed.as_slice(),
                [Message::ForkInstallCompleted(
                    _,
                    installer::Message::CubeSaved(Ok(_), _, _)
                )]
            ));
            assert!(app::session::pin_for("synthetic-save-cube").is_none());
        }
        let startup = tab.invalidate_fork_state(AuthChange::LogOut, false, local_capability);
        // The Home task is preserved and its actual asynchronous directory
        // result is consumed. Auth Init itself is not run by this fixture.
        for result in outputs(startup).await {
            if matches!(result, Message::Launch(home::Message::Checked { .. })) {
                let _ = tab.update(result);
            }
        }
        if let Some(save) = save {
            assert!(outputs(save).await.is_empty());
        }
        for result in completed.drain(..) {
            assert!(outputs(tab.update(result)).await.is_empty());
        }
        assert!(matches!(&tab.state, State::Home(home) if home.is_checked_for_test()));
        assert!(app::session::pin_for("synthetic-save-cube").is_none());
        std::fs::remove_dir_all(root_path).unwrap();
    }
}

#[cfg(test)]
mod layout_tests {
    #[test]
    fn tab_state_stays_within_debug_stack_budget() {
        // #481: inline Panels grew State to 32 KiB, multiplying temporaries
        // in update_inner until Windows' 2 MiB test-thread stack overflowed.
        // The lint compares variants; this also catches balanced growth.
        let bytes = std::mem::size_of::<super::State>();
        assert!(
            bytes <= 8 * 1024,
            "Tab State is {} bytes (Home {}, Installer {}, Loader {}, App {}); review stack use and box bulky state",
            bytes,
            std::mem::size_of::<crate::home::Home>(),
            std::mem::size_of::<crate::installer::Installer>(),
            std::mem::size_of::<crate::loader::Loader>(),
            std::mem::size_of::<crate::app::App>(),
        );
    }
}

#[cfg(test)]
mod seed_export_duress_routing_tests {
    use super::*;

    #[test]
    fn cancelled_or_switched_export_still_activates_verified_duress_without_session() {
        let _guard = app::session::test_guard();
        app::session::close();
        let root = std::env::temp_dir().join(format!("seed-duress-tab-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let directory = CoincubeDirectory::new(root.clone());
        let (home, startup) = home::Home::new_for_chain(directory, None);
        drop(startup);
        let mut tab = Tab::new(1, State::Home(home));
        let app_generation = app::cache::AppGeneration::next();
        tab.seed_export_origins.push((
            app_generation,
            root.clone(),
            "verified-cube".into(),
            crate::chain::ChainId::Bitcoin,
        ));
        let proof = crate::services::unlock::throttle::verified_seed_export_duress_for_test(
            root.clone(),
            "verified-cube".into(),
            crate::chain::ChainId::Bitcoin,
            app_generation,
        );
        drop(tab.update(Message::LocalDuress(proof)));
        assert!(matches!(tab.state, State::DuressActive(_)));
        assert!(!app::session::is_open());
        std::fs::remove_dir_all(root).unwrap();
    }

    /// A duress PIN for the other Cube of a cross-Cube Transfer is minted
    /// with the open Cube as its origin, so the hosting tab activates it.
    #[test]
    fn other_cube_transfer_duress_activates_from_the_open_cubes_app() {
        let _guard = app::session::test_guard();
        app::session::close();
        let root =
            std::env::temp_dir().join(format!("transfer-duress-tab-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let (home, startup) = home::Home::new_for_chain(CoincubeDirectory::new(root.clone()), None);
        drop(startup);
        let mut tab = Tab::new(1, State::Home(home));
        let app_generation = app::cache::AppGeneration::next();
        tab.seed_export_origins.push((
            app_generation,
            root.clone(),
            "open-cube".into(),
            crate::chain::ChainId::Bitcoin,
        ));
        let proof = crate::services::unlock::throttle::destination_unlock_duress(
            &root,
            "open-cube",
            crate::chain::ChainId::Bitcoin,
            app_generation,
        );
        drop(tab.update(Message::Run(app::Message::View(
            app::view::Message::DuressActivateLocal(proof),
        ))));
        assert!(matches!(tab.state, State::DuressActive(_)));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unrelated_or_replayed_proof_cannot_activate_or_clear_another_session() {
        let _guard = app::session::test_guard();
        let root = std::env::temp_dir().join(format!("seed-duress-other-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let (home, startup) = home::Home::new_for_chain(CoincubeDirectory::new(root.clone()), None);
        drop(startup);
        let mut tab = Tab::new(1, State::Home(home));
        let app_generation = app::cache::AppGeneration::next();
        tab.seed_export_origins.push((
            app_generation,
            root.clone(),
            "expected".into(),
            crate::chain::ChainId::Bitcoin,
        ));
        app::session::open("other-cube", zeroize::Zeroizing::new("2468".into()));
        let forged = crate::services::unlock::throttle::verified_seed_export_duress_for_test(
            root.clone(),
            "unrelated".into(),
            crate::chain::ChainId::Bitcoin,
            app_generation,
        );
        assert_eq!(tab.update(Message::LocalDuress(forged)).units(), 0);
        assert!(matches!(tab.state, State::Home(_)));
        assert_eq!(
            app::session::current_cube_id().as_deref(),
            Some("other-cube")
        );
        let proof = crate::services::unlock::throttle::verified_seed_export_duress_for_test(
            root.clone(),
            "expected".into(),
            crate::chain::ChainId::Bitcoin,
            app_generation,
        );
        drop(tab.update(Message::LocalDuress(proof.clone())));
        assert!(matches!(tab.state, State::DuressActive(_)));
        assert!(!app::session::is_open());
        assert_eq!(tab.update(Message::LocalDuress(proof)).units(), 0);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(all(test, feature = "mcp-proposals"))]
mod mcp_proposal_routing_tests {
    use super::*;
    use crate::installer::proposal::tests::TestDir;
    use coincube_core::miniscript::bitcoin::Network;
    fn proposal_tab(
        root: &std::path::Path,
    ) -> (Tab, std::sync::Arc<crate::installer::proposal::Session>) {
        proposal_tab_on_chain(root, crate::chain::ChainId::Regtest)
    }
    fn proposal_tab_on_chain(
        root: &std::path::Path,
        chain: crate::chain::ChainId,
    ) -> (Tab, std::sync::Arc<crate::installer::proposal::Session>) {
        let (ctx, proposal) =
            crate::installer::proposal::tests::authenticated_fixture_for_chain(root, chain);
        let cube = crate::app::settings::Settings::from_file(
            &ctx.coincube_directory.network_directory(chain),
        )
        .unwrap()
        .cubes
        .remove(0);
        let (mut installer, _) = Installer::try_new_for_chain(
            ctx.coincube_directory.clone(),
            chain,
            UserFlow::CreateWallet,
            true,
            Some(cube),
            None,
            None,
            false,
            None,
        )
        .unwrap();
        installer.context = ctx;
        (Tab::new(1, State::Installer(Box::new(installer))), proposal)
    }
    #[test]
    fn mcp_proposal_completion_locks_every_production_chain_before_native_reopen() {
        let _guard = crate::app::session::test_guard();
        for network in coincube_mcp::Chain::ALL {
            let root = TestDir::new();
            let chain = network.as_str().parse().unwrap();
            let (mut tab, proposal) = proposal_tab_on_chain(root.path(), chain);
            let State::Installer(installer) = &mut tab.state else {
                panic!()
            };
            installer.proposal_install_dispatched = true;
            let cube = installer.cube_settings.clone().unwrap();
            let _ = tab.update(Message::Install(installer::Message::CubeSaved(
                Ok((cube, None, None)),
                None,
                None,
            )));
            assert!(matches!(tab.state, State::Home(_)));
            assert!(!proposal.active());
            assert!(!crate::app::session::is_open());
        }
    }
    #[test]
    fn rejected_proposal_handoff_returns_to_the_native_chain_with_an_error() {
        use crate::chain::ChainId;
        let _guard = crate::app::session::test_guard();
        for (target, proposed) in [
            (ChainId::Bitcoin, ChainId::BitcoinBlake2b),
            (ChainId::BitcoinBlake2b, ChainId::Bitcoin),
        ] {
            let root = TestDir::new();
            let (prepared, original) = proposal_tab_on_chain(root.path(), target);
            let State::Installer(mut install) = prepared.state else {
                panic!()
            };
            install.context.mcp_proposal = None;
            drop(original);
            let cube = install.cube_settings.clone().unwrap();

            let foreign = TestDir::new();
            let (mut context, session) =
                crate::installer::proposal::tests::fixture_for_chain(foreign.path(), proposed);
            context.mcp_proposal = None;
            drop(context);
            let session = Arc::try_unwrap(session).ok().unwrap();
            assert!(session.active());
            let (home, startup) = Home::new_for_chain(
                CoincubeDirectory::new(root.path().to_path_buf()),
                Some(target),
            );
            drop(startup);
            let mut tab = Tab::new(1, State::Home(home));
            drop(tab.open_mcp_proposal_handoff(
                *install,
                app::local_mcp::Settings::default(),
                session,
            ));
            let State::Home(home) = &tab.state else {
                panic!("a refused handoff must discard the ordinary installer")
            };
            assert_eq!(home.chain_for_test(), target);
            assert_eq!(home.error(), Some(
                "This proposal belongs to a different network than this Cube. Reopen the Cube and prepare a new proposal."
            ));
            // Deliver the startup feature result through the real tab route;
            // the initial synchronous launcher state is not the final screen.
            drop(
                tab.update(Message::Launch(home::Message::GlobalFeatureLoaded(
                    0, false,
                ))),
            );
            let State::Home(home) = &tab.state else {
                panic!("startup must leave the rejected handoff locked")
            };
            assert_eq!(home.chain_for_test(), target);
            assert_eq!(home.error(), Some(
                "This proposal belongs to a different network than this Cube. Reopen the Cube and prepare a new proposal."
            ));
            let root = CoincubeDirectory::new(root.path().to_path_buf());
            for queued in [
                home::Message::Install(root.clone(), target, UserFlow::CreateWallet, None),
                home::Message::Run(
                    root.clone(),
                    app::config::Config::new(false),
                    target,
                    cube.clone(),
                ),
                home::Message::RunFromFloor(
                    root.clone(),
                    app::config::Config::new(false),
                    target,
                    cube.clone(),
                    0,
                ),
                home::Message::RunToRecovery(
                    root.clone(),
                    app::config::Config::new(false),
                    target,
                    cube.clone(),
                ),
                home::Message::CubeCreated(Ok(cube.clone())),
                home::Message::StartRecovery,
            ] {
                assert!(tab.validate_home_exit(&queued).is_err());
                assert_eq!(tab.update(Message::Launch(queued)).units(), 0);
                assert!(matches!(tab.state, State::Home(_)));
                assert!(tab.pending_recovery_route.is_none());
            }
            assert!(!crate::app::session::is_open());
        }
    }
    #[test]
    fn proposal_back_and_lock_never_reopen_an_app_from_retained_clients() {
        let _guard = crate::app::session::test_guard();
        for manual_lock in [false, true] {
            let root = TestDir::new();
            let (mut tab, proposal) = proposal_tab(root.path());
            assert!(proposal.active());
            let _ = tab.update(if manual_lock {
                Message::LockCube
            } else {
                Message::Install(installer::Message::BackToApp(Network::Regtest))
            });
            assert!(matches!(tab.state, State::Home(_)));
            assert!(!proposal.active());
            assert!(!crate::app::session::is_open());
        }
    }
    #[test]
    fn proposal_exit_preserves_exact_chain_for_back_lock_and_expiry() {
        use crate::app::settings::global::GlobalSettings;
        let _guard = crate::app::session::test_guard();
        for network in coincube_mcp::Chain::ALL {
            for exit in 0..4 {
                let root = TestDir::new();
                // Match native admission of the test networks; retain the
                // launcher's existing developer-mode preference behavior.
                GlobalSettings::update_developer_mode(
                    &GlobalSettings::path(&CoincubeDirectory::new(root.path().to_path_buf())),
                    true,
                )
                .unwrap();
                let chain = network.as_str().parse().unwrap();
                let (mut tab, proposal) = proposal_tab_on_chain(root.path(), chain);
                let task = match exit {
                    // The generic navigation message has only a Bitcoin
                    // encoding; the proposal must retain its own exact chain.
                    0 => tab.update(Message::Install(installer::Message::BackToApp(
                        Network::Regtest,
                    ))),
                    1 => tab.update(Message::LockCube),
                    2 => {
                        crate::app::session::close();
                        tab.on_tick()
                    }
                    _ => {
                        crate::app::session::age_for_proposal_test(
                            Tab::IDLE_LOCK_AFTER + std::time::Duration::from_secs(1),
                        );
                        tab.on_tick()
                    }
                };
                drop(task);
                let State::Home(home) = &tab.state else {
                    panic!("proposal exit must return to the locked launcher")
                };
                assert_eq!(home.chain_for_test(), chain, "exit {exit}");
                assert!(!proposal.active());
                assert!(!crate::app::session::is_open());
            }
        }
    }
    #[test]
    fn proposal_closed_generation_drops_setup_before_admission() {
        let _guard = crate::app::session::test_guard();
        let root = TestDir::new();
        let (mut tab, proposal) = proposal_tab(root.path());
        crate::app::session::close();
        let _ = tab.on_tick();
        assert!(matches!(tab.state, State::Home(_)));
        assert!(!proposal.active());
        assert!(!root.path().join("regtest/data").exists());
    }
    #[test]
    fn proposal_pending_install_cannot_back_out_and_completion_requires_unlock() {
        let _guard = crate::app::session::test_guard();
        let root = TestDir::new();
        let (mut tab, proposal) = proposal_tab(root.path());
        let State::Installer(installer) = &mut tab.state else {
            panic!()
        };
        installer.proposal_install_dispatched = true;
        let cube = installer.cube_settings.clone().unwrap();
        let _ = tab.update(Message::Install(installer::Message::BackToApp(
            Network::Regtest,
        )));
        assert!(matches!(tab.state, State::Installer(_)));
        let _ = tab.update(Message::LockCube);
        let State::Installer(installer) = &tab.state else {
            panic!()
        };
        assert!(installer.proposal_locked);
        assert!(!proposal.active());
        drop(installer.view()); // Only locked text, no old step rendering.
        let _ = tab.update(Message::Install(installer::Message::BackToApp(
            Network::Regtest,
        )));
        assert!(matches!(tab.state, State::Installer(_)));
        let _ = tab.update(Message::Install(installer::Message::CubeSaved(
            Ok((cube, None, None)),
            None,
            None,
        )));
        assert!(matches!(tab.state, State::Home(_)));
        assert!(!crate::app::session::is_open());
    }
    #[test]
    fn proposal_idle_timeout_discards_unadmitted_setup_and_hides_admitted_work() {
        let _guard = crate::app::session::test_guard();
        for pending in [false, true] {
            let root = TestDir::new();
            let (mut tab, proposal) = proposal_tab(root.path());
            if let State::Installer(installer) = &mut tab.state {
                installer.proposal_install_dispatched = pending;
            }
            crate::app::session::age_for_proposal_test(
                Tab::IDLE_LOCK_AFTER + std::time::Duration::from_secs(1),
            );
            let _ = tab.on_tick();
            assert!(!proposal.active());
            assert!(!crate::app::session::is_open());
            if pending {
                let State::Installer(installer) = &tab.state else {
                    panic!()
                };
                assert!(installer.proposal_locked);
            } else {
                assert!(matches!(tab.state, State::Home(_)));
            }
        }
    }
    #[test]
    fn proposal_association_failure_can_exit_without_reopening_or_retrying() {
        let _guard = crate::app::session::test_guard();
        for locked in [false, true] {
            let root = TestDir::new();
            let (mut tab, proposal) = proposal_tab(root.path());
            if let State::Installer(installer) = &mut tab.state {
                installer.proposal_install_dispatched = true;
            }
            if locked {
                let _ = tab.update(Message::LockCube);
            }
            let _ = tab.update(Message::Install(installer::Message::CubeSaved(
                Err("synthetic association failure".into()),
                None,
                None,
            )));
            let State::Installer(installer) = &tab.state else {
                panic!()
            };
            assert!(installer.proposal_finished);
            assert!(installer.proposal_locked);
            assert!(installer.proposal_failure.is_some());
            assert!(!proposal.active());
            drop(installer.view());
            let retry = tab.update(Message::Install(installer::Message::RetryCubeSave));
            assert_eq!(retry.units(), 0);
            let _ = tab.update(Message::Install(installer::Message::BackToApp(
                Network::Regtest,
            )));
            assert!(matches!(tab.state, State::Home(_)));
            assert!(!crate::app::session::is_open());
        }
    }
    #[test]
    fn ordinary_native_failure_releases_reservation_only_after_owned_cleanup() {
        let _guard = crate::app::session::test_guard();
        let root = TestDir::new();
        let (mut tab, _) = proposal_tab(root.path());
        let State::Installer(installer) = &mut tab.state else {
            panic!()
        };
        installer.context.mcp_proposal = None;
        let ctx = installer.context.clone();
        *ctx.creation_lease.lock().unwrap() = crate::installer::proposal::reserve(&ctx).unwrap();
        let wallet = crate::app::settings::WalletId::generate(ctx.descriptor.as_ref().unwrap());
        let directory = ctx
            .coincube_directory
            .network_directory(crate::chain::ChainId::Regtest)
            .coincubed_data_directory(&wallet);
        directory.init().unwrap();
        // A second rejected request does not own the first operation's lease.
        let _ = tab.update(Message::Install(installer::Message::Installed(
            Some(wallet.clone()),
            Err(installer::Error::CreationAdmission("duplicate".into())),
        )));
        assert!(ctx.creation_lease.lock().unwrap().is_some());
        assert!(directory.path().exists());
        // An ordinary failed operation completes its real cleanup before release.
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _enter = runtime.enter();
        let _ = tab.update(Message::Install(installer::Message::Installed(
            Some(wallet),
            Err(installer::Error::Unexpected(
                "synthetic install failure".into(),
            )),
        )));
        assert!(!directory.path().exists());
        assert!(ctx.creation_lease.lock().unwrap().is_none());
        let _retry = crate::installer::proposal::reserve(&ctx)
            .expect("same-context retry must regain the reservation");
        crate::app::session::close();
    }
}

#[cfg(test)]
mod opening_tests {
    use super::{home, unlock_state, Message, State, Tab};
    use crate::app::settings::CubeSettings;
    use crate::chain::ChainId;
    use coincube_core::miniscript::bitcoin::Network;
    use iced::Task;

    fn datadir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "coincube-opening-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn floor() -> home::LauncherSceneContext {
        home::LauncherSceneContext {
            selected: home::CubeCatalogKey {
                chain: ChainId::Bitcoin,
                cube_id: "everyday".into(),
            },
            page: 0,
            selected_slot: 0,
            backup: "Status unknown",
            connect: String::new(),
            wallets: "Not yet known".into(),
            peers: Vec::new(),
            pages: 1,
            chrome: Default::default(),
        }
    }

    /// A tab on a PIN screen that was opened from the floor and has just
    /// had its PIN submitted: the loading view is up, classification runs.
    fn submitted(root: &std::path::Path) -> Tab {
        submitted_with(root, None)
    }

    fn submitted_with(
        root: &std::path::Path,
        wallet_settings: Option<crate::app::settings::WalletSettings>,
    ) -> Tab {
        let cube = CubeSettings::new("Everyday".into(), ChainId::Bitcoin);
        let state = unlock_state(
            cube,
            root.to_path_buf(),
            crate::pin_entry::PinEntrySuccess::LoadApp {
                datadir: crate::dir::CoincubeDirectory::new(root.to_path_buf()),
                config: crate::app::Config::new(false),
                network: Network::Bitcoin,
                internal_bitcoind: None,
                backup: None,
                wallet_settings,
                connect_client: None,
            },
            None,
        );
        let mut tab = Tab::new(1, state);
        let State::PinEntry(pin) = &mut tab.state else {
            unreachable!("a PIN Cube");
        };
        pin.start_loading_for_test();
        tab.unlock_floor = Some(floor());
        tab
    }

    #[test]
    fn the_cube_opens_only_once_its_pin_is_verified() {
        let root = datadir();
        let mut tab = submitted(&root);
        // Classifying: a wrong or duress PIN can still end it, so the Cube
        // keeps its unlock face and the floor it stands on.
        let result = tab.track_opening(false, Task::none());
        drop(result);
        assert!(tab.opening.is_none());
        assert!(tab.unlock_floor.is_some());
        // Verified: the opening takes the floor and holds the load's handle.
        let result = tab.track_opening(true, Task::done(Message::CancelOpening));
        let opening = tab.opening.as_ref().expect("a verified PIN opens the Cube");
        assert!(opening.scene.floor().is_some());
        assert_eq!(opening.loads.len(), 1);
        // Without a Vault there is no daemon: the wallets load next.
        assert_eq!(
            opening.scene.stage(),
            home::launcher_opening::Stage::AlmostThere
        );
        assert!(tab.unlock_floor.is_none());
        drop(result);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// A Cube whose Bitcoin wallet is imported has no Vault id, but its
    /// unlock carries wallet settings and routes to the Loader. The opening
    /// has to start where the Loader does, or the stage line and the light
    /// step back when the Loader takes over.
    #[test]
    fn an_imported_wallet_opens_at_the_loaders_first_stage() {
        let root = datadir();
        let imported: crate::app::settings::WalletSettings =
            serde_json::from_value(serde_json::json!({
                "name": "imported", "alias": null, "descriptor_checksum": "imported",
                "pinned_at": null
            }))
            .unwrap();
        let mut tab = submitted_with(&root, Some(imported));
        let State::PinEntry(pin) = &tab.state else {
            unreachable!("a PIN Cube");
        };
        assert!(pin.cube().vault_wallet_id.is_none());
        drop(tab.track_opening(true, Task::none()));
        let opening = tab.opening.as_ref().expect("a verified PIN opens the Cube");
        assert_eq!(
            opening.scene.stage(),
            home::launcher_opening::Stage::Connecting
        );
        assert_eq!(
            loader_at_connecting(&root).opening_stage(),
            Some(home::launcher_opening::Stage::Connecting)
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    /// A PIN Cube on disk (seed under "1234") and listed in `settings.json`,
    /// with `slot` as its recorded second slot.
    async fn cube_on_disk(
        root: &std::path::Path,
        slot: Option<String>,
    ) -> (CubeSettings, crate::dir::NetworkDirectory) {
        use crate::app::settings::{update_settings_file, Settings};
        use coincube_core::miniscript::bitcoin::secp256k1::Secp256k1;
        use coincube_core::signer::{MasterSigner, MASTER_SEED_LABEL};

        let secp = Secp256k1::signing_only();
        let signer = MasterSigner::generate(Network::Bitcoin).unwrap();
        let mut cube = CubeSettings::new("Everyday".into(), ChainId::Bitcoin);
        cube.created_at = 1000;
        cube.master_signer_fingerprint = Some(signer.fingerprint(&secp));
        cube.duress_slot_file = slot;
        signer
            .store_encrypted(
                root,
                Network::Bitcoin,
                &secp,
                Some((format!("{}{}", MASTER_SEED_LABEL, 1000), 1000)),
                "1234",
                &cube.id,
                None,
            )
            .unwrap();
        let network_dir = crate::dir::CoincubeDirectory::new(root.to_path_buf())
            .network_directory(ChainId::Bitcoin);
        network_dir.init().unwrap();
        let listed = cube.clone();
        update_settings_file(&network_dir, |_| {
            Some(Settings {
                cubes: vec![listed],
                ..Default::default()
            })
        })
        .await
        .unwrap();
        (cube, network_dir)
    }

    /// Start the unlock's seed-file step, poll it once so the work has
    /// begun, then drop it the way Cancel drops the unlock's load.
    async fn start_then_cancel(
        root: &std::path::Path,
        network_dir: &crate::dir::NetworkDirectory,
        cube: &CubeSettings,
    ) {
        use std::future::Future;
        let mut load = Box::pin(super::migrate_seed_files_on_unlock(
            root.to_path_buf(),
            network_dir.clone(),
            cube.clone(),
            zeroize::Zeroizing::new("1234".to_string()),
        ));
        std::future::poll_fn(|cx| {
            assert!(
                load.as_mut().poll(cx).is_pending(),
                "one Argon2 pass is not instant"
            );
            std::task::Poll::Ready(())
        })
        .await;
        drop(load);
    }

    /// Wait for `done`, or fail with `what` after two minutes.
    async fn eventually(what: &str, mut done: impl FnMut() -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        while !done() {
            assert!(std::time::Instant::now() < deadline, "{}", what);
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    /// Cancel drops the unlock's load mid-way. If that lands after the second
    /// slot is written but before its name is recorded, the slot is
    /// unreachable and the next unlock mints another. The name must still
    /// reach `settings.json`, and the next unlock must reuse that slot.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_cancel_mid_backfill_still_records_the_second_slot() {
        use crate::app::settings::Settings;
        use coincube_core::signer::MasterSigner;

        let root = datadir();
        let (cube, network_dir) = cube_on_disk(&root, None).await;
        let folder = MasterSigner::mnemonics_folder(&root, Network::Bitcoin);
        assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 1);

        start_then_cancel(&root, &network_dir, &cube).await;
        let recorded = || Settings::from_file(&network_dir).unwrap().cubes[0].clone();
        eventually("the cancelled backfill never recorded its slot", || {
            recorded().duress_slot_file.is_some()
        })
        .await;
        assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 2);

        // The next unlock finds the recorded slot and mints nothing.
        let again = super::migrate_seed_files_on_unlock(
            root.clone(),
            network_dir.clone(),
            recorded(),
            zeroize::Zeroizing::new("1234".to_string()),
        )
        .await;
        assert!(matches!(
            again,
            Ok(Ok(super::UnlockMigration { new_slot: None, .. }))
        ));
        assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 2);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// The same Cancel, on an enrolled Cube whose slot migration replaces.
    /// The marker is gone once the slot is rewritten; if the disarm did not
    /// follow, `duress.json` would keep claiming an enrolment nothing
    /// triggers, and the next unlock, with current files, would never notice.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_cancel_mid_migration_still_disarms_a_replaced_duress_marker() {
        use crate::services::duress::DuressLocalState;
        use crate::services::unlock::marker;

        let root = datadir();
        // A slot at the wrong wire version: an ungated Cube with no device
        // secret targets v2, so this v3 slot is rewritten as a decoy.
        let slot = marker::new_file_name(1000);
        let (cube, network_dir) = cube_on_disk(&root, Some(slot.clone())).await;
        let secret: coincube_core::seed_crypt::DeviceSecret = zeroize::Zeroizing::new([0x33; 32]);
        marker::write_decoy(&root, ChainId::Bitcoin, &cube.id, &slot, Some(&secret)).unwrap();
        DuressLocalState {
            enrolled: true,
            ..Default::default()
        }
        .save(&root)
        .unwrap();

        start_then_cancel(&root, &network_dir, &cube).await;
        eventually("the cancelled migration never disarmed duress", || {
            !DuressLocalState::load(&root).unwrap().enrolled
        })
        .await;

        // The next unlock: its files are current, so its own migration has
        // nothing to report. The warning the cancelled one never showed has
        // to come from disk, and it stays there until it is shown.
        let again = super::migrate_seed_files_on_unlock(
            root.clone(),
            network_dir.clone(),
            cube.clone(),
            zeroize::Zeroizing::new("1234".to_string()),
        )
        .await;
        assert!(matches!(&again, Ok(Ok(m)) if !m.outcome.duress_was_cleared()));
        assert_eq!(
            super::duress_disarm_notice(&root).as_deref(),
            Some(super::DURESS_DISARM_NOTICE),
            "the duress-off warning was lost with the cancelled unlock"
        );
        assert_eq!(
            super::duress_disarm_notice(&root).as_deref(),
            Some(super::DURESS_DISARM_NOTICE),
            "reading the notice must not clear it"
        );
        super::acknowledge_duress_disarm_notice(&root);
        assert_eq!(super::duress_disarm_notice(&root), None);
        assert!(!DuressLocalState::load(&root).unwrap().enrolled);
        std::fs::remove_dir_all(root).unwrap();
    }

    fn loader_at_connecting(root: &std::path::Path) -> super::Loader {
        let (loader, startup) = super::Loader::new(
            crate::dir::CoincubeDirectory::new(root.to_path_buf()),
            crate::app::Config::new(false),
            Network::Bitcoin,
            None,
            None,
            None,
            CubeSettings::new("Everyday".into(), ChainId::Bitcoin),
            None,
            None,
        );
        // Not polled: no daemon starts in this test.
        drop(startup);
        loader
    }

    #[test]
    fn a_retry_from_a_background_error_page_waits_afresh_before_cancel() {
        use home::launcher_opening::{Opening, Stage, STALL};
        let root = datadir();
        let t0 = std::time::Instant::now();
        let mut tab = Tab::new(1, State::Loader(loader_at_connecting(&root)));
        tab.opening = Some(super::CubeOpening {
            scene: Opening::new(None, false, Stage::Connecting, t0),
            quotes: home::launcher_opening::QuoteCycle::new(
                coincube_ui::component::quote_display::Quote {
                    id: "test".into(),
                    text: "A quote.".into(),
                    author: "Someone".into(),
                    source: "Somewhere".into(),
                    category: String::new(),
                    length: String::new(),
                    contexts: Vec::new(),
                },
                false,
                t0,
            ),
            loads: Vec::new(),
            without_spark: false,
        });
        // The loader fails; the error page holds the wait once, then the
        // tab goes to the background and gets no more ticks.
        let State::Loader(loader) = &mut tab.state else {
            unreachable!()
        };
        loader.fail(crate::loader::Error::Unexpected("refused".into()));
        tab.sync_opening(t0 + std::time::Duration::from_secs(2));
        assert!(tab.opening.is_some(), "an error keeps the opening");
        // A minute later the user is back and presses Retry: the Loader is
        // at Connecting again, the very stage it failed on.
        let retry = t0 + std::time::Duration::from_secs(62);
        tab.state = State::Loader(loader_at_connecting(&root));
        tab.sync_opening(retry);
        let opening = &tab.opening.as_ref().unwrap().scene;
        assert_eq!(opening.stage(), Stage::Connecting);
        assert!(!opening.stalled(retry), "no Cancel straight after Retry");
        assert!(opening.stalled(retry + STALL));
        std::fs::remove_dir_all(root).unwrap();
    }

    /// The opening waits on Spark inside the App, then must still run its
    /// finale: the Ready beat, the rush and the edge-colour wash, with the
    /// App shown only under the wash.
    #[test]
    fn the_finale_washes_once_spark_settles() {
        use crate::app::view::{HomeMessage, Message as ViewMessage};
        use home::launcher_opening::{Opening, Stage};
        use std::sync::Arc;
        use std::time::{Duration, Instant};
        let root = datadir();
        let spark = Arc::new(crate::app::wallets::SparkBackend::new(Arc::new(
            crate::app::breez_spark::SparkClient::disconnected_for_test(),
        )));
        let (app, _load) = crate::app::App::new_without_wallet(
            Arc::new(crate::app::breez_liquid::BreezClient::disconnected(
                Network::Bitcoin,
            )),
            Some(spark),
            crate::app::Config::new(false),
            crate::dir::CoincubeDirectory::new(root.clone()),
            Network::Bitcoin,
            CubeSettings::new("Everyday".into(), ChainId::Bitcoin),
        )
        .unwrap();
        let mut tab = Tab::new(1, State::App(app));
        // Opened from the floor well before: the tumble is long over.
        let t0 = Instant::now() - Duration::from_secs(10);
        tab.opening = Some(super::CubeOpening {
            scene: Opening::new(Some(floor()), false, Stage::AlmostThere, t0),
            quotes: home::launcher_opening::QuoteCycle::new(
                coincube_ui::component::quote_display::Quote {
                    id: "test".into(),
                    text: "A quote.".into(),
                    author: "Someone".into(),
                    source: "Somewhere".into(),
                    category: String::new(),
                    length: String::new(),
                    contexts: Vec::new(),
                },
                false,
                t0,
            ),
            loads: Vec::new(),
            without_spark: false,
        });
        tab.sync_opening(Instant::now());
        let stage = |tab: &Tab| tab.opening.as_ref().map(|o| o.scene.stage());
        assert_eq!(stage(&tab), Some(Stage::Spark), "the App waits on Spark");

        // `Synced` lands while the App's first fetch is in flight, so that
        // fetch's answer is stale and only the next one settles Spark.
        let home = |m| Message::Run(crate::app::Message::View(ViewMessage::Home(m)));
        let balance = || HomeMessage::SparkBalanceUpdated {
            btc: coincube_core::miniscript::bitcoin::Amount::from_sat(1_000),
            usdb_balance: None,
            identity_pubkey: "spark-identity".into(),
        };
        drop(tab.update(home(HomeMessage::SparkSyncedObserved)));
        drop(tab.update(home(balance())));
        drop(tab.update(home(balance())));
        let ready = Instant::now();
        assert_eq!(stage(&tab), Some(Stage::Ready), "a synced balance is Ready");

        let scene = |tab: &Tab, at| {
            tab.opening
                .as_ref()
                .map(|o| (o.scene.wash(at), o.scene.shows_app(at)))
        };
        // The Ready beat holds at full light, App hidden, no wash yet.
        tab.sync_opening(ready + Duration::from_millis(300));
        assert_eq!(
            scene(&tab, ready + Duration::from_millis(300)),
            Some((0.0, false))
        );
        // The rush and wash peak while the face still covers the App.
        let peak = ready + Duration::from_millis(1400);
        tab.sync_opening(peak);
        let (wash, app) = scene(&tab, peak).expect("the finale keeps the opening");
        assert!(wash > 0.9, "the wash peaks, got {}", wash);
        assert!(!app);
        // The App takes over under the wash, which still covers it.
        let under = ready + Duration::from_millis(1550);
        tab.sync_opening(under);
        let (wash, app) = scene(&tab, under).expect("the wash is still fading");
        assert!(app, "the App shows under the wash");
        assert!(wash > 0.5, "the wash still covers the App, got {}", wash);
        // Once the wash has faded, the opening is dropped.
        tab.sync_opening(ready + Duration::from_secs(3));
        assert!(tab.opening.is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_unlock_without_the_launcher_still_honours_reduced_motion() {
        // Auto-lock and a freshly created Cube reach the PIN screen without
        // the Launcher; the system setting alone must still be read.
        let root = datadir();
        let mut tab = submitted(&root);
        tab.unlock_floor = None;
        tab.set_system_reduces_motion(Some(true));
        drop(tab.track_opening(true, Task::none()));
        let opening = tab.opening.as_ref().expect("a verified PIN opens the Cube");
        assert!(opening.scene.floor().is_none());
        assert!(!opening.scene.breathes(), "no flicker or breath");

        let mut tab = submitted(&root);
        tab.set_system_reduces_motion(Some(false));
        drop(tab.track_opening(true, Task::none()));
        assert!(tab.opening.as_ref().unwrap().scene.breathes());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancel_aborts_the_load_and_returns_to_the_launcher_locked() {
        let _session = crate::app::session::test_guard();
        let root = datadir();
        let mut tab = submitted(&root);
        let cube_id = match &tab.state {
            State::PinEntry(pin) => pin.cube().id.clone(),
            _ => unreachable!(),
        };
        crate::app::session::open(cube_id.clone(), zeroize::Zeroizing::new("1234".into()));
        let (load, handle) = Task::<Message>::none().abortable();
        drop(load);
        let result = tab.track_opening(true, Task::none());
        drop(result);
        tab.opening.as_mut().unwrap().loads.push(handle.clone());
        drop(tab.update(Message::CancelOpening));
        assert!(matches!(tab.state, State::Home(_)));
        assert!(tab.opening.is_none());
        assert!(tab.opening_cancelled, "a late load result is dropped");
        assert!(handle.is_aborted());
        assert_ne!(
            crate::app::session::current_cube_id().as_deref(),
            Some(cube_id.as_str()),
            "the Cube is locked"
        );
        // Cancel with nothing opening does nothing.
        drop(tab.update(Message::CancelOpening));
        assert!(matches!(tab.state, State::Home(_)));
        crate::app::session::close();
        std::fs::remove_dir_all(root).unwrap();
    }
}

/// Why an unfinished import's Cube was kept rather than deleted.
const UNFINISHED_CUBE_HOLDS_FUNDS: &str =
    "This Cube holds Spark or Liquid funds, so it was not deleted. Move them to another wallet first.";

#[cfg(test)]
mod lock_to_delete_tests {
    use super::*;

    /// An import abandoned before it finished locks its Cube and asks the
    /// Launcher for that Cube's delete confirmation.
    #[test]
    fn deleting_an_unfinished_cube_locks_it_and_asks_the_launcher() {
        std::thread::Builder::new()
            .name("lock-to-delete".into())
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let _guard = crate::app::session::test_guard();
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let _enter = runtime.enter();
                let path =
                    std::env::temp_dir().join(format!("lock-to-delete-{}", uuid::Uuid::new_v4()));
                let (app, _) = crate::app::shell_tests::bitcoin_app(&path);
                let cube_id = app.cube_settings().id.clone();
                let cube_chain = app.cube_settings().network;
                let mut tab = Tab::new(1, State::App(app));
                let task = tab.update(Message::LockCubeToDelete);
                assert!(matches!(tab.state, State::Home(_)));
                assert!(!crate::app::session::is_open());
                let mut asked = false;
                if let Some(stream) = iced_runtime::task::into_stream(task) {
                    use iced_runtime::futures::futures::StreamExt;
                    let mut stream = Box::pin(stream);
                    let _ = runtime.block_on(tokio::time::timeout(
                        std::time::Duration::from_secs(10),
                        async {
                            while let Some(action) = stream.next().await {
                                if let iced_runtime::Action::Output(Message::Launch(
                                    home::Message::View(home::ViewMessage::DeleteCube(
                                        home::DeleteCubeMessage::ShowModalFor {
                                            cube_id: id,
                                            chain,
                                        },
                                    )),
                                )) = action
                                {
                                    asked = id == cube_id && chain == cube_chain;
                                    break;
                                }
                            }
                        },
                    ));
                }
                assert!(asked, "the Launcher is asked to delete this Cube");
                let _ = std::fs::remove_dir_all(&path);
            })
            .unwrap()
            .join()
            .unwrap();
    }
}

#[cfg(test)]
mod unfinished_cube_funds_tests {
    use super::*;

    /// A Cube whose loaded Spark balance is not zero stays open: Delete
    /// this Cube, or Close on an abandoned import, says why instead of
    /// locking it for deletion.
    #[test]
    fn a_cube_holding_funds_is_kept_rather_than_deleted() {
        std::thread::Builder::new()
            .name("unfinished-cube-funds".into())
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let _guard = crate::app::session::test_guard();
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let _enter = runtime.enter();
                let path = std::env::temp_dir()
                    .join(format!("unfinished-cube-funds-{}", uuid::Uuid::new_v4()));
                let outputs = |tab: &mut Tab| {
                    let task = tab.update(Message::Run(app::Message::View(
                        app::view::Message::DeleteUnfinishedCube,
                    )));
                    let mut outputs = Vec::new();
                    if let Some(stream) = iced_runtime::task::into_stream(task) {
                        use iced_runtime::futures::futures::StreamExt;
                        let mut stream = Box::pin(stream);
                        let _ = runtime.block_on(tokio::time::timeout(
                            std::time::Duration::from_secs(10),
                            async {
                                while let Some(action) = stream.next().await {
                                    // App-bound output is tagged with the
                                    // App's generation.
                                    if let iced_runtime::Action::Output(message) = action {
                                        outputs.push(match message {
                                            Message::AppAsync(_, inner) => *inner,
                                            message => message,
                                        });
                                    }
                                }
                            },
                        ));
                    }
                    outputs
                };

                let (mut app, _) = crate::app::shell_tests::bitcoin_app(&path);
                app.set_loaded_spark_balance_for_test(1_000);
                let mut tab = Tab::new(1, State::App(app));
                let sent = outputs(&mut tab);
                assert!(matches!(tab.state, State::App(_)));
                assert!(!sent
                    .iter()
                    .any(|message| matches!(message, Message::LockCubeToDelete)));
                assert!(sent.iter().any(|message| matches!(
                    message,
                    Message::Run(app::Message::View(app::view::Message::ShowError(error)))
                        if error == UNFINISHED_CUBE_HOLDS_FUNDS
                )));
                drop(tab);

                // No balance loaded: the Launcher's confirmation decides.
                let unloaded = path.join("unloaded");
                let (app, _) = crate::app::shell_tests::bitcoin_app(&unloaded);
                let mut tab = Tab::new(2, State::App(app));
                let sent = outputs(&mut tab);
                assert!(sent
                    .iter()
                    .any(|message| matches!(message, Message::LockCubeToDelete)));
                drop(tab);
                let _ = std::fs::remove_dir_all(&path);
            })
            .unwrap()
            .join()
            .unwrap();
    }
}

#[cfg(test)]
mod spark_nickname_flush_tests {
    use super::*;

    /// Lock Cube replaces the App, so a Spark nickname typed on Balances &
    /// conversion and not yet saved is written as the tab locks, as leaving
    /// the tab would write it.
    #[test]
    fn locking_writes_a_typed_spark_nickname() {
        std::thread::Builder::new()
            .name("spark-nickname-lock".into())
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let _guard = crate::app::session::test_guard();
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let _enter = runtime.enter();
                let path = std::env::temp_dir()
                    .join(format!("spark-nickname-lock-{}", uuid::Uuid::new_v4()));
                let (mut app, _) = crate::app::shell_tests::bitcoin_app(&path);
                let network_dir = app.datadir().network_directory(app.cube_settings().network);
                let cube = app.cube_settings().clone();
                let cube_id = cube.id.clone();
                runtime
                    .block_on(app::settings::update_settings_file(
                        &network_dir,
                        move |mut s| {
                            s.cubes.push(cube);
                            Some(s)
                        },
                    ))
                    .unwrap();
                drop(app.update(app::Message::View(app::view::Message::Menu(
                    app::menu::Menu::Spark(app::menu::SparkSubMenu::Settings(Some(
                        app::menu::SparkSettingsOption::General,
                    ))),
                ))));
                drop(
                    app.update(app::Message::View(app::view::Message::SparkSettings(
                        app::view::SparkSettingsMessage::NicknameEdited("Daily spending".into()),
                    ))),
                );

                let mut tab = Tab::new(1, State::App(app));
                let task = tab.update(Message::LockCubeToLauncher);
                assert!(matches!(tab.state, State::Home(_)));
                // Run the lock's tasks, the write among them; the launcher's
                // own loading may take longer, so this waits only so long.
                if let Some(stream) = iced_runtime::task::into_stream(task) {
                    use iced_runtime::futures::futures::StreamExt;
                    let _ = runtime.block_on(tokio::time::timeout(
                        std::time::Duration::from_secs(10),
                        stream.for_each(|_| async {}),
                    ));
                }
                let saved = app::settings::Settings::from_file(&network_dir)
                    .unwrap()
                    .cubes
                    .into_iter()
                    .find(|cube| cube.id == cube_id)
                    .and_then(|cube| cube.spark_alias);
                assert_eq!(saved.as_deref(), Some("Daily spending"));
                let _ = std::fs::remove_dir_all(&path);
            })
            .unwrap()
            .join()
            .unwrap();
    }
}
