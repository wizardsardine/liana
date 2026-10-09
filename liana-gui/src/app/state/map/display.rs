use std::collections::{HashMap, HashSet};

use iced::{keyboard::Modifiers, Point, Rectangle};
use liana::miniscript::bitcoin::Address;
use liana_ui::{
    component::panels::map::{
        block::{BlockState, SlotState},
        leaf::LeafState,
    },
    widget::graph_view::{ItemId, Side, Target},
};

use crate::app::state::map::{
    coin_ui::CoinUi,
    focus::ShowOnMap,
    graph::{InputSlot, LeafKind, MapItem, OutputSlot, SlotRef, TxGraph},
    layout,
    selection::{self, Selection, TagHighlight},
    wallets::WalletKey,
    LabelTarget, Orders,
};

/// Inverse of `display_row`: the true index of the slot shown at `row`.
pub fn true_index(order: Option<&[u32]>, row: usize) -> usize {
    order
        .and_then(|order| order.get(row))
        .map_or(row, |&index| index as usize)
}

/// The slot shown at display `row` of a block, `None` when `item` is not a block.
pub fn slot_ref(
    graph: &TxGraph,
    orders: &Orders,
    item: ItemId,
    side: Side,
    row: usize,
) -> Option<SlotRef> {
    let MapItem::Tx(tx) = graph.item(item)? else {
        return None;
    };
    let order =
        orders
            .get(&graph.txs()[tx].history().txid)
            .and_then(|(inputs, outputs)| match side {
                Side::Input => inputs.as_deref(),
                Side::Output => outputs.as_deref(),
            });
    Some(SlotRef {
        tx,
        side,
        index: true_index(order, row),
    })
}

/// Key of the label edited from `target` (spec 12.1), in the `LabelItem` string format.
pub fn label_key(graph: &TxGraph, target: &LabelTarget) -> Option<String> {
    match *target {
        LabelTarget::Tx(tx) => Some(graph.txs().get(tx)?.history().txid.to_string()),
        LabelTarget::Slot(slot) => {
            let tx = graph.txs().get(slot.tx)?;
            let outpoint = match slot.side {
                Side::Input => match tx.inputs.get(slot.index)? {
                    InputSlot::OurCoin { outpoint, .. }
                    | InputSlot::CounterpartyCoin { outpoint, .. } => outpoint,
                },
                Side::Output => match tx.outputs.get(slot.index)? {
                    OutputSlot::OurCoin { outpoint, .. }
                    | OutputSlot::Payment { outpoint, .. }
                    | OutputSlot::CounterpartyOutput { outpoint, .. } => outpoint,
                },
            };
            Some(outpoint.to_string())
        }
        LabelTarget::Leaf(leaf) => {
            let leaf = graph.leaves().get(leaf)?;
            Some(match (leaf.kind, &leaf.address) {
                (LeafKind::CounterpartyCoin, _) | (_, None) => leaf.outpoint.to_string(),
                (_, Some(address)) => address.to_string(),
            })
        }
    }
}

/// The wallet the label edited from `target` is saved to: the coin's wallet for a slot holding
/// one, else the primary wallet of the transaction.
pub fn label_wallet<'a>(graph: &'a TxGraph, target: &LabelTarget) -> Option<&'a WalletKey> {
    let tx = match *target {
        LabelTarget::Tx(tx) => tx,
        LabelTarget::Slot(slot) => match graph.slot_wallet(slot) {
            Some(wallet) => return Some(wallet),
            None => slot.tx,
        },
        LabelTarget::Leaf(leaf) => graph.leaves().get(leaf)?.tx,
    };
    Some(graph.txs().get(tx)?.primary())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClickAction {
    None,
    /// Plain click on a block body or a leaf.
    Select(ItemId),
    /// Command click.
    Toggle(ItemId),
    /// Shift click: anchor to target along the shortest chain path.
    Range(ItemId),
    /// Command and shift click: add the whole chain.
    Chain(ItemId),
    /// Command and alt click: every item of the item's wallet.
    SelectWallet(ItemId),
    /// Plain click on a slot.
    TagHighlight(SlotRef),
}

