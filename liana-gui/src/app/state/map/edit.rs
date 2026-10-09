use std::collections::{BTreeSet, HashMap};

use iced::{Point, Vector};
use liana_ui::widget::graph_view::{geometry::snap_to_grid, ItemId, Side};
use lianad::commands::GraphItem;

use crate::app::state::map::{
    graph::TxGraph,
    history::{Change, LayoutState, PlacementKind},
    offsets::Offsets,
    wallets::WalletKey,
    Orders,
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

/// `(id, before, after)` of a space shift by `dx` at `from_x`: the items whose left edge is at
/// or right of it move across.
pub fn space_moves(
    layout: &HashMap<ItemId, Point>,
    from_x: f32,
    dx: f32,
) -> Vec<(ItemId, Point, Point)> {
    let mut moves: Vec<(ItemId, Point, Point)> = layout
        .iter()
        .filter(|(_, p)| p.x >= from_x)
        .map(|(id, p)| (*id, *p, *p + Vector::new(dx, 0.0)))
        .collect();
    moves.sort_by_key(|(id, ..)| *id);
    moves
}

/// The wallet whose items are exactly `items`: dragging them moves its offset.
pub fn dragged_wallet(graph: &TxGraph, items: &[ItemId]) -> Option<WalletKey> {
    let wallet = graph.item_wallet(*items.first()?)?;
    let dragged: BTreeSet<ItemId> = items.iter().copied().collect();
    let items: BTreeSet<ItemId> = graph.wallet_items(wallet).into_iter().collect();
    (dragged == items).then(|| wallet.clone())
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
    wallet: &WalletKey,
    offset: Vector,
) -> bool {
    let Some(before) = offsets.get(wallet) else {
        return false;
    };
    for id in graph.wallet_items(wallet) {
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
    offsets: &Offsets,
) -> LayoutState {
    LayoutState {
        positions: layout
            .iter()
            .filter_map(|(id, point)| Some((graph.graph_item(*id)?, *point)))
            .collect(),
        orders: orders.clone(),
        offsets: offsets.clone(),
    }
}

/// Sets the `after` state of a layout change and returns the items to persist. `layout` holds
/// the positions of the change's placement.
pub fn apply_layout_change(
    graph: &TxGraph,
    layout: &mut HashMap<ItemId, Point>,
    orders: &mut Orders,
    offsets: &mut Offsets,
    change: &Change,
) -> Vec<ItemId> {
    let mut touched = BTreeSet::new();
    match change {
        Change::Move { moves, .. } => {
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
        Change::Layout {
            placement, after, ..
        } => {
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
            if *placement == PlacementKind::Global {
                *offsets = after.offsets.clone();
            }
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
                    moved_offset, moved_positions, reorder_column, space_moves,
                },
                fixture::{self, foreign},
                graph::TxGraph,
                history::{Change, History, PlacementKind},
                lanes,
                layout::{place, reset},
                offsets::{reset_layout, Offsets},
                wallets::WalletKey,
                Orders,
            },
        },
        daemon::model::LabelItem,
    };

    #[test]
    fn space_moves_only_the_items_from_its_line() {
        let layout = HashMap::from([
            (ItemId(0), Point::new(0.0, 0.0)),
            (ItemId(1), Point::new(1392.0, 48.0)),
            (ItemId(2), Point::new(2000.0, 0.0)),
        ]);
        assert_eq!(
            space_moves(&layout, 1392.0, -96.0),
            vec![
                (
                    ItemId(1),
                    Point::new(1392.0, 48.0),
                    Point::new(1296.0, 48.0)
                ),
                (ItemId(2), Point::new(2000.0, 0.0), Point::new(1904.0, 0.0)),
            ]
        );
        assert_eq!(
            space_moves(&layout, 1500.0, 120.0),
            vec![(ItemId(2), Point::new(2000.0, 0.0), Point::new(2120.0, 0.0))]
        );
        assert!(space_moves(&layout, 3000.0, 50.0).is_empty());
    }

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
        let mut offsets = Offsets::default();
        let id = graph.tx_item(0);
        let start = layout[&id];
        let moved = moved_positions(&layout, &[id], Vector::new(48.0, 24.0), false);
        let change = Change::Move {
            placement: PlacementKind::Global,
            moves: moved
                .iter()
                .map(|(id, before, after)| (graph.graph_item(*id).unwrap(), *before, *after))
                .collect(),
        };
        let mut history = History::default();
        history.record(change.clone());
        layout.insert(id, moved[0].2);

        let undo = history.undo();
        let touched = apply_layout_change(
            &graph,
            &mut layout,
            &mut orders,
            &mut offsets,
            &undo.unwrap(),
        );
        assert_eq!(layout[&id], start);
        assert_eq!(touched, vec![id]);

        let redo = history.redo();
        apply_layout_change(
            &graph,
            &mut layout,
            &mut orders,
            &mut offsets,
            &redo.unwrap(),
        );
        assert_eq!(layout[&id], start + Vector::new(48.0, 24.0));
    }

    #[test]
    fn whole_wallet_drag_moves_its_offset() {
        let two = fixture::two_wallets("a", "b");
        let graph = TxGraph::new(two.wallets);
        let b_items = graph.wallet_items(&two.b);
        let mut reversed = b_items.clone();
        reversed.reverse();
        assert_eq!(dragged_wallet(&graph, &reversed), Some(two.b.clone()));
        assert_eq!(dragged_wallet(&graph, &b_items[1..]), None);
        let mut with_current = b_items.clone();
        with_current.push(graph.tx_item(graph.tx_index(&two.funding).unwrap()));
        assert_eq!(dragged_wallet(&graph, &with_current), None);
        let current = graph.wallet_items(&WalletKey::Current);
        assert_eq!(dragged_wallet(&graph, &current), Some(WalletKey::Current));
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
        let b = two.b.clone();
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

        let unknown = WalletKey::Other(WalletId::new("c".to_string(), None));
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
        let mut offsets = Offsets::default();
        let before = layout_state(&graph, &layout, &orders, &offsets);
        let after = layout_state(
            &graph,
            &place(&graph, &WalletKey::Current, &HashMap::new()),
            &Orders::new(),
            &offsets,
        );
        let reset = Change::Layout {
            placement: PlacementKind::Global,
            before,
            after,
        };

        let touched = apply_layout_change(&graph, &mut layout, &mut orders, &mut offsets, &reset);
        assert!(orders.is_empty());
        assert!(touched.contains(&graph.tx_item(0)));

        apply_layout_change(
            &graph,
            &mut layout,
            &mut orders,
            &mut offsets,
            &reset.inverse(),
        );
        assert_eq!(orders[&txid], (None, reordered));
    }

    #[test]
    fn undo_reset_restores_the_moved_offset() {
        let two = fixture::two_wallets("a", "b");
        let graph = TxGraph::new(two.wallets);
        let moved = Vector::new(480.0, -240.0);
        let mut offsets = Offsets::default();
        offsets.set(WalletKey::Current, Vector::ZERO);
        offsets.set(two.b.clone(), moved);
        let mut layout = reset(&graph, &WalletKey::Current);
        layout.extend(
            reset(&graph, &two.b)
                .into_iter()
                .map(|(id, p)| (id, p + moved)),
        );
        let mut orders = Orders::new();
        let initial = layout.clone();
        let placement = reset_layout(&graph, &[WalletKey::Current, two.b.clone()]);
        let reset = Change::Layout {
            placement: PlacementKind::Global,
            before: layout_state(&graph, &layout, &orders, &offsets),
            after: layout_state(
                &graph,
                &placement.layout,
                &Orders::new(),
                &placement.offsets,
            ),
        };

        apply_layout_change(&graph, &mut layout, &mut orders, &mut offsets, &reset);
        assert_eq!(offsets, placement.offsets);
        assert_eq!(layout, placement.layout);

        apply_layout_change(
            &graph,
            &mut layout,
            &mut orders,
            &mut offsets,
            &reset.inverse(),
        );
        assert_eq!(offsets.get(&two.b), Some(moved));
        assert_eq!(layout, initial);
    }

    #[test]
    fn lanes_reset_keeps_the_offsets() {
        let two = fixture::two_wallets("a", "b");
        let graph = TxGraph::new(two.wallets);
        let wallets = [WalletKey::Current, two.b.clone()];
        let mut offsets = Offsets::default();
        offsets.set(two.b.clone(), Vector::new(480.0, -240.0));
        let kept = offsets.clone();
        let funding = graph.tx_item(graph.tx_index(&two.funding).unwrap());
        let mut lane_layout = lanes::reset(&graph, &wallets).positions;
        lane_layout.insert(funding, Point::new(96.0, 48.0));
        let initial = lane_layout.clone();
        let mut orders = Orders::new();
        let none = Offsets::default();
        let reset = Change::Layout {
            placement: PlacementKind::Lanes,
            before: layout_state(&graph, &lane_layout, &orders, &none),
            after: layout_state(
                &graph,
                &lanes::reset(&graph, &wallets).positions,
                &orders,
                &none,
            ),
        };

        apply_layout_change(&graph, &mut lane_layout, &mut orders, &mut offsets, &reset);
        assert_eq!(lane_layout, lanes::reset(&graph, &wallets).positions);
        assert_eq!(offsets, kept);

        apply_layout_change(
            &graph,
            &mut lane_layout,
            &mut orders,
            &mut offsets,
            &reset.inverse(),
        );
        assert_eq!(lane_layout, initial);
        assert_eq!(offsets, kept);
    }

    #[test]
    fn apply_skips_items_not_on_the_map() {
        let graph = fixture::graph();
        let mut layout = place(&graph, &WalletKey::Current, &HashMap::new());
        let expected = layout.clone();
        let mut orders = Orders::new();
        let mut offsets = Offsets::default();
        let unknown = GraphItem::Tx(foreign(200).txid);
        let change = Change::Move {
            placement: PlacementKind::Global,
            moves: vec![(unknown, Point::ORIGIN, Point::new(9.0, 9.0))],
        };
        let touched = apply_layout_change(&graph, &mut layout, &mut orders, &mut offsets, &change);
        assert!(touched.is_empty());
        assert_eq!(layout, expected);
    }

    #[test]
    fn apply_ignores_coin_and_label_changes() {
        let graph = fixture::graph();
        let mut layout = place(&graph, &WalletKey::Current, &HashMap::new());
        let expected = layout.clone();
        let mut orders = Orders::new();
        let mut offsets = Offsets::default();
        let changes = [
            Change::Select {
                coin: foreign(1),
                selected: true,
            },
            Change::Label {
                wallet: WalletKey::Current,
                item: LabelItem::OutPoint(foreign(1)),
                before: None,
                after: Some("a".to_string()),
            },
        ];
        for change in &changes {
            assert!(
                apply_layout_change(&graph, &mut layout, &mut orders, &mut offsets, change)
                    .is_empty()
            );
        }
        assert_eq!(layout, expected);
        assert!(orders.is_empty());
    }
}
