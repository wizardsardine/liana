use std::collections::{HashMap, HashSet};

use iced::{keyboard::Modifiers, Point, Rectangle};
use liana_ui::{
    component::panels::map::{
        block::{BlockState, SlotState},
        leaf::LeafState,
    },
    widget::graph_view::{ItemId, Side, Target},
};

use crate::app::state::map::{
    coin_ui::CoinUi,
    graph::{MapItem, OutputSlot, SlotRef, TxGraph},
    layout,
    selection::{self, Selection, TagHighlight},
    Orders,
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
            .get(&graph.txs()[tx].history.txid)
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
    let modified = |id: ItemId| match (modifiers.command(), modifiers.shift()) {
        (false, false) => ClickAction::Select(id),
        (true, false) => ClickAction::Toggle(id),
        (false, true) => ClickAction::Range(id),
        (true, true) => ClickAction::Chain(id),
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
) -> DisplayState {
    let siblings = selection::siblings(graph, selection);
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
    let highlighted: HashSet<SlotRef> = (0..graph.leaves().len())
        .filter(|index| leaf_lit(*index))
        .map(|index| graph.leaves()[index].slot())
        .collect();

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
    use std::collections::HashMap;

    use iced::{keyboard::Modifiers, Rectangle};
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
        display::{click_action, display_state, slot_ref, ClickAction, DisplayState},
        fixture,
        graph::{OutputSlot, SlotRef, TxGraph},
        layout,
        selection::{Selection, TagHighlight},
        Orders,
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
        let layout = layout::place(graph, &HashMap::new());
        display_state(
            graph,
            &layout,
            &Orders::new(),
            selection,
            hover,
            tag,
            coin_ui,
            unspent,
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
    fn click_reused_leaf_is_select() {
        let f = fixture::sample_wallet();
        let graph = TxGraph::new(f.txs, &f.coins);
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
        let graph = TxGraph::new(f.txs, &f.coins);
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
        let graph = TxGraph::new(f.txs, &f.coins);
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
        let graph = TxGraph::new(f.txs, &f.coins);
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
        let layout = layout::place(&graph, &HashMap::new());
        let rect = |tx: usize| {
            let id = graph.tx_item(tx);
            Rectangle::new(layout[&id], layout::item_size(&graph, id))
        };
        assert_eq!(s.frame, Some(rect(0).union(&rect(1))));
        assert_eq!(s.blocks[0], (BlockState::Default, true));
    }
}
