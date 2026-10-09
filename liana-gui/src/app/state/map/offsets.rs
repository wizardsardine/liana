use std::collections::{BTreeSet, HashMap};

use iced::{Point, Vector};
use liana::miniscript::bitcoin::Txid;
use liana_ui::widget::graph_view::ItemId;
use lianad::commands::{GraphItem, GraphLayoutEntry};

use crate::app::state::map::{
    global, graph::TxGraph, lanes, layout, split_stored, wallets::WalletKey, Orders,
};

/// Offsets of the wallets on the map: map position = own position + offset.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Offsets(HashMap<WalletKey, Vector>);

impl Offsets {
    /// `None` for a wallet not placed yet.
    pub fn get(&self, wallet: &WalletKey) -> Option<Vector> {
        self.0.get(wallet).copied()
    }

    pub fn set(&mut self, wallet: WalletKey, offset: Vector) {
        self.0.insert(wallet, offset);
    }
}

/// The stored layout of a wallet, in its own coordinates.
#[derive(Debug, Default)]
pub struct WalletLayout {
    pub entries: Vec<GraphLayoutEntry>,
    /// `None` for a wallet not placed yet.
    pub offset: Option<Vector>,
}

/// The map positions of every wallet's items and what loading them leaves to save.
#[derive(Debug, Default)]
pub struct Placement {
    pub layout: HashMap<ItemId, Point>,
    /// Positions in the lanes, relative to the lane top.
    pub lane_layout: HashMap<ItemId, Point>,
    pub orders: Orders,
    pub offsets: Offsets,
    /// Height of each wallet's lane laid out by default.
    pub heights: HashMap<WalletKey, f32>,
    /// Items placed by default in either placement or whose stored order was dropped.
    pub save: BTreeSet<ItemId>,
    /// Entries whose item is not on the map anymore, per wallet.
    pub remove: HashMap<WalletKey, Vec<GraphItem>>,
    /// Wallets given a new offset.
    pub placed: Vec<WalletKey>,
}

/// Places each wallet in its own coordinates and shifts it by its offset. Without any stored
/// position, the wallets are placed by structure all together with a zero offset, the slots in
/// the orders of the lanes. Otherwise the current wallet without offset stays at its own
/// coordinates, another wallet without offset lands below the wallets placed before it. The
/// items missing from the lanes get their default lane position; a lane without stored
/// positions is untangled with the others. `wallets` are in lane order.
pub fn place_wallets(graph: &TxGraph, wallets: Vec<(WalletKey, WalletLayout)>) -> Placement {
    let lane_order: Vec<WalletKey> = wallets.iter().map(|(key, _)| key.clone()).collect();
    let (known, new): (Vec<_>, Vec<_>) = wallets
        .into_iter()
        .partition(|(key, wallet)| *key == WalletKey::Current || wallet.offset.is_some());
    let wallets: Vec<_> = known
        .into_iter()
        .chain(new)
        .map(|(key, wallet)| {
            let stored = split_stored(graph, &key, wallet.entries);
            (key, wallet.offset, stored)
        })
        .collect();
    let fresh_map = wallets
        .iter()
        .all(|(_, _, stored)| stored.positions.is_empty());
    let mut lane_seen: HashMap<ItemId, Point> = wallets
        .iter()
        .flat_map(|(_, _, stored)| stored.lane_positions.clone())
        .collect();
    let mut placement = Placement::default();
    let mut fresh = Vec::new();
    for (key, offset, stored) in wallets {
        placement.save.extend(stored.resave);
        if !fresh_map {
            let mut local = stored.positions;
            let placed = layout::place(graph, &key, &local, &stored.orders);
            placement.save.extend(placed.keys().copied());
            local.extend(placed);
            let offset = match (&key, offset) {
                (_, Some(offset)) => offset,
                (WalletKey::Current, None) => Vector::ZERO,
                (added, None) => {
                    placement.placed.push(added.clone());
                    layout::new_offset(graph, &placement.layout, &local)
                }
            };
            placement.offsets.set(key.clone(), offset);
            placement
                .layout
                .extend(local.into_iter().map(|(id, p)| (id, p + offset)));
        } else if offset.map_or(key != WalletKey::Current, |offset| offset != Vector::ZERO) {
            // A fresh map places every wallet together, at a zero offset.
            placement.placed.push(key.clone());
        }
        let mut lane_local = stored.lane_positions;
        if lane_local.is_empty() {
            fresh.push(key.clone());
        }
        let lane_placed = layout::place_lane(graph, &key, &lane_local, &lane_seen, &stored.orders);
        lane_seen.extend(lane_placed.clone());
        placement.save.extend(lane_placed.keys().copied());
        lane_local.extend(lane_placed);
        placement.lane_layout.extend(lane_local);
        let height = layout::lane_height(graph, &key);
        placement.heights.insert(key.clone(), height);
        placement.orders.extend(stored.orders);
        if !stored.remove.is_empty() {
            placement.remove.insert(key, stored.remove);
        }
    }
    if !fresh.is_empty() {
        let untangled = lanes::untangled(
            graph,
            &lane_order,
            &placement.lane_layout,
            &placement.orders,
        );
        let is_fresh = |id: ItemId| graph.item_wallet(id).is_some_and(|w| fresh.contains(w));
        let is_fresh_tx = |txid: &Txid| {
            graph
                .tx_index(txid)
                .is_some_and(|tx| is_fresh(graph.tx_item(tx)))
        };
        placement.lane_layout.extend(
            untangled
                .positions
                .into_iter()
                .filter(|(id, _)| is_fresh(*id)),
        );
        placement.orders.retain(|txid, _| !is_fresh_tx(txid));
        placement.orders.extend(
            untangled
                .orders
                .into_iter()
                .filter(|(txid, _)| is_fresh_tx(txid)),
        );
        placement.heights.extend(
            untangled
                .heights
                .into_iter()
                .filter(|(wallet, _)| fresh.contains(wallet)),
        );
    }
    if fresh_map {
        let global = global::place(
            graph,
            &lane_order,
            &HashMap::new(),
            &placement.orders,
            false,
            false,
        );
        placement.save.extend(global.positions.keys().copied());
        placement.layout = global.positions;
        placement.offsets = global.offsets;
    }
    placement
}

