use std::collections::HashMap;
use std::sync::Arc;
use std::{cmp::Ordering, collections::HashSet};

use iced::Command;

use liana_ui::widget::Element;
use lianad::commands::CoinStatus;

use crate::{
    app::{
        cache::Cache,
        error::Error,
        menu::Menu,
        message::Message,
        state::{label::LabelsEdited, State},
        view,
        wallet::Wallet,
    },
    daemon::{
        model::{Coin, LabelItem, Labelled},
        Daemon,
    },
};

#[derive(Debug, Default)]
pub struct Coins {
    list: Vec<Coin>,
    labels: HashMap<String, String>,
}

impl Labelled for Coins {
    fn labelled(&self) -> Vec<LabelItem> {
        let mut items = Vec::new();
        for coin in &self.list {
            items.push(LabelItem::OutPoint(coin.outpoint));
            items.push(LabelItem::Txid(coin.outpoint.txid));
            items.push(LabelItem::Address(coin.address.clone()));
        }
        items
    }
    fn labels(&mut self) -> &mut HashMap<String, String> {
        &mut self.labels
    }
}

pub struct CoinsPanel {
    coins: Coins,
    selected: Vec<usize>,
    labels_edited: LabelsEdited,
    warning: Option<Error>,
    /// timelock value to pass for the heir to consume a coin.
    timelock: u16,
}

impl CoinsPanel {
    pub fn new(coins: &[Coin], timelock: u16) -> Self {
        let mut panel = Self {
            labels_edited: LabelsEdited::default(),
            coins: Coins::default(),
            selected: Vec::new(),
            warning: None,
            timelock,
        };
        panel.update_coins(coins);
        panel
    }

    fn update_coins(&mut self, coins: &[Coin]) {
        self.coins.list = coins
            .iter()
            .filter(|coin| coin.spend_info.is_none())
            .cloned()
            .collect();

        self.coins
            .list
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
            view::coins::coins_view(
                cache,
                &self.coins.list,
                self.timelock,
                &self.selected,
                &self.coins.labels,
                self.labels_edited.cache(),
            ),
        )
    }

    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
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
            Message::Labels(res) => match res {
                Err(e) => self.warning = Some(e),
                Ok(labels) => {
                    self.coins.labels = labels;
                }
            },
            Message::View(view::Message::Label(_, _)) | Message::LabelsUpdated(_) => {
                match self.labels_edited.update(
                    daemon,
                    message,
                    std::iter::once(&mut self.coins).map(|a| a as &mut dyn Labelled),
                ) {
                    Ok(cmd) => return cmd,
                    Err(e) => {
                        self.warning = Some(e);
                    }
                }
            }
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

    fn reload(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        _wallet: Arc<Wallet>,
    ) -> Command<Message> {
        let daemon1 = daemon.clone();
        let daemon2 = daemon.clone();
        Command::batch(vec![
            Command::perform(
                async move {
                    daemon1
                        .list_coins(&[CoinStatus::Unconfirmed, CoinStatus::Confirmed], &[])
                        .await
                        .map(|res| res.coins)
                        .map_err(|e| e.into())
                },
                Message::Coins,
            ),
            Command::perform(
                async move {
                    let coins = daemon2
                        .list_coins(&[CoinStatus::Unconfirmed, CoinStatus::Confirmed], &[])
                        .await
                        .map(|res| res.coins)
                        .map_err(Error::from)?;
                    let mut targets = HashSet::<LabelItem>::new();
                    for coin in coins {
                        targets.insert(LabelItem::OutPoint(coin.outpoint));
                        targets.insert(LabelItem::Txid(coin.outpoint.txid));
                        targets.insert(LabelItem::Address(coin.address));
                    }
                    daemon2.get_labels(&targets).await.map_err(|e| e.into())
                },
                Message::Labels,
            ),
        ])
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
        let dummy_address =
            bitcoin::Address::from_str("bc1qvrl2849aggm6qry9ea7xqp2kk39j8vaa8r3cwg")
                .unwrap()
                .assume_checked();

        panel.update_coins(&[
            Coin {
                outpoint: bitcoin::OutPoint { txid, vout: 2 },
                amount: bitcoin::Amount::from_sat(1),
                block_height: Some(3),
                spend_info: None,
                is_immature: false,
                address: dummy_address.clone(),
                derivation_index: 0.into(),
                is_change: false,
            },
            Coin {
                outpoint: bitcoin::OutPoint { txid, vout: 3 },
                amount: bitcoin::Amount::from_sat(1),
                block_height: None,
                spend_info: None,
                is_immature: false,
                address: dummy_address.clone(),
                derivation_index: 1.into(),
                is_change: false,
            },
            Coin {
                outpoint: bitcoin::OutPoint { txid, vout: 0 },
                amount: bitcoin::Amount::from_sat(1),
                block_height: Some(2),
                spend_info: None,
                is_immature: false,
                address: dummy_address.clone(),
                derivation_index: 2.into(),
                is_change: false,
            },
            Coin {
                outpoint: bitcoin::OutPoint { txid, vout: 1 },
                amount: bitcoin::Amount::from_sat(1),
                block_height: Some(3),
                spend_info: None,
                is_immature: false,
                address: dummy_address,
                derivation_index: 3.into(),
                is_change: false,
            },
        ]);

        assert_eq!(
            panel
                .coins
                .list
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
