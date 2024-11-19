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
    selected_event: Option<(usize, usize)>,
    labels_edited: LabelsEdited,
    warning: Option<Error>,
}

impl Home {
    pub fn new(wallet: Arc<Wallet>, coins: &[Coin], sync_status: SyncStatus) -> Self {
        let (balance, unconfirmed_balance) = coins.iter().fold(
            (Amount::from_sat(0), Amount::from_sat(0)),
            |(balance, unconfirmed_balance), coin| {
                if coin.spend_info.is_some() {
                    (balance, unconfirmed_balance)
                } else if coin.block_height.is_some() {
                    (balance + coin.amount, unconfirmed_balance)
                } else {
                    (balance, unconfirmed_balance + coin.amount)
                }
            },
        );

        Self {
            wallet,
            sync_status,
            balance,
            unconfirmed_balance,
            remaining_sequence: None,
            expiring_coins: Vec::new(),
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
        if let Some((i, output_index)) = self.selected_event {
            view::home::payment_view(
                cache,
                &self.events[i],
                output_index,
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
                    self.balance = Amount::from_sat(0);
                    self.unconfirmed_balance = Amount::from_sat(0);
                    self.remaining_sequence = None;
                    self.expiring_coins = Vec::new();
                    for coin in coins {
                        if coin.spend_info.is_none() {
                            if coin.block_height.is_some() {
                                self.balance += coin.amount;
                                let timelock = self.wallet.main_descriptor.first_timelock_value();
                                let seq =
                                    remaining_sequence(&coin, cache.blockheight as u32, timelock);
                                // Warn user for coins that are expiring in less than 10 percent of
                                // the timelock.
                                if seq <= timelock as u32 * 10 / 100 {
                                    self.expiring_coins.push(coin.outpoint);
                                }
                                if let Some(last) = &mut self.remaining_sequence {
                                    if seq < *last {
                                        *last = seq
                                    }
                                } else {
                                    self.remaining_sequence = Some(seq);
                                }
                            } else {
                                self.unconfirmed_balance += coin.amount;
                            }
                        }
                    }
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
            Message::View(view::Message::Label(_, _)) | Message::LabelsUpdated(_) => {
                match self.labels_edited.update(
                    daemon,
                    message,
                    self.events.iter_mut().map(|tx| tx as &mut dyn Labelled),
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
            Message::View(view::Message::SelectSub(i, j)) => {
                self.selected_event = Some((i, j));
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
