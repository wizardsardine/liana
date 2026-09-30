use std::{
    collections::{HashMap, HashSet},
    fmt,
    str::FromStr,
};

use miniscript::bitcoin::{Address, Amount, Network, OutPoint, Transaction, Txid};
use serde::{Deserialize, Serialize};

use crate::transaction::{Coin, PaymentKind, TransactionKind, WalletTransaction};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LabelItem {
    Address(Address),
    Txid(Txid),
    OutPoint(OutPoint),
}

impl From<Address> for LabelItem {
    fn from(value: Address) -> Self {
        Self::Address(value)
    }
}

impl From<Txid> for LabelItem {
    fn from(value: Txid) -> Self {
        Self::Txid(value)
    }
}

impl From<OutPoint> for LabelItem {
    fn from(value: OutPoint) -> Self {
        Self::OutPoint(value)
    }
}

impl fmt::Display for LabelItem {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            LabelItem::Address(a) => write!(f, "{a}"),
            LabelItem::Txid(a) => write!(f, "{a}"),
            LabelItem::OutPoint(a) => write!(f, "{a}"),
        }
    }
}

impl LabelItem {
    pub fn from_str(s: &str, network: Network) -> Option<LabelItem> {
        if let Ok(addr) = Address::from_str(s) {
            if !addr.is_valid_for_network(network) {
                None
            } else {
                Some(LabelItem::Address(addr.assume_checked()))
            }
        } else if let Ok(txid) = Txid::from_str(s) {
            Some(LabelItem::Txid(txid))
        } else if let Ok(outpoint) = OutPoint::from_str(s) {
            Some(LabelItem::OutPoint(outpoint))
        } else {
            None
        }
    }

