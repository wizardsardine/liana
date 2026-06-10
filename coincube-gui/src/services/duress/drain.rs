//! Background retry-queue drain loop (Phase 4).
//!
//! On app launch and on a network-state-change-to-online, the drainer walks the
//! durable [`DuressQueue`](super::queue::DuressQueue) and tries to land each
//! pending activation `POST`. The queue was committed before the wipe, so this
//! is what guarantees an offline-at-activation device eventually signals
//! Connect — possibly days later, on the next online launch.
//!
//! Backoff between whole-queue passes: 5s → 30s → 5m → 30m → 1h, then 1h
//! forever. Per-entry outcomes:
//!   * success → dequeue,
//!   * terminal (4xx≠429) → log + dequeue (the wipe already happened; we did
//!     our best),
//!   * retriable (429 / 5xx / network) → leave, bump `attempts`, retry next
//!     pass.

use std::sync::Arc;
use std::time::Duration;

use super::cipher::DeviceKey;
use super::orchestrator::{DuressTrigger, TriggerError};
use super::queue::DuressQueue;

/// Backoff delay before the next whole-queue pass, given how many passes have
/// already happened. Saturates at 1 hour.
pub fn backoff_for_attempt(attempt: u32) -> Duration {
    match attempt {
        0 => Duration::from_secs(5),
        1 => Duration::from_secs(30),
        2 => Duration::from_secs(5 * 60),
        3 => Duration::from_secs(30 * 60),
        _ => Duration::from_secs(60 * 60),
    }
}

/// Drives the retry queue toward empty. Cheap to clone (all fields are shared
/// handles).
#[derive(Clone)]
pub struct DuressDrainer {
    queue: DuressQueue,
    cipher: DeviceKey,
    client: Arc<dyn DuressTrigger>,
}

impl DuressDrainer {
    pub fn new(queue: DuressQueue, cipher: DeviceKey, client: Arc<dyn DuressTrigger>) -> Self {
        Self {
            queue,
            cipher,
            client,
        }
    }

    /// One pass over every queued entry. Returns the number of entries still
    /// pending after the pass (0 means the queue drained).
    ///
    /// Each entry's stored code is the encrypted envelope; it's decrypted to a
    /// short-lived plaintext only for the POST. A code that won't decrypt is
    /// treated as terminal and dropped — we can never make that POST succeed.
    pub async fn drain_once(&self) -> std::io::Result<usize> {
        let entries = self.queue.entries()?;
        for entry in entries {
            let plaintext = match self.cipher.decrypt(&entry.duress_code) {
                Ok(p) => p,
                Err(_) => {
                    // Undecryptable — abandon it.
                    self.queue.dequeue(&entry.account_id)?;
                    continue;
                }
            };
            match self.client.trigger(&entry.account_id, &plaintext).await {
                Ok(()) => {
                    self.queue.dequeue(&entry.account_id)?;
                }
                Err(TriggerError::Terminal) => {
                    // The wipe already happened; stop retrying a request the
                    // server will never accept.
                    self.queue.dequeue(&entry.account_id)?;
                }
                Err(TriggerError::Retriable) => {
                    // Leave it; bump attempts so diagnostics can see progress.
                    let mut all = self.queue.entries()?;
                    if let Some(e) = all.iter_mut().find(|e| e.account_id == entry.account_id) {
                        e.attempts = e.attempts.saturating_add(1);
                    }
                    self.queue.replace_all(&all)?;
                }
            }
        }
        Ok(self.queue.entries()?.len())
    }

