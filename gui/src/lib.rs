pub mod app;
pub mod bitcoind;
pub mod daemon;
pub mod download;
pub mod hw;
pub mod installer;
pub mod launcher;
pub mod loader;
pub mod logger;
pub mod signer;
pub mod utils;

use liana::Version;

pub const VERSION: Version = Version {
    major: 5,
    minor: 0,
    patch: 0,
};
