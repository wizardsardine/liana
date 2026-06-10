//! Durable activation retry queue (Phase 3 persistence; Phase 4 drain loop).
//!
//! This file is the **source of truth** that guarantees the activation `POST`
//! eventually fires. It is committed *before* the wipe starts and *before* the
//! POST is kicked off, so even if power is pulled in the next millisecond the
//! queue + journal together drive both the POST and the wipe to completion on
//! next launch.
//!
//! It lives at `duress-queue.json` in the data-directory root and is **never**
//! touched by [`CubeWiper`](super::wipe::CubeWiper).
//!
//! Phase 4 adds the background drain loop (exponential backoff, online-state
//! triggers) on top of the primitives here.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::{PendingActivation, DURESS_QUEUE_FILE};

/// A cheaply-clonable handle to the persistent queue file. Cloning shares the
/// same path and serialization lock, so the orchestrator's main thread and the
/// spawned POST task can both touch the queue without racing.
#[derive(Clone)]
pub struct DuressQueue {
    path: Arc<PathBuf>,
    lock: Arc<Mutex<()>>,
}

impl DuressQueue {
    pub fn new(coincube_dir: &Path) -> Self {
        Self {
            path: Arc::new(coincube_dir.join(DURESS_QUEUE_FILE)),
            lock: Arc::new(Mutex::new(())),
        }
    }

    /// All pending activations currently queued (empty when the file is
    /// absent).
    pub fn entries(&self) -> io::Result<Vec<PendingActivation>> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        self.read_locked()
    }

    pub fn is_empty(&self) -> io::Result<bool> {
        Ok(self.entries()?.is_empty())
    }

    /// Appends an activation, de-duplicating by `account_id` so a repeated
    /// activation doesn't pile up multiple rows for the same account.
    pub fn enqueue(&self, item: PendingActivation) -> io::Result<()> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut items = self.read_locked()?;
        items.retain(|e| e.account_id != item.account_id);
        items.push(item);
        self.write_locked(&items)
    }

    /// Removes the queued activation for `account_id` (called when its POST
    /// succeeds). A no-op if it isn't present.
    pub fn dequeue(&self, account_id: &str) -> io::Result<()> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut items = self.read_locked()?;
        let before = items.len();
        items.retain(|e| e.account_id != account_id);
        if items.len() == before {
            return Ok(());
        }
        self.write_locked(&items)
    }

    /// Replaces the persisted entries wholesale (used by the Phase 4 drain loop
    /// to bump `attempts`).
    pub fn replace_all(&self, items: &[PendingActivation]) -> io::Result<()> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        self.write_locked(items)
    }

    fn read_locked(&self) -> io::Result<Vec<PendingActivation>> {
        match std::fs::read(self.path.as_path()) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }

    fn write_locked(&self, items: &[PendingActivation]) -> io::Result<()> {
        let bytes = serde_json::to_vec_pretty(items)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = self.path.with_extension("tmp");
        {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(&bytes)?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, self.path.as_path())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "coincube-duress-queue-{}-{}-{:p}",
            tag,
            std::process::id(),
            &0u8
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn item(account: &str) -> PendingActivation {
        PendingActivation {
            account_id: account.to_string(),
            duress_code: "enc:abc".to_string(),
            enqueued_at: Utc::now(),
            attempts: 0,
        }
    }

    #[test]
    fn enqueue_dequeue_roundtrip() {
        let dir = temp_dir("rt");
        let q = DuressQueue::new(&dir);
        assert!(q.is_empty().unwrap());
        q.enqueue(item("acct_1")).unwrap();
        assert_eq!(q.entries().unwrap().len(), 1);
        q.dequeue("acct_1").unwrap();
        assert!(q.is_empty().unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn enqueue_dedupes_by_account() {
        let dir = temp_dir("dedupe");
        let q = DuressQueue::new(&dir);
        q.enqueue(item("acct_1")).unwrap();
        q.enqueue(item("acct_1")).unwrap();
        assert_eq!(q.entries().unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dequeue_missing_is_noop() {
        let dir = temp_dir("noop");
        let q = DuressQueue::new(&dir);
        q.dequeue("nope").unwrap();
        assert!(q.is_empty().unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn survives_reload_via_new_handle() {
        let dir = temp_dir("reload");
        DuressQueue::new(&dir).enqueue(item("acct_9")).unwrap();
        // A fresh handle (simulating relaunch) sees the persisted entry.
        let q2 = DuressQueue::new(&dir);
        assert_eq!(q2.entries().unwrap()[0].account_id, "acct_9");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
