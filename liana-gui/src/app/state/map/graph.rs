use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashMap, HashSet, VecDeque},
};

use liana::{
    label::{self, Label},
    miniscript::bitcoin::{Address, Amount, OutPoint, SignedAmount, Txid},
    transaction::PaymentKind,
};
use liana_ui::widget::graph_view::{ItemId, Side};
use lianad::commands::GraphItem;

use crate::{
    app::state::map::wallets::WalletKey,
    daemon::model::{Coin, HistoryTransaction, LabelsLoader, Payment, TransactionKind},
};

/// The transactions and coins of a wallet drawn on the map.
#[derive(Debug)]
pub struct WalletTxs {
    pub key: WalletKey,
    /// Descriptor checksum: a transaction belongs to the wallet whose checksum sorts first.
    pub checksum: String,
    pub txs: Vec<HistoryTransaction>,
    pub coins: Vec<Coin>,
}

/// The histories of a transaction, one per wallet having it, primary wallet first.
type Histories = Vec<(WalletKey, HistoryTransaction)>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SlotRef {
    pub tx: usize,
    pub side: Side,
    /// Index in the true transaction order.
    pub index: usize,
}

#[derive(Debug, Clone)]
pub enum InputSlot {
    OurCoin {
        wallet: WalletKey,
        outpoint: OutPoint,
        amount: Amount,
    },
    CounterpartyCoin {
        outpoint: OutPoint,
        leaf: Option<usize>,
    },
}

