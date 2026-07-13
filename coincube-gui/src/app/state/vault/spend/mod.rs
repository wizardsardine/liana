mod step;

use std::collections::HashSet;
use std::convert::TryInto;
use std::sync::Arc;

use iced::Task;

use coincube_core::miniscript::bitcoin::{Network, OutPoint};
use coincube_ui::widget::Element;
use coincubed::commands::CoinStatus;

use super::{super::redirect, super::State};
use crate::{
    app::{
        cache::Cache,
        error::Error,
        menu::{Menu, VaultSubMenu},
        message::Message,
        view,
        wallet::{SyncStatus, Wallet},
    },
    daemon::{
        model::{Coin, LabelItem},
        Daemon,
    },
};

use coincube_core::miniscript::bitcoin::Amount;
use coincube_ui::component::amount::BitcoinDisplayUnit;

pub struct CreateSpendPanel {
    draft: step::TransactionDraft,
    current: usize,
    steps: Vec<Box<dyn step::Step>>,
    /// All coins that may be required by any of the steps in the panel.
    /// Additional filtering should be performed by individual steps.
    coins: Vec<Coin>,
    tip_height: i32,
    /// Whether the one-time "default to Normal feerate" fetch has been
    /// dispatched. Fired on the first `reload` so the fee/amount compute
    /// immediately, without the user first picking a feerate — but only once,
    /// so a later re-entry doesn't clobber a feerate they set.
    initial_feerate_requested: bool,
}

