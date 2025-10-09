use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use liana::descriptors::LianaDescriptor;
pub use liana::{
    descriptors::{LianaPolicy, PartialSpendInfo, PathSpendInfo},
    miniscript::bitcoin::{
        bip32::{DerivationPath, Fingerprint},
        psbt::Psbt,
        secp256k1, Address, Amount, Network, OutPoint, Transaction, Txid,
    },
};
pub use lianad::commands::{
    CreateSpendResult, GetAddressResult, GetInfoResult, GetLabelsResult, LabelItem, ListCoinsEntry,
    ListCoinsResult, ListRevealedAddressesEntry, ListRevealedAddressesResult, ListSpendEntry,
    ListSpendResult, ListTransactionsResult, TransactionInfo,
};
use lianad::payjoin::types::PayjoinStatus;

pub type Coin = ListCoinsEntry;

pub fn remaining_sequence(coin: &Coin, blockheight: u32, timelock: u16) -> u32 {
    if let Some(coin_blockheight) = coin.block_height {
        if blockheight > coin_blockheight as u32 + timelock as u32 {
            0
        } else {
            coin_blockheight as u32 + timelock as u32 - blockheight
        }
    } else {
        timelock as u32
    }
}

/// Whether the coin is owned by this wallet.
/// This comprises all confirmed coins together with those
/// unconfirmed coins from self.
pub fn coin_is_owned(coin: &Coin) -> bool {
    coin.block_height.is_some() || coin.is_from_self
}

#[derive(Debug, Clone)]
pub struct SpendTx {
    pub network: Network,
    pub coins: HashMap<OutPoint, Coin>,
    pub labels: HashMap<String, String>,
    pub psbt: Psbt,
    pub change_indexes: Vec<usize>,
    pub spend_amount: Amount,
    pub fee_amount: Option<Amount>,
    /// Maximum possible size of the unsigned transaction after satisfaction
    /// (assuming all inputs are for the same descriptor).
    pub max_vbytes: u64,
    pub status: SpendStatus,
    pub sigs: PartialSpendInfo,
    pub updated_at: Option<u32>,
    pub kind: TransactionKind,
    pub payjoin_status: Option<PayjoinStatus>,
    // // TODO: use a stronger type like bitcoin_uri
    // pub bip21: String,
}

#[derive(PartialOrd, Ord, Debug, Clone, PartialEq, Eq)]
pub enum SpendStatus {
    Pending,
    Broadcast,
    Spent,
    Deprecated,
    PayjoinInitiated,
    PayjoinProposalReady,
}

