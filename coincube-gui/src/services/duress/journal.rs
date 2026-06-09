//! Wipe-pending journal (Phase 3).
//!
//! The atomicity invariant: either every Cube file is gone or none is. The
//! journal is the durability anchor — a marker is written *before* the wipe
//! enumeration begins and removed only *after* the last file is deleted. On the
//! next launch, if the marker still exists, the wipe was interrupted (power
//! pulled, crash) and must be completed before anything else runs.
//!
//! The marker lives at the data-directory root, outside `cubes/`, so it
//! survives a partial wipe.

use chrono::Utc;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Filename of the wipe-pending marker, relative to the data-directory root.
pub const WIPE_JOURNAL_FILE: &str = "duress-wipe.journal";

/// Durable marker that a duress wipe is in progress / pending completion.
#[derive(Clone)]
pub struct WipeJournal {
    path: PathBuf,
}

impl WipeJournal {
    pub fn new(coincube_dir: &Path) -> Self {
        Self {
            path: coincube_dir.join(WIPE_JOURNAL_FILE),
        }
    }

    /// Writes the wipe-pending marker. Fast, synchronous, durable (`fsync`).
    /// Records the activating account id and a timestamp for diagnostics; the
    /// mere existence of the file is what drives wipe-completion on relaunch.
    pub fn mark_pending_activation(&self, account_id: &str) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut f = std::fs::File::create(&self.path)?;
        writeln!(f, "{}\t{}", account_id, Utc::now().to_rfc3339())?;
        f.sync_all()?;
        Ok(())
    }

    /// True when a wipe is pending (marker present).
    pub fn is_pending(&self) -> bool {
        self.path.exists()
    }

    /// The account id recorded in the marker, if any (best-effort, for
    /// reconciling the retry queue on relaunch).
    pub fn pending_account_id(&self) -> Option<String> {
        let contents = std::fs::read_to_string(&self.path).ok()?;
        contents
            .lines()
            .next()
            .and_then(|line| line.split('\t').next())
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
    }

    /// Removes the marker — called only after the wipe has fully completed.
    pub fn clear(&self) -> io::Result<()> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "coincube-duress-journal-{}-{}-{:p}",
            tag,
            std::process::id(),
            &0u8
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn mark_is_pending_then_clear() {
        let dir = temp_dir("basic");
        let j = WipeJournal::new(&dir);
        assert!(!j.is_pending());
        j.mark_pending_activation("acct_1").unwrap();
        assert!(j.is_pending());
        assert_eq!(j.pending_account_id().as_deref(), Some("acct_1"));
        j.clear().unwrap();
        assert!(!j.is_pending());
        // Clearing an absent marker is a no-op.
        j.clear().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
