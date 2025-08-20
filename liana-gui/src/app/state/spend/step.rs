use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    convert::{TryFrom, TryInto},
    iter::FromIterator,
    str::FromStr,
    sync::Arc,
};

use iced::{Subscription, Task};
use liana::{
    descriptors::LianaDescriptor,
    miniscript::bitcoin::{
        address,
        bip32::{DerivationPath, Fingerprint},
        psbt::Psbt,
        secp256k1, Address, Amount, Denomination, Network, OutPoint,
    },
    spend::{SpendCreationError, MAX_FEERATE},
};
use lianad::{commands::ListCoinsEntry, payjoin::types::PayjoinStatus};

use liana_ui::{component::form, widget::Element};
use payjoin::Uri;

use crate::{
    app::{cache::Cache, error::Error, message::Message, state::psbt, view, wallet::Wallet},
    daemon::{
        model::{coin_is_owned, remaining_sequence, Coin, CreateSpendResult, SpendTx},
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
    generated: Option<(Psbt, Vec<String>)>,
    batch_label: Option<String>,
    labels: HashMap<String, String>,
    /// The timelock of the recovery path to use for spending.
    ///
    /// If the primary path will be used, this value will remain as `None`.
    /// Otherwise, its value should always be set to a recovery path,
    /// which may change from one to another.
    recovery_timelock: Option<u16>,
}

impl TransactionDraft {
    pub fn new(network: Network, recovery_timelock: Option<u16>) -> Self {
        Self {
            network,
            inputs: Vec::new(),
            recipients: Vec::new(),
            generated: None,
            batch_label: None,
            labels: HashMap::new(),
            recovery_timelock,
        }
    }

    pub fn is_recovery(&self) -> bool {
        self.recovery_timelock.is_some()
    }
}

pub trait Step {
    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message>;
    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message>;
    fn apply(&self, _draft: &mut TransactionDraft) {}
    fn interrupt(&mut self) {}
    fn load(&mut self, _coins: &[Coin], _tip_height: i32, _draft: &TransactionDraft) {}
    fn reload_wallet(&mut self, _wallet: Arc<Wallet>) {}
    fn subscription(&self) -> Subscription<Message> {
        Subscription::none()
    }
}

/// Filter the coins that should be available for selection.
///
/// `selected` will be `None` if the coins are being filtered for the first time.
/// In that case, all suitable coins will be selected for a recovery spend,
/// while for a primary path spend, no coins will be selected.
fn filter_coins(
    coins: &[Coin],
    recovery_timelock: Option<u16>,
    tip_height: i32,
    selected: Option<HashSet<OutPoint>>,
) -> Vec<(Coin, bool)> {
    coins
        .iter()
        .filter_map(|c| {
            if c.spend_info.is_none() && !c.is_immature {
                if let Some(recovery_timelock) = recovery_timelock {
                    c.block_height
                        .filter(|bh| {
                            tip_height + 1 >= bh + <u16 as Into<i32>>::into(recovery_timelock)
                        })
                        .map(|_| {
                            (
                                c.clone(),
                                selected
                                    .as_ref()
                                    .map(|sel| sel.contains(&c.outpoint))
                                    .unwrap_or(true),
                            )
                        })
                } else {
                    Some((
                        c.clone(),
                        selected
                            .as_ref()
                            .map(|sel| sel.contains(&c.outpoint))
                            .unwrap_or(false),
                    ))
                }
            } else {
                None
            }
        })
        .collect()
}

pub struct DefineSpend {
    recipients: Vec<Recipient>,
    /// If set, this is the index of a recipient that should
    /// receive the max amount.
    send_max_to_recipient: Option<usize>,
    /// Will be `true` if coins for spend were manually selected by user.
    /// Otherwise, will be `false` (including for self-send & recovery).
    is_user_coin_selection: bool,
    is_valid: bool,
    is_duplicate: bool,

    network: Network,
    descriptor: LianaDescriptor,
    curve: secp256k1::Secp256k1<secp256k1::VerifyOnly>,
    /// Leave as `None` for a primary path spend. Otherwise, this is the timelock
    /// corresponding to the recovery path to use for the spend.
    ///
    /// For a recovery path spend, this value can change from one timelock to another, but
    /// it must never be set to `None`.
    recovery_timelock: Option<u16>,
    tip_height: u32,
    coins: Vec<(Coin, bool)>,
    coins_labels: HashMap<String, String>,
    batch_label: form::Value<String>,
    amount_left_to_select: Option<Amount>,
    feerate: form::Value<String>,
    fee_amount: Option<Amount>,
    generated: Option<(Psbt, Vec<String>)>,
    warning: Option<Error>,
    /// Whether this is the first step of the spend creation.
    /// Required in order to know whether the user can navigate to a previous step.
    is_first_step: bool,
}

impl DefineSpend {
    pub fn new(
        network: Network,
        descriptor: LianaDescriptor,
        coins: &[Coin],
        tip_height: u32,
        recovery_timelock: Option<u16>,
        is_first_step: bool,
    ) -> Self {
        let coins = filter_coins(
            coins,
            recovery_timelock,
            tip_height.try_into().expect("i32 by consensus"),
            None,
        );

        Self {
            network,
            descriptor,
            curve: secp256k1::Secp256k1::verification_only(),
            recovery_timelock,
            tip_height,
            generated: None,
            coins,
            coins_labels: HashMap::new(),
            batch_label: form::Value::default(),
            recipients: vec![Recipient::new(recovery_timelock.is_some())],
            // For recovery, send max to the (single) recipient.
            send_max_to_recipient: recovery_timelock.map(|_| 0),
            is_user_coin_selection: false,
            is_valid: false,
            is_duplicate: false,
            feerate: form::Value::default(),
            fee_amount: None,
            amount_left_to_select: None,
            warning: None,
            is_first_step,
        }
    }

    pub fn with_preselected_coins(mut self, preselected_coins: &[OutPoint]) -> Self {
        for (coin, selected) in &mut self.coins {
            *selected = preselected_coins.contains(&coin.outpoint);
        }
        self
    }

    pub fn with_coins_sorted(mut self, blockheight: u32) -> Self {
        self.sort_coins(blockheight);
        self
    }

    fn sort_coins(&mut self, blockheight: u32) {
        let timelock = self.timelock();
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
    }

    pub fn self_send(mut self) -> Self {
        self.recipients = Vec::new();
        self
    }

    /// This is used for calculating a coin's remaining sequence.
    ///
    /// Use the first timelock if this is a primary path spend and otherwise the same
    /// timelock as used for the recovery.
    pub fn timelock(&self) -> u16 {
        self.recovery_timelock
            .unwrap_or_else(|| self.descriptor.first_timelock_value())
    }

    // If `is_redraft`, the validation of recipients will take into account
    // whether any should receive the max amount. Otherwise, all recipients
    // will be fully validated.
    fn form_values_are_valid(&self, is_redraft: bool) -> bool {
        self.feerate.valid
            && !self.feerate.value.is_empty()
            && (self.batch_label.valid || self.recipients.len() < 2)
            // Recipients will be empty for self-send.
            && self.recipients.iter().enumerate().all(|(i, r)|
            r.valid() || (is_redraft && self.send_max_to_recipient == Some(i) && r.address_valid()))
    }

    fn exists_duplicate(&self) -> bool {
        for (i, recipient) in self.recipients.iter().enumerate() {
            if !recipient.address.value.is_empty()
                && self.recipients[..i]
                    .iter()
                    .any(|r| r.address.value == recipient.address.value)
            {
                return true;
            }
        }
        false
    }

    fn check_valid(&mut self) {
        self.is_valid =
            self.form_values_are_valid(false) && self.coins.iter().any(|(_, selected)| *selected);
        self.is_duplicate = self.exists_duplicate();
    }
    /// redraft calculates the amount left to select and auto selects coins
    /// if the user did not select a coin manually
    fn redraft(&mut self, daemon: Arc<dyn Daemon + Sync + Send>) {
        if !self.form_values_are_valid(true) || self.exists_duplicate() {
            // The current form details are not valid to draft a spend, so remove any previously
            // calculated amount as it will no longer be valid and could be misleading, e.g. if
            // the user removes the amount from one of the recipients.
            // We can leave any coins selected as they will either be automatically updated
            // as soon as the form is valid or the user has selected these specific coins and
            // so we should not touch them.
            self.amount_left_to_select = None;
            // Remove any max amount from a recipient as it could be misleading.
            if let Some(i) = self.send_max_to_recipient {
                self.recipients
                    .get_mut(i)
                    .expect("max has been requested for this recipient so it must exist")
                    .update(
                        self.network,
                        view::CreateSpendMessage::RecipientEdited(i, "amount", "".to_string()),
                    );
            }
            self.fee_amount = None;
            return;
        }
        let is_self_transfer = self.recipients.is_empty();
        // Define the destinations for a primary path spend from all non-max recipients.
        // TODO: Set this variable only in the non-recovery case. For now, we use it later below
        // for setting `amount_left_to_select`, which is only required in the non-recovery case.
        let destinations: HashMap<Address<address::NetworkUnchecked>, u64> = self
            .recipients
            .iter()
            .enumerate()
            .filter_map(|(i, recipient)| {
                // A recipient that receives the max should be treated as change for coin selection.
                // Note that we only give a change output if its value is above the dust
                // threshold, but a user can only send payments above the same dust threshold,
                // so using change output to determine the max amount for a recipient will
                // not prevent a value that could otherwise be entered manually by the user.
                if self.send_max_to_recipient == Some(i) {
                    None
                } else {
                    Some((
                        Address::from_str(&recipient.address.value).expect("Checked before"),
                        recipient.amount().expect("Checked before"),
                    ))
                }
            })
            .collect();

        let recipient_with_max = if let Some(i) = self.send_max_to_recipient {
            Some((
                i,
                self.recipients
                    .get_mut(i)
                    .expect("max has been requested for this recipient so it must exist"),
            ))
        } else {
            None
        };
        let outpoints = if self.is_user_coin_selection
            || is_self_transfer
            || self.recovery_timelock.is_some()
        {
            // If user has edited selection, or otherwise for self-transfers and recovery spends, we pass the outpoints list.
            // (We could also in principle pass an empty outpoints list for a recovery spend if all should
            // be selected, i.e. not user edited, but this way we don't need to worry about checking selected
            // coins match those used in the recovery).
            let outpoints: Vec<_> = self
                .coins
                .iter()
                .filter_map(
                    |(c, selected)| {
                        if *selected {
                            Some(c.outpoint)
                        } else {
                            None
                        }
                    },
                )
                .collect();
            if outpoints.is_empty() {
                // If the user has deselected all coins, set any recipient's max amount to 0.
                if let Some((i, recipient)) = recipient_with_max {
                    recipient.update(
                        self.network,
                        view::CreateSpendMessage::RecipientEdited(i, "amount", "0".to_string()),
                    );
                }
                // Simply set the amount left to select as the total destination value. Note this
                // doesn't take account of the fee, but passing an empty list to `create_spend_tx`
                // would use auto-selection and so we settle for this approximation.
                // Note that for a recovery, the amount left to select is ignored by the view.
                self.amount_left_to_select = Some(Amount::from_sat(destinations.values().sum()));
                self.fee_amount = None;
                return;
            }
            outpoints
        } else if !self.is_user_coin_selection && self.send_max_to_recipient.is_some() {
            // If user has not selected coins, send the max available from all owned coins.
            self.coins
                .iter()
                .filter_map(|(c, _)| coin_is_owned(c).then_some(c.outpoint))
                .collect()
        } else {
            Vec::new() // pass empty list for auto-selection
        };

        // If sending the max to a recipient, use that recipient's address as the
        // change/recovery address.
        // Otherwise, for a primary path spend, use a fixed change address from the user's
        // own wallet so that we don't increment the change index.
        let max_address = if let Some((_, recipient)) = &recipient_with_max {
            Address::from_str(&recipient.address.value)
                .expect("Checked before")
                .as_unchecked()
                .clone()
        } else {
            self.descriptor
                .change_descriptor()
                .derive(0.into(), &self.curve)
                .address(self.network)
                .as_unchecked()
                .clone()
        };

        let feerate_vb = self.feerate.value.parse::<u64>().expect("Checked before");
        let recovery_timelock = self.recovery_timelock;
        match tokio::runtime::Handle::current().block_on(async {
            // If recovery timelock is set, create a recovery transaction. Otherwise, a regular spend.
            if let Some(reco_tl) = recovery_timelock {
                daemon
                    .create_recovery(max_address.clone(), &outpoints, feerate_vb, Some(reco_tl))
                    .await
                    // Map the PSBT to `CreateSpendResult` result. We only need the PSBT below.
                    .map(|psbt| CreateSpendResult::Success {
                        psbt,
                        warnings: vec![],
                    })
            } else {
                daemon
                    .create_spend_tx(
                        &outpoints,
                        &destinations,
                        feerate_vb,
                        Some(max_address.clone()),
                    )
                    .await
            }
        }) {
            Ok(CreateSpendResult::Success { psbt, .. }) => {
                self.warning = None;
                self.fee_amount = Some(psbt.fee().expect("Valid fees"));
                // Update selected coins for auto-selection (non-recovery case).
                if !self.is_user_coin_selection && self.recovery_timelock.is_none() {
                    let selected_coins: Vec<OutPoint> = psbt
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
                if let Some((i, recipient)) = recipient_with_max {
                    // If there's no change output, any excess must be below the dust threshold
                    // and so the max available for this recipient is 0.
                    let amount = psbt
                        .unsigned_tx
                        .output
                        .iter()
                        .find(|o| {
                            o.script_pubkey == max_address.clone().assume_checked().script_pubkey()
                        })
                        .map(|change_output| change_output.value.to_btc())
                        .unwrap_or(0.0)
                        .to_string();
                    recipient.update(
                        self.network,
                        view::CreateSpendMessage::RecipientEdited(i, "amount", amount),
                    );
                }
            }
            Ok(CreateSpendResult::InsufficientFunds { missing }) => {
                self.fee_amount = None;
                self.amount_left_to_select = Some(Amount::from_sat(missing));
                // To be sure, we exclude recovery transactions here, although they
                // can't currently reach this part of code.
                if !self.is_user_coin_selection && self.recovery_timelock.is_none() {
                    // The missing amount is based on all candidates for coin selection
                    // being used, which are all owned coins.
                    for (coin, selected) in &mut self.coins {
                        *selected = coin_is_owned(coin);
                    }
                }
                if let Some((i, recipient)) = recipient_with_max {
                    let amount = Amount::from_sat(if destinations.is_empty() {
                        // If there are no other recipients, then the missing value will
                        // be the amount left to select in order to create an output at the dust
                        // threshold. Therefore, set this recipient's amount to this value so
                        // that the information shown is consistent.
                        // Otherwise, there are already insufficient funds for the other
                        // recipients and so the max available for this recipient is 0.
                        DUST_OUTPUT_SATS
                    } else {
                        0
                    })
                    .to_btc()
                    .to_string();
                    recipient.update(
                        self.network,
                        view::CreateSpendMessage::RecipientEdited(i, "amount", amount),
                    );
                }
            }
            Err(e) => {
                self.warning = Some(e.into());
                self.fee_amount = None;
            }
        }
    }
}

impl Step for DefineSpend {
    fn load(&mut self, coins: &[Coin], tip_height: i32, draft: &TransactionDraft) {
        self.tip_height = tip_height as u32;
        match (self.recovery_timelock, draft.recovery_timelock) {
            (Some(old_tl), Some(new_tl)) => {
                if old_tl != new_tl {
                    // If the timelock has changed, reinitialise this step.
                    let new = Self::new(
                        self.network,
                        self.descriptor.clone(),
                        coins,
                        tip_height as u32,
                        Some(new_tl),
                        self.is_first_step,
                    );
                    *self = new;
                    return;
                } else {
                    // If the timelock has not changed, we keep the existing coins selection if it has already been edited
                    // by the user or otherwise if the form values are valid. The form values being valid means a redraft has
                    // been performed and so we keep the currently selected coins in order that the recipient amount displayed
                    // matches the selection.
                    let selected = (self.is_user_coin_selection
                        || self.form_values_are_valid(true))
                    .then_some(
                        self.coins
                            .iter()
                            .filter_map(|(coin, sel)| sel.then_some(coin.outpoint))
                            .collect(),
                    );
                    self.coins = filter_coins(coins, self.recovery_timelock, tip_height, selected);
                }
            }
            _ => {
                // We don't handle the case of a recovery timelock being added or removed:
                // A spend is either primary or recovery and cannot change from one to the other.
            }
        }
        self.check_valid();
    }

    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        match message {
            Message::View(view::Message::CreateSpend(msg)) => {
                match msg {
                    view::CreateSpendMessage::BatchLabelEdited(label) => {
                        self.batch_label.valid = label.len() <= 100;
                        self.batch_label.value = label;
                    }
                    view::CreateSpendMessage::Clear => {
                        *self = Self::new(
                            self.network,
                            self.descriptor.clone(),
                            self.coins
                                .iter()
                                .map(|(c, _)| c.clone())
                                .collect::<Vec<ListCoinsEntry>>()
                                .as_slice(),
                            self.tip_height,
                            self.recovery_timelock,
                            self.is_first_step,
                        );
                        return Task::none();
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
                        if let Some(j) = self.send_max_to_recipient {
                            match j.cmp(&i) {
                                Ordering::Equal => {
                                    self.send_max_to_recipient = None;
                                }
                                Ordering::Greater => {
                                    self.send_max_to_recipient = Some(
                                        j.checked_sub(1)
                                            .expect("j must be greater than 0 in this case"),
                                    );
                                }
                                _ => {}
                            }
                        }
                    }
                    view::CreateSpendMessage::RecipientEdited(i, _, _) => {
                        self.recipients
                            .get_mut(i)
                            .unwrap()
                            .update(cache.network, msg);
                    }

                    view::CreateSpendMessage::Bip21Edited(i, bip21) => {
                        if let Some(recipient) = self.recipients.get_mut(i) {
                            recipient.bip21.value = bip21.clone();
                            if let Ok(uri) = Uri::try_from(bip21.as_str()) {
                                if let Ok(address) = uri.address.require_network(cache.network) {
                                    recipient.address.value = address.to_string();
                                    recipient.update(
                                        cache.network,
                                        view::CreateSpendMessage::RecipientEdited(
                                            i,
                                            "address",
                                            address.to_string(),
                                        ),
                                    );
                                }
                                if let Some(amount) = uri.amount {
                                    recipient.amount.value =
                                        amount.to_string_in(Denomination::Bitcoin);
                                    recipient.update(
                                        cache.network,
                                        view::CreateSpendMessage::RecipientEdited(
                                            i,
                                            "amount",
                                            amount.to_string_in(Denomination::Bitcoin),
                                        ),
                                    );
                                }
                            } else {
                                self.warning = Some(SpendCreationError::InvalidBip21.into());
                            }
                        }
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
                        let feerate_vb = self.feerate.value.parse::<u64>().unwrap_or(0);
                        self.warning = None;
                        if let Some(reco_tl) = self.recovery_timelock {
                            let recovery_address = Address::from_str(
                                &self
                                    .recipients
                                    .first()
                                    .expect("recovery spend has a recipient")
                                    .address
                                    .value,
                            )
                            .expect("Checked before");
                            return Task::perform(
                                async move {
                                    daemon
                                        .create_recovery(
                                            recovery_address,
                                            &inputs,
                                            feerate_vb,
                                            Some(reco_tl),
                                        )
                                        .await
                                        .map_err(|e| e.into())
                                        .map(|psbt| (psbt, vec![]))
                                },
                                Message::Psbt,
                            );
                        } else {
                            for recipient in &self.recipients {
                                let address = Address::from_str(&recipient.address.value)
                                    .expect("Checked before");
                                outputs
                                    .insert(address, recipient.amount().expect("Checked before"));
                            }
                            return Task::perform(
                                async move {
                                    daemon
                                        .create_spend_tx(&inputs, &outputs, feerate_vb, None)
                                        .await
                                        .map_err(|e| e.into())
                                        .and_then(|res| match res {
                                            CreateSpendResult::Success { psbt, warnings } => {
                                                Ok((psbt, warnings))
                                            }
                                            CreateSpendResult::InsufficientFunds { missing } => {
                                                Err(SpendCreationError::CoinSelection(
                                                    liana::spend::InsufficientFunds { missing },
                                                )
                                                .into())
                                            }
                                        })
                                },
                                Message::Psbt,
                            );
                        }
                    }
                    view::CreateSpendMessage::SelectCoin(i) => {
                        if let Some(coin) = self.coins.get_mut(i) {
                            coin.1 = !coin.1;
                            // Once user edits selection, auto-selection can no longer be used.
                            self.is_user_coin_selection = true;
                        }
                    }
                    view::CreateSpendMessage::SendMaxToRecipient(i) => {
                        if self.recipients.get(i).is_some() {
                            if self.send_max_to_recipient == Some(i) {
                                // If already set to this recipient, then unset it.
                                self.send_max_to_recipient = None;
                            } else {
                                // Either it's set to some other recipient or not at all.
                                self.send_max_to_recipient = Some(i);
                            };
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
                    return Task::perform(async {}, |_| Message::View(view::Message::Next));
                }
                Err(e) => self.warning = Some(e),
            },
            Message::Labels(res) => match res {
                Ok(labels) => {
                    self.coins_labels = labels;
                }
                Err(e) => self.warning = Some(e),
            },
            Message::CoinsTipHeight(res_coins, res_tip) => match (res_coins, res_tip) {
                (Ok(coins), Ok(tip)) => {
                    self.tip_height = tip as u32;
                    // If it's a recovery spend and user has never edited the selection, then we won't pass
                    // any selection and will let `filter_coins` select all by default.
                    let selected = (self.recovery_timelock.is_none()
                        || self.is_user_coin_selection)
                        .then_some(HashSet::from_iter(self.coins.iter().filter_map(
                            |(c, selected)| {
                                if *selected {
                                    Some(c.outpoint)
                                } else {
                                    None
                                }
                            },
                        )));
                    self.coins = filter_coins(&coins, self.recovery_timelock, tip, selected);
                    self.sort_coins(tip as u32);
                    // In case some selected coins are not spendable anymore and
                    // new coins make more sense to be selected. A redraft is triggered
                    // if all forms are valid (checked in the redraft method)
                    self.redraft(daemon);
                    self.check_valid();
                }
                (Err(e), _) | (Ok(_), Err(e)) => self.warning = Some(e),
            },
            _ => {}
        };
        Task::none()
    }

    fn apply(&self, draft: &mut TransactionDraft) {
        draft.inputs = self
            .coins
            .iter()
            .filter_map(|(coin, selected)| if *selected { Some(coin) } else { None })
            .cloned()
            .collect();
        if let Some((psbt, _)) = &self.generated {
            draft.labels.clone_from(&self.coins_labels);
            for (i, output) in psbt.unsigned_tx.output.iter().enumerate() {
                if let Some(label) = self
                    .recipients
                    .iter()
                    .find(|recipient| {
                        !recipient.label.value.is_empty()
                            && Address::from_str(&recipient.address.value)
                                .unwrap()
                                .assume_checked()
                                .matches_script_pubkey(&output.script_pubkey)
                            && output.value.to_sat() == recipient.amount().unwrap()
                    })
                    .map(|recipient| recipient.label.value.to_string())
                {
                    draft.labels.insert(
                        OutPoint {
                            txid: psbt.unsigned_tx.compute_txid(),
                            vout: i as u32,
                        }
                        .to_string(),
                        label,
                    );
                }
            }
        }
        draft.recipients.clone_from(&self.recipients);
        if self.recipients.len() > 1 {
            draft.batch_label = Some(self.batch_label.value.clone());
        }
        draft.generated.clone_from(&self.generated);
    }

    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        view::spend::create_spend_tx(
            cache,
            self.recipients
                .iter()
                .enumerate()
                .map(|(i, recipient)| {
                    recipient
                        .view(i, self.send_max_to_recipient == Some(i))
                        .map(view::Message::CreateSpend)
                })
                .collect(),
            self.is_valid,
            self.is_duplicate,
            self.timelock(),
            self.recovery_timelock,
            &self.coins,
            &self.coins_labels,
            &self.batch_label,
            self.amount_left_to_select.as_ref(),
            &self.feerate,
            self.fee_amount.as_ref(),
            self.warning.as_ref(),
            self.is_first_step,
        )
    }
}

#[derive(Default, Clone)]
struct Recipient {
    label: form::Value<String>,
    address: form::Value<String>,
    amount: form::Value<String>,
    bip21: form::Value<String>,
    is_recovery: bool,
}

impl Recipient {
    fn new(is_recovery: bool) -> Self {
        Self {
            is_recovery,
            ..Default::default()
        }
    }

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
            if amount <= address.assume_checked().script_pubkey().minimal_non_dust() {
                return Err(Error::Unexpected(
                    "Amount must be superior to script dust value".to_string(),
                ));
            }
        }

        Ok(amount.to_sat())
    }

    fn address_valid(&self) -> bool {
        !self.address.value.is_empty() && self.address.valid
    }

    fn valid(&self) -> bool {
        self.address_valid()
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
            view::CreateSpendMessage::Bip21Edited(_, bip21) => {
                self.bip21.value = bip21;
            }
            _ => {}
        };
    }

    fn view(&self, i: usize, is_max_selected: bool) -> Element<view::CreateSpendMessage> {
        view::spend::recipient_view(
            i,
            &self.address,
            &self.amount,
            &self.label,
            is_max_selected,
            self.is_recovery,
            &self.bip21,
        )
    }
}