impl SpendTx {
    pub fn new(
        updated_at: Option<u32>,
        psbt: Psbt,
        coins: Vec<Coin>,
        desc: &LianaDescriptor,
        secp: &secp256k1::Secp256k1<impl secp256k1::Verification>,
        network: Network,
        payjoin_status: Option<PayjoinStatus>,
    ) -> Self {
        // Use primary path if no inputs are using a relative locktime.
        let use_primary_path = !psbt
            .unsigned_tx
            .input
            .iter()
            .map(|txin| txin.sequence)
            .any(|seq| seq.is_relative_lock_time());
        let max_vbytes = desc.unsigned_tx_max_vbytes(&psbt.unsigned_tx, use_primary_path);
        let change_indexes: Vec<usize> = desc
            .change_indexes(&psbt, secp)
            .into_iter()
            .map(|c| c.index())
            .collect();
        let (change_amount, spend_amount) = psbt.unsigned_tx.output.iter().enumerate().fold(
            (Amount::from_sat(0), Amount::from_sat(0)),
            |(change, spend), (i, output)| {
                if change_indexes.contains(&i) {
                    (change + output.value, spend)
                } else {
                    (change, spend + output.value)
                }
            },
        );

        let mut status = SpendStatus::Pending;
        let mut coins_map = HashMap::<OutPoint, Coin>::with_capacity(coins.len());
        for coin in coins {
            if let Some(info) = coin.spend_info {
                if info.txid == psbt.unsigned_tx.compute_txid() {
                    if info.height.is_some() {
                        status = SpendStatus::Spent
                    } else {
                        status = SpendStatus::Broadcast
                    }
                // The txid will be different if this PSBT is to replace another transaction
                // that is currently spending the coin.
                // The PSBT status should remain as Pending so that it can be signed and broadcast.
                // Once the replacement transaction has been confirmed, the PSBT for the
                // transaction currently spending this coin will be shown as Deprecated.
                } else if info.height.is_some() {
                    status = SpendStatus::Deprecated
                }
            }
            coins_map.insert(coin.outpoint, coin);
        }

        let inputs_amount = {
            let mut inputs_amount = Amount::from_sat(0);
            for (i, input) in psbt.inputs.iter().enumerate() {
                if let Some(utxo) = &input.witness_utxo {
                    inputs_amount += utxo.value;
                // we try to have it from the coin
                } else if let Some(coin) = psbt
                    .unsigned_tx
                    .input
                    .get(i)
                    .and_then(|inpt| coins_map.get(&inpt.previous_output))
                {
                    inputs_amount += coin.amount;
                // Information is missing, it is better to set inputs_amount to None.
                } else {
                    inputs_amount = Amount::from_sat(0);
                    break;
                }
            }
            if inputs_amount.to_sat() == 0 {
                None
            } else {
                Some(inputs_amount)
            }
        };

        // One input coin is missing, the psbt is deprecated for now.
        if coins_map.len() != psbt.inputs.len() && payjoin_status.is_none() {
            status = SpendStatus::Deprecated
        }

        let sigs = desc
            .partial_spend_info(&psbt)
            .expect("PSBT must be generated by Liana");

        Self {
            labels: HashMap::new(),
            kind: if spend_amount == Amount::from_sat(0) {
                TransactionKind::SendToSelf
            } else {
                let outpoints: Vec<OutPoint> = psbt
                    .unsigned_tx
                    .output
                    .iter()
                    .enumerate()
                    .filter_map(|(i, _)| {
                        if !change_indexes.contains(&i) {
                            Some(OutPoint {
                                txid: psbt.unsigned_tx.compute_txid(),
                                vout: i as u32,
                            })
                        } else {
                            None
                        }
                    })
                    .collect();
                if outpoints.len() == 1 {
                    TransactionKind::OutgoingSinglePayment(outpoints[0])
                } else {
                    TransactionKind::OutgoingPaymentBatch(outpoints)
                }
            },
            updated_at,
            coins: coins_map,
            psbt,
            change_indexes,
            spend_amount,
            fee_amount: inputs_amount.and_then(|a| a.checked_sub(spend_amount + change_amount)),
            max_vbytes,
            status,
            sigs,
            network,
            payjoin_status,
        }
    }

    /// Returns the path ready if it exists.
    pub fn path_ready(&self) -> Option<&PathSpendInfo> {
        let path = self.sigs.primary_path();

        // Check if we have signatures for all of our inputs
        let has_sigs =
            self.psbt.inputs.iter().any(|psbtin| {
                !psbtin.partial_sigs.is_empty() && !psbtin.bip32_derivation.is_empty()
            });
        if has_sigs {
            return Some(path);
        }

        if path.sigs_count >= path.threshold {
            return Some(path);
        }

        self.sigs
            .recovery_paths()
            .values()
            .find(|&path| path.sigs_count >= path.threshold)
    }

    pub fn recovery_timelock(&self) -> Option<u16> {
        self.sigs.recovery_paths().keys().max().cloned()
    }

