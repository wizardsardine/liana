pub mod app;
pub mod args;
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
pub mod services;
pub mod signer;
pub mod utils;
pub mod window;

pub const VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION_MAJOR"),
    ".",
    env!("CARGO_PKG_VERSION_MINOR"),
    "-rc1"
);

#[cfg(test)]
mod tests {
    #[test]
    fn gui_version() {
        let gui_major: u32 = env!("CARGO_PKG_VERSION_MAJOR").parse().unwrap();
        let lianad_major: u32 = lianad::VERSION.split('.').next().unwrap().parse().unwrap();
        assert!(gui_major >= lianad_major);
    }
}
