use std::collections::HashSet;

use liana::miniscript::bitcoin::Address;
use liana_ui::widget::graph_view::ItemId;

use crate::app::state::map::graph::{MapItem, SlotRef, TxGraph};

/// Selected blocks and leaves, with the anchor of the next shift click.
#[derive(Debug, Clone, Default)]
pub struct Selection {
    items: HashSet<ItemId>,
    anchor: Option<ItemId>,
}

impl Selection {
    pub fn items(&self) -> &HashSet<ItemId> {
        &self.items
    }

    pub fn anchor(&self) -> Option<ItemId> {
        self.anchor
    }

    pub fn contains(&self, id: ItemId) -> bool {
        self.items.contains(&id)
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Two or more items: group frame and member outlines.
    pub fn is_group(&self) -> bool {
        self.items.len() >= 2
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.anchor = None;
    }

    pub fn click(&mut self, id: ItemId) {
        self.items.clear();
        self.items.insert(id);
        self.anchor = Some(id);
    }

    /// On an empty selection, a reused leaf selects every leaf on its address.
    pub fn command_click(&mut self, graph: &TxGraph, id: ItemId) {
        let reused_address = match graph.item(id) {
            Some(MapItem::Leaf(leaf)) if self.items.is_empty() => {
                let leaf = &graph.leaves()[leaf];
                leaf.address.as_ref().filter(|_| leaf.reused)
            }
            _ => None,
        };
        match reused_address {
            Some(address) => self.items.extend(address_leaves(graph, address)),
            None => {
                if !self.items.remove(&id) {
                    self.items.insert(id);
                }
            }
        }
        self.anchor = Some(id);
    }

    /// Selects the path from the anchor to `id`; the anchor stays. Without an
    /// anchor, selects the transaction of `id` with its leaves.
    pub fn shift_click(&mut self, graph: &TxGraph, id: ItemId) {
        let Some(anchor) = self.anchor else {
            self.click(id);
            if let Some(tx) = graph.tx_of(id) {
                add_txs_with_leaves(graph, &mut self.items, [tx]);
            }
            return;
        };
        let mut items = HashSet::from([anchor, id]);
        if let (Some(from), Some(to)) = (graph.tx_of(anchor), graph.tx_of(id)) {
            if let Some(path) = graph.path(from, to) {
                add_txs_with_leaves(graph, &mut items, path);
            }
        }
        self.items = items;
    }

    /// Adds the whole chain of `id`; the anchor stays.
    pub fn command_shift_click(&mut self, graph: &TxGraph, id: ItemId) {
        if let Some(tx) = graph.tx_of(id) {
            add_txs_with_leaves(graph, &mut self.items, graph.chain(tx));
        }
    }

    /// `ids` come from the area selection of the widget.
    pub fn area(&mut self, ids: impl IntoIterator<Item = ItemId>, additive: bool) {
        if !additive {
            self.items.clear();
        }
        self.items.extend(ids);
        if self
            .anchor
            .is_some_and(|anchor| !self.items.contains(&anchor))
        {
            self.anchor = None;
        }
    }

    /// Indices of the selected blocks in time order, leaves excluded.
    pub fn selected_txs(&self, graph: &TxGraph) -> Vec<usize> {
        let mut txs: Vec<usize> = self
            .items
            .iter()
            .filter_map(|id| match graph.item(*id) {
                Some(MapItem::Tx(tx)) => Some(tx),
                _ => None,
            })
            .collect();
        txs.sort_unstable();
        txs
    }

    /// Drops the ids that no longer exist.
    pub fn retain(&mut self, keep: impl Fn(ItemId) -> bool) {
        self.items.retain(|id| keep(*id));
        if self
            .anchor
            .is_some_and(|anchor| !self.items.contains(&anchor))
        {
            self.anchor = None;
        }
    }
}

fn add_txs_with_leaves(
    graph: &TxGraph,
    items: &mut HashSet<ItemId>,
    txs: impl IntoIterator<Item = usize>,
) {
    for tx in txs {
        items.insert(graph.tx_item(tx));
        items.extend(
            graph.txs()[tx]
                .leaves
                .iter()
                .map(|leaf| graph.leaf_item(*leaf)),
        );
    }
}

/// Leaves sharing the address or the label of a selected leaf. A selected leaf can be the
/// sibling of another one.
pub fn siblings(graph: &TxGraph, selection: &Selection) -> HashSet<ItemId> {
    let mut result = HashSet::new();
    for id in selection.items() {
        let Some(MapItem::Leaf(source)) = graph.item(*id) else {
            continue;
        };
        let address = graph.leaves()[source].address.as_ref();
        let label = graph.leaf_label(source);
        let label = label.value();
        for (index, leaf) in graph.leaves().iter().enumerate() {
            if index == source {
                continue;
            }
            let same_address = address.is_some() && leaf.address.as_ref() == address;
            let same_label = label.is_some() && graph.leaf_label(index).value() == label;
            if same_address || same_label {
                result.insert(graph.leaf_item(index));
            }
        }
    }
    result
}

/// All leaves on `address`.
pub fn address_leaves(graph: &TxGraph, address: &Address) -> HashSet<ItemId> {
    graph
        .leaves_on_address(address)
        .iter()
        .map(|leaf| graph.leaf_item(*leaf))
        .collect()
}

/// The tags of a clicked slot, one of them active.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagHighlight {
    slot: SlotRef,
    tags: Vec<usize>,
    active: usize,
}

