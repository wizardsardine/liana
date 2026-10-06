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
    /// Tx label inherited from the payment
    Payment(String),
    /// Label of the transaction creating the item.
    Transaction(String),
    /// Label of the address receiving the payment.
    Address(String),
    /// Label passed on by the transactions funding the item.
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

pub fn resolve(own: Option<&str>, inherited: &Label) -> Label {
    own.filter(|label| !label.is_empty())
        .map(|label| Label::Own(label.to_string()))
        .unwrap_or_else(|| inherited.clone())
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

/// The default label of a payment: its address label if incoming, or the origin of its
/// transaction if it is a change or a send-to-self.
fn payment_default_label(
    kind: PaymentKind,
    address_label: Option<&str>,
    tx_origin: Option<&str>,
) -> Label {
    let label = match kind {
        PaymentKind::Incoming => address_label
            .filter(|label| !label.is_empty())
            .map(|label| Label::Address(label.to_string())),
        PaymentKind::SendToSelf => tx_origin.map(|origin| Label::Transaction(origin.to_string())),
        PaymentKind::Outgoing => None,
    };
    label.unwrap_or_default()
}

/// The txids of `txs`, each transaction placed after the transactions of `txs` funding it.
///
/// The default label of a send-to-self is the origin of the transactions funding it, so when both
/// are labelled in the same batch the funding ones must be labelled first. For example a rescan
/// finding an incoming payment and a send-to-self spending it must label the payment first, or
/// the send-to-self would find no origin and keep no default label.
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

/// The default labels of the `unlabelled_txs` and of their coins, and of the coins of the
/// `labelled` transactions listed in `unlabelled_coin_txids`.
///
/// `labelled` are the transactions already given a default label, among which the parents of
/// `unlabelled_txs`. `owned_coins` are our coins, with their amounts.
pub fn default_labels<'a>(
    unlabelled_txs: &HashMap<Txid, Transaction>,
    labelled: impl IntoIterator<Item = (&'a Transaction, &'a Label)>,
    unlabelled_coin_txids: &HashSet<Txid>,
    owned_coins: &HashMap<OutPoint, Amount>,
    labels: &HashMap<String, String>,
    network: Network,
) -> DefaultLabels {
    let mut origins = HashMap::new();
    let mut coins = HashMap::new();
    for (tx, default_label) in labelled {
        let txid = tx.compute_txid();
        let wallet_tx = wallet_transaction(tx, owned_coins);
        let origin = tx_origin(txid, &wallet_tx.kind(), labels, default_label);
        if unlabelled_coin_txids.contains(&txid) {
            coins.extend(coin_default_labels(
                tx,
                &wallet_tx,
                origin.as_deref(),
                network,
                labels,
            ));
        }
        origins.insert(txid, origin);
    }
    let mut txs = HashMap::with_capacity(unlabelled_txs.len());
    for txid in sort_parents_first(unlabelled_txs) {
        let tx = &unlabelled_txs[&txid];
        let wallet_tx = wallet_transaction(tx, owned_coins);
        let kind = wallet_tx.kind();
        let tx_label = tx_default_label(tx, &kind, network, labels, |parent| {
            origins.get(parent).cloned().flatten()
        });
        let origin = tx_origin(txid, &kind, labels, &tx_label);
        coins.extend(coin_default_labels(
            tx,
            &wallet_tx,
            origin.as_deref(),
            network,
            labels,
        ));
        origins.insert(txid, origin);
        txs.insert(txid, tx_label);
    }
    DefaultLabels { txs, coins }
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

/// The label of the single payment of an outgoing transaction spending only our coins.
fn single_payment_label<'l>(
    kind: &TransactionKind,
    labels: &'l HashMap<String, String>,
) -> Option<&'l str> {
    match kind {
        TransactionKind::Outgoing(_) => kind
            .single_payment()
            .and_then(|outpoint| get(labels, outpoint)),
        TransactionKind::Incoming(_)
        | TransactionKind::SendToSelf
        | TransactionKind::PayjoinReceive(_)
        | TransactionKind::PayjoinSend(_) => None,
    }
}

