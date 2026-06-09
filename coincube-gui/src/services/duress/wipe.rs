//! Atomic Cube wipe (Phase 3, Task 3.2/3.3).
//!
//! The wipe is the on-device trust anchor: at duress activation every byte of
//! Cube data on disk is obliterated. The sequence per file is
//! `rename → flush → fsync → overwrite-with-zeros → delete`, bracketed by the
//! [`WipeJournal`](super::journal::WipeJournal) so a power-pull mid-wipe is
//! completed on next launch.
//!
//! ## What is and isn't wiped
//!
//! Wiped: everything under the configured Cube data roots — wallet databases
//! (BDK), seed/signer material, Spark working dirs, any decrypted-at-rest
//! tempfiles, and the per-Cube PIN hashes.
//!
//! **Never** wiped (these live at the data-directory root, outside the Cube
//! roots, and are what make recovery and the eventual `POST` possible):
//! `duress-state.json`, `duress-queue.json`, `duress.key`,
//! `duress-wipe.journal`, and the cached Connect auth under `connect/`.
//!
//! Secure-erase guarantees on SSDs are weak (wear-levelling relocates blocks),
//! so the zero-overwrite is defence-in-depth layered on top of the
//! filesystem-level delete — the real guarantee is "the file is gone", not
//! "the physical sectors are scrubbed".

use std::io::{self, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use super::journal::WipeJournal;

/// One overwrite pass of zeros. SSD wear-levelling makes additional passes
/// pointless; the delete is the real guarantee.
const ZERO_CHUNK: [u8; 4096] = [0u8; 4096];

/// Obliterates a fixed set of Cube data roots atomically with respect to the
/// wipe journal.
#[derive(Clone)]
pub struct CubeWiper {
    /// Absolute paths to wipe — typically the per-network `data/` directories
    /// plus any auxiliary Cube working dirs. May include files or directories.
    targets: Vec<PathBuf>,
    journal: WipeJournal,
}

impl CubeWiper {
    pub fn new(targets: Vec<PathBuf>, journal: WipeJournal) -> Self {
        Self { targets, journal }
    }

    /// Runs the wipe. Requires the journal marker to already be set by the
    /// caller (the orchestrator writes it *before* spawning the POST so the
    /// ordering invariant holds); this method clears the marker only after the
    /// last target is gone.
    ///
    /// Idempotent: targets that don't exist are skipped, so re-running after an
    /// interrupted wipe completes cleanly.
    pub fn execute_atomic(&self) -> io::Result<()> {
        for target in &self.targets {
            obliterate(target)?;
        }
        self.journal.clear()?;
        Ok(())
    }

    /// Completes a wipe that the journal says was interrupted. Called on launch
    /// when `journal.is_pending()`. Same operation as [`execute_atomic`] —
    /// every target is re-enumerated and removed.
    pub fn complete_if_pending(&self) -> io::Result<bool> {
        if !self.journal.is_pending() {
            return Ok(false);
        }
        self.execute_atomic()?;
        Ok(true)
    }
}

/// Recursively obliterates a path. Directories are emptied depth-first then
/// removed; files are zero-overwritten then unlinked. Missing paths are a
/// no-op so the operation is idempotent.
fn obliterate(path: &Path) -> io::Result<()> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };

    if meta.file_type().is_dir() {
        // Recurse into children first, then remove the now-empty directory.
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            obliterate(&entry.path())?;
        }
        match std::fs::remove_dir(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    } else if meta.file_type().is_symlink() {
        // Never follow a symlink out of the Cube tree; just unlink it.
        remove_file_idempotent(path)
    } else {
        zero_and_remove_file(path, meta.len())
    }
}

