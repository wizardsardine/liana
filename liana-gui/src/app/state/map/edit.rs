use std::collections::{BTreeSet, HashMap};

use iced::{Point, Vector};
use liana_ui::widget::graph_view::{geometry::snap_to_grid, ItemId, Side};
use lianad::commands::GraphItem;

use crate::app::{
    settings::WalletId,
    state::map::{
        graph::TxGraph,
        history::{Change, LayoutState},
        offsets::Offsets,
        wallets::WalletKey,
        Orders,
    },
};

/// `(id, before, after)` for each moved item. With `snap`, each item lands on the grid on its own.
pub fn moved_positions(
    layout: &HashMap<ItemId, Point>,
    items: &[ItemId],
    delta: Vector,
    snap: bool,
) -> Vec<(ItemId, Point, Point)> {
    items
        .iter()
        .filter_map(|id| {
            let before = *layout.get(id)?;
            let after = before + delta;
            Some((*id, before, if snap { snap_to_grid(after) } else { after }))
        })
        .collect()
}

/// The other wallet whose items are exactly `items`: dragging them moves its offset.
pub fn dragged_wallet(graph: &TxGraph, items: &[ItemId]) -> Option<WalletId> {
    let Some(WalletKey::Other(id)) = graph.item_wallet(*items.first()?) else {
        return None;
    };
    let dragged: BTreeSet<ItemId> = items.iter().copied().collect();
    let wallet: BTreeSet<ItemId> = graph
        .wallet_items(&WalletKey::Other(id.clone()))
        .into_iter()
        .collect();
    (dragged == wallet).then(|| id.clone())
}

/// The offset after a drag by `delta`. With `snap`, it lands on the grid.
pub fn moved_offset(offset: Vector, delta: Vector, snap: bool) -> Vector {
    let after = Point::ORIGIN + offset + delta;
    (if snap { snap_to_grid(after) } else { after }) - Point::ORIGIN
}

/// Sets the offset of `wallet` and moves its items with it, false when it is not on the map.
pub fn apply_offset(
    graph: &TxGraph,
    layout: &mut HashMap<ItemId, Point>,
    offsets: &mut Offsets,
    wallet: &WalletId,
    offset: Vector,
) -> bool {
    let key = WalletKey::Other(wallet.clone());
    let Some(before) = offsets.get(&key) else {
        return false;
    };
    for id in graph.wallet_items(&key) {
        if let Some(p) = layout.get_mut(&id) {
            *p += offset - before;
        }
    }
    offsets.set(wallet.clone(), offset);
    true
}

/// The column in live order, the slot at row `from` moved to row `to`.
pub fn live_column<T: Clone>(slots: &[T], from: usize, to: usize) -> Vec<T> {
    let mut column = slots.to_vec();
    if from < column.len() && to < column.len() {
        let slot = column.remove(from);
        column.insert(to, slot);
    }
    column
}

/// The new display order of a column, `None` when it is the true order.
pub fn reorder_column(
    order: Option<&[u32]>,
    len: usize,
    from: usize,
    to: usize,
) -> Option<Vec<u32>> {
    let current: Vec<u32> = order.map_or_else(|| (0..len as u32).collect(), <[u32]>::to_vec);
    let moved = live_column(&current, from, to);
    let identity = moved.iter().copied().eq(0..len as u32);
    (!identity).then_some(moved)
}

pub fn layout_state(
    graph: &TxGraph,
    layout: &HashMap<ItemId, Point>,
    orders: &Orders,
) -> LayoutState {
    LayoutState {
        positions: layout
            .iter()
            .filter_map(|(id, point)| Some((graph.graph_item(*id)?, *point)))
            .collect(),
        orders: orders.clone(),
    }
}

