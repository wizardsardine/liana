use std::process::Command;

/// Fetch the short git commit hash.
pub fn get_git_hash() -> String {
    let output = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .expect("Git command should succeed.");

    String::from_utf8_lossy(&output.stdout).trim().to_string()
}
