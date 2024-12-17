mod coins;
mod label;
mod psbt;
mod psbts;
mod receive;
mod recovery;
mod settings;
mod spend;
mod transactions;

use std::convert::TryInto;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use iced::{Command, Subscription};
use liana::miniscript::bitcoin::{Amount, OutPoint};
use liana_ui::widget::*;
use lianad::commands::CoinStatus;

use super::{
    cache::Cache,
    error::Error,
    menu::Menu,
    message::Message,
    view,
    wallet::{sync_status, SyncStatus, Wallet},
};

pub const HISTORY_EVENT_PAGE_SIZE: u64 = 20;

use crate::daemon::{
    model::{remaining_sequence, Coin, HistoryTransaction, Labelled},
    Daemon,
};
pub use coins::CoinsPanel;
use label::LabelsEdited;
pub use psbts::PsbtsPanel;
pub use receive::ReceivePanel;
pub use recovery::RecoveryPanel;
pub use settings::SettingsState;
pub use spend::CreateSpendPanel;
pub use transactions::TransactionsPanel;

pub trait State {
    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message>;
    fn update(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _cache: &Cache,
        _message: Message,
    ) -> Command<Message> {
        Command::none()
    }
    fn subscription(&self) -> Subscription<Message> {
        Subscription::none()
    }
    fn interrupt(&mut self) {}
    fn reload(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _wallet: Arc<Wallet>,
    ) -> Command<Message> {
        Command::none()
    }
}

/// redirect to another state with a message menu
pub fn redirect(menu: Menu) -> Command<Message> {
    Command::perform(async { menu }, |menu| {
        Message::View(view::Message::Menu(menu))
    })
}

/// Returns the confirmed and unconfirmed balances from `coins`, as well
/// as:
/// - the `OutPoint`s of those coins, if any, for which the current
///   `tip_height` is within 10% of the `timelock` expiring.
/// - the smallest number of blocks until the expiry of `timelock` among
///   all confirmed coins, if any.
///
/// The confirmed balance includes the values of any unconfirmed coins
/// from self.
fn coins_summary(
    coins: &[Coin],
    tip_height: u32,
    timelock: u16,
) -> (Amount, Amount, Vec<OutPoint>, Option<u32>) {
    let mut balance = Amount::from_sat(0);
    let mut unconfirmed_balance = Amount::from_sat(0);
    let mut expiring_coins = Vec::new();
    let mut remaining_seq = None;
    for coin in coins {
        if coin.spend_info.is_none() {
            // Include unconfirmed coins from self in confirmed balance.
            if coin.block_height.is_some() || coin.is_from_self {
                balance += coin.amount;
                // Only consider confirmed coins for remaining seq
                // (they would not be considered as expiring so we can also skip that part)
                if coin.block_height.is_none() {
                    continue;
                }
                let seq = remaining_sequence(coin, tip_height, timelock);
                // Warn user for coins that are expiring in less than 10 percent of
                // the timelock.
                if seq <= timelock as u32 * 10 / 100 {
                    expiring_coins.push(coin.outpoint);
                }
                if let Some(last) = &mut remaining_seq {
                    if seq < *last {
                        *last = seq
                    }
                } else {
                    remaining_seq = Some(seq);
                }
            } else {
                unconfirmed_balance += coin.amount;
            }
        }
    }
    (balance, unconfirmed_balance, expiring_coins, remaining_seq)
}

pub struct Home {
    wallet: Arc<Wallet>,
    sync_status: SyncStatus,
    balance: Amount,
    unconfirmed_balance: Amount,
    remaining_sequence: Option<u32>,
    expiring_coins: Vec<OutPoint>,
    events: Vec<HistoryTransaction>,
    is_last_page: bool,
    processing: bool,
    selected_event: Option<(HistoryTransaction, usize)>,
    labels_edited: LabelsEdited,
    warning: Option<Error>,
}

impl Home {
    pub fn new(
        wallet: Arc<Wallet>,
        coins: &[Coin],
        sync_status: SyncStatus,
        tip_height: i32,
    ) -> Self {
        let (balance, unconfirmed_balance, expiring_coins, remaining_seq) = coins_summary(
            coins,
            tip_height as u32,
            wallet.main_descriptor.first_timelock_value(),
        );

        Self {
            wallet,
            sync_status,
            balance,
            unconfirmed_balance,
            remaining_sequence: remaining_seq,
            expiring_coins,
            selected_event: None,
            events: Vec::new(),
            labels_edited: LabelsEdited::default(),
            warning: None,
            is_last_page: false,
            processing: false,
        }
    }
}

