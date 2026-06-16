//! Duress activation orchestrator (Phase 3, Task 3.2).
//!
//! This is the heart of the feature. The ordering invariant is sacred:
//!
//!   1. Validate the duress PIN (done by the caller, fast + local).
//!   2. Write the wipe journal marker (fast, local, durable).
//!   3. Enqueue the `PendingActivation` (fast, local, durable) — the source of
//!      truth that the POST will eventually fire.
//!   4. **In parallel:** kick off the activation `POST` as a fire-and-forget
//!      background task **AND** begin the atomic wipe. The wipe is *never*
//!      gated on any network condition.
//!   5. After the wipe completes, transition to the cryptic screen.
//!
//! An attacker who controls the network must not be able to keep Cube data on
//! disk by stalling the POST. With this ordering the wipe starts the instant
//! the queue commit returns, and the queue's durability guarantees the POST
//! eventually lands (immediately if it succeeds, else via the Phase 4 drain
//! loop). Both the POST and the wipe may finish in either order; every code
//! path handles both orderings.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::mpsc::UnboundedSender;

use super::cipher::DeviceKey;
use super::drain::DuressDrainer;
use super::journal::WipeJournal;
use super::queue::DuressQueue;
use super::wipe::CubeWiper;
use super::{DuressEvent, DuressLocalState, PendingActivation};

/// Whether a failed activation POST should be retried by the drain loop or
/// abandoned. The wipe has already happened, so a terminal error just means
/// "we did our best" — drop the entry rather than retry forever.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerError {
    /// Transient (network down, 429, 5xx) — keep the entry, retry later.
    Retriable,
    /// Permanent (4xx other than 429: bad request, already cleared, etc.) —
    /// log and drop the entry.
    Terminal,
}

/// The network side of activation, abstracted so the orchestrator can be tested
/// without a live HTTP stack and so the wipe ordering can be exercised against
/// a slow/failing/hanging server.
#[async_trait]
pub trait DuressTrigger: Send + Sync + 'static {
    /// Fire the unauthenticated `trigger-with-code` POST. `Ok(())` on success.
    async fn trigger(&self, account_id: &str, duress_code: &str) -> Result<(), TriggerError>;
}

#[async_trait]
impl DuressTrigger for crate::services::coincube::CoincubeClient {
    async fn trigger(&self, account_id: &str, duress_code: &str) -> Result<(), TriggerError> {
        use crate::services::coincube::CoincubeError;
        match self.trigger_duress_with_code(account_id, duress_code).await {
            Ok(_) => Ok(()),
            Err(CoincubeError::Network(_)) | Err(CoincubeError::RateLimited { .. }) => {
                Err(TriggerError::Retriable)
            }
            Err(CoincubeError::Unsuccessful(info)) if info.status_code >= 500 => {
                Err(TriggerError::Retriable)
            }
            // 4xx (other than 429, handled above), parse errors, etc. — the
            // server will never accept this request; stop retrying.
            Err(_) => Err(TriggerError::Terminal),
        }
    }
}

/// How many times the wipe-journal marker write is retried on a transient IO
/// error. The marker is the durability anchor that lets the launch-time
/// reconcile finish an interrupted wipe, so a dropped write removes the
/// crash-recovery safety net — worth a few retries. The write is idempotent (it
/// re-creates the same marker), so retrying can never corrupt it.
const JOURNAL_RETRIES: u32 = 3;

/// How many times the durable enqueue is retried on a transient IO error. The
/// queue entry is the only thing that drives the server-side lock, so a dropped
/// commit means the account is never locked — worth a few retries. enqueue is
/// atomic and idempotent per account, so retrying can never duplicate.
const ENQUEUE_RETRIES: u32 = 3;

/// How many times the atomic wipe is retried on a transient lock/IO error
/// before giving up for this session. CubeWiper never clears the journal on a
/// failed pass, so even if every attempt fails the launch-time reconcile
/// finishes the wipe on next launch.
const WIPE_RETRIES: u32 = 3;

