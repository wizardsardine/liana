//! Settings UI trait for abstracting settings panel behavior.
//!
//! This trait follows the same pattern as the `Installer` trait, allowing
//! different applications (liana-gui, liana-business) to define their own
//! settings UI while sharing the core framework.

use std::sync::Arc;

use iced::{Subscription, Task};
use liana_ui::widget::Element;

use crate::{
    app::{cache::Cache, wallet::Wallet, Config},
    daemon::{Daemon, DaemonBackend},
    dir::LianaDirectory,
};

/// Trait for settings UI state management.
///
/// Implementors define what settings sections exist and how they behave.
/// Each implementation has its own message type for self-contained communication.
///
/// # Type Parameter
///
/// * `Message` - The message type used by this settings UI implementation.
///   This allows each implementation to define its own message enum.
pub trait SettingsUI<Message>
where
    Message: Send + 'static,
{
    /// Create settings UI with an optional initial async task.
    ///
    /// # Arguments
    ///
    /// * `data_dir` - The application data directory
    /// * `wallet` - The current wallet
    /// * `daemon` - The daemon interface
    /// * `daemon_backend` - The type of backend (local/remote)
    /// * `internal_bitcoind` - Whether an internal bitcoind is running
    /// * `config` - The application configuration
    fn new(
        data_dir: LianaDirectory,
        wallet: Arc<Wallet>,
        daemon: Arc<dyn Daemon + Sync + Send>,
        daemon_backend: DaemonBackend,
        internal_bitcoind: bool,
        config: Arc<Config>,
    ) -> (Self, Task<Message>)
    where
        Self: Sized;

    /// Process a message and return an async task.
    ///
    /// This is the core state machine method where all state transitions
    /// and side effects are handled.
    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message>;

    /// Render the current state as a view element.
    ///
    /// The returned element uses the implementation's own message type,
    /// which will be wrapped by the App before being sent to the GUI.
    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, Message>;

    /// Provide event streams for subscription-based updates.
    ///
    /// Override this to provide subscriptions like hardware wallet detection,
    /// price updates, etc. Default implementation returns no subscriptions.
    fn subscription(&self) -> Subscription<Message> {
        Subscription::none()
    }

    /// Cleanup when the settings panel is closed or the app is shutting down.
    ///
    /// Override this to perform cleanup like stopping hardware wallet listeners.
    fn stop(&mut self) {}

    /// Reload state when the wallet changes.
    ///
    /// Called when the user switches wallets or when wallet data is updated.
    fn reload(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        wallet: Arc<Wallet>,
    ) -> Task<Message>;
}
