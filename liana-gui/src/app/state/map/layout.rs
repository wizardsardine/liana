use std::collections::HashMap;

use iced::{Point, Rectangle, Size, Vector};
use liana_ui::{
    component::panels::map::{BLOCK_WIDTH, LEAF_WIDTH, SLOT_HEIGHT, U},
    widget::graph_view::{geometry::snap_to_grid, ItemId, Shape},
};

use crate::app::state::map::{
    display_row,
    graph::{LeafKind, MapItem, TxGraph},
    wallets::WalletKey,
    Orders,
};

/// Horizontal distance from a parent to a linked transaction with the lanes off.
pub const COLUMN_PITCH: f32 = 86.0 * U;
pub const LEAF_OFFSET: f32 = 6.0 * U;
pub const BLOCK_CLEARANCE: f32 = 2.0 * U;
const LEAF_CLEARANCE: f32 = U;
pub const UNLINKED_GAP: f32 = 10.0 * U;
/// Least horizontal distance between two transactions of a wallet: a block, its output leaves
/// and the counterparty coin leaves of the next block fit in between.
pub const LANE_PITCH: f32 = 2.0 * BLOCK_WIDTH;
/// Least horizontal distance between consecutive transactions of different wallets, about a
/// quarter of a block on the grid.
pub const CROSS_STEP: f32 = 14.0 * U;
/// Space below the tallest content of a lane.
pub const LANE_GAP: f32 = 6.0 * U;
const LEAF_TOP_INSET: f32 = U;

pub fn item_size(graph: &TxGraph, id: ItemId) -> Size {
    match graph.item(id) {
        Some(MapItem::Tx(tx)) => {
            let tx = &graph.txs()[tx];
            Shape::Block {
                inputs: tx.inputs.len(),
                outputs: tx.outputs.len(),
            }
            .size()
        }
        _ => Shape::Leaf.size(),
    }
}

/// Blocks keep a wider margin; a block next to a leaf uses it too.
pub fn clearance(graph: &TxGraph, a: ItemId, b: ItemId) -> f32 {
    let is_block = |id| matches!(graph.item(id), Some(MapItem::Tx(_)));
    if is_block(a) || is_block(b) {
        BLOCK_CLEARANCE
    } else {
        LEAF_CLEARANCE
    }
}

/// Top of the leaf next to slot `index`, below the top of its block.
fn leaf_inset(index: usize) -> f32 {
    index as f32 * SLOT_HEIGHT + LEAF_TOP_INSET
}

/// Position of a leaf next to the slot shown at `row` of its block at `block`.
pub fn leaf_position(kind: LeafKind, block: Point, row: usize) -> Point {
    let x = match kind {
        LeafKind::CounterpartyCoin => block.x - LEAF_OFFSET - LEAF_WIDTH,
        _ => block.x + BLOCK_WIDTH + LEAF_OFFSET,
    };
    Point::new(x, block.y + leaf_inset(row))
}

/// Height of a transaction with its leaves next to their slots.
fn tx_extent(graph: &TxGraph, tx: usize) -> f32 {
    graph.txs()[tx]
        .leaves
        .iter()
        .map(|leaf| leaf_inset(graph.leaves()[*leaf].index) + Shape::Leaf.size().height)
        .fold(item_size(graph, graph.tx_item(tx)).height, f32::max)
}

pub fn overlaps(a: Rectangle, b: Rectangle, clearance: f32) -> bool {
    a.x < b.x + b.width + clearance
        && b.x < a.x + a.width + clearance
        && a.y < b.y + b.height + clearance
        && b.y < a.y + a.height + clearance
}

/// Shifts `at` down by `U` until the item overlaps nothing occupied.
fn settle(graph: &TxGraph, occupied: &[(ItemId, Rectangle)], id: ItemId, mut at: Point) -> Point {
    let size = item_size(graph, id);
    while occupied.iter().any(|(other, rect)| {
        overlaps(
            Rectangle::new(at, size),
            *rect,
            clearance(graph, id, *other),
        )
    }) {
        at.y += U;
    }
    at
}

/// Default x of the transactions walked in graph order: at least `CROSS_STEP` right of every
/// transaction before, `LANE_PITCH` right of the ones of the same wallet and of each funding
/// parent, on the grid.
#[derive(Default)]
struct Columns {
    last: Option<f32>,
    wallets: HashMap<WalletKey, f32>,
    txs: HashMap<usize, f32>,
}

impl Columns {
    /// Takes the transactions at `positions` as already walked.
    fn walked(&mut self, graph: &TxGraph, positions: &HashMap<ItemId, Point>) {
        for (id, p) in positions {
            if let Some(MapItem::Tx(tx)) = graph.item(*id) {
                self.record(graph, tx, p.x);
            }
        }
    }

    fn record(&mut self, graph: &TxGraph, tx: usize, x: f32) {
        self.last = Some(self.last.map_or(x, |last| last.max(x)));
        let wallet = self
            .wallets
            .entry(graph.txs()[tx].primary().clone())
            .or_insert(x);
        *wallet = wallet.max(x);
        self.txs.insert(tx, x);
    }

