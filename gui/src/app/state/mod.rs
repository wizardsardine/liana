mod settings;

use std::sync::Arc;

use bitcoin::Amount;
use iced::pure::{column, Element};
use iced::{widget::qr_code, Command, Subscription};

use super::{cache::Cache, error::Error, menu::Menu, message::Message, view};
use crate::daemon::{model::Coin, Daemon};
pub use settings::SettingsState;

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
}

impl Home {
    pub fn new(coins: &[Coin]) -> Self {
        Self {
            balance: Amount::from_sat(coins.iter().map(|coin| coin.amount.as_sat()).sum()),
        }
    }
}

impl State for Home {
    fn view<'a>(&'a self, _cache: &'a Cache) -> Element<'a, view::Message> {
        view::dashboard(&Menu::Home, None, view::home::home_view(&self.balance))
    }

    fn update(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _cache: &Cache,
        _message: Message,
    ) -> Command<Message> {
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

impl From<Home> for Box<dyn State> {
    fn from(s: Home) -> Box<dyn State> {
        Box::new(s)
    }
}

#[derive(Default)]
pub struct ReceivePanel {
    address: Option<bitcoin::Address>,
    qr_code: Option<qr_code::State>,
    warning: Option<Error>,
}

impl State for ReceivePanel {
    fn view<'a>(&'a self, _cache: &'a Cache) -> Element<'a, view::Message> {
        if let Some(address) = &self.address {
            view::dashboard(
                &Menu::Receive,
                self.warning.as_ref(),
                view::receive::receive(address, self.qr_code.as_ref().unwrap()),
            )
        } else {
            view::dashboard(&Menu::Receive, self.warning.as_ref(), column())
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

    use bitcoin::Address;
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
