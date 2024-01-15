use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

use iced::{Command, Subscription};
use liana::{
    descriptors::LianaDescriptor,
    miniscript::bitcoin::{
        self, address, psbt::Psbt, secp256k1, Address, Amount, Denomination, Network, OutPoint,
    },
    spend::{
        create_spend, CandidateCoin, SpendCreationError, SpendOutputAddress, SpendTxFees, TxGetter,
        MAX_FEERATE,
    },
};

use liana_ui::{component::form, widget::Element};

use crate::{
    app::{cache::Cache, error::Error, message::Message, state::psbt, view, wallet::Wallet},
    daemon::{
        model::{remaining_sequence, Coin, SpendTx},
        Daemon,
    },
};

/// See: https://github.com/wizardsardine/liana/blob/master/src/commands/mod.rs#L32
const DUST_OUTPUT_SATS: u64 = 5_000;

#[derive(Clone)]
pub struct TransactionDraft {
    network: Network,
    inputs: Vec<Coin>,
    recipients: Vec<Recipient>,
    generated: Option<Psbt>,
    batch_label: Option<String>,
    labels: HashMap<String, String>,
}

impl TransactionDraft {
    pub fn new(network: Network) -> Self {
        Self {
            network,
            inputs: Vec::new(),
            recipients: Vec::new(),
            generated: None,
            batch_label: None,
            labels: HashMap::new(),
        }
    }
}

pub trait Step {
    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message>;
    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        cache: &Cache,
        message: Message,
    ) -> Command<Message>;
    fn apply(&self, _draft: &mut TransactionDraft) {}
    fn load(&mut self, _draft: &TransactionDraft) {}
    fn subscription(&self) -> Subscription<Message> {
        Subscription::none()
    }
}

pub struct DefineSpend {
    balance_available: Amount,
    recipients: Vec<Recipient>,
    /// Will be `true` if coins for spend were manually selected by user.
    /// Otherwise, will be `false` (including for self-send).
    is_user_coin_selection: bool,
    is_valid: bool,
    is_duplicate: bool,

    network: Network,
    descriptor: LianaDescriptor,
    curve: secp256k1::Secp256k1<secp256k1::VerifyOnly>,
    timelock: u16,
    coins: Vec<(Coin, bool)>,
    coins_labels: HashMap<String, String>,
    batch_label: form::Value<String>,
    amount_left_to_select: Option<Amount>,
    feerate: form::Value<String>,
    generated: Option<Psbt>,
    warning: Option<Error>,
}

impl DefineSpend {
    pub fn new(
        network: Network,
        descriptor: LianaDescriptor,
        coins: &[Coin],
        timelock: u16,
    ) -> Self {
        let balance_available = coins
            .iter()
            .filter_map(|coin| {
                if coin.spend_info.is_none() {
                    Some(coin.amount)
                } else {
                    None
                }
            })
            .sum();
        let coins: Vec<(Coin, bool)> = coins
            .iter()
            .filter_map(|c| {
                if c.spend_info.is_none() && !c.is_immature {
                    Some((c.clone(), false))
                } else {
                    None
                }
            })
            .collect();

        Self {
            balance_available,
            network,
            descriptor,
            curve: secp256k1::Secp256k1::verification_only(),
            timelock,
            generated: None,
            coins,
            coins_labels: HashMap::new(),
            batch_label: form::Value::default(),
            recipients: vec![Recipient::default()],
            is_user_coin_selection: false, // Start with auto-selection until user edits selection.
            is_valid: false,
            is_duplicate: false,
            feerate: form::Value::default(),
            amount_left_to_select: None,
            warning: None,
        }
    }

    pub fn with_preselected_coins(mut self, preselected_coins: &[OutPoint]) -> Self {
        for (coin, selected) in &mut self.coins {
            *selected = preselected_coins.contains(&coin.outpoint);
        }
        self
    }

    pub fn with_coins_sorted(mut self, blockheight: u32) -> Self {
        let timelock = self.timelock;
        self.coins.sort_by(|(a, a_selected), (b, b_selected)| {
            if *a_selected && !b_selected || !a_selected && *b_selected {
                b_selected.cmp(a_selected)
            } else if remaining_sequence(a, blockheight, timelock)
                == remaining_sequence(b, blockheight, timelock)
            {
                // bigger amount first
                b.amount.cmp(&a.amount)
            } else {
                // smallest blockheight (remaining_sequence) first
                a.block_height.cmp(&b.block_height)
            }
        });
        self
    }