impl State for Home {
    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        if let Some((tx, output_index)) = &self.selected_event {
            view::home::payment_view(
                cache,
                &tx,
                *output_index,
                self.labels_edited.cache(),
                self.warning.as_ref(),
            )
        } else {
            view::dashboard(
                &Menu::Home,
                cache,
                None,
                view::home::home_view(
                    &self.balance,
                    &self.unconfirmed_balance,
                    &self.remaining_sequence,
                    &self.expiring_coins,
                    &self.events,
                    self.is_last_page,
                    self.processing,
                    &self.sync_status,
                ),
            )
        }
    }

    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        cache: &Cache,
        message: Message,
    ) -> Command<Message> {
        match message {
            Message::Coins(res) => match res {
                Err(e) => self.warning = Some(e),
                Ok(coins) => {
                    self.warning = None;
                    (
                        self.balance,
                        self.unconfirmed_balance,
                        self.expiring_coins,
                        self.remaining_sequence,
                    ) = coins_summary(
                        &coins,
                        cache.blockheight as u32,
                        self.wallet.main_descriptor.first_timelock_value(),
                    );
                }
            },
            Message::HistoryTransactions(res) => match res {
                Err(e) => self.warning = Some(e),
                Ok(events) => {
                    self.warning = None;
                    self.events = events;
                    self.is_last_page = (self.events.len() as u64) < HISTORY_EVENT_PAGE_SIZE;
                }
            },
            Message::HistoryTransactionsExtension(res) => match res {
                Err(e) => self.warning = Some(e),
                Ok(events) => {
                    self.processing = false;
                    self.warning = None;
                    self.is_last_page = (events.len() as u64) < HISTORY_EVENT_PAGE_SIZE;
                    if let Some(event) = events.first() {
                        if let Some(position) = self
                            .events
                            .iter()
                            .position(|event2| event2.txid == event.txid)
                        {
                            let len = self.events.len();
                            for event in events {
                                if !self.events[position..len]
                                    .iter()
                                    .any(|event2| event2.txid == event.txid)
                                {
                                    self.events.push(event);
                                }
                            }
                        } else {
                            self.events.extend(events);
                        }
                    }
                }
            },
            Message::UpdatePanelCache(is_current, Ok(cache)) => {
                let wallet_was_syncing = !self.sync_status.is_synced();
                self.sync_status = sync_status(
                    daemon.backend(),
                    cache.blockheight,
                    cache.sync_progress,
                    cache.last_poll_timestamp,
                    cache.last_poll_at_startup,
                );
                // If this is the current panel, reload it if wallet is no longer syncing.
                if is_current && wallet_was_syncing && self.sync_status.is_synced() {
                    return self.reload(daemon, self.wallet.clone());
                }
            }
            Message::Payment(res) => match res {
                Ok(event) => {
                    self.selected_event = Some(event);
                }
                Err(e) => {
                    self.warning = Some(e);
                }
            },
            Message::View(view::Message::SelectSub(i, j)) => {
                let txid = self.events[i].txid;
                return Command::perform(
                    async move {
                        let tx = daemon.get_history_txs(&[txid]).await?.remove(0);
                        Ok((tx, j))
                    },
                    Message::Payment,
                );
            }
            Message::View(view::Message::Label(_, _)) | Message::LabelsUpdated(_) => {
                match self.labels_edited.update(
                    daemon,
                    message,
                    self.events
                        .iter_mut()
                        .map(|tx| tx as &mut dyn Labelled)
                        .chain(
                            self.selected_event
                                .iter_mut()
                                .map(|(tx, _)| tx as &mut dyn Labelled),
                        ),
                ) {
                    Ok(cmd) => {
                        return cmd;
                    }
                    Err(e) => {
                        self.warning = Some(e);
                    }
                };
            }
            Message::View(view::Message::Reload) => {
                return self.reload(daemon, self.wallet.clone());
            }
            Message::View(view::Message::Close) => {
                self.selected_event = None;
            }

            Message::View(view::Message::Next) => {
                if let Some(last) = self.events.last() {
                    let daemon = daemon.clone();
                    let last_event_date = last.time.unwrap();
                    self.processing = true;
                    return Command::perform(
                        async move {
                            let mut limit = HISTORY_EVENT_PAGE_SIZE;
                            let mut events = daemon
                                .list_history_txs(0_u32, last_event_date, limit)
                                .await?;

                            // because gethistory cursor is inclusive and use blocktime
                            // multiple events can occur in the same block.
                            // If there is more event in the same block that the
                            // HISTORY_EVENT_PAGE_SIZE they can not be retrieved by changing
                            // the cursor value (blocktime) but by increasing the limit.
                            //
                            // 1. Check if the events retrieved have all the same blocktime
                            let blocktime = if let Some(event) = events.first() {
                                event.time
                            } else {
                                return Ok(events);
                            };

                            // 2. Retrieve a larger batch of event with the same cursor but
                            //    a larger limit.
                            while !events.iter().any(|evt| evt.time != blocktime)
                                && events.len() as u64 == limit
                            {
                                // increments of the equivalent of one page more.
                                limit += HISTORY_EVENT_PAGE_SIZE;
                                events = daemon.list_history_txs(0, last_event_date, limit).await?;
                            }
                            events.sort_by(|a, b| a.compare(b));
                            Ok(events)
                        },
                        Message::HistoryTransactionsExtension,
                    );
                }
            }
            _ => {}
        };
        Command::none()
    }

    fn reload(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        wallet: Arc<Wallet>,
    ) -> Command<Message> {
        // If the wallet is syncing, we expect it to finish soon and so better to wait for
        // updated data before reloading. Besides, if the wallet is syncing, the DB may be
        // locked if the poller is running and we wouldn't be able to reload data until
        // syncing completes anyway.
        if self.sync_status.wallet_is_syncing() {
            return Command::none();
        }
        self.selected_event = None;
        self.wallet = wallet;
        let daemon2 = daemon.clone();
        let now: u32 = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .try_into()
            .unwrap();
        Command::batch(vec![
            Command::perform(
                async move {
                    let mut txs = daemon
                        .list_history_txs(0, now, HISTORY_EVENT_PAGE_SIZE)
                        .await?;
                    txs.sort_by(|a, b| a.compare(b));

                    let mut pending_txs = daemon.list_pending_txs().await?;
                    pending_txs.extend(txs);
                    Ok(pending_txs)
                },
                Message::HistoryTransactions,
            ),
            Command::perform(
                async move {
                    daemon2
                        .list_coins(&[CoinStatus::Unconfirmed, CoinStatus::Confirmed], &[])
                        .await
                        .map(|res| res.coins)
                        .map_err(|e| e.into())
                },
                Message::Coins,
            ),
        ])
    }
}