/// `rename → fsync → zero-overwrite → delete` for a single regular file.
fn zero_and_remove_file(path: &Path, len: u64) -> io::Result<()> {
    // Rename to a sibling `.wiping` name first so a crash leaves an obviously
    // partial artifact rather than a file that still looks like live data.
    let wiping = path.with_extension("wiping");
    let working = match std::fs::rename(path, &wiping) {
        Ok(()) => wiping,
        // If the rename target collides or the FS refuses, fall back to
        // operating on the original path.
        Err(_) => path.to_path_buf(),
    };

    if len > 0 {
        if let Ok(mut f) = std::fs::OpenOptions::new().write(true).open(&working) {
            f.seek(SeekFrom::Start(0))?;
            let mut remaining = len;
            while remaining > 0 {
                let n = remaining.min(ZERO_CHUNK.len() as u64) as usize;
                f.write_all(&ZERO_CHUNK[..n])?;
                remaining -= n as u64;
            }
            f.flush()?;
            let _ = f.sync_all();
        }
    }

    remove_file_idempotent(&working)
}

fn remove_file_idempotent(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "coincube-duress-wipe-{}-{}-{:p}",
            tag,
            std::process::id(),
            &0u8
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_file(path: &Path, contents: &[u8]) {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p).unwrap();
        }
        std::fs::write(path, contents).unwrap();
    }

    #[test]
    fn wipe_removes_all_cube_files_and_clears_journal() {
        let root = temp_dir("removes");
        let cube_root = root.join("bitcoin").join("data");
        write_file(&cube_root.join("cube_a").join("wallet.db"), b"SECRET-A");
        write_file(&cube_root.join("cube_a").join("seed"), b"SEED-A");
        write_file(&cube_root.join("cube_b").join("wallet.db"), b"SECRET-B");

        // Files that must survive: duress stores at the root.
        write_file(&root.join("duress-queue.json"), b"[{...}]");
        write_file(&root.join("duress-state.json"), b"{...}");

        let journal = WipeJournal::new(&root);
        journal.mark_pending_activation("acct_1").unwrap();
        let wiper = CubeWiper::new(vec![cube_root.clone()], journal.clone());
        wiper.execute_atomic().unwrap();

        assert!(!cube_root.exists(), "cube data root must be gone");
        assert!(!journal.is_pending(), "journal marker must be cleared");
        // Survivors intact.
        assert!(root.join("duress-queue.json").exists());
        assert!(root.join("duress-state.json").exists());

        // Penetration check: the known cube string must not appear anywhere
        // under the data root.
        let mut found = false;
        fn scan(p: &Path, needle: &[u8], found: &mut bool) {
            if let Ok(rd) = std::fs::read_dir(p) {
                for e in rd.flatten() {
                    let path = e.path();
                    if path.is_dir() {
                        scan(&path, needle, found);
                    } else if let Ok(bytes) = std::fs::read(&path) {
                        if bytes.windows(needle.len()).any(|w| w == needle) {
                            *found = true;
                        }
                    }
                }
            }
        }
        scan(&root, b"SECRET-A", &mut found);
        assert!(!found, "no cube material may remain on disk");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn wipe_is_idempotent() {
        let root = temp_dir("idempotent");
        let cube_root = root.join("data");
        write_file(&cube_root.join("x.db"), b"x");
        let journal = WipeJournal::new(&root);
        journal.mark_pending_activation("a").unwrap();
        let wiper = CubeWiper::new(vec![cube_root.clone()], journal);
        wiper.execute_atomic().unwrap();
        // Second run on already-wiped targets must succeed.
        wiper.execute_atomic().unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn complete_if_pending_runs_only_when_marked() {
        let root = temp_dir("pending");
        let cube_root = root.join("data");
        write_file(&cube_root.join("x.db"), b"x");
        let journal = WipeJournal::new(&root);
        let wiper = CubeWiper::new(vec![cube_root.clone()], journal.clone());

        // No marker → nothing happens, files remain.
        assert!(!wiper.complete_if_pending().unwrap());
        assert!(cube_root.join("x.db").exists());

        // With marker (simulating an interrupted wipe) → completes.
        journal.mark_pending_activation("a").unwrap();
        assert!(wiper.complete_if_pending().unwrap());
        assert!(!cube_root.exists());
        assert!(!journal.is_pending());
        let _ = std::fs::remove_dir_all(&root);
    }
}