    fn next(&mut self, graph: &TxGraph, tx: usize) -> f32 {
        let after_wallet = self
            .wallets
            .get(graph.txs()[tx].primary())
            .map(|x| x + LANE_PITCH);
        let after_parents = graph
            .parents(tx)
            .iter()
            .filter_map(|parent| self.txs.get(parent))
            .map(|x| x + LANE_PITCH);
        let x = self
            .last
            .map(|x| x + CROSS_STEP)
            .into_iter()
            .chain(after_wallet)
            .chain(after_parents)
            .reduce(f32::max)
            .map_or(0.0, |x| (x / U).ceil() * U);
        self.record(graph, tx, x);
        x
    }
}

/// Default x of every transaction of the map, in graph order.
fn default_columns(graph: &TxGraph) -> Vec<f32> {
    let mut columns = Columns::default();
    (0..graph.txs().len())
        .map(|tx| columns.next(graph, tx))
        .collect()
}

/// Default positions of the items of `wallet` missing from `stored`, each transaction at
/// `wanted(tx, occupied, placed transactions)` shifted down clear of everything occupied and
/// its leaves next to their slots shown in `orders`. Stored items are fixed obstacles and are
/// not returned.
fn place_items(
    graph: &TxGraph,
    wallet: &WalletKey,
    stored: &HashMap<ItemId, Point>,
    orders: &Orders,
    mut wanted: impl FnMut(usize, &[(ItemId, Rectangle)], &[Option<Point>]) -> Point,
) -> HashMap<ItemId, Point> {
    let mut occupied: Vec<(ItemId, Rectangle)> = stored
        .iter()
        .map(|(id, p)| (*id, Rectangle::new(*p, item_size(graph, *id))))
        .collect();
    let mut placed = HashMap::new();
    let mut tx_positions: Vec<Option<Point>> = vec![None; graph.txs().len()];

    for tx in 0..graph.txs().len() {
        if graph.txs()[tx].primary() != wallet {
            continue;
        }
        let id = graph.tx_item(tx);
        let position = match stored.get(&id) {
            Some(position) => *position,
            None => {
                let at = wanted(tx, &occupied, &tx_positions);
                let position = settle(graph, &occupied, id, at);
                occupied.push((id, Rectangle::new(position, item_size(graph, id))));
                placed.insert(id, position);
                position
            }
        };
        tx_positions[tx] = Some(position);

        for leaf in &graph.txs()[tx].leaves {
            let leaf_id = graph.leaf_item(*leaf);
            if stored.contains_key(&leaf_id) {
                continue;
            }
            let leaf = &graph.leaves()[*leaf];
            let (inputs, outputs) = orders
                .get(&graph.txs()[tx].history().txid)
                .cloned()
                .unwrap_or_default();
            let column = match leaf.kind {
                LeafKind::CounterpartyCoin => inputs,
                LeafKind::Payment | LeafKind::CounterpartyOutput => outputs,
            };
            let row = display_row(column.as_deref(), leaf.index);
            let at = leaf_position(leaf.kind, position, row);
            let at = settle(graph, &occupied, leaf_id, at);
            occupied.push((leaf_id, Rectangle::new(at, item_size(graph, leaf_id))));
            placed.insert(leaf_id, at);
        }
    }
    placed
}

/// Default positions with the lanes off of the items of `wallet` missing from `stored`, in the
/// wallet's own coordinates: a linked transaction right of its right-most parent, an unlinked
/// one at the left below everything.
pub fn place(
    graph: &TxGraph,
    wallet: &WalletKey,
    stored: &HashMap<ItemId, Point>,
    orders: &Orders,
) -> HashMap<ItemId, Point> {
    place_items(
        graph,
        wallet,
        stored,
        orders,
        |tx, occupied, tx_positions| {
            let parent = graph
                .parents(tx)
                .iter()
                .filter_map(|p| Some((*p, tx_positions[*p]?)))
                .max_by(|(a, pa), (b, pb)| pa.x.total_cmp(&pb.x).then(a.cmp(b)));
            match parent {
                Some((_, parent)) => Point::new(parent.x + COLUMN_PITCH, parent.y),
                None if occupied.is_empty() => Point::ORIGIN,
                None => {
                    let bottom = occupied
                        .iter()
                        .map(|(_, r)| r.y + r.height)
                        .fold(f32::MIN, f32::max);
                    Point::new(0.0, bottom + UNLINKED_GAP)
                }
            }
        },
    )
}

pub fn reset(graph: &TxGraph, wallet: &WalletKey) -> HashMap<ItemId, Point> {
    place(graph, wallet, &HashMap::new(), &Orders::new())
}

/// Rows of the lane of a wallet, its top at 0. A transaction without a funding parent in the
/// lane is on the top row, any other one row below its lowest parent in the lane. A row is as
/// tall as its tallest transaction and the clearance below it.
struct LaneRows {
    rows: HashMap<usize, usize>,
    /// Top of each row, then the bottom of the last one.
    tops: Vec<f32>,
}

