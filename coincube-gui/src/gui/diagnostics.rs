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
}

impl DiagnosticsState {
    pub fn load(root: &CoincubeDirectory) -> Self {
        let logs_dir = logs_dir(root.path());
        let log_files = count_log_files(&logs_dir);
        Self {
            can_open_logs: logs_dir.is_dir(),
            support_summary: support_summary(log_files),
            logs_dir,
        }
    }

    pub fn can_open_logs(&self) -> bool {
        self.can_open_logs
    }

    pub fn support_summary(&self) -> &str {
        &self.support_summary
    }

    /// Opens the log folder in the platform file manager.
    pub fn open_logs(&self) {
        if !self.can_open_logs {
            return;
        }
        if let Err(e) = crate::browser::open_url(&self.logs_dir.to_string_lossy()) {
            log::warn!("Could not open the log folder: {}", e);
        }
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

    fn temp_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "coincube-diagnostics-{}-{}",
            name,
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn opens_logs_only_when_the_folder_exists() {
        let root = temp_root("missing");
        let state = DiagnosticsState::load(&CoincubeDirectory::new(root.clone()));
        assert!(!state.can_open_logs());
        assert!(state.support_summary().contains("Log files unavailable"));

        std::fs::create_dir_all(root.join(LOGS_DIR)).unwrap();
        let state = DiagnosticsState::load(&CoincubeDirectory::new(root.clone()));
        assert!(state.can_open_logs());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn summary_counts_log_files_and_never_includes_a_path() {
        let root = temp_root("summary");
        let logs = root.join(LOGS_DIR);
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::write(logs.join("a.log"), "user@example.com secret").unwrap();
        std::fs::write(logs.join("b.log"), "").unwrap();
        std::fs::write(logs.join("notes.txt"), "").unwrap();

        let state = DiagnosticsState::load(&CoincubeDirectory::new(root.clone()));
        let summary = state.support_summary();
        assert!(summary.contains("Log files 2"), "{}", summary);
        assert!(summary.contains(&crate::VERSION.to_string()));
        assert!(summary.contains(std::env::consts::OS));
        assert!(!summary.contains(&*root.to_string_lossy()));
        assert!(!summary.contains('/') && !summary.contains('\\'));
        assert!(!summary.contains("example.com"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