    /// Runs passes with exponential backoff until the queue drains. The caller
    /// spawns this on launch / online-transition; it exits once empty.
    pub async fn run_until_empty(&self) {
        let mut pass = 0u32;
        loop {
            match self.drain_once().await {
                Ok(0) => return,
                Ok(_) => {}
                Err(e) => {
                    log::warn!("duress drain pass failed: {e}");
                }
            }
            tokio::time::sleep(backoff_for_attempt(pass)).await;
            pass = pass.saturating_add(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::duress::{queue::DuressQueue, PendingActivation};
    use async_trait::async_trait;
    use chrono::Utc;
    use std::path::PathBuf;
    use std::sync::Mutex;

    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    fn temp_dir() -> PathBuf {
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "coincube-duress-drain-{}-{}",
            std::process::id(),
            seq
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    struct ScriptedTrigger {
        // account_id -> sequence of outcomes (popped front each call)
        plan: Mutex<Vec<Result<(), TriggerError>>>,
        calls: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl DuressTrigger for ScriptedTrigger {
        async fn trigger(&self, account_id: &str, _code: &str) -> Result<(), TriggerError> {
            self.calls.lock().unwrap().push(account_id.to_string());
            let mut plan = self.plan.lock().unwrap();
            if plan.is_empty() {
                Ok(())
            } else {
                plan.remove(0)
            }
        }
    }

    fn enqueue(q: &DuressQueue, cipher: &DeviceKey, account: &str) {
        q.enqueue(PendingActivation {
            account_id: account.to_string(),
            duress_code: cipher.encrypt("raw-code").unwrap(),
            enqueued_at: Utc::now(),
            attempts: 0,
        })
        .unwrap();
    }

    #[test]
    fn backoff_schedule_matches_spec() {
        assert_eq!(backoff_for_attempt(0), Duration::from_secs(5));
        assert_eq!(backoff_for_attempt(1), Duration::from_secs(30));
        assert_eq!(backoff_for_attempt(2), Duration::from_secs(300));
        assert_eq!(backoff_for_attempt(3), Duration::from_secs(1800));
        assert_eq!(backoff_for_attempt(4), Duration::from_secs(3600));
        assert_eq!(backoff_for_attempt(99), Duration::from_secs(3600));
    }

    #[tokio::test]
    async fn drain_once_success_empties_queue() {
        let dir = temp_dir();
        let cipher = DeviceKey::from_bytes([5u8; 32]);
        let q = DuressQueue::new(&dir);
        enqueue(&q, &cipher, "acct_1");
        let client = Arc::new(ScriptedTrigger {
            plan: Mutex::new(vec![Ok(())]),
            calls: Mutex::new(vec![]),
        });
        let drainer = DuressDrainer::new(q.clone(), cipher, client);
        assert_eq!(drainer.drain_once().await.unwrap(), 0);
        assert!(q.is_empty().unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn retriable_keeps_entry_and_bumps_attempts() {
        let dir = temp_dir();
        let cipher = DeviceKey::from_bytes([6u8; 32]);
        let q = DuressQueue::new(&dir);
        enqueue(&q, &cipher, "acct_2");
        let client = Arc::new(ScriptedTrigger {
            plan: Mutex::new(vec![Err(TriggerError::Retriable)]),
            calls: Mutex::new(vec![]),
        });
        let drainer = DuressDrainer::new(q.clone(), cipher, client);
        assert_eq!(drainer.drain_once().await.unwrap(), 1);
        let entries = q.entries().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].attempts, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn terminal_drops_entry() {
        let dir = temp_dir();
        let cipher = DeviceKey::from_bytes([7u8; 32]);
        let q = DuressQueue::new(&dir);
        enqueue(&q, &cipher, "acct_3");
        let client = Arc::new(ScriptedTrigger {
            plan: Mutex::new(vec![Err(TriggerError::Terminal)]),
            calls: Mutex::new(vec![]),
        });
        let drainer = DuressDrainer::new(q.clone(), cipher, client);
        assert_eq!(drainer.drain_once().await.unwrap(), 0);
        assert!(q.is_empty().unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn undecryptable_code_is_abandoned() {
        let dir = temp_dir();
        let real = DeviceKey::from_bytes([8u8; 32]);
        let q = DuressQueue::new(&dir);
        enqueue(&q, &real, "acct_4");
        // Drainer holds a DIFFERENT key, so the stored code can't be decrypted.
        let wrong = DeviceKey::from_bytes([99u8; 32]);
        let client = Arc::new(ScriptedTrigger {
            plan: Mutex::new(vec![]),
            calls: Mutex::new(vec![]),
        });
        let drainer = DuressDrainer::new(q.clone(), wrong, client.clone());
        assert_eq!(drainer.drain_once().await.unwrap(), 0);
        assert!(q.is_empty().unwrap());
        // The POST was never attempted.
        assert!(client.calls.lock().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
