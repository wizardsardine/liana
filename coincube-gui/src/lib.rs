pub mod app;
pub mod backup;
pub mod browser;
pub mod chain;
pub(crate) mod cube_display;
pub mod daemon;
pub mod delete;
pub mod dir;
pub mod download;
pub mod export;
pub mod feature_flags;
pub mod gui;
pub mod help;
pub mod home;
pub mod hw;
pub mod hw_advisory;
pub mod installer;
#[cfg(test)]
mod l10n_guard_tests;
#[cfg(target_os = "linux")]
pub mod linux_backend;
pub mod loader;
pub mod loading;
pub mod logger;
pub mod network_visibility;
pub mod node;
pub mod passkey_unlock;
pub mod phone_signer;
pub mod pin_entry;
pub mod pin_input;
pub mod recover_vault;
#[cfg(feature = "integration-tests")]
pub mod recovery_native;
pub mod services;
pub mod signer;
pub mod split_hardware;
pub mod user_error;
pub mod utils;
pub mod wallet_source_form;
#[cfg(test)]
mod worldview_art_tests;

use coincubed::ApiVersion;

pub const VERSION: ApiVersion = ApiVersion(env!("CARGO_PKG_VERSION"));

#[cfg(test)]
mod tests {
    #[test]
    fn gui_version() {
        // coincube-gui major version should always be superior or equal to coincubed version.
        let coincubed_version = coincubed::VERSION.major();
        assert!(super::VERSION.major() >= coincubed_version);
    }
}
