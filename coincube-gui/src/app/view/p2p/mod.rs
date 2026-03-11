pub mod components;
pub mod config;
pub mod mostro;
pub mod panel;

pub use panel::*;

use std::path::PathBuf;

/// Returns the mostro data directory under the main coincube config directory
/// (`~/.coincube/mostro/` on Linux), creating it if necessary.
pub(crate) fn mostro_dir() -> Result<PathBuf, String> {
    let dir = crate::dir::CoincubeDirectory::new_default()
        .map_err(|e| format!("Cannot determine coincube directory: {e}"))?;
    let mostro = dir.path().join("mostro");
    std::fs::create_dir_all(&mostro)
        .map_err(|e| format!("Failed to create mostro data dir: {e}"))?;
    Ok(mostro)
}
