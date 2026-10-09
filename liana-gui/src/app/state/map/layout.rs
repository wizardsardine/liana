use std::collections::HashMap;

use iced::{Point, Rectangle, Size};
use liana_ui::{
    component::panels::map::{BLOCK_WIDTH, LEAF_WIDTH, SLOT_HEIGHT, U},
    widget::graph_view::{geometry::snap_to_grid, ItemId, Shape},
};

use crate::app::state::map::graph::{LeafKind, MapItem, TxGraph};

const COLUMN_PITCH: f32 = 86.0 * U;
const LEAF_OFFSET: f32 = 6.0 * U;
const BLOCK_CLEARANCE: f32 = 2.0 * U;
const LEAF_CLEARANCE: f32 = U;
const UNLINKED_GAP: f32 = 10.0 * U;
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
fn clearance(graph: &TxGraph, a: ItemId, b: ItemId) -> f32 {
    let is_block = |id| matches!(graph.item(id), Some(MapItem::Tx(_)));
    if is_block(a) || is_block(b) {
        BLOCK_CLEARANCE
    } else {
        LEAF_CLEARANCE
    }
}

fn overlaps(a: Rectangle, b: Rectangle, clearance: f32) -> bool {
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

/// Default positions of every item missing from `stored`. Stored items are
/// fixed obstacles and are not returned.
pub fn place(graph: &TxGraph, stored: &HashMap<ItemId, Point>) -> HashMap<ItemId, Point> {
    let mut occupied: Vec<(ItemId, Rectangle)> = stored
        .iter()
        .map(|(id, p)| (*id, Rectangle::new(*p, item_size(graph, *id))))
        .collect();
    let mut placed = HashMap::new();
    let mut tx_positions: Vec<Point> = Vec::with_capacity(graph.txs().len());

    for tx in 0..graph.txs().len() {
        let id = graph.tx_item(tx);
        let position = match stored.get(&id) {
            Some(position) => *position,
            None => {
                let parent = graph
                    .parents(tx)
                    .iter()
                    .copied()
                    .filter(|p| *p < tx_positions.len())
                    .max_by(|a, b| {
                        tx_positions[*a]
                            .x
                            .total_cmp(&tx_positions[*b].x)
                            .then(a.cmp(b))
                    });
                let wanted = match parent {
                    Some(parent) => Point::new(
                        tx_positions[parent].x + COLUMN_PITCH,
                        tx_positions[parent].y,
                    ),
                    None if occupied.is_empty() => Point::ORIGIN,
                    None => {
                        let bottom = occupied
                            .iter()
                            .map(|(_, r)| r.y + r.height)
                            .fold(f32::MIN, f32::max);
                        Point::new(0.0, bottom + UNLINKED_GAP)
                    }
                };
                let position = settle(graph, &occupied, id, wanted);
                occupied.push((id, Rectangle::new(position, item_size(graph, id))));
                placed.insert(id, position);
                position
            }
        };
        tx_positions.push(position);

        for leaf in &graph.txs()[tx].leaves {
            let leaf_id = graph.leaf_item(*leaf);
            if stored.contains_key(&leaf_id) {
                continue;
            }
            let leaf = &graph.leaves()[*leaf];
            let x = match leaf.kind {
                LeafKind::CounterpartyCoin => position.x - LEAF_OFFSET - LEAF_WIDTH,
                _ => position.x + BLOCK_WIDTH + LEAF_OFFSET,
            };
            let y = position.y + leaf.index as f32 * SLOT_HEIGHT + LEAF_TOP_INSET;
            let at = settle(graph, &occupied, leaf_id, Point::new(x, y));
            occupied.push((leaf_id, Rectangle::new(at, item_size(graph, leaf_id))));
            placed.insert(leaf_id, at);
        }
    }
    placed
}

pub fn reset(graph: &TxGraph) -> HashMap<ItemId, Point> {
    place(graph, &HashMap::new())
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

    use iced::{Point, Rectangle};
    use liana_ui::widget::graph_view::ItemId;

    use crate::app::state::map::{
        fixture::{self, foreign, ours, Builder},
        graph::{LeafKind, TxGraph},
        layout::{
            align_horizontal, align_targets, align_vertical, clearance, item_size, overlaps, place,
            reset,
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
        let layout = reset(&graph);
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
        let layout = reset(&graph);
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
        assert_eq!(tx_pos(&graph, &reset(&graph), salary), Point::ORIGIN);
    }

    #[test]
    fn unlinked_lands_bottom_left() {
        let f = fixture::sample_wallet();
        let graph = fixture::graph();
        let layout = reset(&graph);
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
        let layout = reset(&graph);
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
        let layout = reset(&graph);
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
        let default = reset(&graph);
        let salary = graph.tx_item(graph.tx_index(&f.ids.salary).unwrap());
        let change = graph.tx_item(graph.tx_index(&f.ids.incoming_change).unwrap());
        let rent0 = graph.tx_item(graph.tx_index(&f.ids.rent[0]).unwrap());
        let stored = HashMap::from([(salary, default[&salary]), (change, default[&rent0])]);
        let placed = place(&graph, &stored);
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
        let placed = place(&graph, &stored);
        assert!(!placed.contains_key(&kept));
        for leaf in &leaves[1..] {
            assert!(placed.contains_key(&graph.leaf_item(*leaf)));
        }
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
        let layout = reset(&graph);
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
    fn reset_ignores_stored() {
        let graph = fixture::graph();
        assert_eq!(reset(&graph), place(&graph, &HashMap::new()));
        assert_eq!(reset(&graph), reset(&graph));
    }
}
