mod coins;
mod recovery;
mod settings;
mod spend;

use std::convert::TryInto;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use iced::{widget::qr_code, Command, Subscription};
use iced::{widget::Column, Element};
use liana::miniscript::bitcoin::{Address, Amount};

use super::{cache::Cache, error::Error, menu::Menu, message::Message, view, wallet::Wallet};

use crate::daemon::{
    model::{remaining_sequence, Coin, HistoryTransaction},
    Daemon,
};
pub use coins::CoinsPanel;
pub use recovery::RecoveryPanel;
pub use settings::SettingsState;
pub use spend::{CreateSpendPanel, SpendPanel};

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
    recovery_warning: Option<(Amount, usize)>,
    recovery_alert: Option<(Amount, usize)>,
    pending_events: Vec<HistoryTransaction>,
    events: Vec<HistoryTransaction>,
    selected_event: Option<usize>,
    warning: Option<Error>,
}

impl Home {
    pub fn new(wallet: Arc<Wallet>, coins: &[Coin]) -> Self {
        Self {
            wallet,
            balance: Amount::from_sat(
                coins
                    .iter()
                    .map(|coin| {
                        // If the coin is not spent and is its transaction is confirmed
                        if coin.spend_info.is_none() && coin.block_height.is_some() {
                            coin.amount.to_sat()
                        } else {
                            0
                        }
                    })
                    .sum(),
            ),
            recovery_alert: None,
            recovery_warning: None,
            selected_event: None,
            events: Vec::new(),
            pending_events: Vec::new(),
            warning: None,
        }
    }
}

impl State for Home {
    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        if let Some(i) = self.selected_event {
            let event = if i < self.pending_events.len() {
                &self.pending_events[i]
            } else {
                &self.events[i - self.pending_events.len()]
            };
            return view::modal(
                false,
                self.warning.as_ref(),
                view::home::event_view(cache, event),
                None::<Element<view::Message>>,
            );
        }
        view::dashboard(
            &Menu::Home,
            cache,
            None,
            view::home::home_view(
                &self.balance,
                self.recovery_warning.as_ref(),
                self.recovery_alert.as_ref(),
                &self.pending_events,
                &self.events,
            ),
        )
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
                    let mut recovery_warning = (Amount::from_sat(0), 0);
                    let mut recovery_alert = (Amount::from_sat(0), 0);
                    for coin in coins {
                        if coin.spend_info.is_none() && coin.block_height.is_some() {
                            self.balance += coin.amount;
                            let timelock = self.wallet.main_descriptor.timelock_value();
                            let seq = remaining_sequence(&coin, cache.blockheight as u32, timelock);
                            if seq == 0 {
                                recovery_alert.0 += coin.amount;
                                recovery_alert.1 += 1;
                            } else if seq < timelock * 10 / 100 {
                                recovery_warning.0 += coin.amount;
                                recovery_warning.1 += 1;
                            }
                        }
                    }
                    if recovery_warning.1 > 0 {
                        self.recovery_warning = Some(recovery_warning);
                    }
                    if recovery_alert.1 > 0 {
                        self.recovery_alert = Some(recovery_alert);
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
            Message::View(view::Message::Select(i)) => {
                self.selected_event = Some(i);
            }
            Message::View(view::Message::Next) => {
                if let Some(last) = self.events.last() {
                    let daemon = daemon.clone();
                    let last_event_date = last.time.unwrap() as u32;
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
    address: Option<Address>,
    qr_code: Option<qr_code::State>,
    warning: Option<Error>,
}

impl State for ReceivePanel {
    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        if let Some(address) = &self.address {
            view::dashboard(
                &Menu::Receive,
                cache,
                self.warning.as_ref(),
                view::receive::receive(address, self.qr_code.as_ref().unwrap()),
            )
        } else {
            view::dashboard(&Menu::Receive, cache, self.warning.as_ref(), Column::new())
        }
    }
    fn update(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _cache: &Cache,
        message: Message,
    ) -> Command<Message> {
        if let Message::ReceiveAddress(res) = message {
            match res {
                Ok(address) => {
                    self.warning = None;
                    self.qr_code = Some(qr_code::State::new(&address.to_qr_uri()).unwrap());
                    self.address = Some(address);
                }
                Err(e) => self.warning = Some(e),
            }
        };
        Command::none()
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
        assert_eq!(panel.address, Some(addr));
    }
}