/// Spec 8: a slot clicked with a modifier counts as its transaction.
pub fn click_action(
    graph: &TxGraph,
    orders: &Orders,
    target: &Target,
    modifiers: Modifiers,
) -> ClickAction {
    let modified = |id: ItemId| match (modifiers.command(), modifiers.alt(), modifiers.shift()) {
        (true, true, _) => ClickAction::SelectWallet(id),
        (false, _, false) => ClickAction::Select(id),
        (true, false, false) => ClickAction::Toggle(id),
        (false, _, true) => ClickAction::Range(id),
        (true, false, true) => ClickAction::Chain(id),
    };
    match *target {
        Target::Item(id) => modified(id),
        Target::Slot(item, _, _) if modifiers.command() || modifiers.shift() => modified(item),
        Target::Slot(item, side, row) => slot_ref(graph, orders, item, side, row)
            .map_or(ClickAction::None, ClickAction::TagHighlight),
        Target::Edge(_) | Target::Frame => ClickAction::None,
    }
}

pub struct DisplayState {
    /// Per transaction index: state and group membership.
    pub blocks: Vec<(BlockState, bool)>,
    /// Per leaf index: state and group membership.
    pub leaves: Vec<(LeafState, bool)>,
    /// Missing slots are `SlotState::Default`.
    pub slots: HashMap<SlotRef, SlotState>,
    /// Active flags, `graph.coin_edges()` order.
    pub coin_edges: Vec<bool>,
    /// Active flags, `graph.leaves()` order.
    pub leaf_edges: Vec<bool>,
    /// Bounding box of the selected items when 2 or more are selected.
    pub frame: Option<Rectangle>,
}

