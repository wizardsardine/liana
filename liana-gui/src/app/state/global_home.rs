use std::sync::Arc;

use iced::Task;
use liana_ui::widget::*;

use super::{Cache, Menu, State};
use crate::app::{message::Message, view, wallet::Wallet};
use crate::daemon::Daemon;

/// GlobalHome is a placeholder panel for the top-level Home page
/// This is separate from the Vault Home page
pub struct GlobalHome {
    wallet: Option<Arc<Wallet>>,
}

impl GlobalHome {
    pub fn new(wallet: Arc<Wallet>) -> Self {
        Self { wallet: Some(wallet) }
    }
    
    pub fn new_without_wallet() -> Self {
        Self { wallet: None }
    }
}

impl State for GlobalHome {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        let wallet_name = self.wallet.as_ref()
            .map(|w| w.name.as_str())
            .unwrap_or("No Vault");
        
        view::dashboard(
            menu,
            cache,
            None,
            view::global_home::global_home_view(wallet_name),
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
