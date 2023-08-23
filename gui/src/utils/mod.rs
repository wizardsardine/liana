use std::path::Path;

#[cfg(test)]
pub mod sandbox;

#[cfg(test)]
pub mod mock;

/// Polls for a file's existence at given interval up to a maximum number of polls.
/// Returns `true` once file exists and otherwise `false`.
pub fn poll_for_file(path: &Path, interval_millis: u64, max_polls: u16) -> bool {
    for i in 0..max_polls {
        if path.exists() {
            return true;
        }
        if i < max_polls.saturating_sub(1) {
            std::thread::sleep(std::time::Duration::from_millis(interval_millis));
        }
    }
    false
}