pub struct SaveSpend {
    wallet: Arc<Wallet>,
    spend: Option<(psbt::PsbtState, Vec<String>)>,
    curve: secp256k1::Secp256k1<secp256k1::VerifyOnly>,
}

impl SaveSpend {
    pub fn new(wallet: Arc<Wallet>) -> Self {
        Self {
            wallet,
            spend: None,
            curve: secp256k1::Secp256k1::verification_only(),
        }
    }
}

impl Step for SaveSpend {
    fn load(&mut self, _coins: &[Coin], _tip_height: i32, draft: &TransactionDraft) {
        let (psbt, warnings) = draft.generated.clone().unwrap();

        let bip21 = draft
            .recipients
            .first()
            .expect("one recipient")
            .bip21
            .value
            .clone();

        let payjoin_status = if let Ok(uri) = Uri::try_from(bip21.as_str()) {
            if uri.assume_checked().extras.pj_is_supported() {
                Some(PayjoinStatus::Pending)
            } else {
                None
            }
        } else {
            None
        };

        let mut tx = SpendTx::new(
            None,
            psbt,
            draft.inputs.clone(),
            &self.wallet.main_descriptor,
            &self.curve,
            draft.network,
            payjoin_status,
        );
        tx.labels.clone_from(&draft.labels);

        if tx.is_batch() {
            if let Some(label) = &draft.batch_label {
                tx.labels.insert(
                    tx.psbt.unsigned_tx.compute_txid().to_string(),
                    label.clone(),
                );
            }
        } else if let Some(recipient) = draft.recipients.first() {
            if !recipient.label.value.is_empty() {
                let label = recipient.label.value.clone();
                tx.labels
                    .insert(tx.psbt.unsigned_tx.compute_txid().to_string(), label);
            }
        }

        self.spend = Some((
            psbt::PsbtState::new(
                self.wallet.clone(),
                tx,
                false,
                if bip21.is_empty() { None } else { Some(bip21) },
            ),
            warnings,
        ));
    }