/// The origin every parent of `tx` passes on, `parent_origin` giving the one of a parent.
///
/// A send-to-self only inherits a label when all the coins it spends come from transactions
/// passing on the same text: moving two "salary" coins gives "from: salary", while consolidating a
/// "salary" coin with a "rent" coin, or with a coin whose parent has no origin, gives nothing.
fn common_parent_origin(
    tx: &Transaction,
    parent_origin: impl Fn(&Txid) -> Option<String>,
) -> Option<String> {
    let mut origins = tx
        .input
        .iter()
        .map(|input| parent_origin(&input.previous_output.txid));
    let first = origins.next()??;
    origins
        .all(|origin| origin.as_ref() == Some(&first))
        .then_some(first)
}

/// The text a transaction passes on to the transactions it funds.
fn tx_origin(
    txid: Txid,
    kind: &TransactionKind,
    labels: &HashMap<String, String>,
    default_label: &Label,
) -> Option<String> {
    get(labels, txid)
        .or_else(|| single_payment_label(kind, labels))
        .or_else(|| default_label.value())
        .map(str::to_string)
}

/// The default label of a transaction: the label of the address receiving its single incoming
/// payment, or the origin shared by the parents of a send-to-self.
fn tx_default_label(
    tx: &Transaction,
    kind: &TransactionKind,
    network: Network,
    labels: &HashMap<String, String>,
    parent_origin: impl Fn(&Txid) -> Option<String>,
) -> Label {
    let label = match kind {
        TransactionKind::Incoming(_) | TransactionKind::PayjoinReceive(_) => kind
            .single_payment()
            .and_then(|outpoint| output_address_label(tx, &outpoint, network, labels))
            .map(|label| Label::Address(label.to_string())),
        TransactionKind::SendToSelf => common_parent_origin(tx, parent_origin).map(Label::Funding),
        TransactionKind::Outgoing(_) | TransactionKind::PayjoinSend(_) => None,
    };
    label.unwrap_or_default()
}

