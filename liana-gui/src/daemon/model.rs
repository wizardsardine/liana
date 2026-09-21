use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};

use liana::{
    descriptors::LianaDescriptor,
    label::{self, Label},
    transaction::{PaymentKind, WalletTransaction},
};
pub use liana::{
    descriptors::{LianaPolicy, PartialSpendInfo, PathSpendInfo},
    miniscript::bitcoin::{
        bip32::{DerivationPath, Fingerprint},
        psbt::Psbt,
        secp256k1, Address, Amount, Network, OutPoint, Transaction, Txid,
    },
    spend::SpendStatus,
    transaction::TransactionKind,
};
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
    pub wallet_tx: WalletTransaction,
    /// Maximum possible size of the unsigned transaction after satisfaction
    /// (assuming all inputs are for the same descriptor).
    pub max_vbytes: u64,
    pub status: SpendStatus,
    pub sigs: PartialSpendInfo,
    pub updated_at: Option<u32>,
}

/// Status of a spend transaction as it can be told from the coins it spends and its signatures,
/// for the transactions the daemon did not give us a status for.
pub fn spend_status_from_coins(
    psbt: &Psbt,
    coins: &[Coin],
    desc: &LianaDescriptor,
    tip_height: i32,
) -> SpendStatus {
    let txid = psbt.unsigned_tx.compute_txid();
    let mut status = None;
    let mut coins_heights = Vec::with_capacity(psbt.unsigned_tx.input.len());
    for txin in &psbt.unsigned_tx.input {
        let Some(coin) = coins
            .iter()
            .find(|coin| coin.outpoint == txin.previous_output)
        else {
            return SpendStatus::Deprecated;
        };

        coins_heights.push(coin.block_height);
        if let Some(info) = &coin.spend_info {
            if info.txid == txid {
                status = Some(if info.height.is_some() {
                    SpendStatus::Confirmed
                } else {
                    SpendStatus::Broadcast
                });
            } else if info.height.is_some() {
                return SpendStatus::Deprecated;
            }
        }
    }

    if let Some(status) = status {
        return status;
    }
    // Whether this psbt can replace an unconfirmed transaction spending one of its coins depends
    // on the fees of the mempool entries it would evict, which we cannot see from the coins alone.
    // Stay optimistic and let the node decide at broadcast time.
    desc.partial_spend_info(psbt)
        .map_or(SpendStatus::Unknown, |sigs| {
            SpendStatus::from_signatures(&sigs, &coins_heights, tip_height)
        })
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
        desc: &LianaDescriptor,
        secp: &secp256k1::Secp256k1<impl secp256k1::Verification>,
        network: Network,
    ) -> Self {
        let status = spend_status_from_coins(&psbt, &coins, desc, 0);
        Self::new_with_status(updated_at, psbt, coins, status, desc, secp, network)
    }

    pub fn new_with_status(
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

        let mut coins_map = HashMap::<OutPoint, Coin>::with_capacity(coins.len());
        for coin in coins {
            coins_map.insert(coin.outpoint, coin);
        }

        let owned_inputs: Vec<liana::transaction::Coin> = psbt
            .unsigned_tx
            .input
            .iter()
            .zip(&psbt.inputs)
            .filter_map(|(txin, input)| {
                let amount = input
                    .witness_utxo
                    .as_ref()
                    .map(|utxo| utxo.value)
                    .or_else(|| coins_map.get(&txin.previous_output).map(|coin| coin.amount))?;
                Some(liana::transaction::Coin {
                    outpoint: txin.previous_output,
                    amount,
                })
            })
            .collect();
        let txid = psbt.unsigned_tx.compute_txid();
        let owned_outputs: Vec<OutPoint> = change_indexes
            .iter()
            .map(|index| OutPoint::new(txid, *index as u32))
            .collect();
        let wallet_tx = WalletTransaction::new(&psbt.unsigned_tx, &owned_inputs, &owned_outputs);

        // A PSBT stored without sanity checks can panic here, see https://github.com/wizardsardine/liana/issues/2300
        let sigs = desc
            .partial_spend_info(&psbt)
            .expect("PSBT must be generated by Liana");

        Self {
            labels: HashMap::new(),
            updated_at,
            coins: coins_map,
            psbt,
            change_indexes,
            wallet_tx,
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
        self.wallet_tx.fee().map(|a| {
            a.to_sat()
                .checked_div(self.max_vbytes)
                .expect("a descriptor's satisfaction size is never 0")
        })
    }

    pub fn is_send_to_self(&self) -> bool {
        self.wallet_tx.kind().is_send_to_self()
    }

    /// Amount the transaction moves: what it sends out, or the total of its outputs for a
    /// self-transfer, which sends nothing out.
    pub fn moved_amount(&self) -> Amount {
        if !self.is_send_to_self() {
            return self.wallet_tx.amount();
        }
        let mut moved = Amount::from_sat(0);
        for output in &self.psbt.unsigned_tx.output {
            moved += output.value;
        }
        moved
    }

    pub fn single_payment(&self) -> Option<OutPoint> {
        self.wallet_tx.kind().single_payment()
    }

    pub fn is_batch(&self) -> bool {
        self.wallet_tx.kind().is_batch()
    }

    pub fn label(&self) -> Label {
        label::tx_label(
            self.psbt.unsigned_tx.compute_txid(),
            &self.wallet_tx.kind(),
            &self.labels,
            &Label::None,
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
    pub owned_outputs: BTreeMap<usize, Label>,
    pub tx: Transaction,
    pub txid: Txid,
    pub wallet_tx: WalletTransaction,
    pub height: Option<i32>,
    pub time: Option<u32>,
    pub default_label: Label,
}

impl HistoryTransaction {
    pub fn new(
        tx: Transaction,
        height: Option<i32>,
        time: Option<u32>,
        coins: Vec<Coin>,
        owned_outputs: BTreeMap<usize, Label>,
        network: Network,
        default_label: Label,
    ) -> Self {
        let mut coins_map = HashMap::<OutPoint, Coin>::with_capacity(coins.len());
        for coin in coins {
            coins_map.insert(coin.outpoint, coin);
        }

        let owned_inputs: Vec<liana::transaction::Coin> = coins_map
            .values()
            .map(liana::transaction::Coin::from)
            .collect();
        let txid = tx.compute_txid();
        let owned_outpoints: Vec<OutPoint> = owned_outputs
            .keys()
            .map(|index| OutPoint::new(txid, *index as u32))
            .collect();
        let wallet_tx = WalletTransaction::new(&tx, &owned_inputs, &owned_outpoints);

        Self {
            labels: HashMap::new(),
            txid,
            tx,
            coins: coins_map,
            owned_outputs,
            wallet_tx,
            height,
            time,
            network,
            default_label,
        }
    }

    /// Positions in `tx.output` of the outputs that are ours, not derivation indexes.
    pub fn owned_output_indexes(&self) -> Vec<usize> {
        self.owned_outputs.keys().copied().collect()
    }

    /// Feerate in sats/vbyte, `None` if the fee is unknown.
    pub fn feerate(&self) -> Option<u64> {
        self.wallet_tx
            .fee()
            .map(|fee| fee.to_sat() / self.tx.vsize() as u64)
    }

    /// The block time, `None` if unconfirmed.
    pub fn datetime(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.time.map(|t| {
            chrono::DateTime::<chrono::Utc>::from_timestamp(t as i64, 0)
                .expect("Correct unix timestamp")
        })
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

    pub fn is_incoming(&self) -> bool {
        self.wallet_tx.is_incoming()
    }

    pub fn is_send_to_self(&self) -> bool {
        self.wallet_tx.kind().is_send_to_self()
    }

    pub fn single_payment(&self) -> Option<OutPoint> {
        self.wallet_tx.kind().single_payment()
    }

    pub fn is_batch(&self) -> bool {
        self.wallet_tx.kind().is_batch()
    }

    pub fn is_payjoin(&self) -> bool {
        self.wallet_tx.kind().is_payjoin()
    }

    pub fn label(&self) -> Label {
        label::tx_label(
            self.txid,
            &self.wallet_tx.kind(),
            &self.labels,
            &self.default_label,
        )
    }
}

#[derive(Debug, Clone)]
pub struct Payment {
    pub label: Option<String>,
    pub address: Option<String>,
    pub default_label: Label,
    pub amount: Amount,
    pub outpoint: OutPoint,
    pub time: Option<chrono::DateTime<chrono::Utc>>,
    pub kind: PaymentKind,
}

impl Payment {
    pub fn label(&self) -> Label {
        self.label
            .as_deref()
            .filter(|label| !label.is_empty())
            .map(|label| Label::Own(label.to_string()))
            .unwrap_or_else(|| self.default_label.clone())
    }

    pub fn from_tx_output(history_tx: &HistoryTransaction, output_index: usize) -> Option<Self> {
        let output = history_tx.tx.output.get(output_index)?;
        let outpoint = OutPoint::new(history_tx.txid, output_index as u32);
        let kind = history_tx.wallet_tx.payment_kind(&outpoint)?;
        let label = history_tx.labels.get(&outpoint.to_string()).cloned();
        let address = Address::from_script(&output.script_pubkey, history_tx.network)
            .ok()
            .map(|addr| addr.to_string());
        let default_label = label::payment_inherited_label(
            kind,
            label::get(&history_tx.labels, history_tx.txid),
            &history_tx
                .owned_outputs
                .get(&output_index)
                .cloned()
                .unwrap_or_default(),
        );
        Some(Payment {
            label,
            address,
            default_label,
            outpoint,
            time: history_tx.datetime(),
            amount: output.value,
            kind,
        })
    }

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
        if let Some(label) = new_labels.get(&self.outpoint.to_string()) {
            self.label = label.clone();
        }
    }
}

pub fn payments_from_tx(history_tx: HistoryTransaction) -> Vec<Payment> {
    (0..history_tx.tx.output.len())
        .filter_map(|output_index| Payment::from_tx_output(&history_tx, output_index))
        .collect()
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
    use liana::{
        label::Label,
        miniscript::bitcoin::{
            absolute, bip32::ChildNumber, transaction, ScriptBuf, Sequence, TxIn, TxOut, Witness,
        },
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
            default_label: Label::None,
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

    const SALARY: &str = "salary";
    const RENT: &str = "rent";

    fn address(index: u8) -> Address {
        Address::p2wsh(&ScriptBuf::from_bytes(vec![index]), Network::Bitcoin)
    }

    fn outpoint(index: u8) -> OutPoint {
        OutPoint::new(Txid::from_str(&format!("{index:0>64x}")).unwrap(), 0)
    }

    /// A history transaction spending `inputs`, of which `owned_inputs` are ours, to one output
    /// per address index, of which the ones in `owned_outputs` are ours.
    fn history_tx(
        inputs: &[OutPoint],
        owned_inputs: &[OutPoint],
        outputs: &[u8],
        owned_outputs: BTreeMap<usize, Label>,
        default_label: Label,
    ) -> HistoryTransaction {
        let tx = Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: inputs
                .iter()
                .map(|outpoint| TxIn {
                    previous_output: *outpoint,
                    script_sig: ScriptBuf::new(),
                    sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                    witness: Witness::new(),
                })
                .collect(),
            output: outputs
                .iter()
                .map(|index| TxOut {
                    value: Amount::from_sat(10_000),
                    script_pubkey: address(*index).script_pubkey(),
                })
                .collect(),
        };
        HistoryTransaction::new(
            tx,
            Some(1),
            Some(1),
            owned_inputs
                .iter()
                .map(|outpoint| dummy_coin(*outpoint, None))
                .collect(),
            owned_outputs,
            Network::Bitcoin,
            default_label,
        )
    }

    /// Paying us on address 1, and a stranger on address 9.
    fn incoming_single(default_label: Label) -> HistoryTransaction {
        let owned_outputs = BTreeMap::from([(0, Label::None)]);
        history_tx(&[outpoint(1)], &[], &[1, 9], owned_outputs, default_label)
    }

    /// Paying us on addresses 1 and 2.
    fn incoming_batch() -> HistoryTransaction {
        let owned_outputs = BTreeMap::from([(0, Label::None), (1, Label::None)]);
        history_tx(&[outpoint(1)], &[], &[1, 2], owned_outputs, Label::None)
    }

    /// Spending our coin to a stranger on address 9, with change on address 3.
    fn outgoing_single(change_label: Label) -> HistoryTransaction {
        let owned_outputs = BTreeMap::from([(1, change_label)]);
        history_tx(
            &[outpoint(1)],
            &[outpoint(1)],
            &[9, 3],
            owned_outputs,
            Label::None,
        )
    }

    /// Spending our coin to strangers on addresses 8 and 9.
    fn outgoing_batch() -> HistoryTransaction {
        history_tx(
            &[outpoint(1)],
            &[outpoint(1)],
            &[8, 9],
            BTreeMap::new(),
            Label::None,
        )
    }

    /// Moving our coin to address 4.
    fn send_to_self(default_label: Label) -> HistoryTransaction {
        let owned_outputs = BTreeMap::from([(0, Label::None)]);
        history_tx(
            &[outpoint(1)],
            &[outpoint(1)],
            &[4],
            owned_outputs,
            default_label,
        )
    }

    /// Spending our coin and the receiver's coin to the receiver on address 9, with change on
    /// address 3.
    fn payjoin_send() -> HistoryTransaction {
        let owned_outputs = BTreeMap::from([(1, Label::None)]);
        let inputs = [outpoint(1), outpoint(2)];
        history_tx(&inputs, &[outpoint(1)], &[9, 3], owned_outputs, Label::None)
    }

    fn labelled(mut tx: HistoryTransaction, labels: &[(String, &str)]) -> HistoryTransaction {
        tx.labels = labels
            .iter()
            .map(|(item, label)| (item.clone(), label.to_string()))
            .collect();
        tx
    }

    fn payment(label: Option<&str>, default_label: Label) -> Payment {
        Payment {
            label: label.map(str::to_string),
            address: None,
            default_label,
            amount: Amount::from_sat(10_000),
            outpoint: outpoint(1),
            time: None,
            kind: PaymentKind::Incoming,
        }
    }

    /// Labels edited by the user, as received after an update.
    fn edited(key: String, label: Option<&str>) -> HashMap<String, Option<String>> {
        HashMap::from([(key, label.map(str::to_string))])
    }

    #[test]
    fn tx_label_falls_back_to_its_default() {
        assert_eq!(incoming_single(Label::None).label(), Label::None);
        assert_eq!(
            incoming_single(Label::Address(SALARY.to_string())).label(),
            Label::Address(SALARY.to_string())
        );
        assert_eq!(
            send_to_self(Label::Funding(SALARY.to_string())).label(),
            Label::Funding(SALARY.to_string())
        );
    }

    #[test]
    fn tx_own_label_wins_over_the_default() {
        let tx = incoming_single(Label::Address(RENT.to_string()));
        let tx = labelled(tx.clone(), &[(tx.txid.to_string(), SALARY)]);
        assert_eq!(tx.label(), Label::Own(SALARY.to_string()));
    }

    #[test]
    fn tx_empty_own_label_is_ignored() {
        let tx = incoming_single(Label::None);
        let tx = labelled(tx.clone(), &[(tx.txid.to_string(), "")]);
        assert_eq!(tx.label(), Label::None);

        let tx = incoming_single(Label::Address(SALARY.to_string()));
        let tx = labelled(tx.clone(), &[(tx.txid.to_string(), "")]);
        assert_eq!(tx.label(), Label::Address(SALARY.to_string()));
    }

    #[test]
    fn tx_ignores_the_address_and_foreign_output_labels() {
        let tx = incoming_single(Label::None);
        // The receiving address only counts through the default.
        let foreign_output = OutPoint::new(tx.txid, 1);
        let tx = labelled(
            tx,
            &[
                (address(1).to_string(), SALARY),
                (foreign_output.to_string(), SALARY),
            ],
        );
        assert_eq!(tx.label(), Label::None);
    }

    #[test]
    fn tx_outgoing_single_shows_the_payment_label() {
        let tx = outgoing_single(Label::None);
        let tx = labelled(tx.clone(), &[(OutPoint::new(tx.txid, 0).to_string(), RENT)]);
        assert_eq!(tx.label(), Label::Payment(RENT.to_string()));
    }

    #[test]
    fn tx_outgoing_single_ignores_the_change_label() {
        let tx = outgoing_single(Label::None);
        let tx = labelled(tx.clone(), &[(OutPoint::new(tx.txid, 1).to_string(), RENT)]);
        assert_eq!(tx.label(), Label::None);
    }

    #[test]
    fn tx_payjoin_send_shows_the_payment_label() {
        let tx = payjoin_send();
        let tx = labelled(tx.clone(), &[(OutPoint::new(tx.txid, 0).to_string(), RENT)]);
        assert_eq!(tx.label(), Label::Payment(RENT.to_string()));
    }

    #[test]
    fn tx_batches_show_no_payment_label() {
        for tx in [incoming_batch(), outgoing_batch()] {
            let labels = [
                (OutPoint::new(tx.txid, 0).to_string(), SALARY),
                (OutPoint::new(tx.txid, 1).to_string(), RENT),
            ];
            assert_eq!(labelled(tx, &labels).label(), Label::None);
        }
    }

    #[test]
    fn tx_own_label_edit_takes_over_the_default() {
        let mut tx = send_to_self(Label::Funding(SALARY.to_string()));
        let txid = tx.txid.to_string();
        tx.load_labels(&edited(txid.clone(), Some(RENT)));
        assert_eq!(tx.label(), Label::Own(RENT.to_string()));
        tx.load_labels(&edited(txid, None));
        assert_eq!(tx.label(), Label::Funding(SALARY.to_string()));
    }

    #[test]
    fn payment_label_falls_back_to_its_default() {
        assert_eq!(payment(None, Label::None).label(), Label::None);
        assert_eq!(
            payment(None, Label::Address(SALARY.to_string())).label(),
            Label::Address(SALARY.to_string())
        );
        assert_eq!(
            payment(None, Label::Transaction(RENT.to_string())).label(),
            Label::Transaction(RENT.to_string())
        );
    }

    #[test]
    fn payment_own_label_wins_over_the_default() {
        assert_eq!(
            payment(Some(SALARY), Label::Address(RENT.to_string())).label(),
            Label::Own(SALARY.to_string())
        );
    }

    #[test]
    fn payment_empty_own_label_is_ignored() {
        assert_eq!(payment(Some(""), Label::None).label(), Label::None);
        assert_eq!(
            payment(Some(""), Label::Address(SALARY.to_string())).label(),
            Label::Address(SALARY.to_string())
        );
    }

    #[test]
    fn outgoing_payment_falls_back_to_the_tx_label() {
        let tx = outgoing_single(Label::None);
        let payment = Payment::from_tx_output(&tx, 0).unwrap();
        assert_eq!(payment.label(), Label::None);

        let tx = labelled(tx.clone(), &[(tx.txid.to_string(), RENT)]);
        let payment = Payment::from_tx_output(&tx, 0).unwrap();
        assert_eq!(payment.label(), Label::Transaction(RENT.to_string()));

        let outpoint = OutPoint::new(tx.txid, 0).to_string();
        let tx = labelled(
            tx.clone(),
            &[(tx.txid.to_string(), RENT), (outpoint, SALARY)],
        );
        let payment = Payment::from_tx_output(&tx, 0).unwrap();
        assert_eq!(payment.label(), Label::Own(SALARY.to_string()));
    }

    #[test]
    fn outgoing_change_ignores_the_tx_label() {
        let tx = outgoing_single(Label::Transaction(SALARY.to_string()));
        let tx = labelled(tx.clone(), &[(tx.txid.to_string(), RENT)]);
        let change = Payment::from_tx_output(&tx, 1).unwrap();
        assert_eq!(change.label(), Label::Transaction(SALARY.to_string()));
    }

    #[test]
    fn payment_own_label_edit_takes_over_the_default() {
        let tx = outgoing_single(Label::Transaction(RENT.to_string()));
        let mut change = Payment::from_tx_output(&tx, 1).unwrap();
        let outpoint = change.outpoint.to_string();
        change.load_labels(&edited(outpoint.clone(), Some(SALARY)));
        assert_eq!(change.label(), Label::Own(SALARY.to_string()));
        change.load_labels(&edited(outpoint, None));
        assert_eq!(change.label(), Label::Transaction(RENT.to_string()));
    }
}
