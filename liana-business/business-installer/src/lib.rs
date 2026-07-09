mod backend;
mod client;
mod installer;
mod state;
mod views;

pub use installer::BusinessInstaller;
pub use state::Msg as Message;

pub const VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION_MAJOR"),
    ".",
    env!("CARGO_PKG_VERSION_MINOR"),
    "-dev"
);
