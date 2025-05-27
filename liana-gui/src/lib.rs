pub mod app;
pub mod backup;
pub mod daemon;
pub mod delete;
pub mod dir;
pub mod download;
pub mod export;
pub mod hw;
pub mod installer;
pub mod launcher;
pub mod loader;
pub mod logger;
pub mod node;
pub mod services;
pub mod signer;
pub mod utils;

use lianad::Version;

pub const VERSION: Version = Version {
    major: 11,
    minor: 0,
    patch: 0,
};

#[cfg(test)]
mod tests {
    #[test]
    fn gui_version() {
        // liana-gui major version should always be superior or equal to lianad version.
        let lianad_version = lianad::VERSION.major;
        assert!(super::VERSION.major >= lianad_version);
    }
}