#[derive(Debug)]
pub enum DuressError {
    /// No duress code is present in local state — this desktop never enrolled
    /// or registered a code, so it cannot activate.
    NotEnrolledLocally,
    /// The stored code envelope could not be decrypted with the device key.
    Decrypt(String),
    /// A durable write (journal / queue / state) failed.
    Io(std::io::Error),
}

impl std::fmt::Display for DuressError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DuressError::NotEnrolledLocally => write!(f, "duress not enrolled on this device"),
            DuressError::Decrypt(e) => write!(f, "duress code decrypt failed: {e}"),
            DuressError::Io(e) => write!(f, "duress durable write failed: {e}"),
        }
    }
}

impl std::error::Error for DuressError {}

/// Owns the durable stores and drives activation. Constructed by the app with
/// the real Cube data roots; constructed by tests with temp dirs and a mock
/// [`DuressTrigger`].
pub struct DuressOrchestrator {
    pub local_state: DuressLocalState,
    data_dir: PathBuf,
    journal: WipeJournal,
    queue: DuressQueue,
    wipe: CubeWiper,
    /// Device key for decrypting the queued duress code before the POST. `None`
    /// when the key is unreadable — construction stays infallible so the wipe
    /// (the trust anchor) always runs; only the server POST is skipped.
    cipher: Option<DeviceKey>,
    client: Arc<dyn DuressTrigger>,
    event_tx: Option<UnboundedSender<DuressEvent>>,
}