    pub fn from_bip329(label: &bip329::Label, network: Network) -> Option<(Self, String)> {
        match label {
            bip329::Label::Transaction(tx_record) => {
                if let (Some(txid), Some(label)) = (
                    Txid::from_str(&tx_record.ref_.to_string()).ok(),
                    tx_record.label.clone(),
                ) {
                    Some((Self::Txid(txid), label))
                } else {
                    None
                }
            }
            bip329::Label::Address(address_record) => {
                if let (Some(addr), Some(label)) = (
                    Address::from_str(&address_record.ref_.clone().assume_checked().to_string())
                        .ok(),
                    address_record.label.clone(),
                ) {
                    if addr.is_valid_for_network(network) {
                        Some((Self::Address(addr.assume_checked()), label))
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            bip329::Label::Output(output_record) => {
                if let (Some(outpoint), Some(label)) = (
                    OutPoint::from_str(&output_record.ref_.to_string()).ok(),
                    output_record.label.clone(),
                ) {
                    Some((Self::OutPoint(outpoint), label))
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}

/// A label to display: the item's own one, or its default label.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Label {
    #[default]
    None,
    Own(String),
    /// Label of the single payment of a transaction, inherited by the transaction and its change.
    Payment(String),
    /// Label of the transaction creating the item.
    Transaction(String),
    /// Label of the address receiving the payment.
    Address(String),
    /// Label displayed on the coins funding the item.
    Funding(String),
}

/// The kind of an inherited label, telling how to display it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelPrefix {
    Payment,
    Transaction,
    Address,
    Funding,
}

impl Label {
    /// The text to display, `prefixed` giving the one of an inherited label.
    pub fn text(&self, prefixed: impl Fn(LabelPrefix, &str) -> String) -> Option<String> {
        match self {
            Label::None => None,
            Label::Own(label) => Some(label.clone()),
            Label::Payment(label) => Some(prefixed(LabelPrefix::Payment, label)),
            Label::Transaction(label) => Some(prefixed(LabelPrefix::Transaction, label)),
            Label::Address(label) => Some(prefixed(LabelPrefix::Address, label)),
            Label::Funding(label) => Some(prefixed(LabelPrefix::Funding, label)),
        }
    }

    pub fn value(&self) -> Option<&str> {
        match self {
            Label::None => None,
            Label::Own(label)
            | Label::Payment(label)
            | Label::Transaction(label)
            | Label::Address(label)
            | Label::Funding(label) => Some(label),
        }
    }

    /// The item's own label, the one editing it starts from.
    pub fn own(&self) -> Option<&str> {
        match self {
            Label::Own(label) => Some(label),
            Label::None
            | Label::Payment(_)
            | Label::Transaction(_)
            | Label::Address(_)
            | Label::Funding(_) => None,
        }
    }
}

/// The non-empty label of `item`.
pub fn get(labels: &HashMap<String, String>, item: impl Into<LabelItem>) -> Option<&str> {
    labels
        .get(&item.into().to_string())
        .map(String::as_str)
        .filter(|label| !label.is_empty())
}

/// The label to display for a transaction: its own one, else the label of its single payment,
/// else its default label.
pub fn tx_label(
    txid: Txid,
    kind: &TransactionKind,
    labels: &HashMap<String, String>,
    default_label: &Label,
) -> Label {
    if let Some(label) = get(labels, txid) {
        Label::Own(label.to_string())
    } else if let Some(pm) = kind.single_payment().and_then(|op| get(labels, op)) {
        Label::Payment(pm.to_string())
    } else {
        default_label.clone()
    }
}

/// The label a payment shows until it gets its own: the label of its transaction if it is
/// outgoing, else its stored default label.
pub fn payment_inherited_label(
    kind: PaymentKind,
    tx_label: Option<&str>,
    default_label: &Label,
) -> Label {
    if let Some(tx_label) =
        tx_label.filter(|label| kind == PaymentKind::Outgoing && !label.is_empty())
    {
        Label::Transaction(tx_label.to_string())
    } else {
        default_label.clone()
    }
}

fn output_address_label<'a>(
    tx: &Transaction,
    outpoint: &OutPoint,
    network: Network,
    labels: &'a HashMap<String, String>,
) -> Option<&'a str> {
    let output = tx.output.get(outpoint.vout as usize)?;
    let address = Address::from_script(&output.script_pubkey, network).ok()?;
    get(labels, address)
}

/// The default label of a payment: its address label if incoming, or `to_self_label` if it is a
/// change or a send-to-self.
fn payment_default_label(
    kind: PaymentKind,
    address_label: Option<&str>,
    to_self_label: &Label,
) -> Label {
    let label = match kind {
        PaymentKind::Incoming => address_label
            .filter(|label| !label.is_empty())
            .map(|label| Label::Address(label.to_string())),
        PaymentKind::SendToSelf => Some(to_self_label.clone()),
        PaymentKind::Outgoing => None,
    };
    label.unwrap_or_default()
}

/// The txids of `txs`, each transaction placed after the transactions of `txs` funding it.
///
/// The default label of a send-to-self comes from the coins it spends, so when both are labelled
/// in the same batch the funding ones must be labelled first. For example a rescan finding an
/// incoming payment and a send-to-self spending it must label the payment first, or the
/// send-to-self would find no funding label and keep no default label.
fn sort_parents_first(txs: &HashMap<Txid, Transaction>) -> Vec<Txid> {
    let mut order = Vec::with_capacity(txs.len());
    let mut visited = HashSet::new();
    for txid in txs.keys() {
        // Each entry is a txid and whether the txs it spends were already pushed.
        let mut stack = vec![(*txid, false)];
        while let Some((txid, funding_pushed)) = stack.pop() {
            if funding_pushed {
                order.push(txid);
            } else if visited.insert(txid) {
                stack.push((txid, true));
                stack.extend(
                    txs[&txid]
                        .input
                        .iter()
                        .map(|input| input.previous_output.txid)
                        .filter(|funding| txs.contains_key(funding) && !visited.contains(funding))
                        .map(|funding| (funding, false)),
                );
            }
        }
    }
    order
}

#[derive(Debug, Default)]
pub struct DefaultLabels {
    pub txs: HashMap<Txid, Label>,
    pub coins: HashMap<OutPoint, Label>,
}

/// The default labels of the coins of `txs`, and of the transactions not in `labelled_txids`.
///
/// `labelled_txids` are the transactions whose default label is already stored. `owned_coins` are
/// our coins, with their amounts. `funding_defaults` are the stored default labels of our coins,
/// among which the ones `txs` spend.
pub fn default_labels(
    txs: &HashMap<Txid, Transaction>,
    labelled_txids: &HashSet<Txid>,
    owned_coins: &HashMap<OutPoint, Amount>,
    funding_defaults: &HashMap<OutPoint, Label>,
    labels: &HashMap<String, String>,
    network: Network,
) -> DefaultLabels {
    let mut tx_labels = HashMap::with_capacity(txs.len());
    let mut coins = HashMap::new();
    for txid in sort_parents_first(txs) {
        let tx = &txs[&txid];
        let wallet_tx = wallet_transaction(tx, owned_coins);
        let kind = wallet_tx.kind();
        let funding_label = common_funding_label(tx, labels, |outpoint| {
            coins
                .get(outpoint)
                .or_else(|| funding_defaults.get(outpoint))
        });
        let to_self_label = to_self_default_label(txid, &kind, labels, funding_label.as_deref());
        coins.extend(coin_default_labels(
            tx,
            &wallet_tx,
            &to_self_label,
            network,
            labels,
        ));
        if !labelled_txids.contains(&txid) {
            let tx_label = tx_default_label(tx, &kind, network, labels);
            tx_labels.insert(txid, tx_label);
        }
    }
    DefaultLabels {
        txs: tx_labels,
        coins,
    }
}

fn wallet_transaction(
    tx: &Transaction,
    owned_coins: &HashMap<OutPoint, Amount>,
) -> WalletTransaction {
    let owned_inputs: Vec<Coin> = tx
        .input
        .iter()
        .filter_map(|input| {
            owned_coins.get(&input.previous_output).map(|amount| Coin {
                outpoint: input.previous_output,
                amount: *amount,
            })
        })
        .collect();
    let txid = tx.compute_txid();
    let owned_outputs: Vec<OutPoint> = (0..tx.output.len())
        .map(|vout| OutPoint::new(txid, vout as u32))
        .filter(|outpoint| owned_coins.contains_key(outpoint))
        .collect();
    WalletTransaction::new(tx, &owned_inputs, &owned_outputs)
}

/// The label displayed on all the coins `tx` spends, `funding_default` giving the default label
/// of a coin.
///
/// Moving two "salary" coins gives "salary", while consolidating a "salary" coin with a "rent"
/// coin, or with an unlabelled coin, gives nothing.
fn common_funding_label<'l>(
    tx: &Transaction,
    labels: &HashMap<String, String>,
    funding_default: impl Fn(&OutPoint) -> Option<&'l Label>,
) -> Option<String> {
    let mut funding_labels = tx.input.iter().map(|input| {
        let outpoint = input.previous_output;
        get(labels, outpoint).or_else(|| funding_default(&outpoint)?.value())
    });
    let first = funding_labels.next()??;
    funding_labels
        .all(|label| label == Some(first))
        .then(|| first.to_string())
}

/// The default label of the coins a transaction sends to ourselves: the funding label of a
/// send-to-self, the label of the single payment of an outgoing transaction, or the own label of
/// an outgoing batch or a payjoin send.
fn to_self_default_label(
    txid: Txid,
    kind: &TransactionKind,
    labels: &HashMap<String, String>,
    funding_label: Option<&str>,
) -> Label {
    let tx_label = || get(labels, txid).map(|label| Label::Transaction(label.to_string()));
    match (kind, kind.single_payment()) {
        (TransactionKind::SendToSelf, _) => {
            funding_label.map(|label| Label::Funding(label.to_string()))
        }
        (TransactionKind::Outgoing(_), Some(payment)) => {
            get(labels, payment).map(|label| Label::Payment(label.to_string()))
        }
        (TransactionKind::Outgoing(_), None) | (TransactionKind::PayjoinSend(_), _) => tx_label(),
        (TransactionKind::Incoming(_) | TransactionKind::PayjoinReceive(_), _) => None,
    }
    .unwrap_or_default()
}

/// The default label of a transaction: the label of the address receiving its single incoming
/// payment. A send-to-self has none, only its coins inherit the funding label.
fn tx_default_label(
    tx: &Transaction,
    kind: &TransactionKind,
    network: Network,
    labels: &HashMap<String, String>,
) -> Label {
    let label = match kind {
        TransactionKind::Incoming(_) | TransactionKind::PayjoinReceive(_) => kind
            .single_payment()
            .and_then(|outpoint| output_address_label(tx, &outpoint, network, labels))
            .map(|label| Label::Address(label.to_string())),
        TransactionKind::SendToSelf
        | TransactionKind::Outgoing(_)
        | TransactionKind::PayjoinSend(_) => None,
    };
    label.unwrap_or_default()
}

fn coin_default_labels(
    tx: &Transaction,
    wallet_tx: &WalletTransaction,
    to_self_label: &Label,
    network: Network,
    labels: &HashMap<String, String>,
) -> Vec<(OutPoint, Label)> {
    wallet_tx
        .owned_outputs
        .iter()
        .filter_map(|coin| {
            let kind = wallet_tx.payment_kind(&coin.outpoint)?;
            let address_label = output_address_label(tx, &coin.outpoint, network, labels);
            Some((
                coin.outpoint,
                payment_default_label(kind, address_label, to_self_label),
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use miniscript::bitcoin::{Amount, Network, OutPoint, Transaction, Txid};

    use crate::{
        label::{
            default_labels, payment_default_label, payment_inherited_label, sort_parents_first,
            tx_label, wallet_transaction, Label,
        },
        transaction::{
            tests::{address, foreign_outpoint, transaction, OUTPUT_AMOUNT},
            PaymentKind, TransactionKind, WalletTransaction,
        },
    };

    const SALARY: &str = "salary";
    const RENT: &str = "rent";
    const GIFT: &str = "gift";

    fn labels(entries: &[(String, &str)]) -> HashMap<String, String> {
        entries
            .iter()
            .map(|(item, label)| (item.clone(), label.to_string()))
            .collect()
    }

    struct TestTx {
        tx: Transaction,
        owned: HashMap<OutPoint, Amount>,
        default_label: Label,
        coin_defaults: HashMap<OutPoint, Label>,
    }

    impl TestTx {
        /// Spending `inputs`, of which `owned_inputs` are ours and worth `OUTPUT_AMOUNT`, to one
        /// output per address index, of which the ones at `owned_output_indexes` are ours.
        fn new(
            inputs: &[OutPoint],
            owned_inputs: &[OutPoint],
            outputs: &[u8],
            owned_output_indexes: &[u32],
        ) -> Self {
            let tx = transaction(inputs, outputs);
            let txid = tx.compute_txid();
            let owned = owned_inputs
                .iter()
                .copied()
                .chain(
                    owned_output_indexes
                        .iter()
                        .map(|vout| OutPoint::new(txid, *vout)),
                )
                .map(|outpoint| (outpoint, OUTPUT_AMOUNT))
                .collect();
            Self {
                tx,
                owned,
                default_label: Label::None,
                coin_defaults: HashMap::new(),
            }
        }

        fn wallet_tx(&self) -> WalletTransaction {
            wallet_transaction(&self.tx, &self.owned)
        }

        fn kind(&self) -> TransactionKind {
            self.wallet_tx().kind()
        }

        fn txid(&self) -> Txid {
            self.tx.compute_txid()
        }

        fn outpoint(&self, vout: u32) -> OutPoint {
            OutPoint::new(self.txid(), vout)
        }

        /// Snapshots the default labels, the coins of `funding_txs` being already seen.
        fn seen(mut self, labels: &HashMap<String, String>, funding_txs: &[&TestTx]) -> Self {
            let txid = self.txid();
            let funding_defaults: HashMap<OutPoint, Label> = funding_txs
                .iter()
                .flat_map(|funding| funding.coin_defaults.clone())
                .collect();
            let mut default_labels = default_labels(
                &HashMap::from([(txid, self.tx.clone())]),
                &HashSet::new(),
                &self.owned,
                &funding_defaults,
                labels,
                Network::Bitcoin,
            );
            self.default_label = default_labels.txs.remove(&txid).unwrap();
            self.coin_defaults = default_labels.coins;
            self
        }

        fn coin_default(&self, vout: u32) -> Label {
            self.coin_defaults[&self.outpoint(vout)].clone()
        }
    }

    /// Paying us on address 1, and a stranger on address 9.
    fn incoming_single() -> TestTx {
        TestTx::new(&[foreign_outpoint(1)], &[], &[1, 9], &[0])
    }

    /// Paying us on addresses 1 and 2.
    fn incoming_batch() -> TestTx {
        TestTx::new(&[foreign_outpoint(1)], &[], &[1, 2], &[0, 1])
    }

    /// Spending `funding`'s first output to a stranger on address 9, with change on address 3.
    fn outgoing_single(funding: &TestTx) -> TestTx {
        let input = funding.outpoint(0);
        TestTx::new(&[input], &[input], &[9, 3], &[1])
    }

    /// Spending `funding`'s first output to strangers on addresses 8 and 9.
    fn outgoing_batch(funding: &TestTx) -> TestTx {
        let input = funding.outpoint(0);
        TestTx::new(&[input], &[input], &[8, 9], &[])
    }

    /// Spending `funding`'s first output to strangers on addresses 8 and 9, with change on address
    /// 3.
    fn outgoing_batch_with_change(funding: &TestTx) -> TestTx {
        let input = funding.outpoint(0);
        TestTx::new(&[input], &[input], &[8, 9, 3], &[2])
    }

    /// Moving `coin` to address 4.
    fn move_coin(coin: OutPoint) -> TestTx {
        TestTx::new(&[coin], &[coin], &[4], &[0])
    }

    /// Moving `funding`'s first output to address 4.
    fn send_to_self(funding: &TestTx) -> TestTx {
        move_coin(funding.outpoint(0))
    }

    /// Spending our first output of `funding` and the receiver's coin, to the receiver on address
    /// 9, with change on address 3.
    fn payjoin_send(funding: &TestTx) -> TestTx {
        let owned = funding.outpoint(0);
        TestTx::new(&[owned, foreign_outpoint(2)], &[owned], &[9, 3], &[1])
    }

    /// Spending a coin of ours worth half an output and the sender's coin, to us on address 1,
    /// with the sender's change on address 9.
    fn payjoin_receive() -> TestTx {
        let owned = foreign_outpoint(1);
        let mut tx = TestTx::new(&[owned, foreign_outpoint(2)], &[owned], &[1, 9], &[0]);
        tx.owned.insert(owned, OUTPUT_AMOUNT / 2);
        tx
    }

    /// Spending the first outputs of `funding_txs`, of which the ones at `owned` are ours, to
    /// address 4.
    fn consolidation(funding_txs: &[&TestTx], owned: &[usize]) -> TestTx {
        let inputs: Vec<OutPoint> = funding_txs.iter().map(|tx| tx.outpoint(0)).collect();
        let owned_inputs: Vec<OutPoint> = owned.iter().map(|i| inputs[*i]).collect();
        TestTx::new(&inputs, &owned_inputs, &[4], &[0])
    }

    /// An incoming transaction on address `index`, seen with an address label.
    fn labelled_incoming(index: u8, label: &str) -> TestTx {
        let tx = TestTx::new(&[foreign_outpoint(index)], &[], &[index], &[0]);
        let labels = labels(&[(address(index).to_string(), label)]);
        tx.seen(&labels, &[])
    }

    #[test]
    fn wallet_tx_kinds() {
        let funding = incoming_single();
        assert_eq!(
            funding.kind(),
            TransactionKind::Incoming(vec![funding.outpoint(0)])
        );
        let tx = outgoing_single(&funding);
        assert_eq!(tx.kind(), TransactionKind::Outgoing(vec![tx.outpoint(0)]));
        let tx = outgoing_batch_with_change(&funding);
        assert_eq!(
            tx.kind(),
            TransactionKind::Outgoing(vec![tx.outpoint(0), tx.outpoint(1)])
        );
        let tx = payjoin_send(&funding);
        assert_eq!(
            tx.kind(),
            TransactionKind::PayjoinSend(vec![tx.outpoint(0)])
        );
        let tx = payjoin_receive();
        assert_eq!(
            tx.kind(),
            TransactionKind::PayjoinReceive(vec![tx.outpoint(0)])
        );
        assert_eq!(send_to_self(&funding).kind(), TransactionKind::SendToSelf);
    }

    #[test]
    fn label_values() {
        assert_eq!(Label::None.value(), None);
        assert_eq!(Label::Own(SALARY.to_string()).value(), Some(SALARY));
        assert_eq!(Label::Payment(SALARY.to_string()).value(), Some(SALARY));
        assert_eq!(Label::Transaction(SALARY.to_string()).value(), Some(SALARY));
        assert_eq!(Label::Address(SALARY.to_string()).value(), Some(SALARY));
    }

    #[test]
    fn label_own_values() {
        assert_eq!(Label::None.own(), None);
        assert_eq!(Label::Own(SALARY.to_string()).own(), Some(SALARY));
        assert_eq!(Label::Payment(SALARY.to_string()).own(), None);
        assert_eq!(Label::Transaction(SALARY.to_string()).own(), None);
        assert_eq!(Label::Address(SALARY.to_string()).own(), None);
    }

    #[test]
    fn tx_label_prefers_its_own_label() {
        let tx = incoming_single();
        let default_label = Label::Address(RENT.to_string());
        let labels = labels(&[
            (tx.txid().to_string(), SALARY),
            (tx.outpoint(0).to_string(), RENT),
        ]);
        assert_eq!(
            tx_label(tx.txid(), &tx.kind(), &labels, &default_label),
            Label::Own(SALARY.to_string())
        );
    }

    #[test]
    fn tx_label_falls_back_to_its_single_payment_label() {
        let funding = labelled_incoming(1, SALARY);
        let tx = outgoing_single(&funding);
        let labels = labels(&[
            (tx.txid().to_string(), ""),
            (tx.outpoint(0).to_string(), RENT),
        ]);
        assert_eq!(
            tx_label(tx.txid(), &tx.kind(), &labels, &Label::None),
            Label::Payment(RENT.to_string())
        );
    }

    #[test]
    fn tx_label_falls_back_to_its_default_label() {
        let tx = incoming_batch();
        let default_label = Label::Address(SALARY.to_string());
        let labels = labels(&[
            (tx.outpoint(0).to_string(), RENT),
            (tx.outpoint(1).to_string(), RENT),
        ]);
        assert_eq!(
            tx_label(tx.txid(), &tx.kind(), &labels, &default_label),
            default_label
        );
        assert_eq!(
            tx_label(tx.txid(), &tx.kind(), &HashMap::new(), &Label::None),
            Label::None
        );
    }

    #[test]
    fn outgoing_payment_inherits_its_tx_label() {
        assert_eq!(
            payment_inherited_label(PaymentKind::Outgoing, Some(RENT), &Label::None),
            Label::Transaction(RENT.to_string())
        );
        assert_eq!(
            payment_inherited_label(PaymentKind::Outgoing, Some(""), &Label::None),
            Label::None
        );
        assert_eq!(
            payment_inherited_label(PaymentKind::Outgoing, None, &Label::None),
            Label::None
        );
    }

    #[test]
    fn other_payments_ignore_the_tx_label() {
        let default_label = Label::Address(SALARY.to_string());
        assert_eq!(
            payment_inherited_label(PaymentKind::Incoming, Some(RENT), &default_label),
            default_label
        );
        let default_label = Label::Transaction(SALARY.to_string());
        assert_eq!(
            payment_inherited_label(PaymentKind::SendToSelf, Some(RENT), &default_label),
            default_label
        );
    }

    #[test]
    fn incoming_single_defaults_to_its_address_label() {
        let labels = labels(&[(address(1).to_string(), SALARY)]);
        let tx = incoming_single().seen(&labels, &[]);
        assert_eq!(tx.default_label, Label::Address(SALARY.to_string()));
    }

    #[test]
    fn incoming_single_ignores_an_empty_address_label() {
        let labels = labels(&[(address(1).to_string(), "")]);
        let tx = incoming_single().seen(&labels, &[]);
        assert_eq!(tx.default_label, Label::None);
    }

    #[test]
    fn incoming_single_ignores_other_labels() {
        let tx = incoming_single();
        assert_eq!(tx.seen(&HashMap::new(), &[]).default_label, Label::None);

        let tx = incoming_single();
        // The own label, the stranger's address and the received outpoint are not defaults.
        let labels = labels(&[
            (tx.txid().to_string(), SALARY),
            (address(9).to_string(), SALARY),
            (tx.outpoint(0).to_string(), SALARY),
        ]);
        assert_eq!(tx.seen(&labels, &[]).default_label, Label::None);
    }

    #[test]
    fn payjoin_receive_defaults_to_its_address_label() {
        let labels = labels(&[(address(1).to_string(), SALARY)]);
        let tx = payjoin_receive().seen(&labels, &[]);
        assert_eq!(tx.default_label, Label::Address(SALARY.to_string()));
    }

    #[test]
    fn incoming_batch_has_no_default() {
        let labels = labels(&[
            (address(1).to_string(), SALARY),
            (address(2).to_string(), SALARY),
        ]);
        let tx = incoming_batch().seen(&labels, &[]);
        assert_eq!(tx.default_label, Label::None);
    }

    #[test]
    fn outgoing_has_no_default() {
        let funding = labelled_incoming(1, SALARY);

        let single = outgoing_single(&funding);
        let labels = labels(&[
            (funding.txid().to_string(), SALARY),
            (single.outpoint(0).to_string(), RENT),
            (address(9).to_string(), RENT),
        ]);
        let single = single.seen(&labels, &[&funding]);
        assert_eq!(single.default_label, Label::None);

        let batch = outgoing_batch(&funding).seen(&labels, &[&funding]);
        assert_eq!(batch.default_label, Label::None);
    }

    fn unlabelled_txs(txs: &[&Transaction]) -> HashMap<Txid, Transaction> {
        txs.iter()
            .map(|tx| (tx.compute_txid(), (*tx).clone()))
            .collect()
    }

    #[test]
    fn sort_parents_first_orders_a_chain() {
        let a = transaction(&[foreign_outpoint(1)], &[1]);
        let b = transaction(&[OutPoint::new(a.compute_txid(), 0)], &[2]);
        let c = transaction(&[OutPoint::new(b.compute_txid(), 0)], &[3]);
        assert_eq!(
            sort_parents_first(&unlabelled_txs(&[&c, &a, &b])),
            vec![a.compute_txid(), b.compute_txid(), c.compute_txid()]
        );
    }

    #[test]
    fn sort_parents_first_orders_a_diamond() {
        let a = transaction(&[foreign_outpoint(1)], &[1, 2]);
        let b = transaction(&[OutPoint::new(a.compute_txid(), 0)], &[3]);
        let c = transaction(&[OutPoint::new(a.compute_txid(), 1)], &[4]);
        let d = transaction(
            &[
                OutPoint::new(b.compute_txid(), 0),
                OutPoint::new(c.compute_txid(), 0),
            ],
            &[5],
        );
        let order = sort_parents_first(&unlabelled_txs(&[&d, &c, &b, &a]));
        assert_eq!(order.len(), 4);
        assert_eq!(order[0], a.compute_txid());
        assert_eq!(order[3], d.compute_txid());
        assert!(order[1..3].contains(&b.compute_txid()));
        assert!(order[1..3].contains(&c.compute_txid()));
    }

    fn funding_label(label: &str) -> Label {
        Label::Funding(label.to_string())
    }

    fn transaction_label(label: &str) -> Label {
        Label::Transaction(label.to_string())
    }

    #[test]
    fn send_to_self_passes_the_funding_label_to_its_coin_only() {
        let funding = labelled_incoming(1, SALARY);
        assert_eq!(funding.coin_default(0), Label::Address(SALARY.to_string()));
        let tx = send_to_self(&funding).seen(&HashMap::new(), &[&funding]);
        assert_eq!(tx.default_label, Label::None);
        assert_eq!(tx.coin_default(0), funding_label(SALARY));
    }

    #[test]
    fn send_to_self_without_funding_label_has_no_default() {
        let funding = incoming_single().seen(&HashMap::new(), &[]);
        let tx = send_to_self(&funding).seen(&HashMap::new(), &[&funding]);
        assert_eq!(tx.default_label, Label::None);
        assert_eq!(tx.coin_default(0), Label::None);
    }

    #[test]
    fn send_to_self_with_an_unseen_funding_has_no_default() {
        let funding = labelled_incoming(1, SALARY);
        let tx = send_to_self(&funding).seen(&HashMap::new(), &[]);
        assert_eq!(tx.default_label, Label::None);
        assert_eq!(tx.coin_default(0), Label::None);
    }

    #[test]
    fn send_to_self_ignores_its_own_tx_label() {
        let funding = labelled_incoming(1, SALARY);
        let tx = send_to_self(&funding);
        let labels = labels(&[(tx.txid().to_string(), RENT)]);
        let tx = tx.seen(&labels, &[&funding]);
        assert_eq!(tx.default_label, Label::None);
        assert_eq!(tx.coin_default(0), funding_label(SALARY));
    }

    #[test]
    fn funding_from_an_own_coin_label() {
        let funding = incoming_single().seen(&HashMap::new(), &[]);
        let labels = labels(&[(funding.outpoint(0).to_string(), RENT)]);
        let tx = send_to_self(&funding).seen(&labels, &[&funding]);
        assert_eq!(tx.default_label, Label::None);
        assert_eq!(tx.coin_default(0), funding_label(RENT));
    }

    #[test]
    fn funding_from_an_address_label_over_the_tx_label() {
        let funding = incoming_single();
        let labels = labels(&[
            (funding.txid().to_string(), RENT),
            (address(1).to_string(), SALARY),
        ]);
        let funding = funding.seen(&labels, &[]);
        let tx = send_to_self(&funding).seen(&labels, &[&funding]);
        assert_eq!(tx.default_label, Label::None);
        assert_eq!(tx.coin_default(0), funding_label(SALARY));
    }

    #[test]
    fn funding_from_one_coin_of_an_incoming_batch() {
        let labels = labels(&[
            (address(1).to_string(), SALARY),
            (address(2).to_string(), RENT),
        ]);
        let batch = incoming_batch().seen(&labels, &[]);
        assert_eq!(batch.default_label, Label::None);
        let tx = move_coin(batch.outpoint(1)).seen(&labels, &[&batch]);
        assert_eq!(tx.default_label, Label::None);
        assert_eq!(tx.coin_default(0), funding_label(RENT));
    }

    #[test]
    fn funding_from_an_outgoing_change() {
        let outgoing = outgoing_single(&incoming_single());
        let labels = labels(&[(outgoing.outpoint(0).to_string(), RENT)]);
        let outgoing = outgoing.seen(&labels, &[]);
        // The change of `outgoing` is its second output.
        assert_eq!(outgoing.coin_default(1), Label::Payment(RENT.to_string()));
        let tx = move_coin(outgoing.outpoint(1)).seen(&labels, &[&outgoing]);
        assert_eq!(tx.default_label, Label::None);
        assert_eq!(tx.coin_default(0), funding_label(RENT));
    }

    #[test]
    fn funding_agreeing_inputs() {
        let first = labelled_incoming(1, SALARY);
        let second = labelled_incoming(2, SALARY);
        let tx =
            consolidation(&[&first, &second], &[0, 1]).seen(&HashMap::new(), &[&first, &second]);
        assert_eq!(tx.default_label, Label::None);
        assert_eq!(tx.coin_default(0), funding_label(SALARY));
    }

    #[test]
    fn funding_disagreeing_inputs() {
        let first = labelled_incoming(1, SALARY);
        let second = labelled_incoming(2, RENT);
        let tx =
            consolidation(&[&first, &second], &[0, 1]).seen(&HashMap::new(), &[&first, &second]);
        assert_eq!(tx.default_label, Label::None);
        assert_eq!(tx.coin_default(0), Label::None);
    }

    #[test]
    fn funding_with_one_unlabelled_input() {
        let first = labelled_incoming(1, SALARY);
        let second =
            TestTx::new(&[foreign_outpoint(2)], &[], &[2], &[0]).seen(&HashMap::new(), &[]);
        let tx =
            consolidation(&[&first, &second], &[0, 1]).seen(&HashMap::new(), &[&first, &second]);
        assert_eq!(tx.default_label, Label::None);
        assert_eq!(tx.coin_default(0), Label::None);
    }

    #[test]
    fn funding_with_a_foreign_input() {
        let first = labelled_incoming(1, SALARY);
        // A stranger's transaction funding the foreign input.
        let second = TestTx::new(&[foreign_outpoint(2)], &[], &[2], &[]);
        let labels = labels(&[(second.outpoint(0).to_string(), SALARY)]);
        let tx = consolidation(&[&first, &second], &[0]).seen(&labels, &[&first]);
        assert!(matches!(tx.kind(), TransactionKind::PayjoinSend(_)));
        assert_eq!(tx.default_label, Label::None);
        assert_eq!(tx.coin_default(0), Label::None);
    }

    #[test]
    fn change_defaults_to_the_payment_label() {
        let tx = outgoing_single(&incoming_single());
        let labels = labels(&[
            (tx.txid().to_string(), SALARY),
            (tx.outpoint(0).to_string(), RENT),
        ]);
        let tx = tx.seen(&labels, &[]);
        assert_eq!(tx.coin_default(1), Label::Payment(RENT.to_string()));
    }

    #[test]
    fn change_ignores_the_tx_label() {
        let tx = outgoing_single(&incoming_single());
        let labels = labels(&[(tx.txid().to_string(), SALARY)]);
        let tx = tx.seen(&labels, &[]);
        assert_eq!(tx.coin_default(1), Label::None);
    }

    #[test]
    fn change_without_labels_has_no_default() {
        let tx = outgoing_single(&incoming_single()).seen(&HashMap::new(), &[]);
        assert_eq!(tx.coin_default(1), Label::None);
    }

    #[test]
    fn change_ignores_the_funding_label() {
        let funding = labelled_incoming(1, SALARY);
        let tx = outgoing_single(&funding).seen(&HashMap::new(), &[&funding]);
        assert_eq!(tx.coin_default(1), Label::None);
    }

    #[test]
    fn batch_change_defaults_to_the_tx_label() {
        let tx = outgoing_batch_with_change(&incoming_single());
        let labels = labels(&[
            (tx.txid().to_string(), SALARY),
            (tx.outpoint(0).to_string(), RENT),
        ]);
        let tx = tx.seen(&labels, &[]);
        assert_eq!(tx.default_label, Label::None);
        assert_eq!(tx.coin_default(2), transaction_label(SALARY));
    }

    #[test]
    fn batch_change_ignores_the_payment_labels() {
        let tx = outgoing_batch_with_change(&incoming_single());
        let labels = labels(&[
            (tx.outpoint(0).to_string(), RENT),
            (tx.outpoint(1).to_string(), RENT),
        ]);
        let tx = tx.seen(&labels, &[]);
        assert_eq!(tx.coin_default(2), Label::None);
    }

    #[test]
    fn payjoin_send_change_defaults_to_the_tx_label() {
        let tx = payjoin_send(&incoming_single());
        let labels = labels(&[
            (tx.txid().to_string(), SALARY),
            (tx.outpoint(0).to_string(), RENT),
        ]);
        let tx = tx.seen(&labels, &[]);
        assert_eq!(tx.coin_default(1), transaction_label(SALARY));
    }

    #[test]
    fn chain_passes_the_root_label_on() {
        let salary = labelled_incoming(1, SALARY);
        let first = send_to_self(&salary).seen(&HashMap::new(), &[&salary]);
        let second = send_to_self(&first).seen(&HashMap::new(), &[&first]);
        let third = send_to_self(&second).seen(&HashMap::new(), &[&second]);
        for tx in [&first, &second, &third] {
            assert_eq!(tx.default_label, Label::None);
            assert_eq!(tx.coin_default(0), funding_label(SALARY));
        }
    }

    #[test]
    fn chain_follows_an_own_coin_label() {
        let salary = labelled_incoming(1, SALARY);
        let first = send_to_self(&salary);
        let second = send_to_self(&first);
        // Labelling a send-to-self transaction changes nothing.
        let labels = labels(&[
            (first.outpoint(0).to_string(), RENT),
            (second.txid().to_string(), GIFT),
        ]);
        let first = first.seen(&labels, &[&salary]);
        let second = second.seen(&labels, &[&first]);
        let third = send_to_self(&second).seen(&labels, &[&second]);

        assert_eq!(first.default_label, Label::None);
        assert_eq!(first.coin_default(0), funding_label(SALARY));
        for tx in [&second, &third] {
            assert_eq!(tx.default_label, Label::None);
            assert_eq!(tx.coin_default(0), funding_label(RENT));
        }
    }

    #[test]
    fn chain_breaks_on_a_disagreeing_consolidation() {
        let salary = labelled_incoming(1, SALARY);
        let first = send_to_self(&salary).seen(&HashMap::new(), &[&salary]);
        let rent = labelled_incoming(2, RENT);
        let merged =
            consolidation(&[&first, &rent], &[0, 1]).seen(&HashMap::new(), &[&first, &rent]);
        assert_eq!(merged.default_label, Label::None);
        assert_eq!(merged.coin_default(0), Label::None);

        let child = send_to_self(&merged).seen(&HashMap::new(), &[&merged]);
        assert_eq!(child.default_label, Label::None);
        assert_eq!(child.coin_default(0), Label::None);
    }

    #[test]
    fn chain_keeps_the_snapshot_after_a_label_edit() {
        let salary = labelled_incoming(1, SALARY);
        let first = send_to_self(&salary).seen(&HashMap::new(), &[&salary]);
        let second = send_to_self(&first).seen(&HashMap::new(), &[&first]);

        // The root address label changes after the snapshots.
        let edited = labels(&[(address(1).to_string(), RENT)]);
        let third = send_to_self(&second).seen(&edited, &[&second]);
        assert_eq!(third.default_label, Label::None);
        assert_eq!(third.coin_default(0), funding_label(SALARY));

        // Only an own label of the last coin passes the new text on.
        let fourth = send_to_self(&third);
        let edited = labels(&[
            (address(1).to_string(), RENT),
            (third.outpoint(0).to_string(), GIFT),
        ]);
        let fourth = fourth.seen(&edited, &[&third]);
        assert_eq!(fourth.default_label, Label::None);
        assert_eq!(fourth.coin_default(0), funding_label(GIFT));
    }

    #[test]
    fn one_pass_labels_the_parents_first() {
        let incoming = incoming_single();
        let to_self = send_to_self(&incoming);
        let txs = HashMap::from([
            (to_self.txid(), to_self.tx.clone()),
            (incoming.txid(), incoming.tx.clone()),
        ]);
        let owned: HashMap<OutPoint, Amount> = incoming
            .owned
            .clone()
            .into_iter()
            .chain(to_self.owned.clone())
            .collect();
        let labels = labels(&[(address(1).to_string(), SALARY)]);
        let default_labels = default_labels(
            &txs,
            &HashSet::new(),
            &owned,
            &HashMap::new(),
            &labels,
            Network::Bitcoin,
        );
        assert_eq!(
            default_labels.txs,
            HashMap::from([
                (incoming.txid(), Label::Address(SALARY.to_string())),
                (to_self.txid(), Label::None),
            ])
        );
        assert_eq!(
            default_labels.coins,
            HashMap::from([
                (incoming.outpoint(0), Label::Address(SALARY.to_string())),
                (to_self.outpoint(0), funding_label(SALARY)),
            ])
        );
    }

    #[test]
    fn labelled_tx_coins_get_their_funding_label() {
        let salary = labelled_incoming(1, SALARY);
        let to_self = send_to_self(&salary);
        let default_labels = default_labels(
            &HashMap::from([(to_self.txid(), to_self.tx.clone())]),
            &HashSet::from([to_self.txid()]),
            &to_self.owned,
            &salary.coin_defaults,
            &HashMap::new(),
            Network::Bitcoin,
        );
        assert!(default_labels.txs.is_empty());
        assert_eq!(
            default_labels.coins,
            HashMap::from([(to_self.outpoint(0), funding_label(SALARY))])
        );
    }

    #[test]
    fn incoming_payment_defaults_to_its_address_label() {
        assert_eq!(
            payment_default_label(PaymentKind::Incoming, Some(SALARY), &Label::None),
            Label::Address(SALARY.to_string())
        );
        assert_eq!(
            payment_default_label(PaymentKind::Incoming, Some(""), &Label::None),
            Label::None
        );
        assert_eq!(
            payment_default_label(PaymentKind::Incoming, None, &Label::None),
            Label::None
        );
        // The label of the coins sent to ourselves is not inherited.
        assert_eq!(
            payment_default_label(PaymentKind::Incoming, None, &funding_label(RENT)),
            Label::None
        );
    }

    #[test]
    fn send_to_self_payment_defaults_to_the_to_self_label() {
        assert_eq!(
            payment_default_label(PaymentKind::SendToSelf, None, &funding_label(RENT)),
            funding_label(RENT)
        );
        assert_eq!(
            payment_default_label(PaymentKind::SendToSelf, Some(SALARY), &Label::None),
            Label::None
        );
    }

    #[test]
    fn outgoing_payment_has_no_default() {
        assert_eq!(
            payment_default_label(PaymentKind::Outgoing, Some(SALARY), &funding_label(RENT)),
            Label::None
        );
    }

    #[test]
    fn incoming_coins_default_to_their_address_label() {
        let labels = labels(&[(address(2).to_string(), SALARY)]);
        let tx = incoming_batch().seen(&labels, &[]);
        assert_eq!(tx.coin_default(0), Label::None);
        assert_eq!(tx.coin_default(1), Label::Address(SALARY.to_string()));
    }
}