/// Display states of blocks, slots, leaves and edges (spec 5.1 to 5.3, 9, 10.3 to 10.5).
#[allow(clippy::too_many_arguments)]
pub fn display_state(
    graph: &TxGraph,
    layout: &HashMap<ItemId, Point>,
    orders: &Orders,
    selection: &Selection,
    hover: Option<&Target>,
    tag: Option<&TagHighlight>,
    coin_ui: &CoinUi,
    unspent: bool,
    reuse: Option<&Address>,
    show_on_map: Option<&ShowOnMap>,
) -> DisplayState {
    let mut siblings = selection::siblings(graph, selection);
    if let Some(leaf) = show_on_map.and_then(|show| show.leaf) {
        siblings.insert(graph.leaf_item(leaf));
    }
    if let Some(address) = reuse {
        siblings.extend(
            graph
                .leaves_on_address(address)
                .iter()
                .map(|leaf| graph.leaf_item(*leaf)),
        );
    }
    let in_group = selection.is_group();
    let single_tx = selection
        .items()
        .iter()
        .next()
        .filter(|_| selection.len() == 1)
        .and_then(|id| match graph.item(*id) {
            Some(MapItem::Tx(tx)) => Some(tx),
            _ => None,
        });

    let hovered_item = match hover {
        Some(Target::Item(id) | Target::Slot(id, _, _)) => Some(*id),
        _ => None,
    };
    let hovered_tx = hovered_item.and_then(|id| match graph.item(id) {
        Some(MapItem::Tx(tx)) => Some(tx),
        _ => None,
    });
    let hovered_edge = match hover {
        Some(Target::Edge(edge)) => Some(*edge),
        _ => None,
    };
    let coin_edge_count = graph.coin_edges().len();
    let mut hovered_slots = HashSet::new();
    match hover {
        Some(Target::Slot(item, side, row)) => {
            hovered_slots.extend(slot_ref(graph, orders, *item, *side, *row));
        }
        Some(Target::Edge(edge)) if *edge < coin_edge_count => {
            let edge = &graph.coin_edges()[*edge];
            hovered_slots.extend([edge.from, edge.to]);
        }
        Some(Target::Edge(edge)) => {
            if let Some(leaf) = graph.leaves().get(*edge - coin_edge_count) {
                hovered_slots.insert(leaf.slot());
            }
        }
        _ => {}
    }

    let mut tag_siblings = HashSet::new();
    if let Some(tag) = tag {
        for coin in coin_ui.coins_with_tag(tag.active_tag()) {
            tag_siblings.extend(graph.output_slot(&coin));
            tag_siblings.extend(graph.spending_input(&coin));
        }
    }

    let leaf_lit = |index: usize| {
        let id = graph.leaf_item(index);
        selection.contains(id) || siblings.contains(&id)
    };
    let mut highlighted: HashSet<SlotRef> = (0..graph.leaves().len())
        .filter(|index| leaf_lit(*index))
        .map(|index| graph.leaves()[index].slot())
        .collect();
    highlighted.extend(
        show_on_map
            .iter()
            .flat_map(|show| show.slots.iter().copied()),
    );

    let blocks = (0..graph.txs().len())
        .map(|tx| {
            let id = graph.tx_item(tx);
            let selected = selection.contains(id);
            let mut state = if selected && !in_group {
                BlockState::Selected
            } else if hovered_item == Some(id) {
                BlockState::Hover
            } else {
                BlockState::Default
            };
            let has_unspent_output = graph.txs()[tx].outputs.iter().any(|slot| {
                matches!(slot, OutputSlot::OurCoin { outpoint, .. } if graph.is_unspent(outpoint))
            });
            if unspent && !selected && !has_unspent_output && state == BlockState::Default {
                state = BlockState::Dimmed;
            }
            (state, selected && in_group)
        })
        .collect();

    let leaves = (0..graph.leaves().len())
        .map(|index| {
            let id = graph.leaf_item(index);
            let selected = selection.contains(id);
            let mut state = if selected && !in_group {
                LeafState::Selected
            } else if siblings.contains(&id) && !selected {
                LeafState::Sibling
            } else if hovered_item == Some(id) {
                LeafState::Hover
            } else {
                LeafState::Default
            };
            if unspent && !selected && state == LeafState::Default {
                state = LeafState::Dimmed;
            }
            (state, selected && in_group)
        })
        .collect();

    let mut slots = HashMap::new();
    for (tx, map_tx) in graph.txs().iter().enumerate() {
        let sides = [
            (Side::Input, map_tx.inputs.len()),
            (Side::Output, map_tx.outputs.len()),
        ];
        for (side, len) in sides {
            for index in 0..len {
                let slot = SlotRef { tx, side, index };
                let holds_unspent = graph
                    .slot_coin(slot)
                    .is_some_and(|coin| side == Side::Output && graph.is_unspent(&coin));
                let state = if tag_siblings.contains(&slot) {
                    SlotState::TagSibling
                } else if highlighted.contains(&slot) {
                    SlotState::Highlighted
                } else if hovered_slots.contains(&slot) {
                    SlotState::Hover
                } else if holds_unspent && unspent {
                    SlotState::Unspent
                } else if unspent {
                    SlotState::Dimmed
                } else {
                    continue;
                };
                slots.insert(slot, state);
            }
        }
    }

    let coin_edges = graph
        .coin_edges()
        .iter()
        .enumerate()
        .map(|(index, edge)| {
            let ends = [edge.from, edge.to];
            hovered_edge == Some(index)
                || ends.iter().any(|end| hovered_slots.contains(end))
                || ends.iter().any(|end| Some(end.tx) == hovered_tx)
                || ends.iter().any(|end| Some(end.tx) == single_tx)
                || ends.iter().all(|end| tag_siblings.contains(end))
                || ends.iter().all(|end| highlighted.contains(end))
        })
        .collect();

    let leaf_edges = graph
        .leaves()
        .iter()
        .enumerate()
        .map(|(index, leaf)| {
            hovered_edge == Some(coin_edge_count + index)
                || hovered_item == Some(graph.leaf_item(index))
                || hovered_slots.contains(&leaf.slot())
                || Some(leaf.tx) == hovered_tx
                || Some(leaf.tx) == single_tx
                || leaf_lit(index)
        })
        .collect();

    let frame = in_group
        .then(|| {
            selection
                .items()
                .iter()
                .filter_map(|id| {
                    let at = layout.get(id)?;
                    Some(Rectangle::new(*at, layout::item_size(graph, *id)))
                })
                .reduce(|a, b| a.union(&b))
        })
        .flatten();

    DisplayState {
        blocks,
        leaves,
        slots,
        coin_edges,
        leaf_edges,
        frame,
    }
}