impl DuressOrchestrator {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        local_state: DuressLocalState,
        data_dir: PathBuf,
        journal: WipeJournal,
        queue: DuressQueue,
        wipe: CubeWiper,
        cipher: Option<DeviceKey>,
        client: Arc<dyn DuressTrigger>,
        event_tx: Option<UnboundedSender<DuressEvent>>,
    ) -> Self {
        Self {
            local_state,
            data_dir,
            journal,
            queue,
            wipe,
            cipher,
            client,
            event_tx,
        }
    }

    /// Activates duress: durably records intent, kicks off the server POST in
    /// the background (Connect enrollments only), wipes Cube data in parallel,
    /// persists `active = true`, and emits [`DuressEvent::Activated`].
    ///
    /// **This is the single production trust anchor for local duress
    /// activation** — the GUI's PIN-entry path delegates here (see
    /// `gui/tab.rs`). Do NOT re-introduce an inline copy of this sequence:
    /// drift between two copies of a security-critical path is exactly what this
    /// consolidation removed.
    ///
    /// `account_id` is the enrolled Connect account, threaded explicitly from
    /// the PIN-entry success path (`None` for sovereign / no-Connect
    /// enrollment). It is cross-checked against the locally-persisted enrollment
    /// id, and the device falls back to the persisted id if the threaded value
    /// is missing, so the server-side lock is never silently skipped.
    ///
    /// The ordering is sacred: journal → enqueue → spawn POST → wipe. The wipe
    /// is the anchor and runs **regardless** of whether the journal write,
    /// enqueue, or POST succeed — never gated on any network condition. Showing
    /// a normal app with Cube data after a duress trigger would be far worse
    /// than a missed server signal, so only a wipe that fails every retry is
    /// surfaced as an error (the journal is retained so the launch-time
    /// reconcile finishes it); the caller locks into the cryptic screen either
    /// way.
    pub async fn activate(&mut self, account_id: Option<String>) -> Result<(), DuressError> {
        // Prefer the explicitly-threaded account id; fall back to the enrolled
        // id in local state so a failed read at PIN-entry time can't silently
        // drop the server lock. Warn (don't fail) if the two ever diverge — they
        // shouldn't, there is one Connect account per desktop.
        let enrolled = self.local_state.account_id.clone();
        if let (Some(passed), Some(enrolled)) = (account_id.as_ref(), enrolled.as_ref()) {
            if passed != enrolled {
                // Don't log the raw account ids — keep identifiers out of
                // warn-level logs (which surface in support bundles); the values
                // live in duress-state.json if ever needed for debugging. This is
                // a should-never-happen divergence (one Connect account per
                // desktop); we proceed with the explicitly-threaded value.
                log::warn!(
                    "duress: activation account id differs from the enrolled id; \
                     using the threaded value"
                );
            }
        }
        let account_id = account_id.or(enrolled);

        // 1. Journal marker FIRST. Fast, local, durable. The recorded account id
        //    lets the launch-time reconcile finish an interrupted wipe, so it's
        //    the crash-recovery anchor — retry it on a transient error. A total
        //    failure is logged distinctly but never fatal: the wipe (step 4)
        //    still runs (showing Cube data after a duress trigger is worse than a
        //    non-recoverable partial wipe), but operators must know that an
        //    interrupted wipe will NOT be auto-completed on next launch.
        let mut journaled = false;
        for attempt in 1..=JOURNAL_RETRIES {
            match self
                .journal
                .mark_pending_activation(account_id.as_deref().unwrap_or(""))
            {
                Ok(()) => {
                    journaled = true;
                    break;
                }
                Err(e) => log::error!(
                    "duress: wipe-journal write attempt {attempt}/{JOURNAL_RETRIES} failed: {e}"
                ),
            }
        }
        if !journaled {
            log::error!(
                "duress: wipe journal could not be persisted; the wipe will still run, but an \
                 interrupted wipe will NOT be auto-completed on next launch"
            );
        }

        // 2 + 3. Connect tiers only — a Some account AND a stored code. Durably
        //    enqueue the *encrypted* code (never plaintext) BEFORE the wipe, then
        //    drive the POST from the background drainer IN PARALLEL with the
        //    wipe. The drainer fires immediately and KEEPS retrying with backoff
        //    until it lands, so a coerced account is locked even if the first
        //    attempt is offline and the user never leaves the cryptic screen. The
        //    wipe never waits on any of this. Sovereign devices skip straight to
        //    the wipe with no server signal.
        //
        //    The enqueue stores the already-encrypted code, so it needs NO device
        //    key — the durable queue entry (the only thing that drives the
        //    server-side lock) is committed even when the key is missing or
        //    transiently unreadable. Only the in-session drainer needs the key to
        //    decrypt in-flight; without it, the launch-time drainer fires the
        //    POST on a later launch once the key is readable again.
        if let (Some(acct), Some(encrypted)) =
            (account_id.as_ref(), self.local_state.duress_code.clone())
        {
            let pending = PendingActivation {
                account_id: acct.clone(),
                duress_code: encrypted,
                enqueued_at: Utc::now(),
                attempts: 0,
            };
            let mut enqueued = false;
            for attempt in 1..=ENQUEUE_RETRIES {
                match self.queue.enqueue(pending.clone()) {
                    Ok(()) => {
                        enqueued = true;
                        break;
                    }
                    Err(e) => log::error!(
                        "duress: enqueue activation attempt {attempt}/{ENQUEUE_RETRIES} failed: {e}"
                    ),
                }
            }
            if !enqueued {
                log::error!("duress: activation not durably queued; server-side lock may not fire");
            }
            // Fire-and-forget background POST driver — only when the device key
            // is available to decrypt the queued code in-flight. It retries with
            // backoff until the queue drains and is never awaited here. With no
            // key, the durable entry above waits for the launch-time drainer.
            match self.cipher.clone() {
                Some(cipher) => {
                    let drainer =
                        DuressDrainer::new(self.queue.clone(), cipher, self.client.clone());
                    // Guard the spawn: in some executor contexts there is no
                    // current Tokio runtime, and a bare `tokio::spawn` would
                    // PANIC — unwinding `activate` before the wipe (step 4)
                    // runs. No runtime → skip the in-session drain; the durable
                    // queue entry is already committed, so the launch-time
                    // drainer fires the POST on next start.
                    match tokio::runtime::Handle::try_current() {
                        Ok(handle) => {
                            handle.spawn(async move { drainer.run_until_empty().await });
                        }
                        Err(_) => log::warn!(
                            "duress: no Tokio runtime to start the activation drainer now; \
                             server POST left for the launch-time drainer"
                        ),
                    }
                }
                None => log::warn!(
                    "duress: device key unavailable at activation; server POST left for the \
                     launch-time drainer to fire once the key is readable"
                ),
            }
        }

        // 4. Wipe (anchor) — runs IN PARALLEL with the POST above; starts
        //    immediately and is never gated on any network condition. Retry so a
        //    transient lock/IO error doesn't leave Cube seeds or PIN material on
        //    disk.
        let mut wiped = false;
        let mut last_err = None;
        for attempt in 1..=WIPE_RETRIES {
            match self.wipe.execute_atomic() {
                Ok(()) => {
                    wiped = true;
                    break;
                }
                Err(e) => {
                    log::error!("duress: wipe attempt {attempt}/{WIPE_RETRIES} failed: {e}");
                    last_err = Some(e);
                }
            }
        }

        // 5. Persist active state (the file survives the wipe, so on relaunch we
        //    route straight to the cryptic screen). Logged-not-fatal: the journal
        //    already guards an interrupted wipe.
        self.local_state.active = true;
        self.local_state.last_activation_attempt = Some(Utc::now());
        if let Err(e) = self.local_state.save(&self.data_dir) {
            log::error!("duress: failed to persist active state: {e}");
        }

        // 6. Transition UI to the cryptic activation screen.
        if let Some(tx) = &self.event_tx {
            let _ = tx.send(DuressEvent::Activated);
        }

        match (wiped, last_err) {
            (true, _) => Ok(()),
            (false, Some(e)) => Err(DuressError::Io(e)),
            // Unreachable: WIPE_RETRIES >= 1, so a non-wipe leaves an error.
            (false, None) => Ok(()),
        }
    }

    /// Remote activation received over the gRPC stream (Phase 7b). Locks the
    /// screen but does **NOT** wipe — remote activation can be accidental
    /// (Keychain Settings tap, mis-entered CRK password) and wiping then would
    /// be too destructive.
    pub fn handle_remote_activation(
        &mut self,
        unlock_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<(), DuressError> {
        self.local_state.active = true;
        self.local_state.unlock_at = unlock_at;
        self.local_state
            .save(&self.data_dir)
            .map_err(DuressError::Io)?;
        if let Some(tx) = &self.event_tx {
            let _ = tx.send(DuressEvent::ActivatedRemote);
        }
        Ok(())
    }

    /// Duress cleared server-side (gRPC `DuressCleared` or a launch/sign-in
    /// reconcile). Exits the cryptic screen.
    pub fn handle_remote_clear(&mut self) -> Result<(), DuressError> {
        self.local_state.active = false;
        self.local_state.unlock_at = None;
        self.local_state
            .save(&self.data_dir)
            .map_err(DuressError::Io)?;
        if let Some(tx) = &self.event_tx {
            let _ = tx.send(DuressEvent::Cleared);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::Duration;
    use tokio::sync::Notify;

    /// Mock trigger that records calls and can be configured to succeed, fail,
    /// or hang past any reasonable timeout.
    struct MockTrigger {
        calls: Arc<Mutex<Vec<String>>>,
        notify: Arc<Notify>,
        behavior: Behavior,
    }

    #[derive(Clone, Copy)]
    enum Behavior {
        Ok,
        Err,
        Hang,
    }

    #[async_trait]
    impl DuressTrigger for MockTrigger {
        async fn trigger(&self, account_id: &str, _code: &str) -> Result<(), TriggerError> {
            self.calls.lock().unwrap().push(account_id.to_string());
            self.notify.notify_one();
            match self.behavior {
                Behavior::Ok => Ok(()),
                Behavior::Err => Err(TriggerError::Retriable),
                Behavior::Hang => {
                    // Sleep effectively forever so a caller that (wrongly)
                    // awaited the POST would hang with it.
                    tokio::time::sleep(Duration::from_secs(3600)).await;
                    Ok(())
                }
            }
        }
    }

    struct Harness {
        dir: PathBuf,
        orch: DuressOrchestrator,
        calls: Arc<Mutex<Vec<String>>>,
        notify: Arc<Notify>,
        rx: tokio::sync::mpsc::UnboundedReceiver<DuressEvent>,
    }

    static TEST_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    fn build(behavior: Behavior) -> Harness {
        let seq = TEST_SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "coincube-duress-orch-{}-{}",
            std::process::id(),
            seq
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Seed Cube data to be wiped.
        let cube_root = dir.join("bitcoin").join("data");
        std::fs::create_dir_all(cube_root.join("cube_a")).unwrap();
        std::fs::write(cube_root.join("cube_a").join("wallet.db"), b"SECRET").unwrap();

        let cipher = DeviceKey::from_bytes([3u8; 32]);
        let encrypted = cipher.encrypt("raw-duress-code").unwrap();
        let state = DuressLocalState {
            enrolled: true,
            duress_code: Some(encrypted),
            ..Default::default()
        };
        state.save(&dir).unwrap();

        let journal = WipeJournal::new(&dir);
        let queue = DuressQueue::new(&dir);
        let wipe = CubeWiper::new(vec![cube_root], journal.clone());

        let calls = Arc::new(Mutex::new(Vec::new()));
        let notify = Arc::new(Notify::new());
        let client = Arc::new(MockTrigger {
            calls: calls.clone(),
            notify: notify.clone(),
            behavior,
        });
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

        let orch = DuressOrchestrator::new(
            state,
            dir.clone(),
            journal,
            queue,
            wipe,
            Some(cipher),
            client,
            Some(tx),
        );
        Harness {
            dir,
            orch,
            calls,
            notify,
            rx,
        }
    }

    #[tokio::test]
    async fn activate_wipes_and_emits_event() {
        let mut h = build(Behavior::Ok);
        h.orch.activate(Some("acct_1".to_string())).await.unwrap();

        // Cube data gone.
        assert!(!h.dir.join("bitcoin").join("data").exists());
        // Active state persisted.
        let reloaded = DuressLocalState::load(&h.dir).unwrap();
        assert!(reloaded.active);
        // Activated event emitted.
        assert_eq!(h.rx.recv().await, Some(DuressEvent::Activated));
        // Background drainer fired the POST and drained the queue.
        h.notify.notified().await;
        // Give the spawned dequeue a moment to land.
        for _ in 0..50 {
            if h.orch_queue_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(h.calls.lock().unwrap().as_slice(), &["acct_1".to_string()]);
        let _ = std::fs::remove_dir_all(&h.dir);
    }

    #[tokio::test]
    async fn activate_with_post_error_retains_queue_entry() {
        let mut h = build(Behavior::Err);
        h.orch.activate(Some("acct_2".to_string())).await.unwrap();
        assert!(
            !h.dir.join("bitcoin").join("data").exists(),
            "wipe still ran"
        );
        // Wait for the POST to be attempted and fail.
        h.notify.notified().await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        // Entry retained for the drain loop.
        assert!(!h.orch_queue_empty());
        let _ = std::fs::remove_dir_all(&h.dir);
    }

    #[tokio::test(start_paused = true)]
    async fn activate_does_not_wait_on_hung_post() {
        let mut h = build(Behavior::Hang);
        // activate() must return promptly even though the POST hangs forever —
        // the wipe is not gated on the network.
        h.orch.activate(Some("acct_3".to_string())).await.unwrap();
        assert!(!h.dir.join("bitcoin").join("data").exists());
        assert!(DuressLocalState::load(&h.dir).unwrap().active);
        let _ = std::fs::remove_dir_all(&h.dir);
    }

    #[tokio::test]
    async fn activate_without_code_still_wipes() {
        // A duress signal MUST wipe even with no server code to POST — the local
        // wipe is the trust anchor; an absent code just means no server signal
        // fires. (Contrast the old behaviour, which bailed without wiping.)
        let mut h = build(Behavior::Ok);
        h.orch.local_state.duress_code = None;
        h.orch.activate(Some("acct_x".to_string())).await.unwrap();
        assert!(
            !h.dir.join("bitcoin").join("data").exists(),
            "wipe still ran without a code"
        );
        // Nothing enqueued (no code → no POST), journal cleared after the wipe.
        assert!(
            h.orch_queue_empty(),
            "no server POST enqueued without a code"
        );
        assert!(!h.orch.journal.is_pending());
        let _ = std::fs::remove_dir_all(&h.dir);
    }

    #[tokio::test]
    async fn activate_enqueues_even_without_device_key() {
        // Regression: a missing/transiently-unreadable device key must NOT drop
        // the durable server-lock entry. The encrypted code is already in local
        // state, so the queue commit needs no cipher; the launch-time drainer
        // fires the POST once the key is readable again.
        let mut h = build(Behavior::Ok);
        h.orch.cipher = None;
        h.orch.activate(Some("acct_k".to_string())).await.unwrap();
        assert!(
            !h.dir.join("bitcoin").join("data").exists(),
            "wipe still ran"
        );
        // Durable entry committed despite the absent key.
        assert!(
            !h.orch_queue_empty(),
            "server lock queued without a device key"
        );
        // No in-session drainer could be spawned (no key to decrypt), so the
        // POST was not attempted now — it waits for the launch-time drainer.
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(
            h.calls.lock().unwrap().is_empty(),
            "no POST without the device key"
        );
        let _ = std::fs::remove_dir_all(&h.dir);
    }

    #[tokio::test]
    async fn activate_sovereign_wipes_without_server_post() {
        // Sovereign (no account id): wipe locally with no enqueue and no POST.
        let mut h = build(Behavior::Ok);
        h.orch.activate(None).await.unwrap();
        assert!(!h.dir.join("bitcoin").join("data").exists());
        assert!(DuressLocalState::load(&h.dir).unwrap().active);
        assert_eq!(h.rx.recv().await, Some(DuressEvent::Activated));
        assert!(
            h.orch_queue_empty(),
            "sovereign never enqueues a server POST"
        );
        // The trigger was never spawned/called.
        assert!(h.calls.lock().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&h.dir);
    }

    #[tokio::test]
    async fn remote_activation_does_not_wipe() {
        let mut h = build(Behavior::Ok);
        h.orch.handle_remote_activation(None).unwrap();
        // Cube data still present — remote activation never wipes.
        assert!(h.dir.join("bitcoin").join("data").join("cube_a").exists());
        assert!(DuressLocalState::load(&h.dir).unwrap().active);
        assert_eq!(h.rx.recv().await, Some(DuressEvent::ActivatedRemote));
        let _ = std::fs::remove_dir_all(&h.dir);
    }

    #[tokio::test]
    async fn remote_clear_exits() {
        let mut h = build(Behavior::Ok);
        h.orch.handle_remote_activation(None).unwrap();
        let _ = h.rx.recv().await;
        h.orch.handle_remote_clear().unwrap();
        assert!(!DuressLocalState::load(&h.dir).unwrap().active);
        assert_eq!(h.rx.recv().await, Some(DuressEvent::Cleared));
        let _ = std::fs::remove_dir_all(&h.dir);
    }

    impl Harness {
        fn orch_queue_empty(&self) -> bool {
            DuressQueue::new(&self.dir).is_empty().unwrap()
        }
    }
}