impl CreateSpendPanel {
    /// Create a new instance to be used for a primary path spend.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        wallet: Arc<Wallet>,
        coins: &[Coin],
        blockheight: u32,
        network: Network,
        balance: Amount,
        unconfirmed_balance: Amount,
        sync_status: SyncStatus,
        bitcoin_unit: BitcoinDisplayUnit,
    ) -> Self {
        Self {
            draft: step::TransactionDraft::new(network, None),
            current: 0,
            steps: vec![
                Box::new(
                    step::DefineSpend::new(
                        network,
                        wallet.clone(),
                        coins,
                        blockheight,
                        None,
                        true,
                        balance,
                        unconfirmed_balance,
                        sync_status,
                        bitcoin_unit,
                    )
                    .with_coins_sorted(blockheight),
                ),
                Box::new(step::SaveSpend::new(wallet)),
            ],
            coins: coins.to_vec(),
            tip_height: blockheight.try_into().expect("i32 by consensus"),
            initial_feerate_requested: false,
        }
    }

    /// Create a new instance to be used for a recovery spend.
    ///
    /// By default, the wallet's first timelock value is used for `DefineSpend`.
    #[allow(clippy::too_many_arguments)]
    pub fn new_recovery(
        wallet: Arc<Wallet>,
        coins: &[Coin],
        blockheight: u32,
        network: Network,
        balance: Amount,
        unconfirmed_balance: Amount,
        sync_status: SyncStatus,
        bitcoin_unit: BitcoinDisplayUnit,
    ) -> Self {
        let timelock = wallet.as_ref().main_descriptor.first_timelock_value();
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
                        wallet.clone(),
                        coins,
                        blockheight,
                        Some(timelock), // the recovery timelock must always be set to a value
                        false,
                        balance,
                        unconfirmed_balance,
                        sync_status,
                        bitcoin_unit,
                    )
                    .with_coins_sorted(blockheight),
                ),
                Box::new(step::SaveSpend::new(wallet)),
            ],
            coins: coins.to_vec(),
            tip_height: blockheight.try_into().expect("i32 by consensus"),
            initial_feerate_requested: false,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_self_send(
        wallet: Arc<Wallet>,
        coins: &[Coin],
        blockheight: u32,
        preselected_coins: &[OutPoint],
        network: Network,
        balance: Amount,
        unconfirmed_balance: Amount,
        sync_status: SyncStatus,
        bitcoin_unit: BitcoinDisplayUnit,
    ) -> Self {
        Self {
            draft: step::TransactionDraft::new(network, None),
            current: 0,
            steps: vec![
                Box::new(
                    step::DefineSpend::new(
                        network,
                        wallet.clone(),
                        coins,
                        blockheight,
                        None,
                        true,
                        balance,
                        unconfirmed_balance,
                        sync_status,
                        bitcoin_unit,
                    )
                    .with_preselected_coins(preselected_coins)
                    .with_coins_sorted(blockheight)
                    .self_send(),
                ),
                Box::new(step::SaveSpend::new(wallet)),
            ],
            coins: coins.to_vec(),
            tip_height: blockheight.try_into().expect("i32 by consensus"),
            initial_feerate_requested: false,
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
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        self.steps.get(self.current).unwrap().view(menu, cache)
    }

    fn subscription(&self) -> iced::Subscription<Message> {
        self.steps.get(self.current).unwrap().subscription()
    }

    fn interrupt(&mut self) {
        self.steps.get_mut(self.current).unwrap().interrupt();
    }

    fn update(
        &mut self,
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        let daemon = daemon.expect("Daemon required for vault spend panel");
        if matches!(message, Message::View(view::Message::Close)) {
            return redirect(Menu::Vault(VaultSubMenu::PSBTs(None)));
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

        // A recovery spend can't take the `reload` default (it opens on the
        // path-selection step, which would drop the message). Instead fire the
        // same "Normal" (~1h, 6-block target) default the first time we advance
        // onto the DefineSpend step (index 1), so amounts/fees compute without
        // the user first picking a feerate. The `initial_feerate_requested`
        // guard keeps a later re-entry from clobbering a feerate they set.
        let feerate_task =
            if self.draft.is_recovery() && !self.initial_feerate_requested && self.current == 1 {
                self.initial_feerate_requested = true;
                Task::done(Message::View(view::Message::CreateSpend(
                    view::CreateSpendMessage::FetchFeeEstimate(6),
                )))
            } else {
                Task::none()
            };

        if let Some(step) = self.steps.get_mut(self.current) {
            return Task::batch([feerate_task, step.update(daemon, cache, message)]);
        }

        Task::none()
    }

    fn reload(
        &mut self,
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        wallet: Option<Arc<Wallet>>,
    ) -> Task<Message> {
        let daemon = daemon.expect("Vault panels require daemon");
        let wallet = wallet.expect("Vault panels require wallet");
        for step in self.steps.iter_mut() {
            step.reload_wallet(wallet.clone());
        }
        let daemon1 = daemon.clone();
        let daemon2 = daemon.clone();
        let coin_statuses_1 = if self.draft.is_recovery() {
            // only confirmed coins can be included in a recovery spend.
            vec![CoinStatus::Confirmed]
        } else {
            vec![CoinStatus::Unconfirmed, CoinStatus::Confirmed]
        };
        let coin_statuses_2 = coin_statuses_1.clone();
        // The daemon already excludes coins with daemon-known
        // `spend_info` via the `CoinStatus` filter, but its mempool
        // poller can lag a freshly-broadcast tx. The Wallet override
        // fills in synthetic `spend_info` for those inputs; we then
        // drop any coin with `spend_info` set so the Send form's
        // spendable balance reflects the optimistic spend immediately.
        let wallet_for_coins_1 = wallet.clone();
        let wallet_for_coins_2 = wallet.clone();
        let mut tasks = vec![
            Task::perform(
                async move {
                    (
                        daemon1
                            .clone()
                            .list_coins(&coin_statuses_1, &[])
                            .await
                            .map(|res| {
                                let mut coins = res.coins;
                                wallet_for_coins_1.apply_coin_overrides(&mut coins);
                                coins.retain(|c| c.spend_info.is_none());
                                coins
                            })
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
                        .map(|res| {
                            let mut coins = res.coins;
                            wallet_for_coins_2.apply_coin_overrides(&mut coins);
                            coins.retain(|c| c.spend_info.is_none());
                            coins
                        })
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
        ];
        // Default the feerate to "Normal" (~1h, 6-block target) the first time
        // this panel loads, so amounts/fees compute immediately. Recovery
        // starts on the path-selection step, which wouldn't consume it, so it
        // fires the same default from `update` on entering DefineSpend instead.
        if !self.initial_feerate_requested && !self.draft.is_recovery() {
            self.initial_feerate_requested = true;
            tasks.push(Task::done(Message::View(view::Message::CreateSpend(
                view::CreateSpendMessage::FetchFeeEstimate(6),
            ))));
        }
        Task::batch(tasks)
    }
}

impl From<CreateSpendPanel> for Box<dyn State> {
    fn from(s: CreateSpendPanel) -> Box<dyn State> {
        Box::new(s)
    }
}
