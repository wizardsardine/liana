pub mod app;
pub mod backup;
pub mod daemon;
pub mod delete;
pub mod dir;
pub mod download;
pub mod export;
pub mod gui;
pub mod help;
pub mod hw;
pub mod installer;
pub mod launcher;
pub mod loader;
pub mod logger;
pub mod node;
pub mod pin_entry;
pub mod services;
pub mod signer;
pub mod utils;

use coincubed::Version;

pub const VERSION: Version = Version {
    major: 13,
    minor: 0,
};

#[cfg(test)]
mod tests {
    #[test]
    fn gui_version() {
        // coincube-gui major version should always be superior or equal to coincubed version.
        let coincubed_version = coincubed::VERSION.major;
        assert!(super::VERSION.major >= coincubed_version);
    }
}