    pub fn signers(&self) -> HashSet<Fingerprint> {
        let mut signers = HashSet::new();
        for fg in self.sigs.primary_path().signed_pubkeys.keys() {
            signers.insert(*fg);
        }

        for path in self.sigs.recovery_paths().values() {
            for fg in path.signed_pubkeys.keys() {
                signers.insert(*fg);
            }
        }

        signers
    }

    /// Feerate obtained if all transaction inputs have the maximum satisfaction size.
    pub fn min_feerate_vb(&self) -> Option<u64> {
        self.fee_amount.map(|a| {
            a.to_sat()
                .checked_div(self.max_vbytes)
                .expect("a descriptor's satisfaction size is never 0")
        })
    }

    pub fn is_send_to_self(&self) -> bool {
        matches!(self.kind, TransactionKind::SendToSelf)
    }

    pub fn is_single_payment(&self) -> Option<OutPoint> {
        match self.kind {
            TransactionKind::IncomingSinglePayment(outpoint) => Some(outpoint),
            TransactionKind::OutgoingSinglePayment(outpoint) => Some(outpoint),
            _ => None,
        }
    }

    pub fn is_batch(&self) -> bool {
        matches!(
            self.kind,
            TransactionKind::IncomingPaymentBatch(_) | TransactionKind::OutgoingPaymentBatch(_)
        )
    }
}

impl Labelled for SpendTx {
    fn labels(&mut self) -> &mut HashMap<String, String> {
        &mut self.labels
    }
    fn labelled(&self) -> Vec<LabelItem> {
        let mut items = Vec::new();
        let txid = self.psbt.unsigned_tx.compute_txid();
        items.push(LabelItem::Txid(txid));
        for coin in self.coins.values() {
            items.push(LabelItem::Address(coin.address.clone()));
        }
        for input in &self.psbt.unsigned_tx.input {
            items.push(LabelItem::OutPoint(input.previous_output));
        }
        for (vout, output) in self.psbt.unsigned_tx.output.iter().enumerate() {
            items.push(LabelItem::OutPoint(OutPoint {
                txid,
                vout: vout as u32,
            }));
            items.push(LabelItem::Address(
                Address::from_script(&output.script_pubkey, self.network).unwrap(),
            ));
        }
        items
    }
}

#[derive(Debug, Clone)]
pub struct HistoryTransaction {
    pub network: Network,
    pub labels: HashMap<String, String>,
    pub coins: HashMap<OutPoint, Coin>,
    pub change_indexes: Vec<usize>,
    pub tx: Transaction,
    pub txid: Txid,
    pub outgoing_amount: Amount,
    pub incoming_amount: Amount,
    pub fee_amount: Option<Amount>,
    pub height: Option<i32>,
    pub time: Option<u32>,
    pub kind: TransactionKind,
}

impl HistoryTransaction {
    pub fn new(
        tx: Transaction,
        height: Option<i32>,
        time: Option<u32>,
        coins: Vec<Coin>,
        change_indexes: Vec<usize>,
        network: Network,
    ) -> Self {
        let (incoming_amount, outgoing_amount) = tx.output.iter().enumerate().fold(
            (Amount::from_sat(0), Amount::from_sat(0)),
            |(change, spend), (i, output)| {
                if change_indexes.contains(&i) {
                    (change + output.value, spend)
                } else {
                    (change, spend + output.value)
                }
            },
        );

        let kind = if coins.is_empty() {
            if change_indexes.len() == 1 {
                TransactionKind::IncomingSinglePayment(OutPoint {
                    txid: tx.compute_txid(),
                    vout: change_indexes[0] as u32,
                })
            } else {
                TransactionKind::IncomingPaymentBatch(
                    change_indexes
                        .iter()
                        .map(|i| OutPoint {
                            txid: tx.compute_txid(),
                            vout: *i as u32,
                        })
                        .collect(),
                )
            }
        } else if outgoing_amount == Amount::from_sat(0) {
            TransactionKind::SendToSelf
        } else {
            let outpoints: Vec<OutPoint> = tx
                .output
                .iter()
                .enumerate()
                .filter_map(|(i, _)| {
                    if !change_indexes.contains(&i) {
                        Some(OutPoint {
                            txid: tx.compute_txid(),
                            vout: i as u32,
                        })
                    } else {
                        None
                    }
                })
                .collect();
            if outpoints.len() == 1 {
                TransactionKind::OutgoingSinglePayment(outpoints[0])
            } else {
                TransactionKind::OutgoingPaymentBatch(outpoints)
            }
        };

        let mut inputs_amount = Amount::from_sat(0);
        let mut coins_map = HashMap::<OutPoint, Coin>::with_capacity(coins.len());
        for coin in coins {
            inputs_amount += coin.amount;
            coins_map.insert(coin.outpoint, coin);
        }

        Self {
            labels: HashMap::new(),
            kind,
            txid: tx.compute_txid(),
            tx,
            coins: coins_map,
            change_indexes,
            outgoing_amount,
            incoming_amount,
            fee_amount: inputs_amount.checked_sub(outgoing_amount + incoming_amount),
            height,
            time,
            network,
        }
    }

