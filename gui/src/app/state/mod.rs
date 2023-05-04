mod coins;
mod psbt;
mod psbts;
mod recovery;
mod settings;
mod spend;
mod transactions;

use std::convert::TryInto;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use iced::{widget::qr_code, Command, Subscription};
use liana::miniscript::bitcoin::{Address, Amount};
use liana_ui::widget::*;

use super::{cache::Cache, error::Error, menu::Menu, message::Message, view, wallet::Wallet};

use crate::daemon::{
    model::{remaining_sequence, Coin, HistoryTransaction},
    Daemon,
};
pub use coins::CoinsPanel;
pub use psbts::PsbtsPanel;
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
    fn load(&self, _daemon: Arc<dyn Daemon + Sync + Send>) -> Command<Message> {
        Command::none()
    }
}

pub struct Home {
    wallet: Arc<Wallet>,
    balance: Amount,
    unconfirmed_balance: Amount,
    remaining_sequence: Option<u32>,
    number_of_expiring_coins: usize,
    pending_events: Vec<HistoryTransaction>,
    events: Vec<HistoryTransaction>,
    selected_event: Option<(usize, usize)>,
    warning: Option<Error>,
}

impl Home {
    pub fn new(wallet: Arc<Wallet>, coins: &[Coin]) -> Self {
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
            balance,
            unconfirmed_balance,
            remaining_sequence: None,
            number_of_expiring_coins: 0,
            selected_event: None,
            events: Vec::new(),
            pending_events: Vec::new(),
            warning: None,
        }
    }
}

impl State for Home {
    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        if let Some((i, output_index)) = self.selected_event {
            let event = if i < self.pending_events.len() {
                &self.pending_events[i]
            } else {
                &self.events[i - self.pending_events.len()]
            };
            view::home::payment_view(cache, event, output_index, self.warning.as_ref())
        } else {
            view::dashboard(
                &Menu::Home,
                cache,
                None,
                view::home::home_view(
                    &self.balance,
                    &self.unconfirmed_balance,
                    &self.remaining_sequence,
                    self.number_of_expiring_coins,
                    &self.pending_events,
                    &self.events,
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
                    self.number_of_expiring_coins = 0;
                    for coin in coins {
                        if coin.spend_info.is_none() {
                            if coin.block_height.is_some() {
                                self.balance += coin.amount;
                                let timelock = self.wallet.main_descriptor.first_timelock_value();
                                let seq =
                                    remaining_sequence(&coin, cache.blockheight as u32, timelock);
                                // number of block in a day
                                if seq <= 144 {
                                    self.number_of_expiring_coins += 1;
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
                    for event in events {
                        if !self.events.iter().any(|other| other.tx == event.tx) {
                            self.events.push(event);
                        }
                    }
                }
            },
            Message::PendingTransactions(res) => match res {
                Err(e) => self.warning = Some(e),
                Ok(events) => {
                    self.warning = None;
                    for event in events {
                        if !self.pending_events.iter().any(|other| other.tx == event.tx) {
                            self.pending_events.push(event);
                        }
                    }
                }
            },
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
                    return Command::perform(
                        async move {
                            let mut limit = view::home::HISTORY_EVENT_PAGE_SIZE;
                            let mut events =
                                daemon.list_history_txs(0_u32, last_event_date, limit)?;

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
                                limit += view::home::HISTORY_EVENT_PAGE_SIZE;
                                events = daemon.list_history_txs(0, last_event_date, limit)?;
                            }
                            Ok(events)
                        },
                        Message::HistoryTransactions,
                    );
                }
            }
            _ => {}
        };
        Command::none()
    }

    fn load(&self, daemon: Arc<dyn Daemon + Sync + Send>) -> Command<Message> {
        let daemon1 = daemon.clone();
        let daemon2 = daemon.clone();
        let daemon3 = daemon.clone();
        let now: u32 = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .try_into()
            .unwrap();
        Command::batch(vec![
            Command::perform(
                async move { daemon3.list_pending_txs().map_err(|e| e.into()) },
                Message::PendingTransactions,
            ),
            Command::perform(
                async move {
                    daemon1
                        .list_history_txs(0, now, view::home::HISTORY_EVENT_PAGE_SIZE)
                        .map_err(|e| e.into())
                },
                Message::HistoryTransactions,
            ),
            Command::perform(
                async move {
                    daemon2
                        .list_coins()
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

#[derive(Default)]
pub struct ReceivePanel {
    addresses: Vec<Address>,
    qr_code: Option<qr_code::State>,
    warning: Option<Error>,
}

impl State for ReceivePanel {
    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        view::dashboard(
            &Menu::Receive,
            cache,
            self.warning.as_ref(),
            view::receive::receive(&self.addresses, self.qr_code.as_ref()),
        )
    }
    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        _cache: &Cache,
        message: Message,
    ) -> Command<Message> {
        match message {
            Message::ReceiveAddress(res) => {
                match res {
                    Ok(address) => {
                        self.warning = None;
                        self.qr_code = Some(qr_code::State::new(address.to_qr_uri()).unwrap());
                        self.addresses.push(address);
                    }
                    Err(e) => self.warning = Some(e),
                }
                Command::none()
            }
            Message::View(view::Message::Next) => self.load(daemon),
            _ => Command::none(),
        }
    }

    fn load(&self, daemon: Arc<dyn Daemon + Sync + Send>) -> Command<Message> {
        let daemon = daemon.clone();
        Command::perform(
            async move {
                daemon
                    .get_new_address()
                    .map(|res| res.address)
                    .map_err(|e| e.into())
            },
            Message::ReceiveAddress,
        )
    }
}

impl From<ReceivePanel> for Box<dyn State> {
    fn from(s: ReceivePanel) -> Box<dyn State> {
        Box::new(s)
    }
}

/// redirect to another state with a message menu
pub fn redirect(menu: Menu) -> Command<Message> {
    Command::perform(async { menu }, |menu| {
        Message::View(view::Message::Menu(menu))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::cache::Cache,
        daemon::{
            client::{Lianad, Request},
            model::*,
        },
        utils::{mock::Daemon, sandbox::Sandbox},
    };

    use liana::miniscript::bitcoin::Address;
    use serde_json::json;
    use std::str::FromStr;

    #[tokio::test]
    async fn test_receive_panel() {
        let addr =
            Address::from_str("tb1qkldgvljmjpxrjq2ev5qxe8dvhn0dph9q85pwtfkjeanmwdue2akqj4twxj")
                .unwrap();
        let daemon = Daemon::new(vec![(
            Some(json!({"method": "getnewaddress", "params": Option::<Request>::None})),
            Ok(json!(GetAddressResult {
                address: addr.clone()
            })),
        )]);

        let sandbox: Sandbox<ReceivePanel> = Sandbox::new(ReceivePanel::default());
        let client = Arc::new(Lianad::new(daemon.run()));
        let sandbox = sandbox.load(client, &Cache::default()).await;

        let panel = sandbox.state();
        assert_eq!(panel.addresses, vec![addr]);
    }
}