#[cfg(test)]
mod tests {
    use iced::{keyboard::Modifiers, Rectangle};
    use liana::miniscript::bitcoin::{Address, OutPoint, Txid};
    use liana_ui::{
        component::panels::map::{
            block::{BlockState, SlotState},
            leaf::LeafState,
        },
        widget::graph_view::{ItemId, Side, Target},
    };

    use crate::app::state::map::{
        coin_ui::CoinUi,
        display::{
            click_action, display_state, label_key, label_wallet, slot_ref, ClickAction,
            DisplayState,
        },
        fixture,
        focus::ShowOnMap,
        graph::{OutputSlot, SlotRef, TxGraph},
        layout,
        selection::{Selection, TagHighlight},
        wallets::WalletKey,
        LabelTarget, Orders,
    };

    const NONE: Modifiers = Modifiers::empty();

    fn state(
        graph: &TxGraph,
        selection: &Selection,
        hover: Option<&Target>,
        tag: Option<&TagHighlight>,
        coin_ui: &CoinUi,
        unspent: bool,
    ) -> DisplayState {
        let layout = layout::reset(graph, &WalletKey::Current);
        display_state(
            graph,
            &layout,
            &Orders::new(),
            selection,
            hover,
            tag,
            coin_ui,
            unspent,
            None,
            None,
        )
    }

    fn landlord_leaves(graph: &TxGraph, address: &Address) -> Vec<ItemId> {
        graph
            .leaves_on_address(address)
            .iter()
            .map(|leaf| graph.leaf_item(*leaf))
            .collect()
    }

    #[test]
    fn click_block_selects() {
        let graph = fixture::graph();
        let id = graph.tx_item(0);
        let action = click_action(&graph, &Orders::new(), &Target::Item(id), NONE);
        assert_eq!(action, ClickAction::Select(id));
    }

    #[test]
    fn click_slot_highlights_tag() {
        let graph = fixture::graph();
        let id = graph.tx_item(0);
        let target = Target::Slot(id, Side::Output, 0);
        let action = click_action(&graph, &Orders::new(), &target, NONE);
        let slot = SlotRef {
            tx: 0,
            side: Side::Output,
            index: 0,
        };
        assert_eq!(action, ClickAction::TagHighlight(slot));
    }

    #[test]
    fn command_click_slot_toggles_its_tx() {
        let graph = fixture::graph();
        let id = graph.tx_item(1);
        let target = Target::Slot(id, Side::Input, 0);
        let action = click_action(&graph, &Orders::new(), &target, Modifiers::COMMAND);
        assert_eq!(action, ClickAction::Toggle(id));
    }

    #[test]
    fn shift_click_leaf_is_range() {
        let graph = fixture::graph();
        let id = graph.leaf_item(0);
        let action = click_action(&graph, &Orders::new(), &Target::Item(id), Modifiers::SHIFT);
        assert_eq!(action, ClickAction::Range(id));
    }

