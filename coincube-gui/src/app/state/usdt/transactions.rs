use std::sync::Arc;

use coincube_ui::widget::*;
use iced::Task;

use crate::app::cache::Cache;
use crate::app::menu::Menu;
use crate::app::message::Message;
use crate::app::state::liquid::transactions::LiquidTransactions;
use crate::app::state::State;
use crate::app::view;
use crate::app::wallet::Wallet;
use crate::daemon::Daemon;

/// USDt transactions screen — wraps `LiquidTransactions` with USDt-only filter.
pub struct UsdtTransactions {
    inner: LiquidTransactions,
}

impl UsdtTransactions {
    pub fn new(inner: LiquidTransactions) -> Self {
        Self { inner }
    }
}

impl State for UsdtTransactions {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        self.inner.view(menu, cache)
    }

    fn update(
        &mut self,
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        self.inner.update(daemon, cache, message)
    }

    fn subscription(&self) -> iced::Subscription<Message> {
        self.inner.subscription()
    }

    fn close(&mut self) -> Task<Message> {
        self.inner.close()
    }

    fn interrupt(&mut self) {
        self.inner.interrupt()
    }

    fn reload(
        &mut self,
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        wallet: Option<Arc<Wallet>>,
    ) -> Task<Message> {
        self.inner.reload(daemon, wallet)
    }
}