impl From<Home> for Box<dyn State> {
    fn from(s: Home) -> Box<dyn State> {
        Box::new(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::model::Coin;
    use liana::miniscript::bitcoin;
    use lianad::commands::LCSpendInfo;
    use std::str::FromStr;
    #[tokio::test]
    async fn test_coins_summary() {
        // Will use the same address for all coins.
        let dummy_address =
            bitcoin::Address::from_str("bc1qvrl2849aggm6qry9ea7xqp2kk39j8vaa8r3cwg")
                .unwrap()
                .assume_checked();
        // Will use the same txid for all outpoints and spend info.
        let dummy_txid = bitcoin::Txid::from_str(
            "f7bd1b2a995b689d326e51eb742eb1088c4a8f110d9cb56128fd553acc9f88e5",
        )
        .unwrap();

        let tip_height = 800_000;
        let timelock = 10_000;
        let mut coins = Vec::new();
        // Without coins, all values are 0 / empty / None:
        assert_eq!(
            coins_summary(&coins, tip_height, timelock),
            (Amount::from_sat(0), Amount::from_sat(0), Vec::new(), None)
        );
        // Add a spending coin.
        coins.push(Coin {
            outpoint: OutPoint::new(dummy_txid, 0),
            amount: Amount::from_sat(100),
            address: dummy_address.clone(),
            derivation_index: bitcoin::bip32::ChildNumber::Normal { index: 0 },
            block_height: Some(1),
            is_immature: false,
            is_change: false,
            is_from_self: false,
            spend_info: Some(LCSpendInfo {
                txid: dummy_txid,
                height: None,
            }),
        });
        // Spending coin is ignored.
        assert_eq!(
            coins_summary(&coins, tip_height, timelock),
            (Amount::from_sat(0), Amount::from_sat(0), Vec::new(), None)
        );
        // Add unconfirmed change coin not from self.
        coins.push(Coin {
            outpoint: OutPoint::new(dummy_txid, 1),
            amount: Amount::from_sat(109),
            address: dummy_address.clone(),
            derivation_index: bitcoin::bip32::ChildNumber::Normal { index: 1 },
            block_height: None,
            is_immature: false,
            is_change: true,
            is_from_self: false,
            spend_info: None,
        });
        // Included in unconfirmed balance. Other values remain the same.
        assert_eq!(
            coins_summary(&coins, tip_height, timelock),
            (Amount::from_sat(0), Amount::from_sat(109), Vec::new(), None)
        );
        // Add unconfirmed coin from self.
        coins.push(Coin {
            outpoint: OutPoint::new(dummy_txid, 2),
            amount: Amount::from_sat(111),
            address: dummy_address.clone(),
            derivation_index: bitcoin::bip32::ChildNumber::Normal { index: 2 },
            block_height: None,
            is_immature: false,
            is_change: false,
            is_from_self: true,
            spend_info: None,
        });
        // Included in confirmed balance. Other values remain the same.
        assert_eq!(
            coins_summary(&coins, tip_height, timelock),
            (
                Amount::from_sat(111),
                Amount::from_sat(109),
                Vec::new(),
                None
            )
        );
        // Add a confirmed coin 1 more than 10% from expiry:
        coins.push(Coin {
            outpoint: OutPoint::new(dummy_txid, 3),
            amount: Amount::from_sat(101),
            address: dummy_address.clone(),
            derivation_index: bitcoin::bip32::ChildNumber::Normal { index: 3 },
            block_height: Some(791_001), // 791_001 + timelock - tip_height = 1_001 > 1_000 = (timelock / 10)
            is_immature: false,
            is_change: false,
            is_from_self: false,
            spend_info: None,
        });
        // Coin is added to confirmed balance. Not expiring, but remaining seq is set.
        assert_eq!(
            coins_summary(&coins, tip_height, timelock),
            (
                Amount::from_sat(212),
                Amount::from_sat(109),
                Vec::new(),
                Some(1_001)
            )
        );
        // Now decrease the last coin's confirmation height by 1 so that
        // it is within 10% of expiry:
        coins.last_mut().unwrap().block_height = Some(791_000);
        // Its outpoint has been added to expiring coins and remaining seq is lower.
        assert_eq!(
            coins_summary(&coins, tip_height, timelock),
            (
                Amount::from_sat(212),
                Amount::from_sat(109),
                vec![OutPoint::new(dummy_txid, 3)],
                Some(1_000)
            )
        );
        // Now add a confirmed coin that is not yet expiring.
        coins.push(Coin {
            outpoint: OutPoint::new(dummy_txid, 4),
            amount: Amount::from_sat(105),
            address: dummy_address.clone(),
            derivation_index: bitcoin::bip32::ChildNumber::Normal { index: 4 },
            block_height: Some(792_000),
            is_immature: false,
            is_change: false,
            is_from_self: false,
            spend_info: None,
        });
        // Only confirmed balance has changed.
        assert_eq!(
            coins_summary(&coins, tip_height, timelock),
            (
                Amount::from_sat(317),
                Amount::from_sat(109),
                vec![OutPoint::new(dummy_txid, 3)],
                Some(1_000)
            )
        );
        // Now add another confirmed coin that is expiring.
        coins.push(Coin {
            outpoint: OutPoint::new(dummy_txid, 5),
            amount: Amount::from_sat(108),
            address: dummy_address.clone(),
            derivation_index: bitcoin::bip32::ChildNumber::Normal { index: 5 },
            block_height: Some(790_500),
            is_immature: false,
            is_change: false,
            is_from_self: false,
            spend_info: None,
        });
        // Confirmed balance updated, as well as expiring coins and the remaining seq.
        assert_eq!(
            coins_summary(&coins, tip_height, timelock),
            (
                Amount::from_sat(425),
                Amount::from_sat(109),
                vec![OutPoint::new(dummy_txid, 3), OutPoint::new(dummy_txid, 5)],
                Some(500)
            )
        );
    }
}
