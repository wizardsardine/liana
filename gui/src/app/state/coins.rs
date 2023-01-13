use std::cmp::Ordering;
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
        let mut panel = Self {
            coins: Vec::new(),
            selected: Vec::new(),
            warning: None,
            timelock,
        };
        panel.update_coins(coins);
        panel
    }

    fn update_coins(&mut self, coins: &[Coin]) {
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

        self.coins
            .sort_by(|a, b| match (a.block_height, b.block_height) {
                (Some(a_height), Some(b_height)) => {
                    if a_height == b_height {
                        a.outpoint.vout.cmp(&b.outpoint.vout)
                    } else {
                        a_height.cmp(&b_height)
                    }
                }
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (None, None) => a.outpoint.vout.cmp(&b.outpoint.vout),
            })
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
                    self.update_coins(&coins);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::model::Coin;
    use liana::miniscript::bitcoin;
    use std::str::FromStr;

    #[test]
    fn test_coins_panel_update_coins() {
        let mut panel = CoinsPanel::new(&[], 0);
        let txid = bitcoin::Txid::from_str(
            "f7bd1b2a995b689d326e51eb742eb1088c4a8f110d9cb56128fd553acc9f88e5",
        )
        .unwrap();

        panel.update_coins(&[
            Coin {
                outpoint: bitcoin::OutPoint { txid, vout: 2 },
                amount: bitcoin::Amount::from_sat(1),
                block_height: Some(3),
                spend_info: None,
            },
            Coin {
                outpoint: bitcoin::OutPoint { txid, vout: 3 },
                amount: bitcoin::Amount::from_sat(1),
                block_height: None,
                spend_info: None,
            },
            Coin {
                outpoint: bitcoin::OutPoint { txid, vout: 0 },
                amount: bitcoin::Amount::from_sat(1),
                block_height: Some(2),
                spend_info: None,
            },
            Coin {
                outpoint: bitcoin::OutPoint { txid, vout: 1 },
                amount: bitcoin::Amount::from_sat(1),
                block_height: Some(3),
                spend_info: None,
            },
        ]);

        assert_eq!(
            panel
                .coins
                .iter()
                .map(|c| c.outpoint)
                .collect::<Vec<bitcoin::OutPoint>>(),
            vec![
                bitcoin::OutPoint { txid, vout: 0 },
                bitcoin::OutPoint { txid, vout: 1 },
                bitcoin::OutPoint { txid, vout: 2 },
                bitcoin::OutPoint { txid, vout: 3 },
            ]
        )
    }
}
