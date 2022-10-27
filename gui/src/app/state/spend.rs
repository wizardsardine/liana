use std::sync::Arc;

use iced::{pure::Element, Command};

use super::State;
use crate::{
    app::{cache::Cache, error::Error, menu::Menu, message::Message, view},
    daemon::{
        model::{Coin, SpendTx},
        Daemon,
    },
};

pub struct SpendPanel {
    spend_txs: Vec<SpendTx>,
    warning: Option<Error>,
}

impl SpendPanel {
    pub fn new(_coins: &[Coin], spend_txs: &[SpendTx]) -> Self {
        Self {
            spend_txs: spend_txs.to_vec(),
            warning: None,
        }
    }
}

impl State for SpendPanel {
    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        view::dashboard(
            &Menu::Spend,
            cache,
            self.warning.as_ref(),
            view::spend::spend_view(&self.spend_txs),
        )
    }

    fn update(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _cache: &Cache,
        _message: Message,
    ) -> Command<Message> {
        Command::none()
    }

    fn load(&self, daemon: Arc<dyn Daemon + Sync + Send>) -> Command<Message> {
        let daemon = daemon.clone();
        Command::perform(
            async move {
                daemon
                    .list_spend_txs()
                    .map(|res| res.spend_txs)
                    .map_err(|e| e.into())
            },
            Message::SpendTxs,
        )
    }
}

impl From<SpendPanel> for Box<dyn State> {
    fn from(s: SpendPanel) -> Box<dyn State> {
        Box::new(s)
    }
}