fn coin_default_labels(
    tx: &Transaction,
    wallet_tx: &WalletTransaction,
    origin: Option<&str>,
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
                payment_default_label(kind, address_label, origin),
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
            coin_default_labels, default_labels, payment_default_label, payment_inherited_label,
            resolve, sort_parents_first, tx_label, tx_origin, wallet_transaction, Label,
        },
        transaction::{
            tests::{address, foreign_outpoint, transaction, OUTPUT_AMOUNT},
            PaymentKind, TransactionKind, WalletTransaction,
        },
    };

    const SALARY: &str = "salary";
    const RENT: &str = "rent";

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

        /// Snapshots the default label, the `funding_txs` being already seen.
        fn seen(mut self, labels: &HashMap<String, String>, funding_txs: &[&TestTx]) -> Self {
            let txid = self.txid();
            let coins: HashMap<OutPoint, Amount> = funding_txs
                .iter()
                .flat_map(|funding| funding.owned.clone())
                .chain(self.owned.clone())
                .collect();
            let mut default_labels = default_labels(
                &HashMap::from([(txid, self.tx.clone())]),
                funding_txs
                    .iter()
                    .map(|funding| (&funding.tx, &funding.default_label)),
                &HashSet::new(),
                &coins,
                labels,
                Network::Bitcoin,
            );
            self.default_label = default_labels.txs.remove(&txid).unwrap();
            self
        }

        fn tx_origin(&self, labels: &HashMap<String, String>) -> Option<String> {
            tx_origin(self.txid(), &self.kind(), labels, &self.default_label)
        }

        fn coin_default_labels(&self, labels: &HashMap<String, String>) -> Vec<(OutPoint, Label)> {
            let origin = self.tx_origin(labels);
            coin_default_labels(
                &self.tx,
                &self.wallet_tx(),
                origin.as_deref(),
                Network::Bitcoin,
                labels,
            )
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

    /// Moving `funding`'s first output to address 4.
    fn send_to_self(funding: &TestTx) -> TestTx {
        let input = funding.outpoint(0);
        TestTx::new(&[input], &[input], &[4], &[0])
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

    /// An incoming transaction on address `index`, seen with a txid label.
    fn labelled_incoming(index: u8, label: &str) -> TestTx {
        let tx = TestTx::new(&[foreign_outpoint(index)], &[], &[index], &[0]);
        let labels = labels(&[(tx.txid().to_string(), label)]);
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
    fn own_label_wins_over_the_inherited_one() {
        let inherited = Label::Address(RENT.to_string());
        assert_eq!(
            resolve(Some(SALARY), &inherited),
            Label::Own(SALARY.to_string())
        );
    }

    #[test]
    fn empty_own_label_is_ignored() {
        let inherited = Label::Address(RENT.to_string());
        assert_eq!(resolve(Some(""), &inherited), inherited);
        assert_eq!(resolve(None, &inherited), inherited);
        assert_eq!(resolve(None, &Label::None), Label::None);
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

    #[test]
    fn send_to_self_defaults_to_its_funding_label() {
        let funding = labelled_incoming(1, SALARY);
        let labels = labels(&[(funding.txid().to_string(), SALARY)]);
        let tx = send_to_self(&funding).seen(&labels, &[&funding]);
        assert_eq!(tx.default_label, Label::Funding(SALARY.to_string()));
    }

    #[test]
    fn send_to_self_without_funding_label_has_no_default() {
        let funding = incoming_single().seen(&HashMap::new(), &[]);
        let tx = send_to_self(&funding).seen(&HashMap::new(), &[&funding]);
        assert_eq!(tx.default_label, Label::None);
    }

    #[test]
    fn send_to_self_with_an_unseen_funding_has_no_default() {
        let funding = labelled_incoming(1, SALARY);
        let labels = labels(&[(funding.txid().to_string(), SALARY)]);
        let tx = send_to_self(&funding).seen(&labels, &[]);
        assert_eq!(tx.default_label, Label::None);
    }

    #[test]
    fn funding_from_an_address_label() {
        let labels = labels(&[(address(1).to_string(), SALARY)]);
        let funding = incoming_single().seen(&labels, &[]);
        let tx = send_to_self(&funding).seen(&labels, &[&funding]);
        assert_eq!(tx.default_label, Label::Funding(SALARY.to_string()));
    }

    #[test]
    fn funding_from_an_outgoing_payment_label() {
        let outgoing = outgoing_single(&incoming_single());
        let labels = labels(&[(outgoing.outpoint(0).to_string(), RENT)]);
        let outgoing = outgoing.seen(&labels, &[]);
        // The change of `outgoing` is its second output.
        let change = outgoing.outpoint(1);
        let tx = TestTx::new(&[change], &[change], &[4], &[0]).seen(&labels, &[&outgoing]);
        assert_eq!(tx.default_label, Label::Funding(RENT.to_string()));
    }

    #[test]
    fn funding_chain_passes_the_root_label_on() {
        let salary = labelled_incoming(1, SALARY);
        let labels = labels(&[(salary.txid().to_string(), SALARY)]);
        let first = send_to_self(&salary).seen(&labels, &[&salary]);
        let second = send_to_self(&first).seen(&labels, &[&first]);
        let third = send_to_self(&second).seen(&labels, &[&second]);
        assert_eq!(third.default_label, Label::Funding(SALARY.to_string()));
    }

    #[test]
    fn funding_chain_stops_at_an_own_label() {
        let salary = labelled_incoming(1, SALARY);
        let first = send_to_self(&salary);
        let labels = labels(&[
            (salary.txid().to_string(), SALARY),
            (first.txid().to_string(), RENT),
        ]);
        let first = first.seen(&labels, &[&salary]);
        let second = send_to_self(&first).seen(&labels, &[&first]);
        assert_eq!(second.default_label, Label::Funding(RENT.to_string()));
    }

    #[test]
    fn funding_chain_breaks_on_an_unlabelled_transaction() {
        let unlabelled = incoming_single().seen(&HashMap::new(), &[]);
        let first = send_to_self(&unlabelled).seen(&HashMap::new(), &[&unlabelled]);
        let second = send_to_self(&first).seen(&HashMap::new(), &[&first]);
        assert_eq!(second.default_label, Label::None);
    }

    #[test]
    fn funding_agreeing_inputs() {
        let first = labelled_incoming(1, SALARY);
        let second = labelled_incoming(2, SALARY);
        let labels = labels(&[
            (first.txid().to_string(), SALARY),
            (second.txid().to_string(), SALARY),
        ]);
        let tx = consolidation(&[&first, &second], &[0, 1]).seen(&labels, &[&first, &second]);
        assert_eq!(tx.default_label, Label::Funding(SALARY.to_string()));
    }

    #[test]
    fn funding_disagreeing_inputs() {
        let first = labelled_incoming(1, SALARY);
        let second = labelled_incoming(2, RENT);
        let labels = labels(&[
            (first.txid().to_string(), SALARY),
            (second.txid().to_string(), RENT),
        ]);
        let tx = consolidation(&[&first, &second], &[0, 1]).seen(&labels, &[&first, &second]);
        assert_eq!(tx.default_label, Label::None);
    }

    #[test]
    fn funding_with_one_unlabelled_input() {
        let first = labelled_incoming(1, SALARY);
        let second =
            TestTx::new(&[foreign_outpoint(2)], &[], &[2], &[0]).seen(&HashMap::new(), &[]);
        let labels = labels(&[(first.txid().to_string(), SALARY)]);
        let tx = consolidation(&[&first, &second], &[0, 1]).seen(&labels, &[&first, &second]);
        assert_eq!(tx.default_label, Label::None);
    }

    #[test]
    fn funding_with_a_foreign_input() {
        let first = labelled_incoming(1, SALARY);
        // A stranger's transaction funding the foreign input.
        let second = TestTx::new(&[foreign_outpoint(2)], &[], &[2], &[]);
        let labels = labels(&[
            (first.txid().to_string(), SALARY),
            (second.txid().to_string(), SALARY),
        ]);
        let second = second.seen(&labels, &[]);
        let tx = consolidation(&[&first, &second], &[0]).seen(&labels, &[&first, &second]);
        assert!(matches!(tx.kind(), TransactionKind::PayjoinSend(_)));
        assert_eq!(tx.default_label, Label::None);
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

    #[test]
    fn origin_own_label_wins_over_the_default() {
        let tx = incoming_single();
        let labels = labels(&[
            (address(1).to_string(), RENT),
            (tx.txid().to_string(), SALARY),
        ]);
        let tx = tx.seen(&labels, &[]);
        assert_eq!(tx.default_label, Label::Address(RENT.to_string()));
        assert_eq!(tx.tx_origin(&labels), Some(SALARY.to_string()));
    }

    #[test]
    fn origin_ignores_an_empty_own_label() {
        let tx = incoming_single();
        let labels = labels(&[
            (address(1).to_string(), SALARY),
            (tx.txid().to_string(), ""),
        ]);
        let tx = tx.seen(&labels, &[]);
        assert_eq!(tx.tx_origin(&labels), Some(SALARY.to_string()));
    }

    #[test]
    fn origin_falls_back_to_the_default() {
        let funding = labelled_incoming(1, SALARY);
        let tx = send_to_self(&funding);
        let snapshot = labels(&[(funding.txid().to_string(), SALARY)]);
        let tx = tx.seen(&snapshot, &[&funding]);
        // The default is a snapshot: it outlives the funding label.
        assert_eq!(tx.tx_origin(&HashMap::new()), Some(SALARY.to_string()));
    }

    #[test]
    fn origin_of_an_unlabelled_transaction() {
        let tx = incoming_single().seen(&HashMap::new(), &[]);
        assert_eq!(tx.tx_origin(&HashMap::new()), None);
    }

    #[test]
    fn origin_of_an_outgoing_single_is_its_payment_label() {
        let tx = outgoing_single(&incoming_single());
        let labels = labels(&[(tx.outpoint(0).to_string(), RENT)]);
        let tx = tx.seen(&labels, &[]);
        assert_eq!(tx.tx_origin(&labels), Some(RENT.to_string()));
    }

    #[test]
    fn origin_of_an_outgoing_single_ignores_the_change_label() {
        let tx = outgoing_single(&incoming_single());
        let labels = labels(&[(tx.outpoint(1).to_string(), RENT)]);
        let tx = tx.seen(&labels, &[]);
        assert_eq!(tx.tx_origin(&labels), None);
    }

    #[test]
    fn origin_of_a_payjoin_send() {
        let tx = payjoin_send(&incoming_single());
        let labels = labels(&[(tx.outpoint(0).to_string(), RENT)]);
        let tx = tx.seen(&labels, &[]);
        assert_eq!(tx.tx_origin(&labels), None);
    }

    #[test]
    fn origin_of_an_outgoing_batch() {
        let tx = outgoing_batch(&incoming_single());
        let labels = labels(&[
            (tx.outpoint(0).to_string(), RENT),
            (tx.outpoint(1).to_string(), RENT),
        ]);
        let tx = tx.seen(&labels, &[]);
        assert_eq!(tx.tx_origin(&labels), None);
    }

    #[test]
    fn incoming_payment_defaults_to_its_address_label() {
        assert_eq!(
            payment_default_label(PaymentKind::Incoming, Some(SALARY), None),
            Label::Address(SALARY.to_string())
        );
        assert_eq!(
            payment_default_label(PaymentKind::Incoming, Some(""), None),
            Label::None
        );
        assert_eq!(
            payment_default_label(PaymentKind::Incoming, None, None),
            Label::None
        );
        // The transaction origin is not inherited.
        assert_eq!(
            payment_default_label(PaymentKind::Incoming, None, Some(RENT)),
            Label::None
        );
    }

    #[test]
    fn send_to_self_payment_defaults_to_the_tx_origin() {
        assert_eq!(
            payment_default_label(PaymentKind::SendToSelf, None, Some(RENT)),
            Label::Transaction(RENT.to_string())
        );
        assert_eq!(
            payment_default_label(PaymentKind::SendToSelf, Some(SALARY), None),
            Label::None
        );
    }

    #[test]
    fn outgoing_payment_has_no_default() {
        assert_eq!(
            payment_default_label(PaymentKind::Outgoing, Some(SALARY), Some(RENT)),
            Label::None
        );
    }

    #[test]
    fn incoming_coins_default_to_their_address_label() {
        let tx = incoming_batch();
        let labels = labels(&[(address(2).to_string(), SALARY)]);
        let tx = tx.seen(&labels, &[]);
        assert_eq!(
            tx.coin_default_labels(&labels),
            vec![
                (tx.outpoint(0), Label::None),
                (tx.outpoint(1), Label::Address(SALARY.to_string())),
            ]
        );
    }

    #[test]
    fn change_coin_defaults_to_the_tx_origin() {
        let tx = outgoing_single(&incoming_single());
        let labels = labels(&[(tx.txid().to_string(), RENT)]);
        let tx = tx.seen(&labels, &[]);
        assert_eq!(
            tx.coin_default_labels(&labels),
            vec![(tx.outpoint(1), Label::Transaction(RENT.to_string()))]
        );
    }
}
