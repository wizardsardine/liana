use std::sync::Arc;

use coincube_ui::widget::Element;
use iced::Task;

use crate::app::view::LiquidSettingsMessage;
use crate::app::wallets::LiquidBackend;
use crate::app::{cache::Cache, menu::Menu, state::State};
use crate::app::{message::Message, view, wallet::Wallet};
use crate::daemon::Daemon;

/// LiquidSettings panel — Liquid wallet-specific settings.
///
/// NOTE: The master seed backup flow has been moved to General Settings
/// (Cube-level backup) since the master seed is shared across all wallets.
pub struct LiquidSettings {
    breez_client: Arc<LiquidBackend>,
}

impl LiquidSettings {
    pub fn new(breez_client: Arc<LiquidBackend>) -> Self {
        Self { breez_client }
    }
}

impl State for LiquidSettings {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        view::dashboard(
            menu,
            cache,
            view::liquid::liquid_settings_view(self.breez_client.liquid_signer()),
        )
    }

    fn update(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        _cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        if let Message::View(view::Message::LiquidSettings(LiquidSettingsMessage::ExportPayments)) =
            message
        {
            // Export payments handled elsewhere
        }
        Task::none()
    }

    fn reload(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        _wallet: Option<Arc<Wallet>>,
    ) -> Task<Message> {
        Task::none()
    }
}
