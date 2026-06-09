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
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::mpsc::UnboundedSender;

use super::cipher::DeviceKey;
use super::journal::WipeJournal;
use super::queue::DuressQueue;
use super::wipe::CubeWiper;
use super::{DuressEvent, DuressLocalState, PendingActivation};

/// The network side of activation, abstracted so the orchestrator can be tested
/// without a live HTTP stack and so the wipe ordering can be exercised against
/// a slow/failing/hanging server.
#[async_trait]
pub trait DuressTrigger: Send + Sync + 'static {
    /// Fire the unauthenticated `trigger-with-code` POST. `Ok(())` on success.
    async fn trigger(&self, account_id: &str, duress_code: &str) -> Result<(), String>;
}

#[async_trait]
impl DuressTrigger for crate::services::coincube::CoincubeClient {
    async fn trigger(&self, account_id: &str, duress_code: &str) -> Result<(), String> {
        self.trigger_duress_with_code(account_id, duress_code)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

/// The wall-clock budget the *background* POST task is given before it gives up
/// and leaves the entry for the drain loop. The wipe never waits on this.
const POST_TIMEOUT: Duration = Duration::from_secs(5);

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
    cipher: DeviceKey,
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
        cipher: DeviceKey,
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

    /// Activates duress: durably records intent, kicks off the POST in the
    /// background, wipes in parallel, persists `active = true`, and emits
    /// [`DuressEvent::Activated`].
    pub async fn activate(&mut self, account_id: String) -> Result<(), DuressError> {
        // The code is read from local state, NOT passed in by the caller — the
        // user never sees or types it. It's stored encrypted at rest; decrypt
        // to a short-lived plaintext for the POST only.
        let encrypted = self
            .local_state
            .duress_code
            .clone()
            .ok_or(DuressError::NotEnrolledLocally)?;
        let plaintext = self
            .cipher
            .decrypt(&encrypted)
            .map_err(DuressError::Decrypt)?;

        // 1. Journal marker FIRST. Synchronous, fast, durable.
        self.journal
            .mark_pending_activation(&account_id)
            .map_err(DuressError::Io)?;

        // 2. Enqueue (store the *encrypted* code, never plaintext). Durable
        //    source of truth — survives a power-pull in the next millisecond.
        self.queue
            .enqueue(PendingActivation {
                account_id: account_id.clone(),
                duress_code: encrypted,
                enqueued_at: Utc::now(),
                attempts: 0,
            })
            .map_err(DuressError::Io)?;

        // 3. Kick off the POST in the BACKGROUND. Fire-and-forget. The wipe
        //    (step 4) does NOT wait for this. An attacker who controls the
        //    network MUST NOT be able to delay the wipe.
        let client = self.client.clone();
        let queue = self.queue.clone();
        let acct = account_id.clone();
        tokio::spawn(async move {
            Self::run_post(client, queue, acct, plaintext).await;
        });

        // 4. Wipe runs IN PARALLEL with the POST above. Starts immediately;
        //    never gated on any network condition.
        self.wipe.execute_atomic().map_err(DuressError::Io)?;

        // 5. Persist active state (the file survives the wipe, so on relaunch
        //    we route straight to the cryptic screen).
        self.local_state.active = true;
        self.local_state.last_activation_attempt = Some(Utc::now());
        self.local_state
            .save(&self.data_dir)
            .map_err(DuressError::Io)?;

        // 6. Transition UI to the cryptic activation screen.
        if let Some(tx) = &self.event_tx {
            let _ = tx.send(DuressEvent::Activated);
        }
        Ok(())
    }

    /// The fire-and-forget POST attempt: time-boxed, dequeues on success, leaves
    /// the entry for the drain loop on failure/timeout. Also used directly by
    /// tests to assert the success/failure → queue behaviour deterministically.
    pub async fn run_post(
        client: Arc<dyn DuressTrigger>,
        queue: DuressQueue,
        account_id: String,
        plaintext_code: String,
    ) {
        let result =
            tokio::time::timeout(POST_TIMEOUT, client.trigger(&account_id, &plaintext_code)).await;
        if let Ok(Ok(())) = result {
            let _ = queue.dequeue(&account_id);
        }
        // else: leave in queue; the Phase 4 drain loop retries.
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
    use tokio::sync::Notify;

    /// Mock trigger that records calls and can be configured to succeed, fail,
    /// or hang past the POST timeout.
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
        async fn trigger(&self, account_id: &str, _code: &str) -> Result<(), String> {
            self.calls.lock().unwrap().push(account_id.to_string());
            self.notify.notify_one();
            match self.behavior {
                Behavior::Ok => Ok(()),
                Behavior::Err => Err("boom".to_string()),
                Behavior::Hang => {
                    // Sleep well past POST_TIMEOUT so the timeout fires.
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
            cipher,
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
        h.orch.activate("acct_1".to_string()).await.unwrap();

        // Cube data gone.
        assert!(!h.dir.join("bitcoin").join("data").exists());
        // Active state persisted.
        let reloaded = DuressLocalState::load(&h.dir).unwrap();
        assert!(reloaded.active);
        // Activated event emitted.
        assert_eq!(h.rx.recv().await, Some(DuressEvent::Activated));
        // POST eventually fired and drained the queue.
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
        h.orch.activate("acct_2".to_string()).await.unwrap();
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
        h.orch.activate("acct_3".to_string()).await.unwrap();
        assert!(!h.dir.join("bitcoin").join("data").exists());
        assert!(DuressLocalState::load(&h.dir).unwrap().active);
        let _ = std::fs::remove_dir_all(&h.dir);
    }

    #[tokio::test]
    async fn activate_without_code_errors() {
        let mut h = build(Behavior::Ok);
        h.orch.local_state.duress_code = None;
        let err = h.orch.activate("acct_x".to_string()).await.unwrap_err();
        assert!(matches!(err, DuressError::NotEnrolledLocally));
        // Nothing wiped, no journal left dangling.
        assert!(h.dir.join("bitcoin").join("data").exists());
        assert!(!h.orch.journal.is_pending());
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