    #[test]
    fn command_shift_click_is_chain() {
        let graph = fixture::graph();
        let id = graph.tx_item(2);
        let modifiers = Modifiers::COMMAND | Modifiers::SHIFT;
        let action = click_action(&graph, &Orders::new(), &Target::Item(id), modifiers);
        assert_eq!(action, ClickAction::Chain(id));
    }

    #[test]
    fn command_alt_click_selects_the_wallet() {
        let graph = fixture::graph();
        let modifiers = Modifiers::COMMAND | Modifiers::ALT;
        let tx = graph.tx_item(1);
        let leaf = graph.leaf_item(0);
        let targets = [
            (Target::Item(tx), tx),
            (Target::Slot(tx, Side::Input, 0), tx),
            (Target::Item(leaf), leaf),
        ];
        for (target, id) in targets {
            let action = click_action(&graph, &Orders::new(), &target, modifiers);
            assert_eq!(action, ClickAction::SelectWallet(id));
            let with_shift = click_action(
                &graph,
                &Orders::new(),
                &target,
                modifiers | Modifiers::SHIFT,
            );
            assert_eq!(with_shift, ClickAction::SelectWallet(id));
        }
        let alt = click_action(&graph, &Orders::new(), &Target::Item(tx), Modifiers::ALT);
        assert_eq!(alt, ClickAction::Select(tx));
    }

    #[test]
    fn click_reused_leaf_is_select() {
        let f = fixture::sample_wallet();
        let graph = fixture::current_graph(f.txs, f.coins);
        let id = landlord_leaves(&graph, &f.landlord)[0];
        let action = click_action(&graph, &Orders::new(), &Target::Item(id), NONE);
        assert_eq!(action, ClickAction::Select(id));
    }

    #[test]
    fn click_edge_or_frame_does_nothing() {
        let graph = fixture::graph();
        for target in [Target::Edge(0), Target::Frame] {
            let action = click_action(&graph, &Orders::new(), &target, NONE);
            assert_eq!(action, ClickAction::None);
        }
    }

    #[test]
    fn slot_target_uses_display_row() {
        let f = fixture::sample_wallet();
        let graph = fixture::current_graph(f.txs, f.coins);
        let tx = graph.tx_index(&f.ids.incoming_change).unwrap();
        let id = graph.tx_item(tx);
        let mut orders = Orders::new();
        orders.insert(f.ids.incoming_change, (None, Some(vec![1, 0])));
        let slot = slot_ref(&graph, &orders, id, Side::Output, 0).unwrap();
        assert_eq!(slot.index, 1);
        let target = Target::Slot(id, Side::Output, 0);
        let action = click_action(&graph, &orders, &target, NONE);
        assert_eq!(action, ClickAction::TagHighlight(slot));
        assert!(slot_ref(&graph, &orders, graph.leaf_item(0), Side::Output, 0).is_none());
    }

    #[test]
    fn edge_inactive_by_default() {
        let graph = fixture::graph();
        let s = state(
            &graph,
            &Selection::default(),
            None,
            None,
            &CoinUi::default(),
            false,
        );
        assert!(s
            .coin_edges
            .iter()
            .chain(&s.leaf_edges)
            .all(|active| !active));
    }

    #[test]
    fn edge_active_when_hovered() {
        let graph = fixture::graph();
        let s = state(
            &graph,
            &Selection::default(),
            Some(&Target::Edge(0)),
            None,
            &CoinUi::default(),
            false,
        );
        assert!(s.coin_edges[0]);
        assert_eq!(s.coin_edges.iter().filter(|a| **a).count(), 1);
        let edge = graph.coin_edges()[0];
        assert_eq!(s.slots[&edge.from], SlotState::Hover);
        assert_eq!(s.slots[&edge.to], SlotState::Hover);
    }

