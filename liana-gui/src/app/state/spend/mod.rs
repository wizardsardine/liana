mod step;

use std::collections::HashSet;
use std::convert::TryInto;
use std::sync::Arc;

use iced::Task;

use liana::miniscript::bitcoin::{Network, OutPoint};
use liana_ui::widget::Element;
use lianad::commands::CoinStatus;

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
    /// All coins that may be required by any of the steps in the panel.
    /// Additional filtering should be performed by individual steps.
    coins: Vec<Coin>,
    tip_height: i32,
}

impl CreateSpendPanel {
    /// Create a new instance to be used for a primary path spend.
    pub fn new(wallet: Arc<Wallet>, coins: &[Coin], blockheight: u32, network: Network) -> Self {
        let descriptor = wallet.main_descriptor.clone();
        Self {
            draft: step::TransactionDraft::new(network, None),
            current: 0,
            steps: vec![
                Box::new(
                    step::DefineSpend::new(network, descriptor, coins, blockheight, None, true)
                        .with_coins_sorted(blockheight),
                ),
                Box::new(step::SaveSpend::new(wallet)),
            ],
            coins: coins.to_vec(),
            tip_height: blockheight.try_into().expect("i32 by consensus"),
        }
    }

    /// Create a new instance to be used for a recovery spend.
    ///
    /// By default, the wallet's first timelock value is used for `DefineSpend`.
    pub fn new_recovery(
        wallet: Arc<Wallet>,
        coins: &[Coin],
        blockheight: u32,
        network: Network,
    ) -> Self {
        let descriptor = wallet.main_descriptor.clone();
        let timelock = descriptor.first_timelock_value();
        Self {
            draft: step::TransactionDraft::new(network, Some(timelock)),
            current: 0,
            steps: vec![
                Box::new(step::SelectRecoveryPath::new(
                    wallet.clone(),
                    coins,
                    blockheight.try_into().expect("i32 by consensus"),
                )),
                Box::new(
                    step::DefineSpend::new(
                        network,
                        descriptor,
                        coins,
                        blockheight,
                        Some(timelock), // the recovery timelock must always be set to a value
                        false,
                    )
                    .with_coins_sorted(blockheight),
                ),
                Box::new(step::SaveSpend::new(wallet)),
            ],
            coins: coins.to_vec(),
            tip_height: blockheight.try_into().expect("i32 by consensus"),
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
        Self {
            draft: step::TransactionDraft::new(network, None),
            current: 0,
            steps: vec![
                Box::new(
                    step::DefineSpend::new(network, descriptor, coins, blockheight, None, true)
                        .with_preselected_coins(preselected_coins)
                        .with_coins_sorted(blockheight)
                        .self_send(),
                ),
                Box::new(step::SaveSpend::new(wallet)),
            ],
            coins: coins.to_vec(),
            tip_height: blockheight.try_into().expect("i32 by consensus"),
        }
    }

    pub fn keep_state(&self) -> bool {
        if self.draft.is_recovery() {
            // For recovery spend, retain the state if user is on the first two steps
            // (choosing recovery path and defining spend)
            self.current < 2
        } else {
            self.current == 0
        }
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
    ) -> Task<Message> {
        if matches!(message, Message::View(view::Message::Close)) {
            return redirect(Menu::PSBTs);
        }

        if matches!(message, Message::View(view::Message::Next)) {
            if let Some(step) = self.steps.get(self.current) {
                step.apply(&mut self.draft);
            }

            if let Some(step) = self.steps.get_mut(self.current + 1) {
                self.current += 1;
                step.load(&self.coins, self.tip_height, &self.draft);
            }
        }

        if matches!(message, Message::View(view::Message::Previous)) {
            let previous = self.current.saturating_sub(1);
            if let Some(step) = self.steps.get_mut(previous) {
                self.current = previous;
                // For recovery spends, ensure all steps use the latest coins and tip height.
                // TODO: consider doing this for all spend kinds, not just recovery.
                if self.draft.is_recovery() {
                    step.load(&self.coins, self.tip_height, &self.draft);
                }
            }
        }

        if let Message::CoinsTipHeight(Ok(coins), Ok(tip)) = &message {
            // Save the coins and tip for use in the `load()` method.
            self.coins = coins.clone();
            self.tip_height = *tip;
            // We still send this message to the current step below to update the values directly.
        }

        if let Some(step) = self.steps.get_mut(self.current) {
            return step.update(daemon, cache, message);
        }

        Task::none()
    }

    fn reload(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        _wallet: Arc<Wallet>,
    ) -> Task<Message> {
        let daemon1 = daemon.clone();
        let daemon2 = daemon.clone();
        let coin_statuses_1 = if self.draft.is_recovery() {
            // only confirmed coins can be included in a recovery spend.
            vec![CoinStatus::Confirmed]
        } else {
            vec![CoinStatus::Unconfirmed, CoinStatus::Confirmed]
        };
        let coin_statuses_2 = coin_statuses_1.clone();
        Task::batch(vec![
            Task::perform(
                async move {
                    (
                        daemon1
                            .clone()
                            .list_coins(&coin_statuses_1, &[])
                            .await
                            .map(|res| res.coins)
                            .map_err(|e| e.into()),
                        daemon1
                            .get_info()
                            .await
                            .map(|res| res.block_height)
                            .map_err(|e| e.into()),
                    )
                },
                |(res_coins, res_tip)| Message::CoinsTipHeight(res_coins, res_tip),
            ),
            Task::perform(
                async move {
                    let coins = daemon
                        .list_coins(&coin_statuses_2, &[])
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
