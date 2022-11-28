use std::sync::Arc;

use iced::{Command, Element};

use crate::{
    app::{cache::Cache, error::Error, menu::Menu, message::Message, state::State, view},
    daemon::{model::Coin, Daemon},
};

pub struct CoinsPanel {
    coins: Vec<Coin>,
    selected: Vec<usize>,
    warning: Option<Error>,
    /// timelock value to pass for the heir to consume a coin.
    timelock: u32,
}

impl CoinsPanel {
    pub fn new(coins: &[Coin], timelock: u32) -> Self {
        Self {
            coins: coins
                .iter()
                .filter_map(|coin| {
                    if coin.spend_info.is_none() {
                        Some(*coin)
                    } else {
                        None
                    }
                })
                .collect(),
            selected: Vec::new(),
            warning: None,
            timelock,
        }
    }
}

impl State for CoinsPanel {
    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        view::dashboard(
            &Menu::Coins,
            cache,
            self.warning.as_ref(),
            view::coins::coins_view(cache, &self.coins, self.timelock, &self.selected),
        )
    }

    fn update(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _cache: &Cache,
        message: Message,
    ) -> Command<Message> {
        match message {
            Message::Coins(res) => match res {
                Err(e) => self.warning = Some(e),
                Ok(coins) => {
                    self.selected = Vec::new();
                    self.warning = None;
                    self.coins = coins
                        .iter()
                        .filter_map(|coin| {
                            if coin.spend_info.is_none() {
                                Some(*coin)
                            } else {
                                None
                            }
                        })
                        .collect();
                }
            },
            Message::View(view::Message::Select(i)) => {
                if let Some(position) = self.selected.iter().position(|j| *j == i) {
                    self.selected.remove(position);
                } else {
                    self.selected.push(i);
                }
            }
            _ => {}
        };
        Command::none()
    }

    fn load(&self, daemon: Arc<dyn Daemon + Sync + Send>) -> Command<Message> {
        let daemon = daemon.clone();
        Command::perform(
            async move {
                daemon
                    .list_coins()
                    .map(|res| res.coins)
                    .map_err(|e| e.into())
            },
            Message::Coins,
        )
    }
}

impl From<CoinsPanel> for Box<dyn State> {
    fn from(s: CoinsPanel) -> Box<dyn State> {
        Box::new(s)
    }
}