    pub fn compare(&self, other: &Self) -> Ordering {
        match (&self.time, &other.time) {
            // `None` values come first
            (None, Some(_)) => Ordering::Less,
            (Some(_), None) => Ordering::Greater,
            // Both are `None`, so we consider them equal
            (None, None) => self.txid.cmp(&other.txid),
            // Both are `Some`, compare by descending time, then by txid
            (Some(time1), Some(time2)) => time2.cmp(time1).then_with(|| self.txid.cmp(&other.txid)),
        }
    }

    pub fn is_external(&self) -> bool {
        matches!(
            self.kind,
            TransactionKind::IncomingSinglePayment(_) | TransactionKind::IncomingPaymentBatch(_)
        )
    }

    pub fn is_outgoing(&self) -> bool {
        matches!(
            self.kind,
            TransactionKind::OutgoingPaymentBatch(_) | TransactionKind::OutgoingSinglePayment(_)
        )
    }

    pub fn is_send_to_self(&self) -> bool {
        matches!(self.kind, TransactionKind::SendToSelf)
    }

    pub fn is_single_payment(&self) -> Option<OutPoint> {
        match self.kind {
            TransactionKind::IncomingSinglePayment(outpoint) => Some(outpoint),
            TransactionKind::OutgoingSinglePayment(outpoint) => Some(outpoint),
            _ => None,
        }
    }

    pub fn is_batch(&self) -> bool {
        matches!(
            self.kind,
            TransactionKind::IncomingPaymentBatch(_) | TransactionKind::OutgoingPaymentBatch(_)
        )
    }
}

