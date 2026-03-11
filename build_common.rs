use std::process::Command;

/// Expose the liana version. If the `LIANA_VERSION` environment variable is set, we are in release,
/// so we use that, otherwise use the git hash.
pub fn expose_liana_version() {
    if let Some(version) = std::env::var_os("LIANA_VERSION") {
        println!("cargo:rustc-env=LIANA_VERSION={}", version.to_string_lossy());
    } else {
        let git_hash = get_git_hash();
        println!("cargo:rustc-env=GIT_HASH={}", git_hash);
    }
}

/// Fetch the short git commit hash. Returns "unknown" if git is not available
/// (e.g. in reproducible build environments like Nix or Guix).
pub fn get_git_hash() -> String {
    Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                let hash = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if hash.is_empty() { None } else { Some(hash) }
            } else {
                None
            }
        })
        .unwrap_or_else(|| "unknown".to_string())
}
