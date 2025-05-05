include!("../build_common.rs");

fn main() {
    // Inject the short git commit hash as an environment variable `GIT_HASH`.
    println!("cargo:rustc-env=GIT_HASH={}", get_git_hash());
}
