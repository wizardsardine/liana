use std::collections::HashMap;
use std::sync::Arc;

use iced::{widget::qr_code, Command, Subscription};
use liana::miniscript::bitcoin::Address;
use liana_ui::widget::*;

use crate::app::{
    cache::Cache,
    error::Error,
    menu::Menu,
    message::Message,
    state::{label::LabelsEdited, State},
    view,
    wallet::Wallet,
};

use crate::daemon::{
    model::{LabelItem, Labelled},
    Daemon,
};

#[derive(Debug, Default)]
pub struct Addresses {
    list: Vec<Address>,
    labels: HashMap<String, String>,
}

impl Labelled for Addresses {
    fn labelled(&self) -> Vec<LabelItem> {
        self.list
            .iter()
            .map(|a| LabelItem::Address(a.clone()))
            .collect()
    }
    fn labels(&mut self) -> &mut HashMap<String, String> {
        &mut self.labels
    }
}

#[derive(Default)]
pub struct ReceivePanel {
    addresses: Addresses,
    labels_edited: LabelsEdited,
    qr_code: Option<qr_code::State>,
    warning: Option<Error>,
}

impl State for ReceivePanel {
    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        view::dashboard(
            &Menu::Receive,
            cache,
            self.warning.as_ref(),
            view::receive::receive(
                &self.addresses.list,
                self.qr_code.as_ref(),
                &self.addresses.labels,
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
            Message::View(view::Message::Label(_, _)) | Message::LabelsUpdated(_) => {
                match self.labels_edited.update(
                    daemon,
                    message,
                    std::iter::once(&mut self.addresses).map(|a| a as &mut dyn Labelled),
                ) {
                    Ok(cmd) => cmd,
                    Err(e) => {
                        self.warning = Some(e);
                        Command::none()
                    }
                }
            }
            Message::ReceiveAddress(res) => {
                match res {
                    Ok(address) => {
                        self.warning = None;
                        self.qr_code = Some(qr_code::State::new(address.to_qr_uri()).unwrap());
                        self.addresses.list.push(address);
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
                    .map(|res| res.address().clone())
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
                .unwrap()
                .assume_checked();
        let daemon = Daemon::new(vec![(
            Some(json!({"method": "getnewaddress", "params": Option::<Request>::None})),
            Ok(json!(GetAddressResult::new(addr.clone()))),
        )]);

        let sandbox: Sandbox<ReceivePanel> = Sandbox::new(ReceivePanel::default());
        let client = Arc::new(Lianad::new(daemon.run()));
        let sandbox = sandbox.load(client, &Cache::default()).await;

        let panel = sandbox.state();
        assert_eq!(panel.addresses.list, vec![addr]);
    }
}