    #[test]
    fn edge_active_when_end_hovered() {
        let graph = fixture::graph();
        let edge = graph.coin_edges()[0];
        let id = graph.tx_item(edge.from.tx);
        let hover = Target::Slot(id, edge.from.side, edge.from.index);
        let s = state(
            &graph,
            &Selection::default(),
            Some(&hover),
            None,
            &CoinUi::default(),
            false,
        );
        assert!(s.coin_edges[0]);
        assert_eq!(s.slots[&edge.from], SlotState::Hover);
        assert_eq!(s.blocks[edge.from.tx].0, BlockState::Hover);
    }

    #[test]
    fn edge_active_when_touching_selected_tx() {
        let graph = fixture::graph();
        let edge = graph.coin_edges()[0];
        let mut selection = Selection::default();
        selection.click(graph.tx_item(edge.to.tx));
        let s = state(&graph, &selection, None, None, &CoinUi::default(), false);
        assert!(s.coin_edges[0]);
        assert_eq!(s.blocks[edge.to.tx], (BlockState::Selected, false));
    }

    #[test]
    fn edge_active_when_both_ends_tag_siblings() {
        let graph = fixture::graph();
        let edge = graph.coin_edges()[0];
        let mut coin_ui = CoinUi::default();
        coin_ui.toggle_tag(edge.outpoint, 0);
        let highlight = TagHighlight::new(edge.from, vec![0]).unwrap();
        let s = state(
            &graph,
            &Selection::default(),
            None,
            Some(&highlight),
            &coin_ui,
            false,
        );
        assert!(s.coin_edges[0]);
    }

    #[test]
    fn edge_active_when_touching_sibling_leaf() {
        let f = fixture::sample_wallet();
        let graph = fixture::current_graph(f.txs, f.coins);
        let leaves = landlord_leaves(&graph, &f.landlord);
        let mut selection = Selection::default();
        selection.click(leaves[0]);
        let s = state(&graph, &selection, None, None, &CoinUi::default(), false);
        for id in &leaves[1..] {
            let index = id.0 as usize - graph.txs().len();
            assert!(s.leaf_edges[index]);
            assert_eq!(s.leaves[index].0, LeafState::Sibling);
            assert_eq!(
                s.slots[&graph.leaves()[index].slot()],
                SlotState::Highlighted
            );
        }
    }

    #[test]
    fn tag_siblings_include_spending_input() {
        let graph = fixture::graph();
        let edge = graph.coin_edges()[0];
        let mut coin_ui = CoinUi::default();
        coin_ui.toggle_tag(edge.outpoint, 0);
        let highlight = TagHighlight::new(edge.from, vec![0]).unwrap();
        let s = state(
            &graph,
            &Selection::default(),
            None,
            Some(&highlight),
            &coin_ui,
            false,
        );
        assert_eq!(s.slots[&edge.from], SlotState::TagSibling);
        assert_eq!(s.slots[&edge.to], SlotState::TagSibling);
    }

    #[test]
    fn reused_leaf_selection_marks_address_siblings() {
        let f = fixture::sample_wallet();
        let graph = fixture::current_graph(f.txs, f.coins);
        let leaves = landlord_leaves(&graph, &f.landlord);
        assert_eq!(leaves.len(), 4);
        let mut selection = Selection::default();
        selection.click(leaves[0]);
        let s = state(&graph, &selection, None, None, &CoinUi::default(), false);
        let siblings = s
            .leaves
            .iter()
            .filter(|(state, _)| *state == LeafState::Sibling)
            .count();
        assert_eq!(siblings, 3);
    }

    #[test]
    fn reuse_highlights_every_leaf_of_the_address() {
        let f = fixture::sample_wallet();
        let graph = fixture::current_graph(f.txs, f.coins);
        let layout = layout::reset(&graph, &WalletKey::Current);
        let s = display_state(
            &graph,
            &layout,
            &Orders::new(),
            &Selection::default(),
            None,
            None,
            &CoinUi::default(),
            false,
            Some(&f.landlord),
            None,
        );
        let leaves = graph.leaves_on_address(&f.landlord);
        assert_eq!(leaves.len(), 4);
        for index in leaves {
            assert_eq!(s.leaves[*index].0, LeafState::Sibling);
            assert!(s.leaf_edges[*index]);
        }
    }