impl LaneRows {
    fn new(graph: &TxGraph, wallet: &WalletKey) -> Self {
        let mut rows = HashMap::new();
        let mut heights: Vec<f32> = Vec::new();
        for tx in (0..graph.txs().len()).filter(|tx| graph.txs()[*tx].primary() == wallet) {
            let row = graph
                .parents(tx)
                .iter()
                .filter_map(|parent| rows.get(parent))
                .max()
                .map_or(0, |row| row + 1);
            rows.insert(tx, row);
            if heights.len() <= row {
                heights.resize(row + 1, 0.0);
            }
            heights[row] = heights[row].max(tx_extent(graph, tx) + BLOCK_CLEARANCE);
        }
        let mut tops = vec![0.0];
        for height in heights {
            tops.push(tops[tops.len() - 1] + height);
        }
        Self { rows, tops }
    }

    fn top(&self, tx: usize) -> f32 {
        self.tops[self.rows[&tx]]
    }

    fn height(&self) -> f32 {
        self.tops[self.tops.len() - 1]
    }
}

/// Default positions in its lane of the items of `wallet` missing from `stored`, each
/// transaction at the top of its row, its lane top at 0. Without stored positions each
/// transaction takes its default column of the whole map; otherwise a new one lands after the
/// transactions stored and the ones at `seen` (other wallets' items in the lanes).
pub fn place_lane(
    graph: &TxGraph,
    wallet: &WalletKey,
    stored: &HashMap<ItemId, Point>,
    seen: &HashMap<ItemId, Point>,
    orders: &Orders,
) -> HashMap<ItemId, Point> {
    let defaults = stored.is_empty().then(|| default_columns(graph));
    let rows = LaneRows::new(graph, wallet);
    let mut columns = Columns::default();
    columns.walked(graph, seen);
    columns.walked(graph, stored);
    place_items(graph, wallet, stored, orders, |tx, _, _| {
        let x = match &defaults {
            Some(defaults) => defaults[tx],
            None => columns.next(graph, tx),
        };
        Point::new(x, rows.top(tx))
    })
}

pub fn reset_lane(graph: &TxGraph, wallet: &WalletKey) -> HashMap<ItemId, Point> {
    place_lane(
        graph,
        wallet,
        &HashMap::new(),
        &HashMap::new(),
        &Orders::new(),
    )
}

pub fn bounds<'a>(
    graph: &TxGraph,
    positions: impl IntoIterator<Item = (&'a ItemId, &'a Point)>,
) -> Option<Rectangle> {
    positions
        .into_iter()
        .map(|(id, p)| Rectangle::new(*p, item_size(graph, *id)))
        .reduce(|a, b| a.union(&b))
}

/// Offset of a wallet whose `local` items land below everything `placed`, at the left edge.
pub fn new_offset(
    graph: &TxGraph,
    placed: &HashMap<ItemId, Point>,
    local: &HashMap<ItemId, Point>,
) -> Vector {
    let top = bounds(graph, placed).map_or(0.0, |r| r.y + r.height + UNLINKED_GAP);
    let corner = bounds(graph, local).map_or(Point::ORIGIN, |r| r.position());
    Point::new(0.0, top) - corner
}

/// Default height of the lane of `wallet`: its rows and the gap below them.
pub fn lane_height(graph: &TxGraph, wallet: &WalletKey) -> f32 {
    LaneRows::new(graph, wallet).height() + LANE_GAP
}

/// Transactions the align buttons act on, in time order.
pub fn align_targets(graph: &TxGraph, selected_txs: &[usize]) -> Option<Vec<usize>> {
    match selected_txs {
        [] => None,
        [tx] => Some(graph.chain(*tx)).filter(|chain| chain.len() >= 2),
        _ => {
            let mut targets = selected_txs.to_vec();
            targets.sort_unstable();
            Some(targets)
        }
    }
}

/// New positions of the targets and their leaves, aligned on the oldest target along one axis.
fn align(
    graph: &TxGraph,
    positions: &HashMap<ItemId, Point>,
    targets: &[usize],
    snap: bool,
    vertical: bool,
) -> Vec<(ItemId, Point)> {
    let Some(reference) = targets
        .first()
        .and_then(|tx| positions.get(&graph.tx_item(*tx)))
    else {
        return Vec::new();
    };
    let anchor = if snap {
        snap_to_grid(*reference)
    } else {
        *reference
    };
    let mut moved = Vec::new();
    for tx in targets {
        let id = graph.tx_item(*tx);
        let Some(old) = positions.get(&id) else {
            continue;
        };
        let new = if vertical {
            Point::new(anchor.x, old.y)
        } else {
            Point::new(old.x, anchor.y)
        };
        let (dx, dy) = (new.x - old.x, new.y - old.y);
        moved.push((id, new));
        for leaf in &graph.txs()[*tx].leaves {
            let leaf_id = graph.leaf_item(*leaf);
            if let Some(p) = positions.get(&leaf_id) {
                moved.push((leaf_id, Point::new(p.x + dx, p.y + dy)));
            }
        }
    }
    moved
}