/// Sets the `after` state of a layout change and returns the items to persist.
pub fn apply_layout_change(
    graph: &TxGraph,
    layout: &mut HashMap<ItemId, Point>,
    orders: &mut Orders,
    change: &Change,
) -> Vec<ItemId> {
    let mut touched = BTreeSet::new();
    match change {
        Change::Move(moves) => {
            for (item, _, after) in moves {
                if let Some(id) = graph.item_id(item) {
                    layout.insert(id, *after);
                    touched.insert(id);
                }
            }
        }
        Change::Reorder {
            tx, side, after, ..
        } => {
            if let Some(id) = graph.item_id(&GraphItem::Tx(*tx)) {
                let entry = orders.entry(*tx).or_default();
                match side {
                    Side::Input => entry.0 = after.clone(),
                    Side::Output => entry.1 = after.clone(),
                }
                if *entry == (None, None) {
                    orders.remove(tx);
                }
                touched.insert(id);
            }
        }
        Change::Layout { after, .. } => {
            touched.extend(
                orders
                    .keys()
                    .filter_map(|txid| graph.item_id(&GraphItem::Tx(*txid))),
            );
            for (item, point) in &after.positions {
                if let Some(id) = graph.item_id(item) {
                    layout.insert(id, *point);
                    touched.insert(id);
                }
            }
            *orders = after
                .orders
                .iter()
                .filter(|(txid, _)| graph.item_id(&GraphItem::Tx(**txid)).is_some())
                .map(|(txid, order)| (*txid, order.clone()))
                .collect();
            touched.extend(
                orders
                    .keys()
                    .filter_map(|txid| graph.item_id(&GraphItem::Tx(*txid))),
            );
        }
        _ => {}
    }
    touched.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use iced::{Point, Vector};
    use liana_ui::widget::graph_view::{geometry::snap_to_grid, ItemId};
    use lianad::commands::GraphItem;

    use crate::{
        app::{
            settings::WalletId,
            state::map::{
                edit::{
                    apply_layout_change, apply_offset, dragged_wallet, layout_state, live_column,
                    moved_offset, moved_positions, reorder_column,
                },
                fixture::{self, foreign},
                graph::TxGraph,
                history::{Change, History},
                layout::{place, reset},
                offsets::Offsets,
                wallets::WalletKey,
                Orders,
            },
        },
        daemon::model::LabelItem,
    };

    #[test]
    fn moved_items_snap_each_on_their_own() {
        let layout = HashMap::from([
            (ItemId(0), Point::new(5.0, 7.0)),
            (ItemId(1), Point::new(20.0, 31.0)),
        ]);
        let items = [ItemId(0), ItemId(1)];
        let delta = Vector::new(10.0, 10.0);
        let snapped = moved_positions(&layout, &items, delta, true);
        for (id, before, after) in &snapped {
            assert_eq!(*before, layout[id]);
            assert_eq!(*after, snap_to_grid(*before + delta));
        }
        assert_ne!(snapped[0].2 - snapped[0].1, snapped[1].2 - snapped[1].1);
        let raw = moved_positions(&layout, &items, delta, false);
        assert_eq!(raw[0].2, Point::new(15.0, 17.0));
        assert_eq!(raw[1].2, Point::new(30.0, 41.0));
    }

    #[test]
    fn reorder_column_moves_and_resets() {
        let moved = reorder_column(None, 3, 0, 2);
        assert_eq!(moved, Some(vec![1, 2, 0]));
        assert_eq!(reorder_column(moved.as_deref(), 3, 2, 0), None);
    }

    #[test]
    fn live_column_matches_reorder() {
        assert_eq!(live_column(&['a', 'b', 'c'], 0, 2), vec!['b', 'c', 'a']);
    }

    #[test]
    fn undo_move_restores_positions() {
        let graph = fixture::graph();
        let mut layout = place(&graph, &WalletKey::Current, &HashMap::new());
        let mut orders = Orders::new();
        let id = graph.tx_item(0);
        let start = layout[&id];
        let moved = moved_positions(&layout, &[id], Vector::new(48.0, 24.0), false);
        let change = Change::Move(
            moved
                .iter()
                .map(|(id, before, after)| (graph.graph_item(*id).unwrap(), *before, *after))
                .collect(),
        );
        let mut history = History::default();
        history.record(change.clone());
        layout.insert(id, moved[0].2);

        let undo = history.undo();
        let touched = apply_layout_change(&graph, &mut layout, &mut orders, &undo.unwrap());
        assert_eq!(layout[&id], start);
        assert_eq!(touched, vec![id]);

        let redo = history.redo();
        apply_layout_change(&graph, &mut layout, &mut orders, &redo.unwrap());
        assert_eq!(layout[&id], start + Vector::new(48.0, 24.0));
    }

    #[test]
    fn whole_wallet_drag_moves_its_offset() {
        let two = fixture::two_wallets("a", "b");
        let graph = TxGraph::new(two.wallets);
        let b_items = graph.wallet_items(&two.b);
        let mut reversed = b_items.clone();
        reversed.reverse();
        let b = WalletId::new("b".to_string(), None);
        assert_eq!(dragged_wallet(&graph, &reversed), Some(b));
        assert_eq!(dragged_wallet(&graph, &b_items[1..]), None);
        let mut with_current = b_items.clone();
        with_current.push(graph.tx_item(graph.tx_index(&two.funding).unwrap()));
        assert_eq!(dragged_wallet(&graph, &with_current), None);
        let current = graph.wallet_items(&WalletKey::Current);
        assert_eq!(dragged_wallet(&graph, &current), None);
        assert_eq!(dragged_wallet(&graph, &[]), None);
    }

    #[test]
    fn moved_offset_snaps_when_on() {
        let offset = Vector::new(24.0, 600.0);
        let delta = Vector::new(5.0, 7.0);
        assert_eq!(moved_offset(offset, delta, false), Vector::new(29.0, 607.0));
        assert_eq!(moved_offset(offset, delta, true), Vector::new(24.0, 612.0));
    }

    #[test]
    fn undo_offset_moves_the_wallet_back() {
        let two = fixture::two_wallets("a", "b");
        let graph = TxGraph::new(two.wallets);
        let b = WalletId::new("b".to_string(), None);
        let start = Vector::new(0.0, 600.0);
        let mut offsets = Offsets::default();
        offsets.set(b.clone(), start);
        let mut layout = reset(&graph, &WalletKey::Current);
        layout.extend(
            reset(&graph, &two.b)
                .into_iter()
                .map(|(id, p)| (id, p + start)),
        );
        let initial = layout.clone();

        let after = Vector::new(48.0, 720.0);
        let change = Change::Offset {
            wallet: b.clone(),
            before: start,
            after,
        };
        let mut history = History::default();
        history.record(change);
        assert!(apply_offset(&graph, &mut layout, &mut offsets, &b, after));
        for id in graph.wallet_items(&two.b) {
            assert_eq!(layout[&id], initial[&id] + Vector::new(48.0, 120.0));
        }
        for id in graph.wallet_items(&WalletKey::Current) {
            assert_eq!(layout[&id], initial[&id]);
        }

        let Some(Change::Offset { wallet, after, .. }) = history.undo() else {
            panic!("an offset change is undone");
        };
        assert!(apply_offset(
            &graph,
            &mut layout,
            &mut offsets,
            &wallet,
            after
        ));
        assert_eq!(offsets.get(&two.b), Some(start));
        assert_eq!(layout, initial);

        let unknown = WalletId::new("c".to_string(), None);
        assert!(!apply_offset(
            &graph,
            &mut layout,
            &mut offsets,
            &unknown,
            after
        ));
        assert_eq!(layout, initial);
    }

    #[test]
    fn undo_reset_restores_orders() {
        let graph = fixture::graph();
        let mut layout = place(&graph, &WalletKey::Current, &HashMap::new());
        let txid = graph.txs()[0].history().txid;
        let reordered = Some(vec![1, 0]);
        let mut orders = Orders::from([(txid, (None, reordered.clone()))]);
        let before = layout_state(&graph, &layout, &orders);
        let after = layout_state(
            &graph,
            &place(&graph, &WalletKey::Current, &HashMap::new()),
            &Orders::new(),
        );
        let reset = Change::Layout { before, after };

        let touched = apply_layout_change(&graph, &mut layout, &mut orders, &reset);
        assert!(orders.is_empty());
        assert!(touched.contains(&graph.tx_item(0)));

        apply_layout_change(&graph, &mut layout, &mut orders, &reset.inverse());
        assert_eq!(orders[&txid], (None, reordered));
    }

    #[test]
    fn apply_skips_items_not_on_the_map() {
        let graph = fixture::graph();
        let mut layout = place(&graph, &WalletKey::Current, &HashMap::new());
        let expected = layout.clone();
        let mut orders = Orders::new();
        let unknown = GraphItem::Tx(foreign(200).txid);
        let change = Change::Move(vec![(unknown, Point::ORIGIN, Point::new(9.0, 9.0))]);
        let touched = apply_layout_change(&graph, &mut layout, &mut orders, &change);
        assert!(touched.is_empty());
        assert_eq!(layout, expected);
    }

    #[test]
    fn apply_ignores_coin_and_label_changes() {
        let graph = fixture::graph();
        let mut layout = place(&graph, &WalletKey::Current, &HashMap::new());
        let expected = layout.clone();
        let mut orders = Orders::new();
        let changes = [
            Change::Select {
                coin: foreign(1),
                selected: true,
            },
            Change::Label {
                item: LabelItem::OutPoint(foreign(1)),
                before: None,
                after: Some("a".to_string()),
            },
        ];
        for change in &changes {
            assert!(apply_layout_change(&graph, &mut layout, &mut orders, change).is_empty());
        }
        assert_eq!(layout, expected);
        assert!(orders.is_empty());
    }
}
