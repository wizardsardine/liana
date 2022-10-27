use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

use iced::pure::{column, Element};
use iced::Command;
use minisafe::miniscript::bitcoin::{util::psbt::Psbt, Address, Amount, Denomination, OutPoint};

use crate::{
    app::{cache::Cache, error::Error, menu::Menu, message::Message, view},
    daemon::{model::Coin, Daemon},
    ui::component::form,
};

#[derive(Default)]
pub struct TransactionDraft {
    inputs: Vec<OutPoint>,
    outputs: HashMap<Address, Amount>,
    feerate: u64,
    generated: Option<Psbt>,
}

pub trait Step {
    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message>;
    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        cache: &Cache,
        message: Message,
    ) -> Command<Message>;

    fn apply(&self, draft: &mut TransactionDraft);
}

pub struct ChooseRecipients {
    recipients: Vec<Recipient>,
}

impl std::default::Default for ChooseRecipients {
    fn default() -> Self {
        Self {
            recipients: vec![Recipient::default()],
        }
    }
}

impl Step for ChooseRecipients {
    fn update(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _cache: &Cache,
        message: Message,
    ) -> Command<Message> {
        match message {
            Message::View(view::Message::CreateSpend(msg)) => match &msg {
                view::CreateSpendMessage::AddRecipient => {
                    self.recipients.push(Recipient::default());
                }
                view::CreateSpendMessage::DeleteRecipient(i) => {
                    self.recipients.remove(*i);
                }
                view::CreateSpendMessage::RecipientEdited(i, _, _) => {
                    self.recipients.get_mut(*i).unwrap().update(msg);
                }
                _ => {}
            },
            _ => {}
        }
        Command::none()
    }

    fn apply(&self, draft: &mut TransactionDraft) {
        let mut outputs: HashMap<Address, Amount> = HashMap::new();
        for recipient in &self.recipients {
            outputs.insert(
                Address::from_str(&recipient.address.value).expect("Checked before"),
                Amount::from_sat(recipient.amount().expect("Checked before")),
            );
        }
        draft.outputs = outputs;
    }

    fn view<'a>(&'a self, _cache: &'a Cache) -> Element<'a, view::Message> {
        view::spend::step::choose_recipients_view(
            self.recipients
                .iter()
                .enumerate()
                .map(|(i, recipient)| recipient.view(i).map(view::Message::CreateSpend))
                .collect(),
            !self.recipients.iter().any(|recipient| !recipient.valid()),
        )
    }
}

#[derive(Default)]
struct Recipient {
    address: form::Value<String>,
    amount: form::Value<String>,
}

impl Recipient {
    fn amount(&self) -> Result<u64, Error> {
        if self.amount.value.is_empty() {
            return Err(Error::Unexpected("Amount should be non-zero".to_string()));
        }

        let amount = Amount::from_str_in(&self.amount.value, Denomination::Bitcoin)
            .map_err(|_| Error::Unexpected("cannot parse output amount".to_string()))?;

        if amount.to_sat() == 0 {
            return Err(Error::Unexpected("Amount should be non-zero".to_string()));
        }

        if let Ok(address) = Address::from_str(&self.address.value) {
            if amount <= address.script_pubkey().dust_value() {
                return Err(Error::Unexpected(
                    "Amount must be superior to script dust value".to_string(),
                ));
            }
        }

        Ok(amount.to_sat())
    }

    fn valid(&self) -> bool {
        !self.address.value.is_empty()
            && self.address.valid
            && !self.amount.value.is_empty()
            && self.amount.valid
    }

    fn update(&mut self, message: view::CreateSpendMessage) {
        match message {
            view::CreateSpendMessage::RecipientEdited(_, "address", address) => {
                self.address.value = address;
                if self.address.value.is_empty() {
                    // Make the error disappear if we deleted the invalid address
                    self.address.valid = true;
                } else if Address::from_str(&self.address.value).is_ok() {
                    self.address.valid = true;
                    if !self.amount.value.is_empty() {
                        self.amount.valid = self.amount().is_ok();
                    }
                } else {
                    self.address.valid = false;
                }
            }
            view::CreateSpendMessage::RecipientEdited(_, "amount", amount) => {
                self.amount.value = amount;
                if !self.amount.value.is_empty() {
                    self.amount.valid = self.amount().is_ok();
                } else {
                    // Make the error disappear if we deleted the invalid amount
                    self.amount.valid = true;
                }
            }
            _ => {}
        };
    }

    fn view(&self, i: usize) -> Element<view::CreateSpendMessage> {
        view::spend::step::recipient_view(i, &self.address, &self.amount)
    }
}

#[derive(Default)]
pub struct ChooseFeerate {
    feerate: form::Value<String>,
}

impl Step for ChooseFeerate {
    fn update(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _cache: &Cache,
        message: Message,
    ) -> Command<Message> {
        if let Message::View(view::Message::CreateSpend(view::CreateSpendMessage::FeerateEdited(
            s,
        ))) = message
        {
            if s.parse::<u64>().is_ok() {
                self.feerate.value = s;
                self.feerate.valid = true;
            } else if s.is_empty() {
                self.feerate.value = "".to_string();
                self.feerate.valid = true;
            } else {
                self.feerate.valid = false;
            }
        }

        Command::none()
    }

    fn apply(&self, draft: &mut TransactionDraft) {
        draft.feerate = self.feerate.value.parse::<u64>().expect("Checked before");
    }

    fn view<'a>(&'a self, _cache: &'a Cache) -> Element<'a, view::Message> {
        view::spend::step::choose_feerate_view(
            &self.feerate,
            self.feerate.valid && !self.feerate.value.is_empty(),
        )
    }
}