/// Layout entries of `items` in their owning wallet's coordinates, per wallet, with their
/// position in the lanes.
pub fn local_entries(
    graph: &TxGraph,
    layout: &HashMap<ItemId, Point>,
    lane_layout: &HashMap<ItemId, Point>,
    orders: &Orders,
    offsets: &Offsets,
    items: impl IntoIterator<Item = ItemId>,
) -> HashMap<WalletKey, Vec<GraphLayoutEntry>> {
    let mut entries: HashMap<WalletKey, Vec<GraphLayoutEntry>> = HashMap::new();
    for id in items {
        let (Some(item), Some(wallet)) = (graph.graph_item(id), graph.item_wallet(id)) else {
            continue;
        };
        let Some(offset) = offsets.get(wallet) else {
            continue;
        };
        let (input_order, output_order) = match &item {
            GraphItem::Tx(txid) => orders.get(txid).cloned().unwrap_or_default(),
            _ => (None, None),
        };
        let position = layout.get(&id).map(|p| {
            let local = *p - offset;
            (f64::from(local.x), f64::from(local.y))
        });
        let lane_position = lane_layout
            .get(&id)
            .map(|p| (f64::from(p.x), f64::from(p.y)));
        entries
            .entry(wallet.clone())
            .or_default()
            .push(GraphLayoutEntry {
                item,
                position,
                input_order,
                output_order,
                lane_position,
            });
    }
    entries
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use iced::{Point, Vector};
    use liana_ui::widget::graph_view::ItemId;
    use lianad::commands::{GraphItem, GraphLayoutEntry};

    use crate::app::{
        settings::WalletId,
        state::map::{
            fixture::{self, TwoWallets},
            global,
            graph::TxGraph,
            lanes,
            layout::{item_size, new_offset, reset},
            offsets::{local_entries, place_wallets, Offsets, WalletLayout},
            wallets::WalletKey,
            Orders,
        },
    };

    fn entry(item: GraphItem, x: f64, y: f64) -> GraphLayoutEntry {
        GraphLayoutEntry {
            item,
            position: Some((x, y)),
            input_order: None,
            output_order: None,
            lane_position: None,
        }
    }

    fn two_wallets() -> (TwoWallets, TxGraph) {
        let mut two = fixture::two_wallets("a", "b");
        let graph = TxGraph::new(std::mem::take(&mut two.wallets));
        (two, graph)
    }

    fn b_id() -> WalletId {
        WalletId::new("b".to_string(), None)
    }

    fn b_layout(
        entries: Vec<GraphLayoutEntry>,
        offset: Option<Vector>,
    ) -> Vec<(WalletKey, WalletLayout)> {
        vec![
            (WalletKey::Current, WalletLayout::default()),
            (WalletKey::Other(b_id()), WalletLayout { entries, offset }),
        ]
    }

    #[test]
    fn stored_positions_are_shifted_by_the_offset() {
        let (two, graph) = two_wallets();
        let spend = graph.tx_item(graph.tx_index(&two.spend).unwrap());
        let stored = GraphLayoutEntry {
            lane_position: Some((30.0, 4.0)),
            ..entry(GraphItem::Tx(two.spend), 10.0, 20.0)
        };
        let wallets = b_layout(vec![stored], Some(Vector::new(100.0, 1000.0)));
        let placement = place_wallets(&graph, wallets);
        assert_eq!(placement.layout[&spend], Point::new(110.0, 1020.0));
        assert_eq!(placement.lane_layout[&spend], Point::new(30.0, 4.0));
        assert!(!placement.save.contains(&spend));
        assert!(placement.placed.is_empty());
        assert_eq!(
            placement.offsets.get(&two.b),
            Some(Vector::new(100.0, 1000.0))
        );
        assert_eq!(placement.layout.len(), graph.item_ids().count());
    }

    #[test]
    fn missing_lanes_are_untangled() {
        let (two, graph) = two_wallets();
        let spend = graph.tx_item(graph.tx_index(&two.spend).unwrap());
        let wallets = b_layout(
            vec![entry(GraphItem::Tx(two.spend), 10.0, 20.0)],
            Some(Vector::new(100.0, 1000.0)),
        );
        let placement = place_wallets(&graph, wallets);
        let untangled = lanes::reset(&graph, &[WalletKey::Current, two.b.clone()]);
        assert_eq!(placement.lane_layout, untangled.positions);
        assert_eq!(placement.heights, untangled.heights);
        assert_eq!(placement.layout[&spend], Point::new(110.0, 1020.0));
        assert!(placement.save.contains(&spend));
    }

    #[test]
    fn a_new_lane_is_untangled_with_the_stored_ones() {
        let (two, graph) = two_wallets();
        let item = |txid| graph.tx_item(graph.tx_index(&txid).unwrap());
        let lane_entry = |txid, x: f64, y: f64| GraphLayoutEntry {
            lane_position: Some((x, y)),
            ..entry(GraphItem::Tx(txid), x, y)
        };
        let wallets = vec![
            (
                WalletKey::Current,
                WalletLayout {
                    entries: vec![
                        lane_entry(two.funding, 0.0, 0.0),
                        lane_entry(two.payment, 1392.0, 180.0),
                    ],
                    offset: None,
                },
            ),
            (WalletKey::Other(b_id()), WalletLayout::default()),
        ];
        let placement = place_wallets(&graph, wallets);
        assert_eq!(
            placement.lane_layout[&item(two.payment)],
            Point::new(1392.0, 180.0)
        );
        assert_eq!(
            placement.lane_layout[&item(two.spend)],
            Point::new(2784.0, 0.0)
        );
        assert_eq!(placement.heights[&two.b], 252.0);
        assert_eq!(placement.heights[&WalletKey::Current], 432.0);
    }

    #[test]
    fn current_wallet_is_shifted_by_its_offset() {
        let (two, graph) = two_wallets();
        let funding = graph.tx_item(graph.tx_index(&two.funding).unwrap());
        let wallets = vec![
            (
                WalletKey::Current,
                WalletLayout {
                    entries: vec![entry(GraphItem::Tx(two.funding), 12.0, 36.0)],
                    offset: Some(Vector::new(0.0, 500.0)),
                },
            ),
            (
                two.b.clone(),
                WalletLayout {
                    entries: Vec::new(),
                    offset: Some(Vector::ZERO),
                },
            ),
        ];
        let placement = place_wallets(&graph, wallets);
        assert_eq!(placement.layout[&funding], Point::new(12.0, 536.0));
        assert_eq!(
            placement.offsets.get(&WalletKey::Current),
            Some(Vector::new(0.0, 500.0))
        );
        assert!(placement.placed.is_empty());
    }

    #[test]
    fn current_wallet_entries_of_a_shared_tx_are_kept() {
        let (two, graph) = two_wallets();
        let payment = graph.tx_item(graph.tx_index(&two.payment).unwrap());
        let wallets = b_layout(
            vec![entry(GraphItem::Tx(two.payment), -5000.0, -5000.0)],
            Some(Vector::ZERO),
        );
        let placement = place_wallets(&graph, wallets);
        let mixed = global::place(
            &graph,
            &[WalletKey::Current, two.b.clone()],
            &HashMap::new(),
            &Orders::new(),
            false,
            false,
        );
        assert_eq!(placement.layout[&payment], mixed.positions[&payment]);
        assert!(placement.remove.is_empty());
    }

    /// The current wallet's layout shifted by `current`, and B's by `b`.
    fn stacked(
        two: &TwoWallets,
        graph: &TxGraph,
        current: Vector,
        b: Vector,
    ) -> HashMap<ItemId, Point> {
        let shifted = |wallet: &WalletKey, offset: Vector| {
            reset(graph, wallet)
                .into_iter()
                .map(move |(id, p)| (id, p + offset))
        };
        shifted(&WalletKey::Current, current)
            .chain(shifted(&two.b, b))
            .collect()
    }

    /// Offset of B laid out by default below the current wallet, and the top left corner of
    /// its items in `layout`.
    fn b_below(
        two: &TwoWallets,
        graph: &TxGraph,
        layout: &HashMap<ItemId, Point>,
    ) -> (Vector, (f32, f32)) {
        let current = reset(graph, &WalletKey::Current);
        let offset = new_offset(graph, &current, &reset(graph, &two.b));
        let b_items = graph.wallet_items(&two.b);
        let left = b_items
            .iter()
            .map(|id| layout[id].x)
            .fold(f32::MAX, f32::min);
        let top = b_items
            .iter()
            .map(|id| layout[id].y)
            .fold(f32::MAX, f32::min);
        (offset, (left, top))
    }

    /// Bottom of the current wallet laid out by default.
    fn current_bottom(graph: &TxGraph) -> f32 {
        reset(graph, &WalletKey::Current)
            .iter()
            .map(|(id, p)| p.y + item_size(graph, *id).height)
            .fold(f32::MIN, f32::max)
    }

    #[test]
    fn new_wallet_lands_below_everything() {
        let (two, graph) = two_wallets();
        let entries = reset(&graph, &WalletKey::Current)
            .into_iter()
            .map(|(id, p)| entry(graph.graph_item(id).unwrap(), p.x.into(), p.y.into()))
            .collect();
        let mut wallets = b_layout(Vec::new(), None);
        wallets[0].1.entries = entries;
        let placement = place_wallets(&graph, wallets);
        assert_eq!(placement.placed, vec![two.b.clone()]);

        let (offset, corner) = b_below(&two, &graph, &placement.layout);
        assert_eq!(placement.offsets.get(&two.b), Some(offset));
        assert_eq!(corner, (0.0, current_bottom(&graph) + 120.0));
        assert_eq!(
            placement.offsets.get(&WalletKey::Current),
            Some(Vector::ZERO)
        );
        assert_eq!(
            placement.layout,
            stacked(&two, &graph, Vector::ZERO, offset)
        );
        let untangled = lanes::reset(&graph, &[WalletKey::Current, two.b.clone()]);
        assert_eq!(placement.heights, untangled.heights);
        assert_eq!(placement.save.len(), graph.item_ids().count());
    }

    #[test]
    fn local_entries_go_to_the_owner() {
        let (two, graph) = two_wallets();
        let spend = graph.tx_item(graph.tx_index(&two.spend).unwrap());
        let funding = graph.tx_item(graph.tx_index(&two.funding).unwrap());
        let mut offsets = Offsets::default();
        offsets.set(two.b.clone(), Vector::new(24.0, 600.0));
        offsets.set(WalletKey::Current, Vector::new(0.0, 12.0));
        let layout = HashMap::from([
            (spend, Point::new(48.0, 660.0)),
            (funding, Point::new(12.0, 36.0)),
        ]);
        let lane_layout = HashMap::from([(spend, Point::new(96.0, 8.0))]);
        let entries = local_entries(
            &graph,
            &layout,
            &lane_layout,
            &Orders::new(),
            &offsets,
            [spend, funding],
        );
        assert_eq!(
            entries[&two.b],
            vec![GraphLayoutEntry {
                lane_position: Some((96.0, 8.0)),
                ..entry(GraphItem::Tx(two.spend), 24.0, 60.0)
            }]
        );
        assert_eq!(
            entries[&WalletKey::Current],
            vec![entry(GraphItem::Tx(two.funding), 12.0, 24.0)]
        );

        let unplaced = local_entries(
            &graph,
            &layout,
            &lane_layout,
            &Orders::new(),
            &Offsets::default(),
            [spend],
        );
        assert!(unplaced.is_empty());
    }

    #[test]
    fn a_fresh_map_is_placed_mixed() {
        let (two, graph) = two_wallets();
        let wallets = [WalletKey::Current, two.b.clone()];
        let placement = place_wallets(&graph, b_layout(Vec::new(), None));
        let mixed = global::place(
            &graph,
            &wallets,
            &HashMap::new(),
            &placement.orders,
            false,
            false,
        );
        assert_eq!(placement.layout, mixed.positions);
        for wallet in &wallets {
            assert_eq!(placement.offsets.get(wallet), Some(Vector::ZERO));
        }
        assert_eq!(placement.placed, vec![two.b.clone()]);
        assert_eq!(placement.save.len(), graph.item_ids().count());
        let untangled = lanes::reset(&graph, &wallets);
        assert_eq!(placement.lane_layout, untangled.positions);
        assert_eq!(placement.orders, untangled.orders);

        // A stored offset of a fresh map is set back to zero.
        let moved = place_wallets(&graph, b_layout(Vec::new(), Some(Vector::new(0.0, 600.0))));
        assert_eq!(moved.layout, mixed.positions);
        assert_eq!(moved.placed, vec![two.b.clone()]);
    }
}
