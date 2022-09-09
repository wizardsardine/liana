mod coins;
mod settings;
mod spend;

use std::convert::TryInto;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use iced::pure::{column, Element};
use iced::{widget::qr_code, Command, Subscription};
use minisafe::miniscript::bitcoin::{Address, Amount};

use super::{cache::Cache, error::Error, menu::Menu, message::Message, view};

use crate::daemon::{
    model::{Coin, HistoryEvent},
    Daemon,
};
pub use coins::CoinsPanel;
pub use settings::SettingsState;
pub use spend::{CreateSpendPanel, SpendPanel};

pub trait State {
    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message>;
    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        cache: &Cache,
        message: Message,
    ) -> Command<Message>;
    fn subscription(&self) -> Subscription<Message> {
        Subscription::none()
    }
    fn load(&self, _daemon: Arc<dyn Daemon + Sync + Send>) -> Command<Message> {
        Command::none()
    }
}

pub struct Home {
    balance: Amount,
    events: Vec<HistoryEvent>,
    warning: Option<Error>,
}

impl Home {
    pub fn new(coins: &[Coin]) -> Self {
        Self {
            balance: Amount::from_sat(
                coins
                    .iter()
                    .map(|coin| {
                        if coin.spend_info.is_none() {
                            coin.amount.to_sat()
                        } else {
                            0
                        }
                    })
                    .sum(),
            ),

            events: Vec::new(),
            warning: None,
        }
    }
}

impl State for Home {
    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        view::dashboard(
            &Menu::Home,
            cache,
            None,
            view::home::home_view(&self.balance, &self.events),
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
                    self.warning = None;
                    self.balance =
                        Amount::from_sat(coins.iter().map(|coin| coin.amount.to_sat()).sum());
                }
            },
            Message::HistoryEvents(res) => match res {
                Err(e) => self.warning = Some(e),
                Ok(events) => {
                    self.warning = None;
                    for event in events {
                        if !self
                            .events
                            .iter()
                            .any(|other| other.txid == event.txid && other.coins == event.coins)
                        {
                            self.events.push(event);
                        }
                    }
                }
            },
            Message::View(view::Message::Next) => {
                if let Some(last) = self.events.last() {
                    let daemon = daemon.clone();
                    let last_event_date = last.date as u32;
                    return Command::perform(
                        async move {
                            let mut limit = view::home::HISTORY_EVENT_PAGE_SIZE;
                            let mut events =
                                daemon.get_history(0 as u32, last_event_date, limit)?.events;

                            // because gethistory cursor is inclusive and use blocktime
                            // multiple events can occur in the same block.
                            // If there is more event in the same block that the
                            // HISTORY_EVENT_PAGE_SIZE they can not be retrieved by changing
                            // the cursor value (blocktime) but by increasing the limit.
                            //
                            // 1. Check if the events retrieved have all the same blocktime
                            let blocktime = if let Some(event) = events.first() {
                                event.date
                            } else {
                                return Ok(events);
                            };

                            // 2. Retrieve a larger batch of event with the same cursor but
                            //    a larger limit.
                            while !events.iter().any(|evt| evt.date != blocktime)
                                && events.len() as u64 == limit
                            {
                                // increments of the equivalent of one page more.
                                limit += view::home::HISTORY_EVENT_PAGE_SIZE;
                                events = daemon.get_history(0, last_event_date, limit)?.events;
                            }
                            Ok(events)
                        },
                        Message::HistoryEvents,
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
        let now: u32 = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .try_into()
            .unwrap();
        Command::batch(vec![
            Command::perform(
                async move {
                    daemon1
                        .get_history(0, now, view::home::HISTORY_EVENT_PAGE_SIZE)
                        .map(|res| res.events)
                        .map_err(|e| e.into())
                },
                Message::HistoryEvents,
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
            view::dashboard(&Menu::Receive, cache, self.warning.as_ref(), column())
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
            client::{Minisafed, Request},
            model::*,
        },
        utils::{
            mock::{fake_daemon_config, Daemon},
            sandbox::Sandbox,
        },
    };

    use minisafe::miniscript::bitcoin::Address;
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
        let client = Arc::new(Minisafed::new(daemon.run(), fake_daemon_config()));
        let sandbox = sandbox.load(client, &Cache::default()).await;

        let panel = sandbox.state();
        assert_eq!(panel.address, Some(addr));
    }
}
