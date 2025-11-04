use std::sync::Arc;

use iced::Task;
use liana_ui::widget::*;

use super::{Cache, Menu, State};
use crate::app::{message::Message, view, wallet::Wallet};
use crate::daemon::Daemon;

/// ActiveSend is a placeholder panel for the Active Send page
pub struct ActiveSend {
    wallet: Arc<Wallet>,
}

impl ActiveSend {
    pub fn new(wallet: Arc<Wallet>) -> Self {
        Self { wallet }
    }
}

impl State for ActiveSend {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        view::dashboard(
            menu,
            cache,
            None,
            view::active::active_send_view(&self.wallet.name),
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
        wallet: Arc<Wallet>,
    ) -> Task<Message> {
        self.wallet = wallet;
        Task::none()
    }
}

/// ActiveReceive is a placeholder panel for the Active Receive page
pub struct ActiveReceive {
    wallet: Arc<Wallet>,
}

impl ActiveReceive {
    pub fn new(wallet: Arc<Wallet>) -> Self {
        Self { wallet }
    }
}

impl State for ActiveReceive {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        view::dashboard(
            menu,
            cache,
            None,
            view::active::active_receive_view(&self.wallet.name),
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
        wallet: Arc<Wallet>,
    ) -> Task<Message> {
        self.wallet = wallet;
        Task::none()
    }
}

/// ActiveTransactions is a placeholder panel for the Active Transactions page
pub struct ActiveTransactions {
    wallet: Arc<Wallet>,
}

impl ActiveTransactions {
    pub fn new(wallet: Arc<Wallet>) -> Self {
        Self { wallet }
    }
}

impl State for ActiveTransactions {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        view::dashboard(
            menu,
            cache,
            None,
            view::active::active_transactions_view(&self.wallet.name),
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
        wallet: Arc<Wallet>,
    ) -> Task<Message> {
        self.wallet = wallet;
        Task::none()
    }
}

/// ActiveSettings is a placeholder panel for the Active Settings page
pub struct ActiveSettings {
    wallet: Arc<Wallet>,
}

impl ActiveSettings {
    pub fn new(wallet: Arc<Wallet>) -> Self {
        Self { wallet }
    }
}

impl State for ActiveSettings {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        view::dashboard(
            menu,
            cache,
            None,
            view::active::active_settings_view(&self.wallet.name),
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
        wallet: Arc<Wallet>,
    ) -> Task<Message> {
        self.wallet = wallet;
        Task::none()
    }
}
