//! Business Installer Library
//!
//! This library provides the `BusinessInstaller` component which implements
//! the `Installer` trait from liana-gui, enabling it to be used as an
//! installer within the liana-gui application.

mod backend;
mod client;
mod installer;
mod state;
mod views;

// Re-export the main public API
pub use installer::BusinessInstaller;
pub use state::Msg as Message;