    #[test]
    fn show_on_map_marks_slots_highlighted() {
        let f = fixture::sample_wallet();
        let graph = fixture::current_graph(f.txs, f.coins);
        let layout = layout::reset(&graph, &WalletKey::Current);
        let tx = graph.tx_index(&f.ids.rent[0]).unwrap();
        let OutputSlot::Payment { leaf, .. } = graph.txs()[tx].outputs[0] else {
            panic!("rent output 0 is a payment");
        };
        let slot = SlotRef {
            tx,
            side: Side::Output,
            index: 0,
        };
        let show = ShowOnMap {
            slots: vec![slot],
            leaf: Some(leaf),
        };
        let s = display_state(
            &graph,
            &layout,
            &Orders::new(),
            &Selection::default(),
            None,
            None,
            &CoinUi::default(),
            false,
            None,
            Some(&show),
        );
        assert_eq!(s.slots[&slot], SlotState::Highlighted);
        assert_eq!(s.leaves[leaf].0, LeafState::Sibling);
        assert!(s.leaf_edges[leaf]);
    }

    #[test]
    fn unspent_mode_states() {
        let graph = fixture::graph();
        let s = state(
            &graph,
            &Selection::default(),
            None,
            None,
            &CoinUi::default(),
            true,
        );
        let has_unspent = |tx: usize| {
            graph.txs()[tx].outputs.iter().any(|slot| {
                matches!(slot, OutputSlot::OurCoin { outpoint, .. } if graph.is_unspent(outpoint))
            })
        };
        let with = (0..graph.txs().len()).find(|tx| has_unspent(*tx)).unwrap();
        let without = (0..graph.txs().len()).find(|tx| !has_unspent(*tx)).unwrap();
        assert_eq!(s.blocks[without].0, BlockState::Dimmed);
        assert_eq!(s.blocks[with].0, BlockState::Default);
        let (coin, _) = graph.unspent_coins()[0];
        let unspent_slot = graph.output_slot(&coin).unwrap();
        assert_eq!(s.slots[&unspent_slot], SlotState::Unspent);
        let other = SlotRef {
            tx: without,
            side: Side::Input,
            index: 0,
        };
        assert_eq!(s.slots[&other], SlotState::Dimmed);
        assert!(s
            .leaves
            .iter()
            .all(|(state, _)| *state == LeafState::Dimmed));
    }

    #[test]
    fn group_frame_only_for_two_or_more() {
        let graph = fixture::graph();
        let mut selection = Selection::default();
        selection.click(graph.tx_item(0));
        let s = state(&graph, &selection, None, None, &CoinUi::default(), false);
        assert!(s.frame.is_none());

        selection.command_click(&graph, graph.tx_item(1));
        let s = state(&graph, &selection, None, None, &CoinUi::default(), false);
        let layout = layout::reset(&graph, &WalletKey::Current);
        let rect = |tx: usize| {
            let id = graph.tx_item(tx);
            Rectangle::new(layout[&id], layout::item_size(&graph, id))
        };
        assert_eq!(s.frame, Some(rect(0).union(&rect(1))));
        assert_eq!(s.blocks[0], (BlockState::Default, true));
    }

    fn slot(graph: &TxGraph, txid: Txid, side: Side, index: usize) -> LabelTarget {
        LabelTarget::Slot(SlotRef {
            tx: graph.tx_index(&txid).unwrap(),
            side,
            index,
        })
    }

    fn leaf_of(graph: &TxGraph, outpoint: OutPoint) -> LabelTarget {
        LabelTarget::Leaf(
            graph
                .leaves()
                .iter()
                .position(|leaf| leaf.outpoint == outpoint)
                .unwrap(),
        )
    }

