use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

use iced::{Command, Element};
use liana::miniscript::bitcoin::{
    util::psbt::Psbt, Address, Amount, Denomination, OutPoint, Script,
};

use crate::{
    app::{
        cache::Cache, config::Config, error::Error, message::Message, state::spend::detail, view,
    },
    daemon::{
        model::{remaining_sequence, Coin, SpendTx},
        Daemon,
    },
    ui::component::form,
};

#[derive(Default, Clone)]
pub struct TransactionDraft {
    inputs: Vec<Coin>,
    outputs: HashMap<Address, u64>,
    feerate: u64,
    generated: Option<Psbt>,
}

pub trait Step {
    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message>;
    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        cache: &Cache,
        draft: &TransactionDraft,
        message: Message,
    ) -> Command<Message>;
    fn apply(&self, _draft: &mut TransactionDraft) {}
    fn load(&mut self, _draft: &TransactionDraft) {}
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
        _draft: &TransactionDraft,
        message: Message,
    ) -> Command<Message> {
        if let Message::View(view::Message::CreateSpend(msg)) = message {
            match &msg {
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
            }
        }
        Command::none()
    }

    fn apply(&self, draft: &mut TransactionDraft) {
        let mut outputs: HashMap<Address, u64> = HashMap::new();
        for recipient in &self.recipients {
            outputs.insert(
                Address::from_str(&recipient.address.value).expect("Checked before"),
                recipient.amount().expect("Checked before"),
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
            Amount::from_sat(
                self.recipients
                    .iter()
                    .map(|r| r.amount().unwrap_or(0_u64))
                    .sum(),
            ),
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
    generated: Option<Psbt>,
    warning: Option<Error>,
}

impl Step for ChooseFeerate {
    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        _cache: &Cache,
        draft: &TransactionDraft,
        message: Message,
    ) -> Command<Message> {
        match message {
            Message::View(view::Message::CreateSpend(view::CreateSpendMessage::FeerateEdited(
                s,
            ))) => {
                if s.parse::<u64>().is_ok() {
                    self.feerate.value = s;
                    self.feerate.valid = true;
                } else if s.is_empty() {
                    self.feerate.value = "".to_string();
                    self.feerate.valid = true;
                } else {
                    self.feerate.valid = false;
                }
                self.warning = None;
            }
            Message::View(view::Message::CreateSpend(view::CreateSpendMessage::Generate)) => {
                let inputs: Vec<OutPoint> = draft.inputs.iter().map(|c| c.outpoint).collect();
                let outputs = draft.outputs.clone();
                let feerate_vb = self.feerate.value.parse::<u64>().unwrap_or(0);
                self.warning = None;
                return Command::perform(
                    async move {
                        daemon
                            .create_spend_tx(&inputs, &outputs, feerate_vb)
                            .map(|res| res.psbt)
                            .map_err(|e| e.into())
                    },
                    Message::Psbt,
                );
            }
            Message::Psbt(res) => match res {
                Ok(psbt) => {
                    self.generated = Some(psbt);
                    return Command::perform(async {}, |_| Message::View(view::Message::Next));
                }
                Err(e) => self.warning = Some(e),
            },
            _ => {}
        }

        Command::none()
    }

    fn apply(&self, draft: &mut TransactionDraft) {
        draft.feerate = self.feerate.value.parse::<u64>().expect("Checked before");
        draft.generated = self.generated.clone();
    }

    fn view<'a>(&'a self, _cache: &'a Cache) -> Element<'a, view::Message> {
        view::spend::step::choose_feerate_view(
            &self.feerate,
            self.feerate.valid && !self.feerate.value.is_empty(),
            self.warning.as_ref(),
        )
    }
}

#[derive(Default)]
pub struct ChooseCoins {
    timelock: u32,
    coins: Vec<(Coin, bool)>,
    /// draft output amount must be superior to total input amount.
    is_valid: bool,
    total_needed: Option<Amount>,
}

impl ChooseCoins {
    pub fn new(coins: Vec<Coin>, timelock: u32, blockheight: u32) -> Self {
        let mut coins: Vec<(Coin, bool)> = coins
            .into_iter()
            .filter_map(|c| {
                if c.spend_info.is_none() {
                    Some((c, false))
                } else {
                    None
                }
            })
            .collect();
        coins.sort_by(|(a, _), (b, _)| {
            if remaining_sequence(a, blockheight, timelock)
                == remaining_sequence(b, blockheight, timelock)
            {
                // bigger amount first
                b.amount.cmp(&a.amount)
            } else {
                // smallest blockheight (remaining_sequence) first
                a.block_height.cmp(&b.block_height)
            }
        });
        Self {
            timelock,
            coins,
            is_valid: false,
            total_needed: None,
        }
    }
}

impl Step for ChooseCoins {
    fn load(&mut self, draft: &TransactionDraft) {
        self.total_needed = Some(Amount::from_sat(
            draft.outputs.values().fold(0, |acc, a| acc + *a),
        ));
    }

    fn update(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _cache: &Cache,
        _draft: &TransactionDraft,
        message: Message,
    ) -> Command<Message> {
        if let Message::View(view::Message::CreateSpend(view::CreateSpendMessage::SelectCoin(i))) =
            message
        {
            if let Some(coin) = self.coins.get_mut(i) {
                coin.1 = !coin.1;
            }

            self.is_valid = self
                .coins
                .iter()
                .filter_map(|(coin, selected)| {
                    if *selected {
                        Some(coin.amount.to_sat())
                    } else {
                        None
                    }
                })
                .sum::<u64>()
                > self.total_needed.map(|a| a.to_sat()).unwrap_or(0);
        }

        Command::none()
    }

    fn apply(&self, draft: &mut TransactionDraft) {
        draft.inputs = self
            .coins
            .iter()
            .filter_map(|(coin, selected)| if *selected { Some(*coin) } else { None })
            .collect();
    }

    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        view::spend::step::choose_coins_view(
            cache,
            self.timelock,
            &self.coins,
            self.total_needed.as_ref(),
            self.is_valid,
        )
    }
}

pub struct SaveSpend {
    config: Config,
    spend: Option<detail::SpendTxState>,
}

impl SaveSpend {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            spend: None,
        }
    }
}

impl Step for SaveSpend {
    fn load(&mut self, draft: &TransactionDraft) {
        let outputs_script_pubkeys: Vec<Script> = draft
            .outputs
            .keys()
            .map(|addr| addr.script_pubkey())
            .collect();
        let index = if let Some(psbt) = &draft.generated {
            psbt.unsigned_tx
                .output
                .iter()
                .position(|output| !outputs_script_pubkeys.contains(&output.script_pubkey))
        } else {
            None
        };
        self.spend = Some(detail::SpendTxState::new(
            self.config.clone(),
            SpendTx::new(
                draft.generated.clone().unwrap(),
                index,
                draft.inputs.clone(),
            ),
            false,
        ));
    }

    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        cache: &Cache,
        _draft: &TransactionDraft,
        message: Message,
    ) -> Command<Message> {
        if let Some(spend) = &mut self.spend {
            spend.update(daemon, cache, message)
        } else {
            Command::none()
        }
    }

    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        self.spend.as_ref().unwrap().view(cache)
    }
}