#[derive(Debug, Clone)]
pub enum OutputSlot {
    OurCoin {
        wallet: WalletKey,
        outpoint: OutPoint,
        amount: Amount,
    },
    Payment {
        outpoint: OutPoint,
        address: Option<Address>,
        amount: Amount,
        leaf: usize,
    },
    CounterpartyOutput {
        outpoint: OutPoint,
        address: Option<Address>,
        amount: Amount,
        leaf: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LeafKind {
    Payment,
    CounterpartyOutput,
    CounterpartyCoin,
}

#[derive(Debug, Clone)]
pub struct Leaf {
    pub kind: LeafKind,
    pub outpoint: OutPoint,
    pub address: Option<Address>,
    pub tx: usize,
    /// Index of the slot it connects to in the owning transaction.
    pub index: usize,
    pub reused: bool,
}

impl Leaf {
    pub fn slot(&self) -> SlotRef {
        let side = match self.kind {
            LeafKind::CounterpartyCoin => Side::Input,
            LeafKind::Payment | LeafKind::CounterpartyOutput => Side::Output,
        };
        SlotRef {
            tx: self.tx,
            side,
            index: self.index,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MapTx {
    pub histories: Histories,
    pub inputs: Vec<InputSlot>,
    pub outputs: Vec<OutputSlot>,
    pub net: SignedAmount,
    pub fee: Option<Amount>,
    pub kind: TransactionKind,
    pub leaves: Vec<usize>,
}

impl MapTx {
    /// The wallet owning the transaction label, leaves and position.
    pub fn primary(&self) -> &WalletKey {
        &self.histories[0].0
    }

    /// The primary wallet's history.
    pub fn history(&self) -> &HistoryTransaction {
        &self.histories[0].1
    }

    pub fn wallet_history(&self, wallet: &WalletKey) -> Option<&HistoryTransaction> {
        self.histories
            .iter()
            .find(|(key, _)| key == wallet)
            .map(|(_, history)| history)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoinEdge {
    pub outpoint: OutPoint,
    pub from: SlotRef,
    pub to: SlotRef,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MapItem {
    Tx(usize),
    Leaf(usize),
}

#[derive(Debug, Clone)]
pub struct TxGraph {
    txs: Vec<MapTx>,
    leaves: Vec<Leaf>,
    coin_edges: Vec<CoinEdge>,
    coins: HashMap<OutPoint, Coin>,
    tx_index: HashMap<Txid, usize>,
    items: HashMap<GraphItem, ItemId>,
    chain_root: Vec<usize>,
    neighbours: Vec<Vec<usize>>,
    parents: Vec<Vec<usize>>,
    address_leaves: HashMap<Address, Vec<usize>>,
}

/// Confirmed first and oldest first, unconfirmed last, a parent always before its children.
fn time_order(txs: Vec<Histories>) -> Vec<Histories> {
    let primary = |i: usize| &txs[i][0].1;
    let index: HashMap<Txid, usize> = (0..txs.len()).map(|i| (primary(i).txid, i)).collect();
    let mut pending = vec![0; txs.len()];
    let mut children = vec![Vec::new(); txs.len()];
    for (i, histories) in txs.iter().enumerate() {
        let parents: HashSet<usize> = primary(i)
            .tx
            .input
            .iter()
            .filter(|input| {
                histories
                    .iter()
                    .any(|(_, history)| history.coins.contains_key(&input.previous_output))
            })
            .filter_map(|input| index.get(&input.previous_output.txid).copied())
            .filter(|parent| *parent != i)
            .collect();
        pending[i] = parents.len();
        for parent in parents {
            children[parent].push(i);
        }
    }
    let key = |i: usize| {
        let h = primary(i);
        Reverse((h.time.is_none(), h.height, h.time, h.txid, i))
    };
    let mut ready: BinaryHeap<_> = (0..txs.len())
        .filter(|i| pending[*i] == 0)
        .map(key)
        .collect();
    let mut order = Vec::with_capacity(txs.len());
    while let Some(Reverse((.., i))) = ready.pop() {
        order.push(i);
        for child in &children[i] {
            pending[*child] -= 1;
            if pending[*child] == 0 {
                ready.push(key(*child));
            }
        }
    }
    let mut slots: Vec<Option<Histories>> = txs.into_iter().map(Some).collect();
    order.into_iter().filter_map(|i| slots[i].take()).collect()
}

fn push_leaf(leaves: &mut Vec<Leaf>, leaf: Leaf, owned: &mut Vec<usize>) -> usize {
    leaves.push(leaf);
    owned.push(leaves.len() - 1);
    leaves.len() - 1
}

/// Coins received minus coins spent by `history`'s wallet.
fn net(history: &HistoryTransaction) -> SignedAmount {
    let spent: u64 = history
        .tx
        .input
        .iter()
        .filter_map(|input| history.coins.get(&input.previous_output))
        .map(|coin| coin.amount.to_sat())
        .sum();
    let received: u64 = history
        .owned_outputs
        .keys()
        .filter_map(|index| history.tx.output.get(*index))
        .map(|txout| txout.value.to_sat())
        .sum();
    SignedAmount::from_sat(received as i64 - spent as i64)
}

/// Slots owned by any wallet are its coins; leaves come from the primary wallet's view.
fn build_tx(
    tx: usize,
    histories: Histories,
    leaves: &mut Vec<Leaf>,
    input_leaves: &mut HashSet<OutPoint>,
) -> MapTx {
    let primary = &histories[0].1;
    let mut owned = Vec::new();
    let mut inputs = Vec::new();
    for (index, input) in primary.tx.input.iter().enumerate() {
        let outpoint = input.previous_output;
        let coin = histories
            .iter()
            .find_map(|(wallet, history)| history.coins.get(&outpoint).map(|coin| (wallet, coin)));
        if let Some((wallet, coin)) = coin {
            inputs.push(InputSlot::OurCoin {
                wallet: wallet.clone(),
                outpoint,
                amount: coin.amount,
            });
            continue;
        }
        let leaf = (!outpoint.is_null() && input_leaves.insert(outpoint)).then(|| {
            let leaf = Leaf {
                kind: LeafKind::CounterpartyCoin,
                outpoint,
                address: None,
                tx,
                index,
                reused: false,
            };
            push_leaf(leaves, leaf, &mut owned)
        });
        inputs.push(InputSlot::CounterpartyCoin { outpoint, leaf });
    }
    let mut outputs = Vec::new();
    for (index, txout) in primary.tx.output.iter().enumerate() {
        let outpoint = OutPoint::new(primary.txid, index as u32);
        let amount = txout.value;
        let wallet = histories
            .iter()
            .find(|(_, history)| history.owned_outputs.contains_key(&index));
        if let Some((wallet, _)) = wallet {
            outputs.push(OutputSlot::OurCoin {
                wallet: wallet.clone(),
                outpoint,
                amount,
            });
            continue;
        }
        let address = Address::from_script(&txout.script_pubkey, primary.network).ok();
        let is_payment = primary.wallet_tx.payment_kind(&outpoint) == Some(PaymentKind::Outgoing);
        let kind = if is_payment {
            LeafKind::Payment
        } else {
            LeafKind::CounterpartyOutput
        };
        let leaf = Leaf {
            kind,
            outpoint,
            address: address.clone(),
            tx,
            index,
            reused: false,
        };
        let leaf = push_leaf(leaves, leaf, &mut owned);
        outputs.push(if is_payment {
            OutputSlot::Payment {
                outpoint,
                address,
                amount,
                leaf,
            }
        } else {
            OutputSlot::CounterpartyOutput {
                outpoint,
                address,
                amount,
                leaf,
            }
        });
    }
    MapTx {
        net: net(primary),
        fee: primary.wallet_tx.fee(),
        kind: primary.wallet_tx.kind(),
        histories,
        inputs,
        outputs,
        leaves: owned,
    }
}

fn find(union: &mut [usize], mut i: usize) -> usize {
    while union[i] != i {
        union[i] = union[union[i]];
        i = union[i];
    }
    i
}

impl TxGraph {
    /// A transaction several wallets have is drawn once, owned by its primary wallet.
    pub fn new(mut wallets: Vec<WalletTxs>) -> TxGraph {
        wallets.sort_by(|a, b| a.checksum.cmp(&b.checksum));
        let mut coins: HashMap<OutPoint, Coin> = HashMap::new();
        let mut merged: Vec<Histories> = Vec::new();
        let mut merged_index: HashMap<Txid, usize> = HashMap::new();
        for WalletTxs {
            key,
            txs,
            coins: wallet_coins,
            ..
        } in wallets
        {
            for coin in wallet_coins {
                coins.entry(coin.outpoint).or_insert(coin);
            }
            for history in txs {
                let i = *merged_index.entry(history.txid).or_insert_with(|| {
                    merged.push(Vec::new());
                    merged.len() - 1
                });
                merged[i].push((key.clone(), history));
            }
        }
        let mut leaves = Vec::new();
        let mut input_leaves = HashSet::new();
        let txs: Vec<MapTx> = time_order(merged)
            .into_iter()
            .enumerate()
            .map(|(i, histories)| build_tx(i, histories, &mut leaves, &mut input_leaves))
            .collect();
        let tx_index: HashMap<Txid, usize> = txs
            .iter()
            .enumerate()
            .map(|(i, tx)| (tx.history().txid, i))
            .collect();

        let mut coin_edges = Vec::new();
        for (tx, map_tx) in txs.iter().enumerate() {
            for (index, slot) in map_tx.outputs.iter().enumerate() {
                let OutputSlot::OurCoin { outpoint, .. } = slot else {
                    continue;
                };
                let Some(spender) = coins
                    .get(outpoint)
                    .and_then(|coin| coin.spend_info.as_ref())
                    .and_then(|info| tx_index.get(&info.txid).copied())
                else {
                    continue;
                };
                let Some(input) = txs[spender]
                    .history()
                    .tx
                    .input
                    .iter()
                    .position(|input| input.previous_output == *outpoint)
                else {
                    continue;
                };
                coin_edges.push(CoinEdge {
                    outpoint: *outpoint,
                    from: SlotRef {
                        tx,
                        side: Side::Output,
                        index,
                    },
                    to: SlotRef {
                        tx: spender,
                        side: Side::Input,
                        index: input,
                    },
                });
            }
        }

        let mut address_leaves: HashMap<Address, Vec<usize>> = HashMap::new();
        for (i, leaf) in leaves.iter().enumerate() {
            if let (LeafKind::Payment | LeafKind::CounterpartyOutput, Some(address)) =
                (leaf.kind, &leaf.address)
            {
                address_leaves.entry(address.clone()).or_default().push(i);
            }
        }
        for list in address_leaves.values().filter(|list| list.len() > 1) {
            for i in list {
                leaves[*i].reused = true;
            }
        }

        let mut union: Vec<usize> = (0..txs.len()).collect();
        let mut neighbours = vec![Vec::new(); txs.len()];
        let mut parents = vec![Vec::new(); txs.len()];
        for edge in &coin_edges {
            let (from, to) = (edge.from.tx, edge.to.tx);
            let (root_from, root_to) = (find(&mut union, from), find(&mut union, to));
            union[root_to] = root_from;
            if !neighbours[from].contains(&to) {
                neighbours[from].push(to);
                neighbours[to].push(from);
            }
            if !parents[to].contains(&from) {
                parents[to].push(from);
            }
        }
        let chain_root = (0..txs.len()).map(|i| find(&mut union, i)).collect();

        let tx_items = txs
            .iter()
            .enumerate()
            .map(|(i, tx)| (GraphItem::Tx(tx.history().txid), ItemId(i as u64)));
        let leaf_items = leaves
            .iter()
            .enumerate()
            .map(|(j, leaf)| (leaf_graph_item(leaf), ItemId((txs.len() + j) as u64)));
        let items = tx_items.chain(leaf_items).collect();

        TxGraph {
            txs,
            leaves,
            coin_edges,
            coins,
            tx_index,
            items,
            chain_root,
            neighbours,
            parents,
            address_leaves,
        }
    }

    pub fn txs(&self) -> &[MapTx] {
        &self.txs
    }

    pub fn leaves(&self) -> &[Leaf] {
        &self.leaves
    }

    pub fn coin_edges(&self) -> &[CoinEdge] {
        &self.coin_edges
    }

    pub fn is_empty(&self) -> bool {
        self.txs.is_empty()
    }

    pub fn tx_index(&self, txid: &Txid) -> Option<usize> {
        self.tx_index.get(txid).copied()
    }

    pub fn item(&self, id: ItemId) -> Option<MapItem> {
        let value = id.0 as usize;
        if value < self.txs.len() {
            Some(MapItem::Tx(value))
        } else {
            let leaf = value - self.txs.len();
            (leaf < self.leaves.len()).then_some(MapItem::Leaf(leaf))
        }
    }

    pub fn tx_item(&self, tx: usize) -> ItemId {
        ItemId(tx as u64)
    }

    pub fn leaf_item(&self, leaf: usize) -> ItemId {
        ItemId((self.txs.len() + leaf) as u64)
    }

    pub fn item_id(&self, item: &GraphItem) -> Option<ItemId> {
        self.items.get(item).copied()
    }

    pub fn graph_item(&self, id: ItemId) -> Option<GraphItem> {
        match self.item(id)? {
            MapItem::Tx(tx) => Some(GraphItem::Tx(self.txs[tx].history().txid)),
            MapItem::Leaf(leaf) => Some(leaf_graph_item(&self.leaves[leaf])),
        }
    }

    pub fn item_ids(&self) -> impl Iterator<Item = ItemId> {
        (0..self.txs.len() + self.leaves.len()).map(|value| ItemId(value as u64))
    }

    /// A leaf counts as its transaction.
    pub fn tx_of(&self, id: ItemId) -> Option<usize> {
        match self.item(id)? {
            MapItem::Tx(tx) => Some(tx),
            MapItem::Leaf(leaf) => Some(self.leaves[leaf].tx),
        }
    }

    /// The primary wallet of the item's transaction.
    pub fn item_wallet(&self, id: ItemId) -> Option<&WalletKey> {
        Some(self.txs[self.tx_of(id)?].primary())
    }

    /// Transactions whose primary wallet is `wallet`, and their leaves.
    pub fn wallet_items(&self, wallet: &WalletKey) -> Vec<ItemId> {
        self.item_ids()
            .filter(|id| self.item_wallet(*id) == Some(wallet))
            .collect()
    }

    pub fn coin(&self, outpoint: &OutPoint) -> Option<&Coin> {
        self.coins.get(outpoint)
    }

    pub fn is_unspent(&self, outpoint: &OutPoint) -> bool {
        self.coins
            .get(outpoint)
            .is_some_and(|coin| coin.spend_info.is_none())
    }

    /// In time order.
    pub fn unspent_coins(&self) -> Vec<(OutPoint, Amount)> {
        self.txs
            .iter()
            .flat_map(|tx| tx.outputs.iter())
            .filter_map(|slot| match slot {
                OutputSlot::OurCoin {
                    outpoint, amount, ..
                } if self.is_unspent(outpoint) => Some((*outpoint, *amount)),
                _ => None,
            })
            .collect()
    }

    pub fn output_slot(&self, outpoint: &OutPoint) -> Option<SlotRef> {
        let tx = self.tx_index(&outpoint.txid)?;
        let index = outpoint.vout as usize;
        matches!(
            self.txs[tx].outputs.get(index),
            Some(OutputSlot::OurCoin { .. })
        )
        .then_some(SlotRef {
            tx,
            side: Side::Output,
            index,
        })
    }

    pub fn spending_input(&self, outpoint: &OutPoint) -> Option<SlotRef> {
        self.coin_edges
            .iter()
            .find(|edge| edge.outpoint == *outpoint)
            .map(|edge| edge.to)
    }

    /// The coin of a loaded wallet held by an output slot, or spent by an input slot.
    fn our_coin(&self, slot: SlotRef) -> Option<(&WalletKey, OutPoint)> {
        let tx = self.txs.get(slot.tx)?;
        match (
            slot.side,
            tx.outputs.get(slot.index),
            tx.inputs.get(slot.index),
        ) {
            (
                Side::Output,
                Some(OutputSlot::OurCoin {
                    wallet, outpoint, ..
                }),
                _,
            )
            | (
                Side::Input,
                _,
                Some(InputSlot::OurCoin {
                    wallet, outpoint, ..
                }),
            ) => Some((wallet, *outpoint)),
            _ => None,
        }
    }

    pub fn slot_coin(&self, slot: SlotRef) -> Option<OutPoint> {
        self.our_coin(slot).map(|(_, outpoint)| outpoint)
    }

    /// The wallet of the coin held or spent by the slot.
    pub fn slot_wallet(&self, slot: SlotRef) -> Option<&WalletKey> {
        self.our_coin(slot).map(|(wallet, _)| wallet)
    }

    /// All transactions of the chain of `tx`, in time order.
    pub fn chain(&self, tx: usize) -> Vec<usize> {
        match self.chain_root.get(tx) {
            Some(root) => (0..self.txs.len())
                .filter(|i| self.chain_root[*i] == *root)
                .collect(),
            None => Vec::new(),
        }
    }

    pub fn same_chain(&self, a: usize, b: usize) -> bool {
        match (self.chain_root.get(a), self.chain_root.get(b)) {
            (Some(a), Some(b)) => a == b,
            _ => false,
        }
    }

    /// Transactions `tx` spends from.
    pub fn parents(&self, tx: usize) -> &[usize] {
        self.parents.get(tx).map_or(&[], Vec::as_slice)
    }

    /// Transactions spending a coin of `tx`.
    pub fn children(&self, tx: usize) -> Vec<usize> {
        let mut children: Vec<usize> = self
            .coin_edges
            .iter()
            .filter(|edge| edge.from.tx == tx)
            .map(|edge| edge.to.tx)
            .collect();
        children.sort_unstable();
        children.dedup();
        children
    }

    /// Shortest path over undirected coin edges, both ends included.
    pub fn path(&self, from: usize, to: usize) -> Option<Vec<usize>> {
        if !self.same_chain(from, to) {
            return None;
        }
        let mut previous = vec![None; self.txs.len()];
        let mut seen = vec![false; self.txs.len()];
        seen[from] = true;
        let mut frontier = VecDeque::from([from]);
        while let Some(current) = frontier.pop_front() {
            if current == to {
                break;
            }
            for next in &self.neighbours[current] {
                if !seen[*next] {
                    seen[*next] = true;
                    previous[*next] = Some(current);
                    frontier.push_back(*next);
                }
            }
        }
        let mut path = vec![to];
        let mut current = to;
        while let Some(step) = previous[current] {
            path.push(step);
            current = step;
        }
        path.reverse();
        Some(path)
    }

    pub fn leaves_on_address(&self, address: &Address) -> &[usize] {
        self.address_leaves.get(address).map_or(&[], Vec::as_slice)
    }

    pub fn tx_label(&self, tx: usize) -> Label {
        self.txs
            .get(tx)
            .map(|tx| tx.history().label())
            .unwrap_or_default()
    }

    pub fn slot_label(&self, slot: SlotRef) -> Label {
        let Some(tx) = self.txs.get(slot.tx) else {
            return Label::None;
        };
        let resolve = |history: &HistoryTransaction, outpoint: &OutPoint, inherited: Label| {
            label::resolve(label::get(&history.labels, *outpoint), &inherited)
        };
        match slot.side {
            Side::Output => match tx.outputs.get(slot.index) {
                Some(OutputSlot::OurCoin {
                    wallet, outpoint, ..
                }) => tx.wallet_history(wallet).map_or(Label::None, |history| {
                    let default = history.owned_outputs.get(&slot.index).cloned();
                    resolve(history, outpoint, default.unwrap_or_default())
                }),
                Some(OutputSlot::Payment { .. }) => {
                    Payment::from_tx_output(tx.history(), slot.index)
                        .map(|payment| payment.label())
                        .unwrap_or_default()
                }
                Some(OutputSlot::CounterpartyOutput { outpoint, .. }) => {
                    resolve(tx.history(), outpoint, Label::None)
                }
                None => Label::None,
            },
            Side::Input => match tx.inputs.get(slot.index) {
                Some(InputSlot::OurCoin {
                    wallet, outpoint, ..
                }) => tx.wallet_history(wallet).map_or(Label::None, |history| {
                    let default = history.coins.get(outpoint).map(|c| c.default_label.clone());
                    resolve(history, outpoint, default.unwrap_or_default())
                }),
                Some(InputSlot::CounterpartyCoin { outpoint, .. }) => {
                    resolve(tx.history(), outpoint, Label::None)
                }
                None => Label::None,
            },
        }
    }

    /// The address label when the leaf has an address, else the outpoint label.
    /// A payment's own label, else the label of its address.
    pub fn leaf_label(&self, index: usize) -> Label {
        let own = self
            .leaves
            .get(index)
            .filter(|leaf| leaf.kind == LeafKind::Payment)
            .and_then(|leaf| label::get(&self.txs[leaf.tx].history().labels, leaf.outpoint));
        match own {
            Some(own) => label::resolve(Some(own), &Label::None),
            None => self.address_label(index),
        }
    }

    /// Label of the address, or of the outpoint for a counterparty coin.
    pub fn address_label(&self, leaf: usize) -> Label {
        let Some(leaf) = self.leaves.get(leaf) else {
            return Label::None;
        };
        let labels = &self.txs[leaf.tx].history().labels;
        let own = match (leaf.kind, &leaf.address) {
            (LeafKind::CounterpartyCoin, _) | (_, None) => label::get(labels, leaf.outpoint),
            (_, Some(address)) => label::get(labels, address.clone()),
        };
        label::resolve(own, &Label::None)
    }

    /// The primary wallet's history, the only mutable access: slots and leaves never change.
    pub fn history_mut(&mut self, tx: usize) -> &mut HistoryTransaction {
        &mut self.txs[tx].histories[0].1
    }

    /// Loads labels saved in `wallet` into its histories only.
    pub fn load_wallet_labels(
        &mut self,
        wallet: &WalletKey,
        new_labels: &HashMap<String, Option<String>>,
    ) {
        for (_, history) in self
            .txs
            .iter_mut()
            .flat_map(|tx| tx.histories.iter_mut())
            .filter(|(key, _)| key == wallet)
        {
            history.load_labels(new_labels);
        }
    }
}

impl LabelsLoader for TxGraph {
    fn load_labels(&mut self, new_labels: &HashMap<String, Option<String>>) {
        self.load_wallet_labels(&WalletKey::Current, new_labels);
    }
}

fn leaf_graph_item(leaf: &Leaf) -> GraphItem {
    match leaf.kind {
        LeafKind::Payment | LeafKind::CounterpartyOutput => GraphItem::OutputLeaf(leaf.outpoint),
        LeafKind::CounterpartyCoin => GraphItem::InputLeaf(leaf.outpoint),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::map::fixture::{self, foreign, ours, Builder};

    fn index(graph: &TxGraph, txid: Txid) -> usize {
        graph.tx_index(&txid).unwrap()
    }

    fn kinds(slots: &[OutputSlot]) -> Vec<&'static str> {
        slots
            .iter()
            .map(|slot| match slot {
                OutputSlot::OurCoin { .. } => "ours",
                OutputSlot::Payment { .. } => "payment",
                OutputSlot::CounterpartyOutput { .. } => "counterparty",
            })
            .collect()
    }

    #[test]
    fn time_order_unconfirmed_last() {
        let f = fixture::sample_wallet();
        let graph = fixture::current_graph(f.txs, f.coins);
        assert_eq!(graph.txs().len(), 12);
        assert_eq!(index(&graph, f.ids.salary), 0);
        assert_eq!(index(&graph, f.ids.unconfirmed), 11);
        for i in 0..graph.txs().len() {
            assert!(graph.parents(i).iter().all(|p| *p < i));
        }
    }

    #[test]
    fn time_order_parent_first_on_equal_time() {
        for day in [Some(1), None] {
            for sats in 1_000.. {
                let mut b = Builder::new();
                let parent = b.tx(day, &[foreign(1)], &[(ours(0), 10_000, true)]);
                let child = b.tx(day, &[OutPoint::new(parent, 0)], &[(ours(1), sats, true)]);
                if child >= parent {
                    continue;
                }
                let (txs, coins) = b.finish();
                let graph = fixture::current_graph(txs.into_iter().rev().collect(), coins);
                assert_eq!(index(&graph, parent), 0);
                assert_eq!(index(&graph, child), 1);
                break;
            }
        }
    }

    #[test]
    fn slots_follow_transaction_order() {
        let f = fixture::sample_wallet();
        let graph = fixture::current_graph(f.txs, f.coins);
        let batch = &graph.txs()[index(&graph, f.ids.batch)];
        let mut expected = vec!["payment"; 13];
        expected.push("ours");
        assert_eq!(batch.inputs.len(), 1);
        assert_eq!(kinds(&batch.outputs), expected);
        let incoming = &graph.txs()[index(&graph, f.ids.incoming_change)];
        assert_eq!(incoming.inputs.len(), 3);
        assert!(incoming
            .inputs
            .iter()
            .all(|slot| matches!(slot, InputSlot::CounterpartyCoin { .. })));
        assert_eq!(kinds(&incoming.outputs), ["ours", "counterparty"]);
    }

    #[test]
    fn leaves_per_spec() {
        let f = fixture::sample_wallet();
        let graph = fixture::current_graph(f.txs, f.coins);
        assert_eq!(graph.leaves().len(), 27);
        let owned = |txid| graph.txs()[index(&graph, txid)].leaves.clone();
        let kind = |leaf: usize| graph.leaves()[leaf].kind;
        let incoming = owned(f.ids.incoming_change);
        assert_eq!(incoming.len(), 4);
        assert_eq!(
            incoming.iter().map(|l| kind(*l)).collect::<Vec<_>>(),
            [
                LeafKind::CounterpartyCoin,
                LeafKind::CounterpartyCoin,
                LeafKind::CounterpartyCoin,
                LeafKind::CounterpartyOutput
            ]
        );
        let payjoin = owned(f.ids.payjoin);
        assert_eq!(
            payjoin.iter().map(|l| kind(*l)).collect::<Vec<_>>(),
            [LeafKind::CounterpartyCoin, LeafKind::Payment]
        );
        assert!(owned(f.ids.consolidation).is_empty());
        assert!(owned(f.ids.self_transfer).is_empty());
    }

    #[test]
    fn payment_versus_counterparty_output() {
        let f = fixture::sample_wallet();
        let graph = fixture::current_graph(f.txs, f.coins);
        let four = &graph.txs()[index(&graph, f.ids.incoming_four)];
        assert_eq!(
            kinds(&four.outputs),
            ["ours", "ours", "ours", "ours", "counterparty"]
        );
        for rent in f.ids.rent {
            let rent = &graph.txs()[index(&graph, rent)];
            assert_eq!(kinds(&rent.outputs), ["payment", "ours"]);
        }
    }

    #[test]
    fn coin_edges_from_spend_info() {
        let f = fixture::sample_wallet();
        let graph = fixture::current_graph(f.txs, f.coins);
        assert_eq!(graph.coin_edges().len(), 12);
        let rent3 = index(&graph, f.ids.rent[3]);
        let payjoin = index(&graph, f.ids.payjoin);
        let edge = graph
            .coin_edges()
            .iter()
            .find(|edge| edge.from.tx == rent3)
            .unwrap();
        assert_eq!(
            edge.from,
            SlotRef {
                tx: rent3,
                side: Side::Output,
                index: 1
            }
        );
        assert_eq!(
            edge.to,
            SlotRef {
                tx: payjoin,
                side: Side::Input,
                index: 0
            }
        );
        assert_eq!(edge.outpoint, OutPoint::new(f.ids.rent[3], 1));
    }

    #[test]
    fn address_reuse_flags() {
        let f = fixture::sample_wallet();
        let graph = fixture::current_graph(f.txs, f.coins);
        assert_eq!(graph.leaves_on_address(&f.landlord).len(), 4);
        assert_eq!(graph.leaves_on_address(&f.reused_payee).len(), 2);
        for leaf in graph
            .leaves_on_address(&f.landlord)
            .iter()
            .chain(graph.leaves_on_address(&f.reused_payee))
        {
            assert!(graph.leaves()[*leaf].reused);
        }
        let payjoin = &graph.txs()[index(&graph, f.ids.payjoin)];
        let payee = payjoin.leaves[1];
        assert!(!graph.leaves()[payee].reused);
        assert_eq!(graph.leaves().iter().filter(|l| l.reused).count(), 6);
    }

    #[test]
    fn chains_union_find() {
        let f = fixture::sample_wallet();
        let graph = fixture::current_graph(f.txs, f.coins);
        let roots: HashSet<usize> = (0..graph.txs().len()).map(|i| graph.chain(i)[0]).collect();
        assert_eq!(roots.len(), 3);
        let salary = index(&graph, f.ids.salary);
        let chain = graph.chain(salary);
        assert_eq!(chain.len(), 7);
        assert!(chain.windows(2).all(|w| w[0] < w[1]));
        let change = index(&graph, f.ids.incoming_change);
        let transfer = index(&graph, f.ids.self_transfer);
        assert!(graph.same_chain(change, transfer));
        assert!(!graph.same_chain(salary, index(&graph, f.ids.batch)));
    }

    #[test]
    fn shortest_path_bfs() {
        let f = fixture::sample_wallet();
        let graph = fixture::current_graph(f.txs, f.coins);
        let salary = index(&graph, f.ids.salary);
        let unconfirmed = index(&graph, f.ids.unconfirmed);
        let expected: Vec<usize> = std::iter::once(f.ids.salary)
            .chain(f.ids.rent)
            .chain([f.ids.payjoin, f.ids.unconfirmed])
            .map(|txid| index(&graph, txid))
            .collect();
        assert_eq!(graph.path(salary, unconfirmed), Some(expected.clone()));
        let mut reversed = expected;
        reversed.reverse();
        assert_eq!(graph.path(unconfirmed, salary), Some(reversed));
        assert_eq!(graph.path(salary, salary), Some(vec![salary]));
        assert_eq!(graph.path(salary, index(&graph, f.ids.batch)), None);
    }

    #[test]
    fn net_and_fee() {
        let f = fixture::sample_wallet();
        let graph = fixture::current_graph(f.txs, f.coins);
        let tx = |txid| &graph.txs()[index(&graph, txid)];
        let rent = tx(f.ids.rent[0]);
        assert_eq!(rent.net, SignedAmount::from_sat(-501_000));
        assert_eq!(rent.fee, Some(Amount::from_sat(1_000)));
        let salary = tx(f.ids.salary);
        assert_eq!(salary.net, SignedAmount::from_sat(2_000_000));
        assert_eq!(salary.fee, None);
        assert_eq!(tx(f.ids.payjoin).fee, None);
        let consolidation = tx(f.ids.consolidation);
        assert_eq!(consolidation.net, SignedAmount::from_sat(-2_000));
        assert_eq!(consolidation.fee, Some(Amount::from_sat(2_000)));
        assert_eq!(tx(f.ids.self_transfer).net, SignedAmount::from_sat(-1_000));
    }

    #[test]
    fn unspent_coins() {
        let f = fixture::sample_wallet();
        let graph = fixture::current_graph(f.txs, f.coins);
        let expected = [
            (OutPoint::new(f.ids.self_transfer, 0), 799_000),
            (OutPoint::new(f.ids.batch, 13), 867_000),
            (OutPoint::new(f.ids.unconfirmed, 1), 94_000),
        ]
        .map(|(outpoint, sats)| (outpoint, Amount::from_sat(sats)));
        let unspent = graph.unspent_coins();
        assert_eq!(unspent, expected);
        let total: Amount = unspent.iter().map(|(_, amount)| *amount).sum();
        assert_eq!(total, Amount::from_sat(1_760_000));
        assert!(!graph.is_unspent(&OutPoint::new(f.ids.salary, 0)));
        assert!(graph.is_unspent(&expected[0].0));
    }

    #[test]
    fn item_id_round_trip() {
        let f = fixture::sample_wallet();
        let graph = fixture::current_graph(f.txs, f.coins);
        assert_eq!(graph.item_ids().count(), 12 + 27);
        for id in graph.item_ids() {
            assert_eq!(graph.item_id(&graph.graph_item(id).unwrap()), Some(id));
        }
        let payment = graph
            .leaves()
            .iter()
            .position(|l| l.kind == LeafKind::Payment);
        let payment = graph.leaf_item(payment.unwrap());
        assert!(matches!(
            graph.graph_item(payment),
            Some(GraphItem::OutputLeaf(_))
        ));
        let coin = graph
            .leaves()
            .iter()
            .position(|l| l.kind == LeafKind::CounterpartyCoin);
        let coin_leaf = coin.unwrap();
        let coin = graph.leaf_item(coin_leaf);
        assert!(matches!(
            graph.graph_item(coin),
            Some(GraphItem::InputLeaf(_))
        ));
        assert_eq!(graph.tx_of(coin), Some(graph.leaves()[coin_leaf].tx));
        assert_eq!(graph.tx_of(graph.tx_item(3)), Some(3));
        assert_eq!(graph.item(ItemId(1_000)), None);
    }

    #[test]
    fn input_and_output_leaf_same_outpoint() {
        let mut b = Builder::new();
        let paid = b.tx(
            Some(1),
            &[foreign(1)],
            &[(ours(0), 10_000, true), (fixture::address(1), 5_000, false)],
        );
        let shared = OutPoint::new(paid, 1);
        b.tx(Some(2), &[shared, foreign(2)], &[(ours(1), 8_000, true)]);
        let (txs, coins) = b.finish();
        let graph = fixture::current_graph(txs, coins);
        let output = graph.item_id(&GraphItem::OutputLeaf(shared)).unwrap();
        let input = graph.item_id(&GraphItem::InputLeaf(shared)).unwrap();
        assert_ne!(output, input);
    }

    #[test]
    fn slot_coin_and_spending_input() {
        let f = fixture::sample_wallet();
        let graph = fixture::current_graph(f.txs, f.coins);
        let change = OutPoint::new(f.ids.rent[3], 1);
        let slot = SlotRef {
            tx: index(&graph, f.ids.payjoin),
            side: Side::Input,
            index: 0,
        };
        assert_eq!(graph.slot_coin(slot), Some(change));
        assert_eq!(graph.spending_input(&change), Some(slot));
        assert_eq!(
            graph.output_slot(&change).map(|s| (s.tx, s.index)),
            Some((index(&graph, f.ids.rent[3]), 1))
        );
        let counterparty = SlotRef { index: 1, ..slot };
        assert_eq!(graph.slot_coin(counterparty), None);
    }

    #[test]
    fn payment_leaf_prefers_its_own_label() {
        let f = fixture::sample_wallet();
        let mut graph = fixture::current_graph(f.txs, f.coins);
        let landlord = graph.leaves_on_address(&f.landlord)[0];
        let leaf = graph.leaves()[landlord].clone();
        let landlord_label = Label::Own("Landlord".to_string());
        assert_eq!(graph.leaf_label(landlord), landlord_label);

        graph
            .history_mut(leaf.tx)
            .labels
            .insert(leaf.outpoint.to_string(), "March rent".to_string());
        assert_eq!(
            graph.leaf_label(landlord),
            Label::Own("March rent".to_string())
        );
        assert_eq!(graph.address_label(landlord), landlord_label);
    }

    #[test]
    fn effective_labels() {
        let f = fixture::sample_wallet();
        let graph = fixture::current_graph(f.txs, f.coins);
        let salary = index(&graph, f.ids.salary);
        assert_eq!(graph.tx_label(salary), Label::Own("Salary".to_string()));
        let landlord = graph.leaves_on_address(&f.landlord)[0];
        assert_eq!(
            graph.leaf_label(landlord),
            Label::Own("Landlord".to_string())
        );
        let slot = |txid, side, index| SlotRef {
            tx: self::index(&graph, txid),
            side,
            index,
        };
        assert_eq!(
            graph.slot_label(slot(f.ids.self_transfer, Side::Output, 0)),
            Label::Funding("Savings".to_string())
        );
        assert_eq!(
            graph.slot_label(slot(f.ids.batch, Side::Output, 0)),
            Label::None
        );
        assert_eq!(
            graph.slot_label(slot(f.ids.incoming_change, Side::Output, 1)),
            Label::Own("Alice change".to_string())
        );
    }

    #[test]
    fn empty_graph() {
        let graph = TxGraph::new(Vec::new());
        assert!(graph.is_empty());
        assert_eq!(graph.item_ids().count(), 0);
        assert_eq!(graph.path(0, 0), None);
    }

    #[test]
    fn labels_loader_updates_graph() {
        let f = fixture::sample_wallet();
        let mut graph = fixture::current_graph(f.txs, f.coins);
        let (txid, landlord) = (f.ids.salary, f.landlord);
        let salary = index(&graph, txid);
        let coin = SlotRef {
            tx: salary,
            side: Side::Output,
            index: 0,
        };
        let leaf = graph
            .leaves()
            .iter()
            .position(|leaf| leaf.address.as_ref() == Some(&landlord))
            .unwrap();
        let saved = |value: Option<&str>| {
            HashMap::from([
                (txid.to_string(), value.map(str::to_string)),
                (
                    OutPoint::new(txid, 0).to_string(),
                    value.map(str::to_string),
                ),
                (landlord.to_string(), value.map(str::to_string)),
            ])
        };

        graph.load_labels(&saved(Some("New")));
        let new = Label::Own("New".to_string());
        assert_eq!(graph.tx_label(salary), new);
        assert_eq!(graph.slot_label(coin), new);
        assert_eq!(graph.leaf_label(leaf), new);

        graph.load_labels(&saved(None));
        assert_eq!(graph.tx_label(salary), Label::None);
        assert_eq!(graph.slot_label(coin), Label::None);
        assert_eq!(graph.leaf_label(leaf), Label::None);
    }

    #[test]
    fn labels_loader_keeps_default_labels() {
        let f = fixture::sample_wallet();
        let mut graph = fixture::current_graph(f.txs, f.coins);
        let transfer = index(&graph, f.ids.self_transfer);
        let coin = SlotRef {
            tx: transfer,
            side: Side::Output,
            index: 0,
        };
        let key = OutPoint::new(f.ids.self_transfer, 0).to_string();
        let default = Label::Funding("Savings".to_string());
        assert_eq!(graph.slot_label(coin), default);

        graph.load_labels(&HashMap::from([(key.clone(), Some("Mine".to_string()))]));
        assert_eq!(graph.slot_label(coin), Label::Own("Mine".to_string()));

        graph.load_labels(&HashMap::from([(key, None)]));
        assert_eq!(graph.slot_label(coin), default);
    }

    fn two_wallets_graph(a_checksum: &str, b_checksum: &str) -> (TxGraph, fixture::TwoWallets) {
        let mut f = fixture::two_wallets(a_checksum, b_checksum);
        let graph = TxGraph::new(std::mem::take(&mut f.wallets));
        (graph, f)
    }

    fn owner(slot: &OutputSlot) -> Option<&WalletKey> {
        match slot {
            OutputSlot::OurCoin { wallet, .. } => Some(wallet),
            _ => None,
        }
    }

    #[test]
    fn shared_tx_drawn_once_with_owned_slots() {
        let (graph, f) = two_wallets_graph("aaaa", "bbbb");
        assert_eq!(graph.txs().len(), 3);
        let payment = &graph.txs()[index(&graph, f.payment)];
        assert_eq!(payment.histories.len(), 2);
        assert_eq!(payment.primary(), &WalletKey::Current);
        assert_eq!(
            payment.outputs.iter().map(owner).collect::<Vec<_>>(),
            [Some(&f.b), Some(&WalletKey::Current)]
        );
        assert!(matches!(
            &payment.inputs[0],
            InputSlot::OurCoin {
                wallet: WalletKey::Current,
                ..
            }
        ));
        assert!(payment.leaves.is_empty());
        assert_eq!(payment.net, SignedAmount::from_sat(-61_000));
        assert_eq!(payment.fee, Some(Amount::from_sat(1_000)));
        assert_eq!(
            graph.unspent_coins(),
            [(OutPoint::new(f.payment, 1), Amount::from_sat(39_000))]
        );
    }

    #[test]
    fn primary_is_first_checksum() {
        let (graph, f) = two_wallets_graph("cccc", "bbbb");
        let payment = index(&graph, f.payment);
        let tx = &graph.txs()[payment];
        assert_eq!(tx.primary(), &f.b);
        assert_eq!(tx.net, SignedAmount::from_sat(60_000));
        assert_eq!(
            tx.outputs.iter().map(owner).collect::<Vec<_>>(),
            [Some(&f.b), Some(&WalletKey::Current)]
        );
        assert!(tx.leaves.is_empty());
        assert_eq!(graph.tx_label(payment), Label::Own("From A".to_string()));
    }

    #[test]
    fn coin_edge_crosses_wallets() {
        let (graph, f) = two_wallets_graph("aaaa", "bbbb");
        let (payment, spend) = (index(&graph, f.payment), index(&graph, f.spend));
        let paid = OutPoint::new(f.payment, 0);
        let edge = graph
            .coin_edges()
            .iter()
            .find(|edge| edge.outpoint == paid)
            .unwrap();
        assert_eq!(
            (edge.from, edge.to),
            (
                SlotRef {
                    tx: payment,
                    side: Side::Output,
                    index: 0
                },
                SlotRef {
                    tx: spend,
                    side: Side::Input,
                    index: 0
                }
            )
        );
        assert_eq!(graph.slot_wallet(edge.from), Some(&f.b));
        assert_eq!(graph.slot_wallet(edge.to), Some(&f.b));
        assert_eq!(
            graph.item_wallet(graph.tx_item(payment)),
            Some(&WalletKey::Current)
        );
        assert_eq!(graph.item_wallet(graph.tx_item(spend)), Some(&f.b));
        assert_eq!(graph.coin_edges().len(), 2);
        assert!(graph.same_chain(index(&graph, f.funding), spend));
    }

    #[test]
    fn wallet_items_follow_primary() {
        let (graph, f) = two_wallets_graph("aaaa", "bbbb");
        let tx_item = |txid| graph.tx_item(index(&graph, txid));
        let leaf_item = |txid| graph.leaf_item(graph.txs()[index(&graph, txid)].leaves[0]);
        assert_eq!(
            graph.wallet_items(&WalletKey::Current),
            [tx_item(f.funding), tx_item(f.payment), leaf_item(f.funding)]
        );
        assert_eq!(
            graph.wallet_items(&f.b),
            [tx_item(f.spend), leaf_item(f.spend)]
        );
        assert_eq!(graph.item_wallet(leaf_item(f.spend)), Some(&f.b));
        assert_eq!(graph.item_wallet(ItemId(1_000)), None);
    }

    #[test]
    fn labels_from_owning_wallet() {
        let (mut graph, f) = two_wallets_graph("aaaa", "bbbb");
        let (payment, spend) = (index(&graph, f.payment), index(&graph, f.spend));
        let paid = SlotRef {
            tx: payment,
            side: Side::Output,
            index: 0,
        };
        assert_eq!(graph.tx_label(payment), Label::Own("Paid B".to_string()));
        assert_eq!(
            graph.slot_label(paid),
            Label::Own("Seen from B".to_string())
        );
        let shop = graph.txs()[spend].leaves[0];
        assert_eq!(graph.leaf_label(shop), Label::Own("Shop".to_string()));

        let key = OutPoint::new(f.payment, 0).to_string();
        let renamed = HashMap::from([(key.clone(), Some("Renamed".to_string()))]);
        graph.load_wallet_labels(&f.b, &renamed);
        assert_eq!(graph.slot_label(paid), Label::Own("Renamed".to_string()));
        let seen_from_a = graph.txs()[payment]
            .wallet_history(&WalletKey::Current)
            .and_then(|history| history.labels.get(&key));
        assert_eq!(seen_from_a.map(String::as_str), Some("Seen from A"));

        graph.load_labels(&HashMap::from([(key, Some("Ignored".to_string()))]));
        assert_eq!(graph.slot_label(paid), Label::Own("Renamed".to_string()));
    }
}