    #[test]
    fn label_wallet_is_the_owner() {
        let two = fixture::two_wallets("a", "b");
        let (payment, spend) = (two.payment, two.spend);
        let graph = TxGraph::new(two.wallets);
        let tx = graph.tx_index(&payment).unwrap();
        let wallet = |target: LabelTarget| label_wallet(&graph, &target).cloned();
        assert_eq!(wallet(LabelTarget::Tx(tx)), Some(WalletKey::Current));
        assert_eq!(
            wallet(slot(&graph, payment, Side::Output, 0)),
            Some(two.b.clone())
        );
        assert_eq!(
            wallet(slot(&graph, payment, Side::Output, 1)),
            Some(WalletKey::Current)
        );
        let shop = graph.txs()[graph.tx_index(&spend).unwrap()].leaves[0];
        assert_eq!(wallet(LabelTarget::Leaf(shop)), Some(two.b));
    }

    #[test]
    fn label_key_transaction() {
        let f = fixture::sample_wallet();
        let graph = fixture::current_graph(f.txs, f.coins);
        let target = LabelTarget::Tx(graph.tx_index(&f.ids.salary).unwrap());
        assert_eq!(label_key(&graph, &target), Some(f.ids.salary.to_string()));
    }

    #[test]
    fn label_key_own_output_slot_is_the_coin() {
        let f = fixture::sample_wallet();
        let graph = fixture::current_graph(f.txs, f.coins);
        let target = slot(&graph, f.ids.salary, Side::Output, 0);
        let coin = OutPoint::new(f.ids.salary, 0);
        assert_eq!(label_key(&graph, &target), Some(coin.to_string()));
    }

    #[test]
    fn label_key_own_input_slot_is_the_spent_coin() {
        let f = fixture::sample_wallet();
        let graph = fixture::current_graph(f.txs, f.coins);
        let target = slot(&graph, f.ids.rent[0], Side::Input, 0);
        let coin = OutPoint::new(f.ids.salary, 0);
        assert_eq!(label_key(&graph, &target), Some(coin.to_string()));
    }

    #[test]
    fn label_key_counterparty_input_slot_is_its_leaf_key() {
        let f = fixture::sample_wallet();
        let graph = fixture::current_graph(f.txs, f.coins);
        let target = slot(&graph, f.ids.salary, Side::Input, 0);
        let leaf = leaf_of(&graph, fixture::foreign(1));
        assert_eq!(
            label_key(&graph, &target),
            Some(fixture::foreign(1).to_string())
        );
        assert_eq!(label_key(&graph, &target), label_key(&graph, &leaf));
    }

    #[test]
    fn label_key_payment_output_slot_is_the_outpoint() {
        let f = fixture::sample_wallet();
        let graph = fixture::current_graph(f.txs, f.coins);
        let target = slot(&graph, f.ids.rent[0], Side::Output, 0);
        let outpoint = OutPoint::new(f.ids.rent[0], 0);
        assert!(matches!(
            graph.txs()[graph.tx_index(&f.ids.rent[0]).unwrap()].outputs[0],
            OutputSlot::Payment { .. }
        ));
        assert_eq!(label_key(&graph, &target), Some(outpoint.to_string()));
    }

    #[test]
    fn label_key_address_leaf_is_the_address() {
        let f = fixture::sample_wallet();
        let graph = fixture::current_graph(f.txs, f.coins);
        let target = leaf_of(&graph, OutPoint::new(f.ids.rent[0], 0));
        assert_eq!(label_key(&graph, &target), Some(f.landlord.to_string()));
    }

    #[test]
    fn label_key_counterparty_coin_leaf_is_the_outpoint() {
        let graph = fixture::graph();
        let target = leaf_of(&graph, fixture::foreign(1));
        assert_eq!(
            label_key(&graph, &target),
            Some(fixture::foreign(1).to_string())
        );
    }
}
