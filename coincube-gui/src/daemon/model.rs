use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use coincube_core::wallet_descriptor::SpendAnalysis;
pub use coincube_core::{
    descriptors::{CoincubePolicy, PartialSpendInfo, PathSpendInfo},
    miniscript::bitcoin::{
        bip32::{DerivationPath, Fingerprint},
        psbt::Psbt,
        secp256k1, Address, Amount, Network, OutPoint, Transaction, Txid,
    },
};
pub use coincubed::commands::{
    CreateSpendResult, GetAddressResult, GetInfoResult, GetLabelsResult, LabelItem, ListCoinsEntry,
    ListCoinsResult, ListRevealedAddressesEntry, ListRevealedAddressesResult, ListSpendEntry,
    ListSpendResult, ListTransactionsResult, TransactionInfo,
};

pub type Coin = ListCoinsEntry;

pub fn remaining_sequence(coin: &Coin, blockheight: u32, timelock: u16) -> u32 {
    if let Some(coin_blockheight) = coin.block_height {
        (coin_blockheight as u32 + timelock as u32).saturating_sub(blockheight)
    } else {
        timelock as u32
    }
}

/// Earliest recovery timing among confirmed, unspent coins. A sequence of at
/// most one is spendable in the next block, as the recovery spend selector uses.
pub fn next_recovery_sequence(coins: &[Coin], blockheight: u32, timelock: u16) -> Option<u32> {
    coins
        .iter()
        .filter(|coin| coin.block_height.is_some() && coin.spend_info.is_none())
        .map(|coin| remaining_sequence(coin, blockheight, timelock))
        .min()
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
}

#[derive(PartialOrd, Ord, Debug, Clone, PartialEq, Eq)]
pub enum SpendStatus {
    Pending,
    Broadcast,
    Spent,
    Deprecated,
}

impl SpendTx {
    pub fn new(
        updated_at: Option<u32>,
        psbt: Psbt,
        coins: Vec<Coin>,
        desc: &impl SpendAnalysis,
        secp: &secp256k1::Secp256k1<impl secp256k1::Verification>,
        network: Network,
    ) -> Self {
        Self::new_recorded(updated_at, psbt, None, coins, desc, secp, network)
    }

    /// [`Self::new`] for a stored spend, with the transaction ID the daemon
    /// recorded when it broadcast it. Finalizing a `pkh` or `sh(wpkh)` input
    /// adds a `scriptSig`, so the broadcast ID differs from the unsigned
    /// PSBT's; coins spent by either count as spent by this transaction.
    pub fn new_recorded(
        updated_at: Option<u32>,
        psbt: Psbt,
        broadcast_txid: Option<Txid>,
        coins: Vec<Coin>,
        desc: &impl SpendAnalysis,
        secp: &secp256k1::Secp256k1<impl secp256k1::Verification>,
        network: Network,
    ) -> Self {
        Self::from_list_entry(
            ListSpendEntry {
                psbt,
                updated_at,
                broadcast_txid,
            },
            coins,
            desc,
            secp,
            network,
        )
    }

    /// A stored spend as the daemon lists it. See [`Self::new_recorded`].
    pub(super) fn from_list_entry(
        entry: ListSpendEntry,
        coins: Vec<Coin>,
        desc: &impl SpendAnalysis,
        secp: &secp256k1::Secp256k1<impl secp256k1::Verification>,
        network: Network,
    ) -> Self {
        let ListSpendEntry {
            psbt,
            updated_at,
            broadcast_txid,
        } = entry;
        // Use primary path if no inputs are using a relative locktime.
        let use_primary_path = !psbt
            .unsigned_tx
            .input
            .iter()
            .map(|txin| txin.sequence)
            .any(|seq| seq.is_relative_lock_time());
        let max_vbytes = desc.spend_max_vbytes(&psbt.unsigned_tx, use_primary_path);
        let change_indexes: Vec<usize> = desc.spend_change_indexes(&psbt, secp);
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
        let unsigned_txid = psbt.unsigned_tx.compute_txid();
        let mut coins_map = HashMap::<OutPoint, Coin>::with_capacity(coins.len());
        for coin in coins {
            if let Some(info) = coin.spend_info {
                if info.txid == unsigned_txid || Some(info.txid) == broadcast_txid {
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
        if coins_map.len() != psbt.inputs.len() {
            status = SpendStatus::Deprecated
        }

        let sigs = desc
            .spend_info(&psbt)
            .expect("PSBT must be generated by Coincube");

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
                                txid: unsigned_txid,
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
        }
    }

    /// Returns the path ready if it exists.
    pub fn path_ready(&self) -> Option<&PathSpendInfo> {
        let path = self.sigs.primary_path();
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
    /// A change output of an outgoing transaction. The payment it belongs to
    /// is already listed as [`PaymentKind::Outgoing`], so surfacing this one
    /// too would double-count it.
    SendToSelf,
    /// An output of a transaction that pays only this wallet — a refresh of
    /// coins ahead of their timelock, typically. Nothing else represents it
    /// in the list, so it is shown on its own.
    SelfTransfer,
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
                kind: if history_tx.is_send_to_self() {
                    PaymentKind::SelfTransfer
                } else if history_tx.is_outgoing()
                    && history_tx.change_indexes.contains(&output_index)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::split_test_wallets::{imported_vault, Shape};
    use coincube_core::miniscript::bitcoin::{absolute, transaction, ScriptBuf, TxIn, TxOut};
    use coincubed::commands::LCSpendInfo;

    #[test]
    fn imported_psbt_status_follows_the_final_broadcast_txid() {
        let wallet = imported_vault(Shape::ShWpkh);
        let address = wallet.preview_change_address().assume_checked();
        let outpoint = OutPoint::null();
        let unsigned = Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: vec![TxIn {
                previous_output: outpoint,
                ..TxIn::default()
            }],
            output: vec![TxOut {
                value: Amount::from_sat(19_000),
                script_pubkey: address.script_pubkey(),
            }],
        };
        let unsigned_txid = unsigned.compute_txid();
        let mut signed = unsigned.clone();
        signed.input[0].script_sig = ScriptBuf::from_bytes(vec![1, 1]);
        let txid = signed.compute_txid();
        assert_ne!(unsigned_txid, txid);

        let entry = ListSpendEntry {
            updated_at: None,
            broadcast_txid: Some(txid),
            psbt: Psbt::from_unsigned_tx(unsigned).unwrap(),
        };
        for (height, status) in [
            (None, SpendStatus::Broadcast),
            (Some(101), SpendStatus::Spent),
        ] {
            let coin = Coin {
                amount: Amount::from_sat(20_000),
                outpoint,
                address: address.clone(),
                block_height: Some(100),
                derivation_index: 0.into(),
                spend_info: Some(LCSpendInfo { txid, height }),
                is_immature: false,
                is_change: false,
                is_from_self: false,
            };
            let spend = SpendTx::from_list_entry(
                entry.clone(),
                vec![coin],
                &wallet.main_descriptor,
                &secp256k1::Secp256k1::verification_only(),
                Network::Bitcoin,
            );
            assert_eq!(spend.status, status);
            assert_eq!(spend.psbt.unsigned_tx.compute_txid(), unsigned_txid);
        }
    }
}