    pub fn self_send(mut self) -> Self {
        self.recipients = Vec::new();
        self
    }

    fn form_values_are_valid(&self) -> bool {
        self.feerate.valid
            && !self.feerate.value.is_empty()
            && (self.batch_label.valid || self.recipients.len() < 2)
            // Recipients will be empty for self-send.
            && self.recipients.iter().all(|r| r.valid())
    }

    fn check_valid(&mut self) {
        self.is_valid =
            self.form_values_are_valid() && self.coins.iter().any(|(_, selected)| *selected);
        self.is_duplicate = false;
        for (i, recipient) in self.recipients.iter().enumerate() {
            if !self.is_duplicate && !recipient.address.value.is_empty() {
                self.is_duplicate = self.recipients[..i]
                    .iter()
                    .any(|r| r.address.value == recipient.address.value);
            }
        }
    }
    /// redraft calculates the amount left to select and auto selects coins
    /// if the user did not select a coin manually
    fn redraft(&mut self, daemon: Arc<dyn Daemon + Sync + Send>) {
        if !self.form_values_are_valid() || self.recipients.is_empty() {
            return;
        }

        let destinations: Vec<(SpendOutputAddress, Amount)> = self
            .recipients
            .iter()
            .map(|recipient| {
                (
                    SpendOutputAddress {
                        addr: Address::from_str(&recipient.address.value)
                            .expect("Checked before")
                            .assume_checked(),
                        info: None,
                    },
                    Amount::from_sat(recipient.amount().expect("Checked before")),
                )
            })
            .collect();

        let coins: Vec<CandidateCoin> = if self.is_user_coin_selection {
            self.coins
                .iter()
                .filter_map(|(c, selected)| {
                    if *selected {
                        Some(CandidateCoin {
                            amount: c.amount,
                            outpoint: c.outpoint,
                            deriv_index: c.derivation_index,
                            is_change: c.is_change,
                            sequence: None,
                            must_select: *selected,
                        })
                    } else {
                        None
                    }
                })
                .collect()
        } else {
            // For automated coin selection, only confirmed coins are considered
            self.coins
                .iter()
                .filter_map(|(c, _)| {
                    if c.block_height.is_some() {
                        Some(CandidateCoin {
                            amount: c.amount,
                            outpoint: c.outpoint,
                            deriv_index: c.derivation_index,
                            is_change: c.is_change,
                            sequence: None,
                            must_select: false,
                        })
                    } else {
                        None
                    }
                })
                .collect()
        };

        let dummy_address = self
            .descriptor
            .change_descriptor()
            .derive(0.into(), &self.curve)
            .address(self.network);

        let feerate_vb = self.feerate.value.parse::<u64>().expect("Checked before");
        // Create a spend with empty inputs in order to use auto-selection.
        match create_spend(
            &self.descriptor,
            &self.curve,
            &mut DaemonTxGetter(&daemon),
            &destinations,
            &coins,
            SpendTxFees::Regular(feerate_vb),
            SpendOutputAddress {
                addr: dummy_address,
                info: None,
            },
        ) {
            Ok(spend) => {
                self.warning = None;
                if !self.is_user_coin_selection {
                    let selected_coins: Vec<OutPoint> = spend
                        .psbt
                        .unsigned_tx
                        .input
                        .iter()
                        .map(|c| c.previous_output)
                        .collect();
                    // Mark coins as selected.
                    for (coin, selected) in &mut self.coins {
                        *selected = selected_coins.contains(&coin.outpoint);
                    }
                }
                // As coin selection was successful, we can assume there is nothing left to select.
                self.amount_left_to_select = Some(Amount::from_sat(0));
            }
            // For coin selection error (insufficient funds), do not make any changes to
            // selected coins on screen and just show user how much is left to select.
            // User can then either:
            // - modify recipient amounts and/or feerate and let coin selection run again, or
            // - select coins manually.
            Err(SpendCreationError::CoinSelection(amount)) => {
                self.amount_left_to_select = Some(Amount::from_sat(amount.missing));
            }
            Err(e) => {
                self.warning = Some(e.into());
            }
        }
    }
}