    fn reload_wallet(&mut self, wallet: Arc<Wallet>) {
        self.wallet = wallet;
    }

    fn interrupt(&mut self) {
        if let Some((psbt_state, _)) = &mut self.spend {
            psbt_state.interrupt()
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        if let Some((psbt_state, _)) = &self.spend {
            psbt_state.subscription()
        } else {
            Subscription::none()
        }
    }

    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        if let Some((psbt_state, _)) = &mut self.spend {
            psbt_state.update(daemon, cache, message)
        } else {
            Task::none()
        }
    }

    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        let (psbt_state, warnings) = self.spend.as_ref().unwrap();
        let content = view::spend::spend_view(
            cache,
            &psbt_state.tx,
            warnings,
            psbt_state.saved,
            &psbt_state.desc_policy,
            &psbt_state.wallet.keys_aliases,
            psbt_state.labels_edited.cache(),
            cache.network,
            if let Some(psbt::PsbtModal::Sign(m)) = &psbt_state.modal {
                m.is_signing()
            } else {
                false
            },
            psbt_state.warning.as_ref(),
        );
        if let Some(modal) = &psbt_state.modal {
            modal.as_ref().view(content)
        } else {
            content
        }
    }
}

pub struct SelectRecoveryPath {
    wallet: Arc<Wallet>,
    recovery_paths: Vec<RecoveryPath>,
    selected_path: Option<usize>,
    warning: Option<Error>,
}

