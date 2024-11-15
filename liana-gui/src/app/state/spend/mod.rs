mod step;

use std::collections::HashSet;
use std::sync::Arc;

use iced::Command;

use liana::{
    commands::CoinStatus,
    miniscript::bitcoin::{Network, OutPoint},
};
use liana_ui::widget::Element;

use super::{redirect, State};
use crate::{
    app::{cache::Cache, error::Error, menu::Menu, message::Message, view, wallet::Wallet},
    daemon::{
        model::{Coin, LabelItem},
        Daemon,
    },
};

pub struct CreateSpendPanel {
    draft: step::TransactionDraft,
    current: usize,
    steps: Vec<Box<dyn step::Step>>,
}

impl CreateSpendPanel {
    pub fn new(wallet: Arc<Wallet>, coins: &[Coin], blockheight: u32, network: Network) -> Self {
        let descriptor = wallet.main_descriptor.clone();
        let timelock = descriptor.first_timelock_value();
        Self {
            draft: step::TransactionDraft::new(network),
            current: 0,
            steps: vec![
                Box::new(
                    step::DefineSpend::new(network, descriptor, coins, timelock)
                        .with_coins_sorted(blockheight),
                ),
                Box::new(step::SaveSpend::new(wallet)),
            ],
        }
    }

    pub fn new_self_send(
        wallet: Arc<Wallet>,
        coins: &[Coin],
        blockheight: u32,
        preselected_coins: &[OutPoint],
        network: Network,
    ) -> Self {
        let descriptor = wallet.main_descriptor.clone();
        let timelock = descriptor.first_timelock_value();
        Self {
            draft: step::TransactionDraft::new(network),
            current: 0,
            steps: vec![
                Box::new(
                    step::DefineSpend::new(network, descriptor, coins, timelock)
                        .with_preselected_coins(preselected_coins)
                        .with_coins_sorted(blockheight)
                        .self_send(),
                ),
                Box::new(step::SaveSpend::new(wallet)),
            ],
        }
    }

    pub fn is_first_step(&self) -> bool {
        self.current == 0
    }
}

impl State for CreateSpendPanel {
    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        self.steps.get(self.current).unwrap().view(cache)
    }

    fn subscription(&self) -> iced::Subscription<Message> {
        self.steps.get(self.current).unwrap().subscription()
    }

    fn interrupt(&mut self) {
        self.steps.get_mut(self.current).unwrap().interrupt();
    }

    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        cache: &Cache,
        message: Message,
    ) -> Command<Message> {
        if matches!(message, Message::View(view::Message::Close)) {
            return redirect(Menu::PSBTs);
        }

        if matches!(message, Message::View(view::Message::Next)) {
            if let Some(step) = self.steps.get(self.current) {
                step.apply(&mut self.draft);
            }

            if let Some(step) = self.steps.get_mut(self.current + 1) {
                self.current += 1;
                step.load(&self.draft);
            }
        }

        if matches!(message, Message::View(view::Message::Previous))
            && self.steps.get(self.current - 1).is_some()
        {
            self.current -= 1;
        }

        if let Some(step) = self.steps.get_mut(self.current) {
            return step.update(daemon, cache, message);
        }

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
                    let coins = daemon
                        .list_coins(&[CoinStatus::Unconfirmed, CoinStatus::Confirmed], &[])
                        .await
                        .map(|res| res.coins)
                        .map_err(Error::from)?;
                    let mut targets = HashSet::<LabelItem>::new();
                    for coin in coins {
                        targets.insert(LabelItem::OutPoint(coin.outpoint));
                        targets.insert(LabelItem::Txid(coin.outpoint.txid));
                    }
                    daemon2.get_labels(&targets).await.map_err(|e| e.into())
                },
                Message::Labels,
            ),
        ])
    }
}

impl From<CreateSpendPanel> for Box<dyn State> {
    fn from(s: CreateSpendPanel) -> Box<dyn State> {
        Box::new(s)
    }
}