#[derive(Debug, Clone)]
pub struct Payment {
    pub label: Option<String>,
    pub address: Option<String>,
    pub address_label: Option<String>,
    pub amount: Amount,
    pub outpoint: OutPoint,
    pub time: Option<chrono::DateTime<chrono::Utc>>,
    pub kind: PaymentKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PaymentKind {
    Outgoing,
    Incoming,
    /// A payment to self, which could be either from a self-transfer
    /// or a change output from an outgoing transaction.
    SendToSelf,
}

impl Payment {
    pub fn compare(&self, other: &Self) -> Ordering {
        match (&self.time, &other.time) {
            // `None` values come first
            (None, Some(_)) => Ordering::Less,
            (Some(_), None) => Ordering::Greater,
            // Both are `None`, so we consider them equal
            (None, None) => self
                .outpoint
                .txid
                .cmp(&other.outpoint.txid)
                .then_with(|| self.outpoint.vout.cmp(&other.outpoint.vout)),
            // Both are `Some`, compare by descending time, then by txid
            (Some(time1), Some(time2)) => time2
                .cmp(time1)
                .then_with(|| self.outpoint.txid.cmp(&other.outpoint.txid))
                .then_with(|| self.outpoint.vout.cmp(&other.outpoint.vout)),
        }
    }
}

impl LabelsLoader for Payment {
    fn load_labels(&mut self, new_labels: &HashMap<String, Option<String>>) {
        if let Some(label) = self.address.as_ref().and_then(|addr| new_labels.get(addr)) {
            self.address_label = label.clone();
        }
        if let Some(label) = new_labels.get(&self.outpoint.to_string()) {
            self.label = label.clone();
        }
    }
}

pub fn payments_from_tx(history_tx: HistoryTransaction) -> Vec<Payment> {
    let time = history_tx
        .time
        .map(|t| chrono::DateTime::<chrono::Utc>::from_timestamp(t as i64, 0).unwrap());
    history_tx
        .tx
        .output
        .iter()
        .enumerate()
        .fold(Vec::new(), |mut array, (output_index, output)| {
            if history_tx.is_external() && !history_tx.change_indexes.contains(&output_index) {
                return array;
            }
            let outpoint = OutPoint {
                txid: history_tx.tx.compute_txid(),
                vout: output_index as u32,
            };
            let label = history_tx.labels.get(&outpoint.to_string()).cloned();
            let address = Address::from_script(&output.script_pubkey, history_tx.network)
                .ok()
                .map(|addr| addr.to_string());
            let address_label = address
                .as_ref()
                .and_then(|addr| history_tx.labels.get(addr).cloned());
            array.push(Payment {
                label,
                address,
                address_label,
                outpoint,
                time,
                amount: output.value,
                kind: if history_tx.is_send_to_self()
                    || (history_tx.is_outgoing()
                        && history_tx.change_indexes.contains(&output_index))
                {
                    PaymentKind::SendToSelf
                } else if history_tx.is_external() {
                    PaymentKind::Incoming
                } else {
                    PaymentKind::Outgoing
                },
            });
            array
        })
}

#[derive(Debug, Clone)]
pub enum TransactionKind {
    IncomingSinglePayment(OutPoint),
    IncomingPaymentBatch(Vec<OutPoint>),
    SendToSelf,
    OutgoingSinglePayment(OutPoint),
    OutgoingPaymentBatch(Vec<OutPoint>),
}

impl Labelled for HistoryTransaction {
    fn labels(&mut self) -> &mut HashMap<String, String> {
        &mut self.labels
    }
    fn labelled(&self) -> Vec<LabelItem> {
        let mut items = Vec::new();
        let txid = self.tx.compute_txid();
        items.push(LabelItem::Txid(txid));
        for coin in self.coins.values() {
            items.push(LabelItem::Address(coin.address.clone()));
        }
        for input in &self.tx.input {
            items.push(LabelItem::OutPoint(input.previous_output));
        }
        for (vout, output) in self.tx.output.iter().enumerate() {
            items.push(LabelItem::OutPoint(OutPoint {
                txid,
                vout: vout as u32,
            }));
            if let Ok(addr) = Address::from_script(&output.script_pubkey, self.network) {
                items.push(LabelItem::Address(addr));
            }
        }
        items
    }
}

pub trait Labelled {
    fn labelled(&self) -> Vec<LabelItem>;
    fn labels(&mut self) -> &mut HashMap<String, String>;
}

pub trait LabelsLoader {
    fn load_labels(&mut self, new_labels: &HashMap<String, Option<String>>);
}

impl<T: ?Sized> LabelsLoader for T
where
    T: Labelled,
{
    fn load_labels(&mut self, new_labels: &HashMap<String, Option<String>>) {
        let items = self.labelled();
        let labels = self.labels();
        for item in items {
            let item_str = item.to_string();
            if let Some(label) = new_labels.get(&item_str) {
                if let Some(l) = label {
                    labels.insert(item_str, l.to_string());
                } else {
                    labels.remove(&item_str);
                }
            }
        }
    }
}
