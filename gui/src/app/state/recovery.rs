use std::str::FromStr;
use std::sync::Arc;

use iced::{Command, Element};

use crate::{
    app::{
        cache::Cache,
        config::Config,
        error::Error,
        menu::Menu,
        message::Message,
        state::{redirect, State},
        view,
        wallet::Wallet,
    },
    daemon::{
        model::{remaining_sequence, Coin},
        Daemon,
    },
    hw::{list_hardware_wallets, HardwareWallet},
    ui::component::form,
};

use liana::miniscript::bitcoin::{util::psbt::Psbt, Address, Amount, Network};

pub struct RecoveryPanel {
    wallet: Wallet,
    config: Config,
    locked_coins: (usize, Amount),
    recoverable_coins: (usize, Amount),
    warning: Option<Error>,
    feerate: form::Value<String>,
    recipient: form::Value<String>,
    generated: Option<Psbt>,
    hws: Vec<HardwareWallet>,
    selected_hw: Option<usize>,
    signed: bool,
    /// timelock value to pass for the heir to consume a coin.
    timelock: u32,
}

impl RecoveryPanel {
    pub fn new(
        wallet: Wallet,
        config: Config,
        coins: &[Coin],
        timelock: u32,
        blockheight: u32,
    ) -> Self {
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
            config,
            locked_coins,
            recoverable_coins,
            warning: None,
            feerate: form::Value::default(),
            recipient: form::Value::default(),
            generated: None,
            timelock,
            hws: Vec::new(),
            selected_hw: None,
            signed: false,
        }
    }
}

impl State for RecoveryPanel {
    fn view<'a>(&'a self, _cache: &'a Cache) -> Element<'a, view::Message> {
        view::modal(
            false,
            self.warning.as_ref(),
            view::recovery::recovery(
                &self.locked_coins,
                &self.recoverable_coins,
                &self.feerate,
                &self.recipient,
                self.generated.as_ref(),
                &self.hws,
                self.selected_hw,
                self.signed,
            ),
            None::<Element<view::Message>>,
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
            // We add the new hws without dropping the reference of the previous ones.
            Message::ConnectedHardwareWallets(hws) => {
                for h in hws {
                    if !self.hws.iter().any(|hw| hw.fingerprint == h.fingerprint) {
                        self.hws.push(h);
                    }
                }
            }
            Message::Psbt(res) => match res {
                Ok(psbt) => self.generated = Some(psbt),
                Err(e) => self.warning = Some(e),
            },
            Message::Updated(res) => match res {
                Err(e) => self.warning = Some(e),
                Ok(()) => {
                    self.warning = None;
                    self.signed = true;
                }
            },
            Message::View(msg) => match msg {
                view::Message::Reload => return self.load(daemon),
                view::Message::Close => return redirect(Menu::Settings),
                view::Message::Previous => self.generated = None,
                view::Message::CreateSpend(view::CreateSpendMessage::RecipientEdited(
                    _,
                    "address",
                    address,
                )) => {
                    self.recipient.value = address;
                    if let Ok(address) = Address::from_str(&self.recipient.value) {
                        if cache.network == Network::Bitcoin {
                            self.recipient.valid = address.network == Network::Bitcoin;
                        } else {
                            self.recipient.valid = address.network == Network::Testnet;
                        }
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
                    return Command::perform(
                        async move {
                            daemon
                                .create_recovery(address, feerate_vb)
                                .map_err(|e| e.into())
                        },
                        Message::Psbt,
                    );
                }
                view::Message::Spend(view::SpendTxMessage::SelectHardwareWallet(i)) => {
                    if let Some(hw) = self.hws.get(i) {
                        let device = hw.device.clone();
                        self.selected_hw = Some(i);
                        let psbt = self.generated.clone().unwrap();
                        return Command::perform(
                            send_funds(daemon, device, psbt),
                            Message::Updated,
                        );
                    }
                }
                _ => {}
            },
            _ => {}
        };
        Command::none()
    }

    fn load(&self, daemon: Arc<dyn Daemon + Sync + Send>) -> Command<Message> {
        let config = self.config.clone();
        let desc = self.wallet.main_descriptor.to_string();
        let daemon = daemon.clone();
        Command::batch(vec![
            Command::perform(
                async move {
                    daemon
                        .list_coins()
                        .map(|res| res.coins)
                        .map_err(|e| e.into())
                },
                Message::Coins,
            ),
            Command::perform(
                list_hws(config, self.wallet.name.clone(), desc),
                Message::ConnectedHardwareWallets,
            ),
        ])
    }
}

async fn list_hws(config: Config, wallet_name: String, descriptor: String) -> Vec<HardwareWallet> {
    list_hardware_wallets(&config.hardware_wallets, Some((&wallet_name, &descriptor))).await
}

async fn send_funds(
    daemon: Arc<dyn Daemon + Sync + Send>,
    hw: std::sync::Arc<dyn async_hwi::HWI + Send + Sync>,
    mut psbt: Psbt,
) -> Result<(), Error> {
    hw.sign_tx(&mut psbt).await.map_err(Error::from)?;
    daemon.update_spend_tx(&psbt)?;
    daemon.broadcast_spend_tx(&psbt.unsigned_tx.txid())?;
    Ok(())
}

impl From<RecoveryPanel> for Box<dyn State> {
    fn from(s: RecoveryPanel) -> Box<dyn State> {
        Box::new(s)
    }
}
