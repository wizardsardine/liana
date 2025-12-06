use std::sync::Arc;

use coincube_ui::widget::*;
use iced::Task;

use crate::app::{breez::BreezClient, cache::Cache, menu::Menu, state::State};
use crate::app::{message::Message, view, wallet::Wallet};
use crate::daemon::Daemon;

/// ActiveOverview is a placeholder panel for the Active Overview page
pub struct ActiveOverview {
    breez_client: Arc<BreezClient>,
}

impl ActiveOverview {
    pub fn new(breez_client: Arc<BreezClient>) -> Self {
        Self { breez_client }
    }
}

impl State for ActiveOverview {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        let wallet_name = "Active Wallet"; // Active wallet name from BreezClient

        view::dashboard(
            menu,
            cache,
            None,
            view::active::active_overview_view(wallet_name),
        )
    }

    fn update(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _cache: &Cache,
        _message: Message,
    ) -> Task<Message> {
        Task::none()
    }

    fn reload(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _wallet: Arc<Wallet>,
    ) -> Task<Message> {
        // Active wallet doesn't use Vault wallet - uses BreezClient instead
        Task::none()
    }
}