impl TagHighlight {
    /// `None` when the slot has no tags.
    pub fn new(slot: SlotRef, tags: Vec<usize>) -> Option<Self> {
        if tags.is_empty() {
            return None;
        }
        Some(Self {
            slot,
            tags,
            active: 0,
        })
    }

    pub fn slot(&self) -> SlotRef {
        self.slot
    }

    /// Registry id of the active tag.
    pub fn active_tag(&self) -> usize {
        self.tags[self.active]
    }

    /// 1-based index of the active tag and the tag count.
    pub fn position(&self) -> (usize, usize) {
        (self.active + 1, self.tags.len())
    }

    pub fn has_many(&self) -> bool {
        self.tags.len() > 1
    }

    /// Moves the active tag by `steps` (positive is forward), wrapping around. Returns whether
    /// it changed.
    pub fn cycle(&mut self, steps: i32) -> bool {
        let count = self.tags.len() as i64;
        let next = (self.active as i64 + i64::from(steps)).rem_euclid(count) as usize;
        let changed = next != self.active;
        self.active = next;
        changed
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use liana::miniscript::bitcoin::{Address, Txid};
    use liana_ui::widget::graph_view::Side;

    use crate::app::state::map::{
        fixture::{self, foreign, ours, Builder, SampleTxids},
        graph::{SlotRef, TxGraph},
        selection::{address_leaves, siblings, ItemId, Selection, TagHighlight},
    };

    fn setup() -> (TxGraph, SampleTxids, Address) {
        let f = fixture::sample_wallet();
        let landlord = f.landlord.clone();
        (TxGraph::new(f.txs, &f.coins), f.ids, landlord)
    }

    fn tx(graph: &TxGraph, txid: &Txid) -> ItemId {
        graph.tx_item(graph.tx_index(txid).unwrap())
    }

    fn leaf_on(graph: &TxGraph, address: &Address, n: usize) -> ItemId {
        graph.leaf_item(graph.leaves_on_address(address)[n])
    }

    fn leaves_of(graph: &TxGraph, txids: &[&Txid]) -> HashSet<ItemId> {
        let mut items = HashSet::new();
        for txid in txids {
            let index = graph.tx_index(txid).unwrap();
            items.insert(graph.tx_item(index));
            items.extend(
                graph.txs()[index]
                    .leaves
                    .iter()
                    .map(|leaf| graph.leaf_item(*leaf)),
            );
        }
        items
    }

    fn slot() -> SlotRef {
        SlotRef {
            tx: 0,
            side: Side::Output,
            index: 0,
        }
    }

    #[test]
    fn click_replaces_and_sets_anchor() {
        let (graph, ids, _) = setup();
        let mut selection = Selection::default();
        selection.click(tx(&graph, &ids.rent[0]));
        selection.click(tx(&graph, &ids.batch));
        let batch = tx(&graph, &ids.batch);
        assert_eq!(selection.items(), &HashSet::from([batch]));
        assert_eq!(selection.anchor(), Some(batch));
    }

    #[test]
    fn command_click_toggles() {
        let (graph, ids, _) = setup();
        let (rent, batch) = (tx(&graph, &ids.rent[0]), tx(&graph, &ids.batch));
        let mut selection = Selection::default();
        selection.click(rent);
        selection.command_click(&graph, batch);
        assert_eq!(selection.items(), &HashSet::from([rent, batch]));
        assert_eq!(selection.anchor(), Some(batch));
        selection.command_click(&graph, batch);
        assert_eq!(selection.items(), &HashSet::from([rent]));
        assert_eq!(selection.anchor(), Some(batch));
    }

    #[test]
    fn command_click_reused_leaf_on_empty_selection() {
        let (graph, ids, landlord) = setup();
        let leaf = leaf_on(&graph, &landlord, 1);
        let mut selection = Selection::default();
        selection.command_click(&graph, leaf);
        assert_eq!(selection.items(), &address_leaves(&graph, &landlord));
        assert_eq!(selection.len(), 4);
        assert_eq!(selection.anchor(), Some(leaf));

        let mut selection = Selection::default();
        selection.click(tx(&graph, &ids.salary));
        selection.command_click(&graph, leaf);
        assert_eq!(selection.len(), 2);
        assert!(selection.contains(leaf));
    }

    #[test]
    fn shift_click_selects_path() {
        let (graph, ids, _) = setup();
        let payjoin = graph.tx_index(&ids.payjoin).unwrap();
        let payment = graph.txs()[payjoin]
            .leaves
            .iter()
            .copied()
            .find(|leaf| {
                graph.leaves()[*leaf]
                    .address
                    .as_ref()
                    .is_some_and(|a| *a == fixture::address(30))
            })
            .unwrap();
        let payment = graph.leaf_item(payment);
        let rent0 = tx(&graph, &ids.rent[0]);
        let mut selection = Selection::default();
        selection.click(rent0);
        selection.shift_click(&graph, payment);
        let mut expected = leaves_of(
            &graph,
            &[
                &ids.rent[0],
                &ids.rent[1],
                &ids.rent[2],
                &ids.rent[3],
                &ids.payjoin,
            ],
        );
        expected.insert(payment);
        assert_eq!(selection.items(), &expected);
        assert_eq!(selection.anchor(), Some(rent0));
    }

    #[test]
    fn shift_click_other_chain_only_two() {
        let (graph, ids, _) = setup();
        let (salary, batch) = (tx(&graph, &ids.salary), tx(&graph, &ids.batch));
        let mut selection = Selection::default();
        selection.click(salary);
        selection.shift_click(&graph, batch);
        assert_eq!(selection.items(), &HashSet::from([salary, batch]));
        assert_eq!(selection.anchor(), Some(salary));
    }

    #[test]
    fn shift_click_without_anchor_selects_tx_and_leaves() {
        let (graph, ids, _) = setup();
        let batch = tx(&graph, &ids.batch);
        let index = graph.tx_index(&ids.batch).unwrap();
        let leaves = &graph.txs()[index].leaves;
        assert!(!leaves.is_empty());
        let mut expected: HashSet<ItemId> =
            leaves.iter().map(|leaf| graph.leaf_item(*leaf)).collect();
        expected.insert(batch);

        let mut selection = Selection::default();
        selection.shift_click(&graph, graph.leaf_item(leaves[0]));
        assert_eq!(selection.items(), &expected);
        assert_eq!(selection.anchor(), Some(graph.leaf_item(leaves[0])));

        let mut selection = Selection::default();
        selection.shift_click(&graph, batch);
        assert_eq!(selection.items(), &expected);
        assert_eq!(selection.anchor(), Some(batch));
    }

    #[test]
    fn command_shift_click_adds_chain() {
        let (graph, ids, _) = setup();
        let batch = tx(&graph, &ids.batch);
        let mut selection = Selection::default();
        selection.click(batch);
        selection.command_shift_click(&graph, tx(&graph, &ids.self_transfer));
        let mut expected = leaves_of(&graph, &[&ids.incoming_change, &ids.self_transfer]);
        expected.insert(batch);
        assert_eq!(selection.items(), &expected);
        assert_eq!(selection.anchor(), Some(batch));
    }

    #[test]
    fn area_replace_and_add() {
        let (graph, ids, _) = setup();
        let (a, b, c) = (
            tx(&graph, &ids.salary),
            tx(&graph, &ids.batch),
            tx(&graph, &ids.payjoin),
        );
        let mut selection = Selection::default();
        selection.click(a);
        selection.area([a, b], false);
        assert_eq!(selection.items(), &HashSet::from([a, b]));
        assert_eq!(selection.anchor(), Some(a));
        selection.area([c], true);
        assert_eq!(selection.items(), &HashSet::from([a, b, c]));
        assert_eq!(selection.anchor(), Some(a));
        selection.area([c], false);
        assert_eq!(selection.items(), &HashSet::from([c]));
        assert_eq!(selection.anchor(), None);
    }

    #[test]
    fn selected_txs_ignores_leaves() {
        let (graph, ids, landlord) = setup();
        let salary = graph.tx_index(&ids.salary).unwrap();
        let batch = graph.tx_index(&ids.batch).unwrap();
        let mut selection = Selection::default();
        selection.area(
            [
                graph.tx_item(batch),
                leaf_on(&graph, &landlord, 0),
                graph.tx_item(salary),
            ],
            false,
        );
        assert_eq!(selection.selected_txs(&graph), vec![salary, batch]);
    }

    #[test]
    fn siblings_by_address() {
        let (graph, _, landlord) = setup();
        let leaf = leaf_on(&graph, &landlord, 0);
        let mut selection = Selection::default();
        selection.click(leaf);
        let expected: HashSet<ItemId> = (1..4).map(|n| leaf_on(&graph, &landlord, n)).collect();
        assert_eq!(siblings(&graph, &selection), expected);
    }

    #[test]
    fn siblings_by_label() {
        let mut b = Builder::new();
        b.tx(
            Some(1),
            &[foreign(1)],
            &[
                (ours(0), 1_000, true),
                (fixture::address(40), 500, false),
                (fixture::address(41), 500, false),
                (fixture::address(42), 500, false),
                (fixture::address(43), 500, false),
            ],
        );
        b.label(fixture::address(40), "Same");
        b.label(fixture::address(41), "Same");
        let (txs, coins) = b.finish();
        let graph = TxGraph::new(txs, &coins);
        let leaf = |n: u16| leaf_on(&graph, &fixture::address(n), 0);

        let mut selection = Selection::default();
        selection.click(leaf(40));
        assert_eq!(siblings(&graph, &selection), HashSet::from([leaf(41)]));
        selection.click(leaf(42));
        selection.command_click(&graph, leaf(43));
        assert!(siblings(&graph, &selection).is_empty());
    }

    #[test]
    fn tag_highlight_none_without_tags() {
        assert_eq!(TagHighlight::new(slot(), vec![]), None);
    }

    #[test]
    fn tag_highlight_cycle_steps() {
        let mut highlight = TagHighlight::new(slot(), vec![3, 5]).unwrap();
        assert_eq!(highlight.active_tag(), 3);
        assert!(highlight.cycle(1));
        assert_eq!(highlight.active_tag(), 5);
        assert_eq!(highlight.position(), (2, 2));
        assert!(highlight.cycle(1));
        assert_eq!(highlight.active_tag(), 3);
        assert!(highlight.cycle(-1));
        assert_eq!(highlight.active_tag(), 5);
        assert!(!highlight.cycle(2));
        assert_eq!(highlight.active_tag(), 5);
    }

    #[test]
    fn tag_highlight_single_tag_ignores_cycle() {
        let mut highlight = TagHighlight::new(slot(), vec![7]).unwrap();
        assert!(!highlight.has_many());
        assert!(!highlight.cycle(10));
        assert_eq!(highlight.active_tag(), 7);
    }

    #[test]
    fn cycle_wraps_both_ways() {
        let mut highlight = TagHighlight::new(slot(), vec![1, 2, 3]).unwrap();
        assert!(highlight.cycle(-1));
        assert_eq!(highlight.active_tag(), 3);
        let mut highlight = TagHighlight::new(slot(), vec![1, 2, 3]).unwrap();
        assert!(highlight.cycle(4));
        assert_eq!(highlight.active_tag(), 2);
    }

    #[test]
    fn retain_drops_missing_and_anchor() {
        let (graph, ids, _) = setup();
        let (a, b) = (tx(&graph, &ids.salary), tx(&graph, &ids.batch));
        let mut selection = Selection::default();
        selection.click(a);
        selection.command_click(&graph, b);
        selection.shift_click(&graph, b);
        assert_eq!(selection.anchor(), Some(b));
        selection.click(a);
        selection.area([b], true);
        assert_eq!(selection.anchor(), Some(a));
        selection.retain(|id| id == b);
        assert_eq!(selection.items(), &HashSet::from([b]));
        assert_eq!(selection.anchor(), None);
    }
}
