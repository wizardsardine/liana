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

/// Fetch the short git commit hash.
pub fn get_git_hash() -> String {
    let output = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .expect("Git command should succeed.");

    String::from_utf8_lossy(&output.stdout).trim().to_string()
}
