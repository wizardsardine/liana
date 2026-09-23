use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use liana::{
    descriptors::LianaDescriptor,
    spend::{psbt_spend_state, SpendCoin, TxMempoolState},
};
pub use liana::{
    descriptors::{LianaPolicy, PartialSpendInfo, PathSpendInfo},
    miniscript::bitcoin::{
        bip32::{DerivationPath, Fingerprint},
        psbt::Psbt,
        secp256k1, Address, Amount, Network, OutPoint, Transaction, Txid,
    },
    spend::SpendStatus,
};
use liana_ui::component::panels::home::payment::PaymentKind;
pub use lianad::commands::{
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

/// Status of a spend transaction as it can be told from the coins it spends and its signatures,
/// for the transactions the daemon did not give us a status for.
pub fn spend_status_from_coins(
    psbt: &Psbt,
    coins: &[Coin],
    desc: &LianaDescriptor,
    tip_height: i32,
) -> SpendStatus {
    let coins: Vec<_> = psbt
        .unsigned_tx
        .input
        .iter()
        .map(|txin| {
            let coin = coins
                .iter()
                .find(|coin| coin.outpoint == txin.previous_output)?;
            Some(SpendCoin {
                amount: coin.amount,
                block_height: coin.block_height,
                spend_txid: coin.spend_info.as_ref().map(|info| info.txid),
                spend_block_height: coin.spend_info.as_ref().and_then(|info| info.height),
                // The coins don't tell us when the spending transaction was mined.
                spend_block_time: None,
            })
        })
        .collect();
    // Whether this psbt can replace an unconfirmed transaction spending one of its coins depends
    // on the fees of the mempool entries it would evict, which we cannot see from the coins alone.
    // Stay optimistic and let the node decide at broadcast time.
    psbt_spend_state(psbt, desc, &coins, tip_height, TxMempoolState::Unknown).status
}

/// `Unknown` is the escape hatch for a status this client did not get, from an older daemon, or
/// does not understand: fall back to what the coins and the signatures tell.
pub fn spend_status_or_from_coins(
    status: SpendStatus,
    psbt: &Psbt,
    coins: &[Coin],
    desc: &LianaDescriptor,
    tip_height: i32,
) -> SpendStatus {
    match status {
        SpendStatus::Unknown => spend_status_from_coins(psbt, coins, desc, tip_height),
        status => status,
    }
}

impl SpendTx {
    pub fn new(
        updated_at: Option<u32>,
        psbt: Psbt,
        coins: Vec<Coin>,
        status: SpendStatus,
        desc: &LianaDescriptor,
        secp: &secp256k1::Secp256k1<impl secp256k1::Verification>,
        network: Network,
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

        let mut coins_map = HashMap::<OutPoint, Coin>::with_capacity(coins.len());
        for coin in coins {
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

        // A PSBT stored without sanity checks can panic here, see https://github.com/wizardsardine/liana/issues/2300
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
        }
    }

    /// Derive the status again from the current signatures and coins, for when this psbt changed
    /// without asking the daemon.
    pub fn refresh_status(&mut self, desc: &LianaDescriptor, tip_height: i32) {
        let coins: Vec<Coin> = self.coins.values().cloned().collect();
        self.status = spend_status_from_coins(&self.psbt, &coins, desc, tip_height);
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

    /// Amount the transaction moves: what it sends out, or the total of its outputs for a
    /// self-transfer, which sends nothing out.
    pub fn moved_amount(&self) -> Amount {
        if !self.is_send_to_self() {
            return self.spend_amount;
        }
        let mut moved = Amount::from_sat(0);
        for output in &self.psbt.unsigned_tx.output {
            moved += output.value;
        }
        moved
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

#[cfg(test)]
mod tests {
    use super::*;
    use liana::miniscript::bitcoin::{
        absolute, bip32::ChildNumber, transaction, ScriptBuf, Sequence, TxIn, Witness,
    };
    use lianad::commands::LCSpendInfo;
    use std::str::FromStr;

    fn dummy_txid() -> Txid {
        Txid::from_str("f7bd1b2a995b689d326e51eb742eb1088c4a8f110d9cb56128fd553acc9f88e5").unwrap()
    }

    fn dummy_coin(outpoint: OutPoint, spend_info: Option<LCSpendInfo>) -> Coin {
        Coin {
            outpoint,
            amount: Amount::from_sat(100_000),
            address: Address::from_str("bc1qvrl2849aggm6qry9ea7xqp2kk39j8vaa8r3cwg")
                .unwrap()
                .assume_checked(),
            derivation_index: ChildNumber::Normal { index: 0 },
            block_height: Some(1),
            is_immature: false,
            is_change: false,
            is_from_self: false,
            spend_info,
        }
    }

    fn dummy_desc() -> LianaDescriptor {
        LianaDescriptor::from_str("wsh(or_d(pk([f5acc2fd]tpubD6NzVbkrYhZ4YgUx2ZLNt2rLYAMTdYysCRzKoLu2BeSHKvzqPaBDvf17GeBPnExUVPkuBpx4kniP964e2MxyzzazcXLptxLXModSVCVEV1T/<0;1>/*),and_v(v:pkh([8a64f2a9]tpubD6NzVbkrYhZ4WmzFjvQrp7sDa4ECUxTi9oby8K4FZkd3XCBtEdKwUiQyYJaxiJo5y42gyDWEczrFpozEjeLxMPxjf2WtkfcbpUdfvNnozWF/<0;1>/*),older(10))))#d72le4dr").unwrap()
    }

    fn psbt_spending(outpoints: &[OutPoint]) -> Psbt {
        Psbt::from_unsigned_tx(Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: outpoints
                .iter()
                .map(|outpoint| TxIn {
                    previous_output: *outpoint,
                    script_sig: ScriptBuf::new(),
                    sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                    witness: Witness::new(),
                })
                .collect(),
            output: Vec::new(),
        })
        .unwrap()
    }

    #[test]
    fn spend_status_from_coins_missing_coin() {
        let first = OutPoint::new(dummy_txid(), 0);
        let second = OutPoint::new(dummy_txid(), 1);
        let unrelated = OutPoint::new(dummy_txid(), 2);
        let psbt = psbt_spending(&[first, second]);
        let desc = dummy_desc();

        assert_eq!(
            spend_status_from_coins(&psbt, &[dummy_coin(first, None)], &desc, 1),
            SpendStatus::Deprecated
        );
        // An unrelated coin does not stand in for the missing one.
        assert_eq!(
            spend_status_from_coins(
                &psbt,
                &[dummy_coin(first, None), dummy_coin(unrelated, None)],
                &desc,
                1
            ),
            SpendStatus::Deprecated
        );
    }

    #[test]
    fn spend_status_from_coins_unrelated_coin() {
        let input = OutPoint::new(dummy_txid(), 0);
        let unrelated = OutPoint::new(dummy_txid(), 1);
        let psbt = psbt_spending(&[input]);
        let txid = psbt.unsigned_tx.compute_txid();

        let coins = [
            dummy_coin(input, Some(LCSpendInfo { txid, height: None })),
            // Spent by another confirmed transaction: this would deprecate the psbt if the coin
            // was taken into account.
            dummy_coin(
                unrelated,
                Some(LCSpendInfo {
                    txid: dummy_txid(),
                    height: Some(1),
                }),
            ),
        ];
        assert_eq!(
            spend_status_from_coins(&psbt, &coins, &dummy_desc(), 1),
            SpendStatus::Broadcast
        );
    }
}
