use std::collections::{HashMap, HashSet};

pub use liana::{
    commands::{
        CreateSpendResult, GetAddressResult, GetInfoResult, GetLabelsResult, LabelItem,
        ListCoinsEntry, ListCoinsResult, ListSpendEntry, ListSpendResult, ListTransactionsResult,
        TransactionInfo,
    },
    descriptors::{PartialSpendInfo, PathSpendInfo},
    miniscript::bitcoin::{
        bip32::Fingerprint, psbt::Psbt, Address, Amount, Network, OutPoint, Transaction, Txid,
    },
};

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

#[derive(Debug, Clone)]
pub struct SpendTx {
    pub network: Network,
    pub coins: Vec<Coin>,
    pub labels: HashMap<String, String>,
    pub psbt: Psbt,
    pub change_indexes: Vec<usize>,
    pub spend_amount: Amount,
    pub fee_amount: Amount,
    /// The maximum size difference (in virtual bytes) of
    /// an input in this transaction before and after satisfaction.
    pub max_sat_vbytes: usize,
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
        sigs: PartialSpendInfo,
        max_sat_vbytes: usize,
        network: Network,
    ) -> Self {
        let mut change_indexes = Vec::new();
        let (change_amount, spend_amount) = psbt.unsigned_tx.output.iter().enumerate().fold(
            (Amount::from_sat(0), Amount::from_sat(0)),
            |(change, spend), (i, output)| {
                if !psbt.outputs[i].bip32_derivation.is_empty() {
                    change_indexes.push(i);
                    (change + Amount::from_sat(output.value), spend)
                } else {
                    (change, spend + Amount::from_sat(output.value))
                }
            },
        );

        let mut inputs_amount = Amount::from_sat(0);
        let mut status = SpendStatus::Pending;
        for coin in &coins {
            inputs_amount += coin.amount;
            if let Some(info) = coin.spend_info {
                if info.txid == psbt.unsigned_tx.txid() {
                    if info.height.is_some() {
                        status = SpendStatus::Spent
                    } else {
                        status = SpendStatus::Broadcast
                    }
                } else {
                    status = SpendStatus::Deprecated
                }
            }
        }

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
                                txid: psbt.unsigned_tx.txid(),
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
            coins,
            psbt,
            change_indexes,
            spend_amount,
            fee_amount: inputs_amount - spend_amount - change_amount,
            max_sat_vbytes,
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
    pub fn min_feerate_vb(&self) -> u64 {
        // This assumes all inputs are internal (have same max satisfaction size).
        let max_tx_vbytes =
            self.psbt.unsigned_tx.vsize() + (self.max_sat_vbytes * self.psbt.inputs.len());
        self.fee_amount.to_sat() / max_tx_vbytes as u64
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
        let txid = self.psbt.unsigned_tx.txid();
        items.push(LabelItem::Txid(txid));
        for coin in &self.coins {
            items.push(LabelItem::Address(coin.address.clone()));
            items.push(LabelItem::OutPoint(coin.outpoint));
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
    pub coins: Vec<Coin>,
    pub change_indexes: Vec<usize>,
    pub tx: Transaction,
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
                    (change + Amount::from_sat(output.value), spend)
                } else {
                    (change, spend + Amount::from_sat(output.value))
                }
            },
        );

        let mut inputs_amount = Amount::from_sat(0);
        for coin in &coins {
            inputs_amount += coin.amount;
        }

        let fee_amount = if inputs_amount > outgoing_amount + incoming_amount {
            Some(inputs_amount - outgoing_amount - incoming_amount)
        } else {
            None
        };

        Self {
            labels: HashMap::new(),
            kind: if coins.is_empty() {
                if change_indexes.len() == 1 {
                    TransactionKind::IncomingSinglePayment(OutPoint {
                        txid: tx.txid(),
                        vout: change_indexes[0] as u32,
                    })
                } else {
                    TransactionKind::IncomingPaymentBatch(
                        change_indexes
                            .iter()
                            .map(|i| OutPoint {
                                txid: tx.txid(),
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
                                txid: tx.txid(),
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
            tx,
            coins,
            change_indexes,
            outgoing_amount,
            incoming_amount,
            fee_amount,
            height,
            time,
            network,
        }
    }

    pub fn is_external(&self) -> bool {
        matches!(
            self.kind,
            TransactionKind::IncomingSinglePayment(_) | TransactionKind::IncomingPaymentBatch(_)
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
        let txid = self.tx.txid();
        items.push(LabelItem::Txid(txid));
        for coin in &self.coins {
            items.push(LabelItem::Address(coin.address.clone()));
            items.push(LabelItem::OutPoint(coin.outpoint));
        }
        for (vout, output) in self.tx.output.iter().enumerate() {
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

pub trait Labelled {
    fn labelled(&self) -> Vec<LabelItem>;
    fn labels(&mut self) -> &mut HashMap<String, String>;
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