impl SelectRecoveryPath {
    pub fn new(wallet: Arc<Wallet>, coins: &[Coin], tip_height: i32) -> Self {
        Self {
            recovery_paths: recovery_paths(&wallet, coins, tip_height),
            wallet,
            selected_path: None,
            warning: None,
        }
    }

    pub fn load_from_coins_and_tip_height(&mut self, coins: &[Coin], tip_height: i32) {
        self.warning = None;
        // Update the available recovery paths, maintaining any selected path.
        let selected_seq = self.selected_path.and_then(|selected| {
            self.recovery_paths
                .get(selected)
                .map(|reco_path| reco_path.sequence)
        });
        self.recovery_paths = recovery_paths(&self.wallet, coins, tip_height);
        self.selected_path = selected_seq.and_then(|seq| {
            self.recovery_paths
                .iter()
                .enumerate()
                .find_map(|(i, path)| (path.sequence == seq).then_some(i))
        });
    }
}

impl Step for SelectRecoveryPath {
    fn load(&mut self, coins: &[Coin], tip_height: i32, _draft: &TransactionDraft) {
        self.load_from_coins_and_tip_height(coins, tip_height);
    }

    fn reload_wallet(&mut self, wallet: Arc<Wallet>) {
        self.wallet = wallet;
    }

    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
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
            self.warning.as_ref(),
        )
    }

    fn update(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        match message {
            Message::CoinsTipHeight(res_coins, res_tip) => match (res_coins, res_tip) {
                (Ok(coins), Ok(tip)) => {
                    self.load_from_coins_and_tip_height(&coins, tip);
                }
                (Err(e), _) | (Ok(_), Err(e)) => {
                    self.warning = Some(e);
                }
            },
            Message::View(view::Message::CreateSpend(view::CreateSpendMessage::SelectPath(
                index,
            ))) => {
                if Some(index) == self.selected_path {
                    self.selected_path = None;
                } else {
                    self.selected_path = Some(index);
                }
            }
            _ => {}
        };
        Task::none()
    }

    fn apply(&self, draft: &mut TransactionDraft) {
        if let Some(selected_path) = self.selected_path {
            if let Some(path) = self.recovery_paths.get(selected_path) {
                draft.recovery_timelock = Some(path.sequence);
            }
        }
    }
}

pub struct RecoveryPath {
    threshold: usize,
    sequence: u16,
    origins: Vec<(Fingerprint, HashSet<DerivationPath>)>,
    total_amount: Amount,
    number_of_coins: usize,
}

fn recovery_paths(wallet: &Wallet, coins: &[Coin], tip_height: i32) -> Vec<RecoveryPath> {
    wallet
        .main_descriptor
        .policy()
        .recovery_paths()
        .iter()
        .map(|(&sequence, path)| {
            let (number_of_coins, total_amount) = coins
                .iter()
                .filter(|coin| {
                    coin.block_height.is_some() // only confirmed coins are included in a recovery transaction
                        && coin.spend_info.is_none()
                        && remaining_sequence(coin, tip_height as u32, sequence) <= 1
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
