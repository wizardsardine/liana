use std::str::FromStr;
use std::sync::Arc;

use iced::{Command, Element};

use crate::{
    app::{
        cache::Cache,
        error::Error,
        menu::Menu,
        message::Message,
        state::spend::detail,
        state::{redirect, State},
        view,
        wallet::Wallet,
    },
    daemon::{
        model::{remaining_sequence, Coin, SpendTx},
        Daemon,
    },
    ui::component::form,
};

use liana::miniscript::bitcoin::{Address, Amount};

pub struct RecoveryPanel {
    wallet: Arc<Wallet>,
    locked_coins: (usize, Amount),
    recoverable_coins: (usize, Amount),
    warning: Option<Error>,
    feerate: form::Value<String>,
    recipient: form::Value<String>,
    generated: Option<detail::SpendTxState>,
    /// timelock value to pass for the heir to consume a coin.
    timelock: u32,
}

impl RecoveryPanel {
    pub fn new(wallet: Arc<Wallet>, coins: &[Coin], timelock: u32, blockheight: u32) -> Self {
        let mut locked_coins = (0, Amount::from_sat(0));
        let mut recoverable_coins = (0, Amount::from_sat(0));
        for coin in coins {
            if coin.spend_info.is_none() {
                // recoverable coins are coins that can be recoverable next block.
                if remaining_sequence(coin, blockheight, timelock) > 1 {
                    locked_coins.0 += 1;
                    locked_coins.1 += coin.amount;
                } else {
                    recoverable_coins.0 += 1;
                    recoverable_coins.1 += coin.amount;
                }
            }
        }
        Self {
            wallet,
            locked_coins,
            recoverable_coins,
            warning: None,
            feerate: form::Value::default(),
            recipient: form::Value::default(),
            generated: None,
            timelock,
        }
    }
}

impl State for RecoveryPanel {
    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        if let Some(generated) = &self.generated {
            generated.view(cache)
        } else {
            view::modal(
                false,
                self.warning.as_ref(),
                view::recovery::recovery(
                    &self.locked_coins,
                    &self.recoverable_coins,
                    &self.feerate,
                    &self.recipient,
                ),
                None::<Element<view::Message>>,
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
                    self.locked_coins = (0, Amount::from_sat(0));
                    self.recoverable_coins = (0, Amount::from_sat(0));
                    for coin in coins {
                        if coin.spend_info.is_none() {
                            // recoverable coins are coins that can be recoverable next block.
                            if remaining_sequence(&coin, cache.blockheight as u32, self.timelock)
                                > 1
                            {
                                self.locked_coins.0 += 1;
                                self.locked_coins.1 += coin.amount;
                            } else {
                                self.recoverable_coins.0 += 1;
                                self.recoverable_coins.1 += coin.amount;
                            }
                        }
                    }
                }
            },
            Message::Recovery(res) => match res {
                Ok(tx) => {
                    self.generated = Some(detail::SpendTxState::new(self.wallet.clone(), tx, false))
                }
                Err(e) => self.warning = Some(e),
            },
            Message::View(msg) => match msg {
                view::Message::Close => return redirect(Menu::Settings),
                view::Message::Previous => self.generated = None,
                view::Message::CreateSpend(view::CreateSpendMessage::RecipientEdited(
                    _,
                    "address",
                    address,
                )) => {
                    self.recipient.value = address;
                    if let Ok(address) = Address::from_str(&self.recipient.value) {
                        self.recipient.valid = address.is_valid_for_network(cache.network);
                    } else {
                        self.recipient.valid = false;
                    }
                }
                view::Message::CreateSpend(view::CreateSpendMessage::FeerateEdited(feerate)) => {
                    self.feerate.value = feerate;
                    self.feerate.valid =
                        self.feerate.value.parse::<u64>().is_ok() && self.feerate.value != "0";
                }
                view::Message::Next => {
                    let address = Address::from_str(&self.recipient.value).expect("Checked before");
                    let feerate_vb = self.feerate.value.parse::<u64>().expect("Checked before");
                    self.warning = None;
                    let desc = self.wallet.main_descriptor.clone();
                    return Command::perform(
                        async move {
                            let psbt = daemon.create_recovery(address, feerate_vb)?;
                            let coins = daemon.list_coins().map(|res| res.coins)?;
                            let coins = coins
                                .iter()
                                .filter(|coin| {
                                    psbt.unsigned_tx
                                        .input
                                        .iter()
                                        .any(|input| input.previous_output == coin.outpoint)
                                })
                                .copied()
                                .collect();
                            let sigs = desc.partial_spend_info(&psbt).unwrap();
                            Ok(SpendTx::new(psbt, coins, sigs))
                        },
                        Message::Recovery,
                    );
                }
                _ => {
                    if let Some(generated) = &mut self.generated {
                        return generated.update(daemon, cache, Message::View(msg));
                    }
                }
            },
            _ => {
                if let Some(generated) = &mut self.generated {
                    return generated.update(daemon, cache, message);
                }
            }
        };
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

impl From<RecoveryPanel> for Box<dyn State> {
    fn from(s: RecoveryPanel) -> Box<dyn State> {
        Box::new(s)
    }
}
