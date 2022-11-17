use std::sync::Arc;

use iced::pure::Element;
use iced::Command;

use crate::{
    app::{cache::Cache, error::Error, menu::Menu, message::Message, state::State, view},
    daemon::{model::Coin, Daemon},
};

pub struct CoinsPanel {
    coins: Vec<Coin>,
    selected_coin: Option<usize>,
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
            selected_coin: None,
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
            view::coins::coins_view(cache, &self.coins, self.timelock),
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
            Message::View(view::Message::Close) => {
                self.selected_coin = None;
            }
            Message::View(view::Message::Select(i)) => {
                self.selected_coin = Some(i);
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
