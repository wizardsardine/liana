pub mod app;
pub mod daemon;
pub mod datadir;
pub mod download;
pub mod hw;
pub mod installer;
pub mod launcher;
pub mod lianalite;
pub mod loader;
pub mod logger;
pub mod node;
pub mod signer;
pub mod utils;

use liana::Version;

pub const VERSION: Version = Version {
    major: 8,
    minor: 0,
    patch: 0,
};
