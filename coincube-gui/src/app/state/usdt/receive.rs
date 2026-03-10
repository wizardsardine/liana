use std::sync::Arc;

use coincube_ui::widget::*;
use iced::Task;

use crate::app::cache::Cache;
use crate::app::menu::Menu;
use crate::app::message::Message;
use crate::app::state::liquid::receive::LiquidReceive;
use crate::app::state::State;
use crate::app::view;
use crate::app::view::ReceiveMethod;
use crate::app::wallet::Wallet;
use crate::daemon::Daemon;

/// USDt receive screen — wraps `LiquidReceive` with `ReceiveMethod::Usdt` locked in on every reload.
pub struct UsdtReceive {
    inner: LiquidReceive,
}

impl UsdtReceive {
    pub fn new(inner: LiquidReceive) -> Self {
        Self { inner }
    }
}

impl State for UsdtReceive {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        self.inner.view_usdt_only(menu, cache)
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
        let reload_task = self.inner.reload(daemon, wallet);
        // Lock receive method to USDt on every navigation to this screen
        let preset_task = Task::done(Message::View(view::Message::LiquidReceive(
            view::LiquidReceiveMessage::ToggleMethod(ReceiveMethod::Usdt),
        )));
        Task::batch(vec![reload_task, preset_task])
    }
}