pub struct DaemonTxGetter<'a>(&'a Arc<dyn Daemon + Sync + Send>);
impl<'a> TxGetter for DaemonTxGetter<'a> {
    fn get_tx(&mut self, txid: &bitcoin::Txid) -> Option<bitcoin::Transaction> {
        self.0
            .list_txs(&[*txid])
            .ok()
            .and_then(|mut txs| txs.transactions.pop().map(|tx| tx.tx))
    }
}

impl Step for DefineSpend {
    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        cache: &Cache,
        message: Message,
    ) -> Command<Message> {
        match message {
            Message::View(view::Message::CreateSpend(msg)) => {
                match msg {
                    view::CreateSpendMessage::BatchLabelEdited(label) => {
                        self.batch_label.valid = label.len() <= 100;
                        self.batch_label.value = label;
                    }
                    view::CreateSpendMessage::AddRecipient => {
                        self.recipients.push(Recipient::default());
                    }
                    view::CreateSpendMessage::DeleteRecipient(i) => {
                        self.recipients.remove(i);
                        if self.recipients.len() < 2 {
                            self.batch_label.valid = true;
                            self.batch_label.value = "".to_string();
                        }
                    }
                    view::CreateSpendMessage::RecipientEdited(i, _, _) => {
                        self.recipients
                            .get_mut(i)
                            .unwrap()
                            .update(cache.network, msg);
                    }

                    view::CreateSpendMessage::FeerateEdited(s) => {
                        if let Ok(value) = s.parse::<u64>() {
                            self.feerate.value = s;
                            self.feerate.valid = value != 0 && value <= MAX_FEERATE;
                        } else if s.is_empty() {
                            self.feerate.value = "".to_string();
                            self.feerate.valid = true;
                        } else {
                            self.feerate.valid = false;
                        }
                        self.warning = None;
                    }
                    view::CreateSpendMessage::Generate => {
                        let inputs: Vec<OutPoint> = self
                            .coins
                            .iter()
                            .filter_map(
                                |(coin, selected)| {
                                    if *selected {
                                        Some(coin.outpoint)
                                    } else {
                                        None
                                    }
                                },
                            )
                            .collect();
                        let mut outputs: HashMap<Address<address::NetworkUnchecked>, u64> =
                            HashMap::new();
                        for recipient in &self.recipients {
                            outputs.insert(
                                Address::from_str(&recipient.address.value)
                                    .expect("Checked before"),
                                recipient.amount().expect("Checked before"),
                            );
                        }
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
                    view::CreateSpendMessage::SelectCoin(i) => {
                        if let Some(coin) = self.coins.get_mut(i) {
                            coin.1 = !coin.1;
                            // Once user edits selection, auto-selection can no longer be used.
                            self.is_user_coin_selection = true;
                        }
                    }
                    _ => {}
                }

                // Attempt to select coins automatically if:
                // - all form values have been added and validated
                // - not a self-send
                // - user has not yet selected coins manually
                self.redraft(daemon);
                self.check_valid();
            }
            Message::Psbt(res) => match res {
                Ok(psbt) => {
                    self.generated = Some(psbt);
                    return Command::perform(async {}, |_| Message::View(view::Message::Next));
                }
                Err(e) => self.warning = Some(e),
            },
            Message::Labels(res) => match res {
                Ok(labels) => {
                    self.coins_labels = labels;
                }
                Err(e) => self.warning = Some(e),
            },
            _ => {}
        };
        Command::none()
    }

    fn apply(&self, draft: &mut TransactionDraft) {
        draft.inputs = self
            .coins
            .iter()
            .filter_map(|(coin, selected)| if *selected { Some(coin) } else { None })
            .cloned()
            .collect();
        if let Some(psbt) = &self.generated {
            draft.labels = self.coins_labels.clone();
            for (i, output) in psbt.unsigned_tx.output.iter().enumerate() {
                if let Some(label) = self
                    .recipients
                    .iter()
                    .find(|recipient| {
                        !recipient.label.value.is_empty()
                            && Address::from_str(&recipient.address.value)
                                .unwrap()
                                .payload
                                .matches_script_pubkey(&output.script_pubkey)
                            && output.value == recipient.amount().unwrap()
                    })
                    .map(|recipient| recipient.label.value.to_string())
                {
                    draft.labels.insert(
                        OutPoint {
                            txid: psbt.unsigned_tx.txid(),
                            vout: i as u32,
                        }
                        .to_string(),
                        label,
                    );
                }
            }
        }
        draft.recipients = self.recipients.clone();
        if self.recipients.len() > 1 {
            draft.batch_label = Some(self.batch_label.value.clone());
        }
        draft.generated = self.generated.clone();
    }

    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        view::spend::create_spend_tx(
            cache,
            &self.balance_available,
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
            self.is_valid,
            self.is_duplicate,
            self.timelock,
            &self.coins,
            &self.coins_labels,
            &self.batch_label,
            self.amount_left_to_select.as_ref(),
            &self.feerate,
            self.warning.as_ref(),
        )
    }
}

