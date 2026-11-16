//! What App Preferences → Diagnostics can show and do on this device: open
//! the log folder, and copy a support summary.
//!
//! The summary is an explicit allowlist — app and daemon versions, OS and
//! architecture, and how many log files exist. It never includes a path
//! (paths carry the user's account name), a Cube or wallet name, an account
//! email, or anything read from a log file's contents.

use std::path::{Path, PathBuf};

use crate::{dir::CoincubeDirectory, logger::LOGS_DIR};

/// Snapshot taken when the preferences sheet opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticsState {
    logs_dir: PathBuf,
    can_open_logs: bool,
    support_summary: String,
    support_summary_copied: bool,
    open_logs_failed: bool,
}

impl DiagnosticsState {
    pub fn load(root: &CoincubeDirectory) -> Self {
        let logs_dir = logs_dir(root.path());
        let log_files = count_log_files(&logs_dir);
        Self {
            can_open_logs: logs_dir.is_dir(),
            support_summary: support_summary(log_files),
            support_summary_copied: false,
            open_logs_failed: false,
            logs_dir,
        }
    }

    pub fn can_open_logs(&self) -> bool {
        self.can_open_logs
    }

    pub fn support_summary(&self) -> &str {
        &self.support_summary
    }

    pub fn support_summary_copied(&self) -> bool {
        self.support_summary_copied
    }

    pub fn open_logs_failed(&self) -> bool {
        self.open_logs_failed
    }

    /// Returns the summary to put on the clipboard and shows "Copied".
    pub fn copy_support_summary(&mut self) -> String {
        self.support_summary_copied = true;
        self.support_summary.clone()
    }

    /// Opens the log folder in the platform file manager. A failure, including
    /// a folder removed since the sheet opened, is shown under the row.
    pub fn open_logs(&mut self) {
        if !self.can_open_logs {
            return;
        }
        self.open_logs_failed = !self.logs_dir.is_dir()
            || match crate::browser::open_url(&self.logs_dir.to_string_lossy()) {
                Ok(()) => false,
                Err(e) => {
                    log::warn!("Could not open the log folder: {}", e);
                    true
                }
            };
    }
}

fn logs_dir(root: &Path) -> PathBuf {
    root.join(LOGS_DIR)
}

/// `None` when the folder can't be read, so the summary says so rather than
/// reporting zero.
fn count_log_files(dir: &Path) -> Option<usize> {
    let entries = std::fs::read_dir(dir).ok()?;
    Some(
        entries
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "log"))
            .count(),
    )
}

fn support_summary(log_files: Option<usize>) -> String {
    let log_files = log_files.map_or_else(|| "unavailable".to_owned(), |n| n.to_string());
    format!(
        "COINCUBE {}\nVault daemon (built in) {}\nOS {} · {}\nLog files {}",
        crate::VERSION,
        coincubed::VERSION,
        std::env::consts::OS,
        std::env::consts::ARCH,
        log_files,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(root: &Path) -> DiagnosticsState {
        DiagnosticsState::load(&CoincubeDirectory::new(root.to_path_buf()))
    }

    #[test]
    fn opens_logs_only_when_the_folder_exists() {
        let root = tempfile::tempdir().unwrap();
        let state = load(root.path());
        assert!(!state.can_open_logs());
        assert!(state.support_summary().ends_with("Log files unavailable"));

        std::fs::create_dir_all(root.path().join(LOGS_DIR)).unwrap();
        assert!(load(root.path()).can_open_logs());
    }

    #[test]
    fn open_logs_reports_a_folder_removed_after_the_sheet_opened() {
        let root = tempfile::tempdir().unwrap();
        let logs = root.path().join(LOGS_DIR);
        std::fs::create_dir_all(&logs).unwrap();
        let mut state = load(root.path());
        std::fs::remove_dir(&logs).unwrap();

        state.open_logs();
        assert!(state.open_logs_failed());
    }

    #[test]
    fn summary_is_exactly_the_allowlist() {
        let root = tempfile::tempdir().unwrap();
        let logs = root.path().join(LOGS_DIR);
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::write(logs.join("a.log"), "").unwrap();
        std::fs::write(logs.join("b.log"), "").unwrap();
        std::fs::write(logs.join("notes.txt"), "").unwrap();

        let expected = format!(
            "COINCUBE {}\nVault daemon (built in) {}\nOS {} · {}\nLog files 2",
            crate::VERSION,
            coincubed::VERSION,
            std::env::consts::OS,
            std::env::consts::ARCH,
        );
        assert_eq!(load(root.path()).support_summary(), expected);
    }

    #[test]
    fn copy_returns_the_previewed_summary_and_marks_it_copied() {
        let root = tempfile::tempdir().unwrap();
        let mut state = load(root.path());
        assert!(!state.support_summary_copied());
        let preview = state.support_summary().to_owned();
        assert_eq!(state.copy_support_summary(), preview);
        assert!(state.support_summary_copied());
    }
}