pub fn align_horizontal(
    graph: &TxGraph,
    positions: &HashMap<ItemId, Point>,
    targets: &[usize],
    snap: bool,
) -> Vec<(ItemId, Point)> {
    align(graph, positions, targets, snap, false)
}

pub fn align_vertical(
    graph: &TxGraph,
    positions: &HashMap<ItemId, Point>,
    targets: &[usize],
    snap: bool,
) -> Vec<(ItemId, Point)> {
    align(graph, positions, targets, snap, true)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use iced::{Point, Rectangle, Vector};
    use liana::miniscript::bitcoin::{OutPoint, Txid};
    use liana_ui::widget::graph_view::ItemId;

    use crate::app::{
        settings::WalletId,
        state::map::{
            fixture::{self, foreign, ours, Builder},
            graph::{LeafKind, TxGraph, WalletTxs},
            lanes,
            layout::{
                align_horizontal, align_targets, align_vertical, clearance, item_size, lane_height,
                new_offset, overlaps, place, place_lane, reset, reset_lane,
            },
            wallets::WalletKey,
            Orders,
        },
    };

    fn rect(graph: &TxGraph, layout: &HashMap<ItemId, Point>, id: ItemId) -> Rectangle {
        Rectangle::new(layout[&id], item_size(graph, id))
    }

    fn tx_pos(graph: &TxGraph, layout: &HashMap<ItemId, Point>, tx: usize) -> Point {
        layout[&graph.tx_item(tx)]
    }

    #[test]
    fn default_positions_on_grid() {
        let graph = fixture::graph();
        let layout = reset(&graph, &WalletKey::Current);
        for id in graph.item_ids() {
            let p = layout[&id];
            assert_eq!((p.x / 12.0).fract(), 0.0);
            assert_eq!((p.y / 12.0).fract(), 0.0);
        }
        assert_eq!(layout.len(), graph.item_ids().count());
    }

    #[test]
    fn default_layout_has_no_overlap() {
        let graph = fixture::graph();
        let layout = reset(&graph, &WalletKey::Current);
        let ids: Vec<ItemId> = graph.item_ids().collect();
        for (i, a) in ids.iter().enumerate() {
            for b in &ids[i + 1..] {
                assert!(!overlaps(
                    rect(&graph, &layout, *a),
                    rect(&graph, &layout, *b),
                    clearance(&graph, *a, *b)
                ));
            }
        }
    }

    #[test]
    fn first_unlinked_at_origin() {
        let f = fixture::sample_wallet();
        let graph = fixture::graph();
        let salary = graph.tx_index(&f.ids.salary).unwrap();
        assert_eq!(
            tx_pos(&graph, &reset(&graph, &WalletKey::Current), salary),
            Point::ORIGIN
        );
    }

    #[test]
    fn unlinked_lands_bottom_left() {
        let f = fixture::sample_wallet();
        let graph = fixture::graph();
        let layout = reset(&graph, &WalletKey::Current);
        for txid in [&f.ids.incoming_change, &f.ids.incoming_four] {
            let tx = graph.tx_index(txid).unwrap();
            let before = (0..tx)
                .flat_map(|t| {
                    let leaves = graph.txs()[t].leaves.iter().map(|l| graph.leaf_item(*l));
                    std::iter::once(graph.tx_item(t)).chain(leaves)
                })
                .map(|id| {
                    let r = rect(&graph, &layout, id);
                    r.y + r.height
                })
                .fold(f32::MIN, f32::max);
            let p = tx_pos(&graph, &layout, tx);
            assert_eq!(p.x, 0.0);
            assert!(p.y >= before + 120.0);
        }
    }

    #[test]
    fn linked_lands_right_of_parent() {
        let f = fixture::sample_wallet();
        let graph = fixture::graph();
        let layout = reset(&graph, &WalletKey::Current);
        let at = |txid| tx_pos(&graph, &layout, graph.tx_index(txid).unwrap());
        let salary = at(&f.ids.salary);
        let rent0 = at(&f.ids.rent[0]);
        assert_eq!(rent0.x, salary.x + 1032.0);
        assert!(rent0.y >= salary.y);
        assert_eq!(at(&f.ids.payjoin).x, at(&f.ids.rent[3]).x + 1032.0);
        assert_eq!(
            at(&f.ids.consolidation).x,
            at(&f.ids.incoming_four).x + 1032.0
        );
    }

    #[test]
    fn leaves_next_to_their_slot() {
        let f = fixture::sample_wallet();
        let graph = fixture::graph();
        let layout = reset(&graph, &WalletKey::Current);
        let batch = graph.tx_index(&f.ids.batch).unwrap();
        let block = tx_pos(&graph, &layout, batch);
        for leaf in &graph.txs()[batch].leaves {
            let l = &graph.leaves()[*leaf];
            let p = layout[&graph.leaf_item(*leaf)];
            assert_eq!(p.x, block.x + 768.0);
            assert!(p.y >= block.y + l.index as f32 * 48.0 + 12.0);
        }
        let change = graph.tx_index(&f.ids.incoming_change).unwrap();
        let block = tx_pos(&graph, &layout, change);
        let mut seen = 0;
        for leaf in &graph.txs()[change].leaves {
            if graph.leaves()[*leaf].kind == LeafKind::CounterpartyCoin {
                assert_eq!(layout[&graph.leaf_item(*leaf)].x, block.x - 288.0);
                seen += 1;
            }
        }
        assert_eq!(seen, 3);
    }

    #[test]
    fn stored_positions_are_obstacles() {
        let f = fixture::sample_wallet();
        let graph = fixture::graph();
        let default = reset(&graph, &WalletKey::Current);
        let salary = graph.tx_item(graph.tx_index(&f.ids.salary).unwrap());
        let change = graph.tx_item(graph.tx_index(&f.ids.incoming_change).unwrap());
        let rent0 = graph.tx_item(graph.tx_index(&f.ids.rent[0]).unwrap());
        let stored = HashMap::from([(salary, default[&salary]), (change, default[&rent0])]);
        let placed = place(&graph, &WalletKey::Current, &stored, &Orders::new());
        assert!(!placed.contains_key(&salary));
        assert!(!placed.contains_key(&change));
        let moved = Rectangle::new(placed[&rent0], item_size(&graph, rent0));
        let fixed = Rectangle::new(stored[&change], item_size(&graph, change));
        assert!(moved.y >= fixed.y + fixed.height + 24.0);
    }

    #[test]
    fn stored_leaf_of_unstored_tx_kept() {
        let f = fixture::sample_wallet();
        let graph = fixture::graph();
        let batch = graph.tx_index(&f.ids.batch).unwrap();
        let leaves = &graph.txs()[batch].leaves;
        let kept = graph.leaf_item(leaves[0]);
        let stored = HashMap::from([(kept, Point::new(-5000.0, -5000.0))]);
        let placed = place(&graph, &WalletKey::Current, &stored, &Orders::new());
        assert!(!placed.contains_key(&kept));
        for leaf in &leaves[1..] {
            assert!(placed.contains_key(&graph.leaf_item(*leaf)));
        }
    }

    #[test]
    fn blocks_take_their_default_column() {
        let graph = fixture::graph();
        let layout = reset_lane(&graph, &WalletKey::Current);
        for tx in 0..graph.txs().len() {
            assert_eq!(tx_pos(&graph, &layout, tx).x, tx as f32 * 1392.0);
        }
    }

    /// Roots `root` (four outputs) and `small`, `child` of `root`, `merge` of `root` and `child`.
    fn rows_wallet() -> (TxGraph, [Txid; 4]) {
        let mut b = Builder::new();
        let outputs: Vec<_> = (0..4).map(|k| (ours(k), 1_000, true)).collect();
        let root = b.tx(Some(1), &[foreign(1)], &outputs);
        let small = b.tx(Some(2), &[foreign(2)], &[(ours(4), 1_000, true)]);
        let child = b.tx(
            Some(3),
            &[OutPoint::new(root, 0)],
            &[(ours(5), 1_000, true)],
        );
        let merge = b.tx(
            Some(4),
            &[OutPoint::new(root, 1), OutPoint::new(child, 0)],
            &[(ours(6), 1_000, true)],
        );
        let (txs, coins) = b.finish();
        (
            fixture::current_graph(txs, coins),
            [root, small, child, merge],
        )
    }

    #[test]
    fn rows_follow_the_funding_parents() {
        let (graph, txids) = rows_wallet();
        let layout = reset_lane(&graph, &WalletKey::Current);
        let y = |txid| tx_pos(&graph, &layout, graph.tx_index(&txid).unwrap()).y;
        // The top row is as tall as `root` (192) and the clearance, the next one as `child`
        // (156) and the clearance. `merge` goes one row below its lowest parent.
        assert_eq!(txids.map(y), [0.0, 0.0, 216.0, 396.0]);
    }

    #[test]
    fn a_parent_in_another_lane_makes_a_root() {
        let two = fixture::two_wallets("a", "b");
        let graph = TxGraph::new(two.wallets);
        let y = |txid, wallet: &WalletKey| {
            let tx = graph.tx_index(&txid).unwrap();
            tx_pos(&graph, &reset_lane(&graph, wallet), tx).y
        };
        assert_eq!(y(two.funding, &WalletKey::Current), 0.0);
        assert_eq!(y(two.payment, &WalletKey::Current), 180.0);
        assert_eq!(y(two.spend, &two.b), 0.0);
    }

    #[test]
    fn lane_height_grows_with_the_rows() {
        let (graph, _) = rows_wallet();
        assert_eq!(
            lane_height(&graph, &WalletKey::Current),
            216.0 + 180.0 + 180.0 + 72.0
        );

        let mut b = Builder::new();
        b.tx(Some(1), &[foreign(1)], &[(ours(0), 1_000, true)]);
        let (txs, coins) = b.finish();
        let alone = fixture::current_graph(txs, coins);
        assert_eq!(lane_height(&alone, &WalletKey::Current), 180.0 + 72.0);
    }

    #[test]
    fn new_tx_lands_on_its_row() {
        let (graph, [root, small, child, merge]) = rows_wallet();
        let [small, merge] = [small, merge].map(|txid| graph.tx_index(&txid).unwrap());
        let stored: HashMap<ItemId, Point> = reset_lane(&graph, &WalletKey::Current)
            .into_iter()
            .filter(|(id, _)| ![Some(small), Some(merge)].contains(&graph.tx_of(*id)))
            .collect();
        let placed = place_lane(
            &graph,
            &WalletKey::Current,
            &stored,
            &HashMap::new(),
            &Orders::new(),
        );
        assert_eq!(tx_pos(&graph, &placed, small), Point::new(4176.0, 0.0));
        assert_eq!(tx_pos(&graph, &placed, merge), Point::new(5568.0, 396.0));
        for txid in [root, child] {
            assert!(!placed.contains_key(&graph.tx_item(graph.tx_index(&txid).unwrap())));
        }
    }

    #[test]
    fn columns_follow_height_then_funding_order() {
        for sats in 1_000.. {
            let mut b = Builder::new();
            let late = b.tx(Some(3), &[foreign(1)], &[(ours(0), 1_000, true)]);
            let unconfirmed = b.tx(None, &[foreign(2)], &[(ours(1), 1_000, true)]);
            let parent = b.tx(Some(2), &[foreign(3)], &[(ours(2), 10_000, true)]);
            let child = b.tx(
                Some(2),
                &[OutPoint::new(parent, 0)],
                &[(ours(3), sats, true)],
            );
            let early = b.tx(Some(1), &[foreign(4)], &[(ours(4), 1_000, true)]);
            // The funding order wins over the txid order.
            if child >= parent {
                continue;
            }
            let (txs, coins) = b.finish();
            let graph = fixture::current_graph(txs, coins);
            let layout = reset_lane(&graph, &WalletKey::Current);
            let x = |txid| tx_pos(&graph, &layout, graph.tx_index(&txid).unwrap()).x;
            assert_eq!(
                [early, parent, child, late, unconfirmed].map(x),
                [0.0, 1392.0, 2784.0, 4176.0, 5568.0]
            );
            break;
        }
    }

    /// Wallet A pays at heights 1 and 3, wallet B at heights 2 and 4, all from foreign coins.
    fn interleaved() -> (TxGraph, WalletKey, [usize; 4]) {
        let mut a = Builder::new();
        let mut b = Builder::new();
        let a1 = a.tx(Some(1), &[foreign(1)], &[(ours(0), 1_000, true)]);
        let b1 = b.tx(Some(2), &[foreign(2)], &[(ours(1), 1_000, true)]);
        let a2 = a.tx(Some(3), &[foreign(3)], &[(ours(2), 1_000, true)]);
        let b2 = b.tx(Some(4), &[foreign(4)], &[(ours(3), 1_000, true)]);
        let b_key = WalletKey::Other(WalletId::new("b".to_string(), None));
        let wallet = |key: WalletKey, builder: Builder| {
            let (txs, coins) = builder.finish();
            WalletTxs {
                checksum: key.row(),
                key,
                txs,
                coins,
            }
        };
        let graph = TxGraph::new(vec![
            wallet(WalletKey::Current, a),
            wallet(b_key.clone(), b),
        ]);
        let index = [a1, b1, a2, b2].map(|txid| graph.tx_index(&txid).unwrap());
        (graph, b_key, index)
    }

    #[test]
    fn columns_keep_the_order_across_wallets() {
        let (graph, b, [a1, b1, a2, b2]) = interleaved();
        assert_eq!([a1, b1, a2, b2], [0, 1, 2, 3]);
        let a_layout = reset_lane(&graph, &WalletKey::Current);
        let b_layout = reset_lane(&graph, &b);
        let xs = [
            tx_pos(&graph, &a_layout, a1).x,
            tx_pos(&graph, &b_layout, b1).x,
            tx_pos(&graph, &a_layout, a2).x,
            tx_pos(&graph, &b_layout, b2).x,
        ];
        // A transaction of the other wallet one step after, of the same wallet a lane pitch
        // after: no gap left for the other wallet's transactions.
        assert_eq!(xs, [0.0, 168.0, 1392.0, 1560.0]);
        assert!(xs.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn child_lands_a_lane_pitch_after_its_parent() {
        let two = fixture::two_wallets("a", "b");
        let graph = TxGraph::new(two.wallets);
        let x = |txid, wallet: &WalletKey| {
            let tx = graph.tx_index(&txid).unwrap();
            tx_pos(&graph, &reset_lane(&graph, wallet), tx).x
        };
        assert_eq!(x(two.funding, &WalletKey::Current), 0.0);
        assert_eq!(x(two.payment, &WalletKey::Current), 1392.0);
        // B spends a coin of the payment A made: one lane pitch after it, not one step.
        assert_eq!(x(two.spend, &two.b), 2784.0);
    }

    #[test]
    fn new_tx_lands_after_every_wallet() {
        let (graph, b, [a1, b1, a2, _]) = interleaved();
        let stored = HashMap::from([(graph.tx_item(a1), Point::ORIGIN)]);
        let seen = HashMap::from([(graph.tx_item(b1), Point::new(3000.0, 400.0))]);
        let placed = place_lane(&graph, &WalletKey::Current, &stored, &seen, &Orders::new());
        assert_eq!(tx_pos(&graph, &placed, a2), Point::new(3168.0, 0.0));
        let alone = place_lane(
            &graph,
            &WalletKey::Current,
            &stored,
            &HashMap::new(),
            &Orders::new(),
        );
        assert_eq!(tx_pos(&graph, &alone, a2), Point::new(1392.0, 0.0));
        assert!(!placed.contains_key(&graph.tx_item(a1)));
        assert!(placed.keys().all(|id| graph.item_wallet(*id) != Some(&b)));
    }

    #[test]
    fn new_tx_lands_after_the_stored_ones() {
        let f = fixture::sample_wallet();
        let graph = fixture::graph();
        let unconfirmed = graph.tx_index(&f.ids.unconfirmed).unwrap();
        let mut stored: HashMap<ItemId, Point> = reset_lane(&graph, &WalletKey::Current)
            .into_iter()
            .filter(|(id, _)| graph.tx_of(*id) != Some(unconfirmed))
            .collect();
        let salary = graph.tx_index(&f.ids.salary).unwrap();
        let far = graph.leaf_item(graph.txs()[salary].leaves[0]);
        stored.insert(far, Point::new(20_000.0, 500.0));
        let last = (0..graph.txs().len())
            .filter_map(|tx| stored.get(&graph.tx_item(tx)))
            .map(|p| p.x)
            .fold(f32::MIN, f32::max);
        let row = tx_pos(
            &graph,
            &reset_lane(&graph, &WalletKey::Current),
            unconfirmed,
        )
        .y;
        let placed = place_lane(
            &graph,
            &WalletKey::Current,
            &stored,
            &HashMap::new(),
            &Orders::new(),
        );
        let block = tx_pos(&graph, &placed, unconfirmed);
        assert_eq!(block, Point::new(last + 1392.0, row));
        for leaf in &graph.txs()[unconfirmed].leaves {
            assert_eq!(placed[&graph.leaf_item(*leaf)].x, block.x + 768.0);
        }
        assert_eq!(placed.len(), 1 + graph.txs()[unconfirmed].leaves.len());
    }

    #[test]
    fn lane_layout_has_no_overlap() {
        let graph = fixture::graph();
        let layout = reset_lane(&graph, &WalletKey::Current);
        let ids: Vec<ItemId> = graph.item_ids().collect();
        for (i, a) in ids.iter().enumerate() {
            for b in &ids[i + 1..] {
                assert!(!overlaps(
                    rect(&graph, &layout, *a),
                    rect(&graph, &layout, *b),
                    clearance(&graph, *a, *b)
                ));
            }
        }
    }

    #[test]
    fn untangled_lanes_have_no_overlap() {
        let graph = fixture::graph();
        let layout = lanes::reset(&graph, &[WalletKey::Current]).positions;
        let ids: Vec<ItemId> = graph.item_ids().collect();
        assert_eq!(layout.len(), ids.len());
        for (i, a) in ids.iter().enumerate() {
            for b in &ids[i + 1..] {
                assert!(!overlaps(
                    rect(&graph, &layout, *a),
                    rect(&graph, &layout, *b),
                    clearance(&graph, *a, *b)
                ));
            }
        }
    }

    #[test]
    fn place_lane_only_the_wallet_items() {
        let two = fixture::two_wallets("a", "b");
        let graph = TxGraph::new(two.wallets);
        let spend = graph.tx_item(graph.tx_index(&two.spend).unwrap());
        let placed = reset_lane(&graph, &two.b);
        let mut ids: Vec<ItemId> = placed.keys().copied().collect();
        ids.sort();
        assert_eq!(ids, graph.wallet_items(&two.b));
        assert_eq!(placed[&spend], Point::new(2784.0, 0.0));
    }

    #[test]
    fn align_targets_rules() {
        let f = fixture::sample_wallet();
        let graph = fixture::graph();
        let rent = f.ids.rent.map(|t| graph.tx_index(&t).unwrap());
        assert_eq!(
            align_targets(&graph, &[rent[2], rent[0]]),
            Some(vec![rent[0], rent[2]])
        );
        assert_eq!(align_targets(&graph, &[rent[1]]).map(|c| c.len()), Some(7));
        assert_eq!(align_targets(&graph, &[]), None);

        let mut b = Builder::new();
        b.tx(Some(1), &[foreign(1)], &[(ours(0), 1_000, true)]);
        let (txs, coins) = b.finish();
        let alone = fixture::current_graph(txs, coins);
        assert_eq!(align_targets(&alone, &[0]), None);
    }

    fn rent_chain() -> (TxGraph, HashMap<ItemId, Point>, Vec<usize>) {
        let f = fixture::sample_wallet();
        let graph = fixture::graph();
        let layout = reset(&graph, &WalletKey::Current);
        let salary = graph.tx_index(&f.ids.salary).unwrap();
        let targets = align_targets(&graph, &[salary]).unwrap();
        (graph, layout, targets)
    }

    /// Moved targets and their leaves keep their offset; the leaves follow their transaction.
    fn assert_aligned(
        graph: &TxGraph,
        layout: &HashMap<ItemId, Point>,
        moved: &HashMap<ItemId, Point>,
        targets: &[usize],
        expected: impl Fn(Point) -> Point,
    ) {
        for tx in targets {
            let id = graph.tx_item(*tx);
            let old = layout[&id];
            let p = moved[&id];
            assert_eq!(p, expected(old));
            for leaf in &graph.txs()[*tx].leaves {
                let leaf_id = graph.leaf_item(*leaf);
                assert_eq!(moved[&leaf_id] - layout[&leaf_id], p - old);
            }
        }
    }

    #[test]
    fn align_horizontal_keeps_x() {
        let (graph, layout, targets) = rent_chain();
        let moved: HashMap<_, _> = align_horizontal(&graph, &layout, &targets, false)
            .into_iter()
            .collect();
        let reference = layout[&graph.tx_item(targets[0])];
        assert_aligned(&graph, &layout, &moved, &targets, |old| {
            Point::new(old.x, reference.y)
        });
    }

    #[test]
    fn align_vertical_keeps_y() {
        let (graph, layout, targets) = rent_chain();
        let moved: HashMap<_, _> = align_vertical(&graph, &layout, &targets, false)
            .into_iter()
            .collect();
        let reference = layout[&graph.tx_item(targets[0])];
        assert_aligned(&graph, &layout, &moved, &targets, |old| {
            Point::new(reference.x, old.y)
        });
    }

    #[test]
    fn align_snaps_only_the_aligned_axis() {
        let (graph, mut layout, targets) = rent_chain();
        layout.insert(graph.tx_item(targets[0]), Point::new(5.0, 7.0));
        layout.insert(graph.tx_item(targets[1]), Point::new(1301.0, 403.0));
        let second = |moved: Vec<(ItemId, Point)>| {
            moved
                .into_iter()
                .find(|(id, _)| *id == graph.tx_item(targets[1]))
                .map(|(_, p)| p)
        };
        assert_eq!(
            second(align_horizontal(&graph, &layout, &targets, true)),
            Some(Point::new(1301.0, 12.0))
        );
        assert_eq!(
            second(align_vertical(&graph, &layout, &targets, true)),
            Some(Point::new(0.0, 403.0))
        );
        assert_eq!(
            second(align_horizontal(&graph, &layout, &targets, false)),
            Some(Point::new(1301.0, 7.0))
        );
    }

    #[test]
    fn place_only_the_wallet_items() {
        let two = fixture::two_wallets("a", "b");
        let graph = TxGraph::new(two.wallets);
        let spend = graph.tx_item(graph.tx_index(&two.spend).unwrap());
        let placed = reset(&graph, &two.b);
        let mut ids: Vec<ItemId> = placed.keys().copied().collect();
        ids.sort();
        assert_eq!(ids, graph.wallet_items(&two.b));
        // Its parent belongs to the current wallet: placed as unlinked, at the origin.
        assert_eq!(placed[&spend], Point::ORIGIN);
    }

    #[test]
    fn new_offset_lands_below_at_the_left() {
        let graph = fixture::graph();
        let placed = reset(&graph, &WalletKey::Current);
        let bottom = placed
            .iter()
            .map(|(id, p)| p.y + item_size(&graph, *id).height)
            .fold(f32::MIN, f32::max);
        let local = HashMap::from([
            (graph.tx_item(0), Point::new(240.0, 60.0)),
            (graph.tx_item(1), Point::new(1272.0, -36.0)),
        ]);
        let offset = new_offset(&graph, &placed, &local);
        assert_eq!(offset, Vector::new(-240.0, bottom + 120.0 + 36.0));
        assert_eq!(
            Point::new(240.0, -36.0) + offset,
            Point::new(0.0, bottom + 120.0)
        );

        let first = new_offset(&graph, &HashMap::new(), &local);
        assert_eq!(first, Vector::new(-240.0, 36.0));
        assert_eq!(new_offset(&graph, &placed, &HashMap::new()).x, 0.0);
    }

    #[test]
    fn reset_ignores_stored() {
        let graph = fixture::graph();
        assert_eq!(
            reset(&graph, &WalletKey::Current),
            place(&graph, &WalletKey::Current, &HashMap::new(), &Orders::new())
        );
        assert_eq!(
            reset(&graph, &WalletKey::Current),
            reset(&graph, &WalletKey::Current)
        );
    }
}