#[derive(Default, Clone)]
struct Recipient {
    label: form::Value<String>,
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

        if amount.to_sat() < DUST_OUTPUT_SATS {
            return Err(Error::Unexpected("Amount should be non-zero".to_string()));
        }

        if let Ok(address) = Address::from_str(&self.address.value) {
            if amount <= address.payload.script_pubkey().dust_value() {
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
            && self.label.valid
    }

    fn update(&mut self, network: Network, message: view::CreateSpendMessage) {
        match message {
            view::CreateSpendMessage::RecipientEdited(_, "address", address) => {
                self.address.value = address;
                if let Ok(address) = Address::from_str(&self.address.value) {
                    self.address.valid = address.is_valid_for_network(network);
                    if !self.amount.value.is_empty() {
                        self.amount.valid = self.amount().is_ok();
                    }
                } else if self.address.value.is_empty() {
                    // Make the error disappear if we deleted the invalid address
                    self.address.valid = true;
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
            view::CreateSpendMessage::RecipientEdited(_, "label", label) => {
                self.label.valid = label.len() <= 100;
                self.label.value = label;
            }
            _ => {}
        };
    }

    fn view(&self, i: usize) -> Element<view::CreateSpendMessage> {
        view::spend::recipient_view(i, &self.address, &self.amount, &self.label)
    }
}

pub struct SaveSpend {
    wallet: Arc<Wallet>,
    spend: Option<psbt::PsbtState>,
}

impl SaveSpend {
    pub fn new(wallet: Arc<Wallet>) -> Self {
        Self {
            wallet,
            spend: None,
        }
    }
}

impl Step for SaveSpend {
    fn load(&mut self, draft: &TransactionDraft) {
        let psbt = draft.generated.clone().unwrap();
        let mut tx = SpendTx::new(
            None,
            psbt,
            draft.inputs.clone(),
            &self.wallet.main_descriptor,
            draft.network,
        );
        tx.labels = draft.labels.clone();

        if tx.is_batch() {
            if let Some(label) = &draft.batch_label {
                tx.labels
                    .insert(tx.psbt.unsigned_tx.txid().to_string(), label.clone());
            }
        } else if let Some(recipient) = draft.recipients.first() {
            if !recipient.label.value.is_empty() {
                let label = recipient.label.value.clone();
                tx.labels
                    .insert(tx.psbt.unsigned_tx.txid().to_string(), label);
            }
        }

        self.spend = Some(psbt::PsbtState::new(self.wallet.clone(), tx, false));
    }

    fn subscription(&self) -> Subscription<Message> {
        if let Some(spend) = &self.spend {
            spend.subscription()
        } else {
            Subscription::none()
        }
    }

    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        cache: &Cache,
        message: Message,
    ) -> Command<Message> {
        if let Some(spend) = &mut self.spend {
            spend.update(daemon, cache, message)
        } else {
            Command::none()
        }
    }

    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        let spend = self.spend.as_ref().unwrap();
        let content = view::spend::spend_view(
            cache,
            &spend.tx,
            spend.saved,
            &spend.desc_policy,
            &spend.wallet.keys_aliases,
            spend.labels_edited.cache(),
            cache.network,
            spend.warning.as_ref(),
        );
        if let Some(action) = &spend.action {
            action.as_ref().view(content)
        } else {
            content
        }
    }
}
