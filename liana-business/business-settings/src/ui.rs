//! Business-specific settings UI implementation.
//!
//! This module provides a minimal/blank settings UI for liana-business.
//! The UI structure follows the monostate pattern (like business-installer)
//! rather than the `Box<dyn State>` pattern used in liana-gui.
//!
//! Settings UI design for liana-business is not yet finalized, so this
//! implementation returns a blank/placeholder view.

use std::sync::Arc;

use iced::{Subscription, Task};
use liana_gui::{
    app::{
        cache::Cache,
        message::Message,
        settings::SettingsUI,
        state::State,
        view,
        wallet::Wallet,
        Config,
    },
    daemon::{Daemon, DaemonBackend},
    dir::LianaDirectory,
};
use liana_ui::widget::Element;

use crate::message::BusinessSettingsMessage;

/// Business-specific settings UI.
///
/// This is a monostate struct (single nested state) following the same
/// pattern as business-installer's State. This design is more readable
/// and maintainable than dynamic dispatch with `Box<dyn State>`.
///
/// Currently minimal since business settings specs are not finalized.
/// Fields will be added as features are designed and implemented.
pub struct BusinessSettingsUI {
    // Placeholder fields - will be populated as specs are defined
    #[allow(dead_code)]
    wallet: Arc<Wallet>,
}

impl SettingsUI<BusinessSettingsMessage> for BusinessSettingsUI {
    fn new(
        _data_dir: LianaDirectory,
        wallet: Arc<Wallet>,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _daemon_backend: DaemonBackend,
        _internal_bitcoind: bool,
        _config: Arc<Config>,
    ) -> (Self, Task<BusinessSettingsMessage>) {
        let ui = Self { wallet };
        (ui, Task::none())
    }

    fn update(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _cache: &Cache,
        _message: BusinessSettingsMessage,
    ) -> Task<BusinessSettingsMessage> {
        // No-op for now - will be implemented as features are designed
        Task::none()
    }

    fn view<'a>(&'a self, _cache: &'a Cache) -> Element<'a, BusinessSettingsMessage> {
        // Blank/placeholder view - will be designed later
        // Using liana_ui components for consistency
        use iced::widget::container;
        use liana_ui::component::text::text;

        container(text("Settings (coming soon)"))
            .padding(20)
            .into()
    }

    fn subscription(&self) -> Subscription<BusinessSettingsMessage> {
        Subscription::none()
    }

    fn stop(&mut self) {
        // No cleanup needed yet
    }

    fn reload(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        wallet: Arc<Wallet>,
    ) -> Task<BusinessSettingsMessage> {
        self.wallet = wallet;
        Task::none()
    }
}

/// State trait implementation for integration with liana-gui's App panel system.
///
/// This allows BusinessSettingsUI to be used as a panel in the unified App interface.
/// For now, this is a minimal implementation since business settings specs are not finalized.
impl State for BusinessSettingsUI {
    fn view<'a>(&'a self, _cache: &'a Cache) -> Element<'a, view::Message> {
        // Blank/placeholder view using view::Message for State trait compatibility
        use iced::widget::container;
        use liana_ui::component::text::text;

        container(text("Settings (coming soon)"))
            .padding(20)
            .into()
    }

    fn update(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _cache: &Cache,
        _message: Message,
    ) -> Task<Message> {
        // No-op for now - business settings messages will be handled
        // through the SettingsUI trait, not State
        Task::none()
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::none()
    }

    fn reload(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        wallet: Arc<Wallet>,
    ) -> Task<Message> {
        self.wallet = wallet;
        Task::none()
    }
}
