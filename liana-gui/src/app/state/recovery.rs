use std::collections::HashSet;
use std::str::FromStr;
use std::sync::Arc;

use iced::Command;

use liana::{
    commands::CoinStatus,
    miniscript::bitcoin::{
        bip32::{DerivationPath, Fingerprint},
        secp256k1,
    },
};
use liana_ui::{component::form, widget::Element};

use crate::{
    app::{
        cache::Cache,
        error::Error,
        menu::Menu,
        message::Message,
        state::psbt,
        state::{redirect, State},
        view,
        wallet::Wallet,
    },
    daemon::{
        model::{remaining_sequence, Coin, SpendTx},
        Daemon,
    },
};

use liana::miniscript::bitcoin::{Address, Amount};

pub struct RecoveryPanel {
    wallet: Arc<Wallet>,
    recovery_paths: Vec<RecoveryPath>,
    selected_path: Option<usize>,
    warning: Option<Error>,
    feerate: form::Value<String>,
    recipient: form::Value<String>,
    generated: Option<psbt::PsbtState>,
}

impl RecoveryPanel {
    pub fn new(wallet: Arc<Wallet>, coins: &[Coin], blockheight: i32) -> Self {
        Self {
            recovery_paths: recovery_paths(&wallet, coins, blockheight),
            wallet,
            selected_path: None,
            warning: None,
            feerate: form::Value::default(),
            recipient: form::Value::default(),
            generated: None,
        }
    }
}

impl State for RecoveryPanel {
    fn subscription(&self) -> iced::Subscription<Message> {
        if let Some(psbt) = &self.generated {
            psbt.subscription()
        } else {
            iced::Subscription::none()
        }
    }

    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        if let Some(generated) = &self.generated {
            generated.view(cache)
        } else {
            view::recovery::recovery(
                cache,
                self.recovery_paths
                    .iter()
                    .enumerate()
                    .filter_map(|(i, path)| {
                        if path.number_of_coins > 0 {
                            Some(view::recovery::recovery_path_view(
                                i,
                                path.threshold,
                                &path.origins,
                                path.total_amount,
                                path.number_of_coins,
                                &self.wallet.keys_aliases,
                                self.selected_path == Some(i),
                            ))
                        } else {
                            None
                        }
                    })
                    .collect(),
                self.selected_path,
                &self.feerate,
                &self.recipient,
                self.warning.as_ref(),
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
                    self.recovery_paths = recovery_paths(&self.wallet, &coins, cache.blockheight);
                }
            },
            Message::Recovery(res) => match res {
                Ok(tx) => {
                    self.generated = Some(psbt::PsbtState::new(self.wallet.clone(), tx, false))
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
                view::Message::CreateSpend(view::CreateSpendMessage::SelectPath(index)) => {
                    if Some(index) == self.selected_path {
                        self.selected_path = None;
                    } else {
                        self.selected_path = Some(index);
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
                    let sequence = self
                        .recovery_paths
                        .get(self.selected_path.expect("A path must be selected"))
                        .map(|p| p.sequence);
                    let network = cache.network;
                    return Command::perform(
                        async move {
                            let psbt = daemon
                                .create_recovery(address, feerate_vb, sequence)
                                .await?;
                            let outpoints: Vec<_> = psbt
                                .unsigned_tx
                                .input
                                .iter()
                                .map(|txin| txin.previous_output)
                                .collect();
                            let coins = daemon
                                .list_coins(&[], &outpoints)
                                .await
                                .map(|res| res.coins)?;
                            Ok(SpendTx::new(
                                None,
                                psbt,
                                coins,
                                &desc,
                                &secp256k1::Secp256k1::verification_only(),
                                network,
                            ))
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

    fn reload(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        wallet: Arc<Wallet>,
    ) -> Command<Message> {
        let daemon = daemon.clone();
        self.wallet = wallet;
        self.selected_path = None;
        self.warning = None;
        self.feerate = form::Value::default();
        self.recipient = form::Value::default();
        self.generated = None;
        Command::perform(
            async move {
                daemon
                    .list_coins(&[CoinStatus::Unconfirmed, CoinStatus::Confirmed], &[])
                    .await
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

pub struct RecoveryPath {
    threshold: usize,
    sequence: u16,
    origins: Vec<(Fingerprint, HashSet<DerivationPath>)>,
    total_amount: Amount,
    number_of_coins: usize,
}

fn recovery_paths(wallet: &Wallet, coins: &[Coin], blockheight: i32) -> Vec<RecoveryPath> {
    wallet
        .main_descriptor
        .policy()
        .recovery_paths()
        .iter()
        .map(|(&sequence, path)| {
            let (number_of_coins, total_amount) = coins
                .iter()
                .filter(|coin| {
                    coin.spend_info.is_none()
                        && remaining_sequence(coin, blockheight as u32, sequence) <= 1
                })
                .fold(
                    (0, Amount::from_sat(0)),
                    |(number_of_coins, total_amount), coin| {
                        (number_of_coins + 1, total_amount + coin.amount)
                    },
                );

            let (threshold, origins) = path.thresh_origins();
            RecoveryPath {
                total_amount,
                number_of_coins,
                sequence,
                threshold,
                origins: origins.into_iter().collect(),
            }
        })
        .collect()
}
