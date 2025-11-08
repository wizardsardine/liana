use std::sync::Arc;

use iced::Task;
use liana_ui::widget::*;

use super::{Cache, Menu, State};
use crate::app::{message::Message, view, wallet::Wallet};
use crate::daemon::Daemon;

/// ActiveSend is a placeholder panel for the Active Send page
pub struct ActiveSend {
    wallet: Option<Arc<Wallet>>,
}

impl ActiveSend {
    pub fn new(wallet: Arc<Wallet>) -> Self {
        Self { wallet: Some(wallet) }
    }
    
    pub fn new_without_wallet() -> Self {
        Self { wallet: None }
    }
}

impl State for ActiveSend {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        let wallet_name = self.wallet.as_ref()
            .map(|w| w.name.as_str())
            .unwrap_or("No Wallet");
        
        view::dashboard(
            menu,
            cache,
            None,
            view::active::active_send_view(wallet_name),
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
        self.wallet = Some(wallet);
        Task::none()
    }
}

/// ActiveReceive is a placeholder panel for the Active Receive page
pub struct ActiveReceive {
    wallet: Option<Arc<Wallet>>,
}

impl ActiveReceive {
    pub fn new(wallet: Arc<Wallet>) -> Self {
        Self { wallet: Some(wallet) }
    }
    
    pub fn new_without_wallet() -> Self {
        Self { wallet: None }
    }
}

impl State for ActiveReceive {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        let wallet_name = self.wallet.as_ref()
            .map(|w| w.name.as_str())
            .unwrap_or("No Wallet");
        
        view::dashboard(
            menu,
            cache,
            None,
            view::active::active_receive_view(wallet_name),
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
        self.wallet = Some(wallet);
        Task::none()
    }
}

/// ActiveTransactions is a placeholder panel for the Active Transactions page
pub struct ActiveTransactions {
    wallet: Option<Arc<Wallet>>,
}

impl ActiveTransactions {
    pub fn new(wallet: Arc<Wallet>) -> Self {
        Self { wallet: Some(wallet) }
    }
    
    pub fn new_without_wallet() -> Self {
        Self { wallet: None }
    }

    pub fn preselect(&mut self, _tx: crate::daemon::model::HistoryTransaction) {
        // Placeholder: In the future, this will preselect a transaction
    }
}

impl State for ActiveTransactions {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        let wallet_name = self.wallet.as_ref()
            .map(|w| w.name.as_str())
            .unwrap_or("No Wallet");
        
        view::dashboard(
            menu,
            cache,
            None,
            view::active::active_transactions_view(wallet_name),
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
        self.wallet = Some(wallet);
        Task::none()
    }
}

/// ActiveSettings is a placeholder panel for the Active Settings page
pub struct ActiveSettings {
    wallet: Option<Arc<Wallet>>,
}

impl ActiveSettings {
    pub fn new(wallet: Arc<Wallet>) -> Self {
        Self { wallet: Some(wallet) }
    }
    
    pub fn new_without_wallet() -> Self {
        Self { wallet: None }
    }
}

impl State for ActiveSettings {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        let wallet_name = self.wallet.as_ref()
            .map(|w| w.name.as_str())
            .unwrap_or("No Wallet");
        
        view::dashboard(
            menu,
            cache,
            None,
            view::active::active_settings_view(wallet_name),
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
        self.wallet = Some(wallet);
        Task::none()
    }
}
