use std::{cmp::Reverse, collections::HashMap};

use iced::{Point, Rectangle, Size, Vector};
use liana_ui::{
    component::panels::map::{BLOCK_WIDTH, U},
    widget::graph_view::ItemId,
};

use crate::app::state::map::{
    graph::TxGraph,
    lanes::{self, LaneTx},
    layout::BLOCK_CLEARANCE,
    offsets::Offsets,
    untangle::{
        clusters, crossings, linked_first, row_tops, slot_offset, Arrangement, Layered, Link, Node,
        Slot, Spine,
    },
    wallets::WalletKey,
    Orders,
};

/// Passes of the vertical placement, up the columns then down.
const PASSES: usize = 24;
/// Most rounds of passes, each from the best placement found.
const ROUNDS: usize = 8;
/// Room kept above and below a link drawn across a column.
const PASS_CLEARANCE: f32 = U;
/// Room between two clusters packed side by side or one below the other.
const CLUSTER_GAP: f32 = 12.0 * U;

/// The columns of a placement by structure and the links drawn across each one.
struct Columns {
    /// Left edge of the blocks of each column.
    xs: Vec<f32>,
    /// Nodes of each column, the oldest first.
    nodes: Vec<Vec<usize>>,
    /// Links drawn across each column, their ends in other columns.
    passing: Vec<Vec<usize>>,
}

impl Columns {
    fn new(links: &[Link], column: &[usize], xs: Vec<f32>) -> Self {
        let mut nodes = vec![Vec::new(); xs.len()];
        for (node, column) in column.iter().enumerate() {
            nodes[*column].push(node);
        }
        let mut passing = vec![Vec::new(); xs.len()];
        for (index, link) in links.iter().enumerate() {
            for across in &mut passing[column[link.from.node] + 1..column[link.to.node]] {
                across.push(index);
            }
        }
        Self { xs, nodes, passing }
    }

    /// The nodes at `tops`, a column whose blocks would meet packed from 0 instead, its nodes by
    /// their top then in time order.
    fn settled(&self, layered: &Layered, tops: &[f32]) -> Vec<f32> {
        let mut settled = tops.to_vec();
        for nodes in &self.nodes {
            let mut nodes = nodes.clone();
            nodes.sort_by(|a, b| tops[*a].total_cmp(&tops[*b]).then(a.cmp(b)));
            let height = |node: usize| layered.nodes()[node].height() + BLOCK_CLEARANCE;
            if nodes
                .windows(2)
                .all(|pair| tops[pair[0]] + height(pair[0]) <= tops[pair[1]])
            {
                continue;
            }
            let mut top = 0.0;
            for node in nodes {
                settled[node] = top;
                top += height(node);
            }
        }
        settled
    }
}

/// Column of each node: one right of its right-most parent, the first one without parent.
fn columns(layered: &Layered) -> Vec<usize> {
    let count = layered.nodes().len();
    let mut children = vec![Vec::new(); count];
    let mut parents = vec![0; count];
    for link in layered.links() {
        children[link.from.node].push(link.to.node);
        parents[link.to.node] += 1;
    }
    let mut column = vec![0; count];
    let mut ready: Vec<usize> = (0..count).filter(|node| parents[*node] == 0).collect();
    while let Some(node) = ready.pop() {
        for child in &children[node] {
            column[*child] = column[*child].max(column[node] + 1);
            parents[*child] -= 1;
            if parents[*child] == 0 {
                ready.push(*child);
            }
        }
    }
    column
}

/// Left edge of each column: the output leaves of a column and the input leaves of the next one
/// fit between their blocks, with the clearance.
fn column_xs(nodes: &[Node], column: &[usize]) -> Vec<f32> {
    let count = column.iter().max().map_or(0, |last| last + 1);
    let (mut left, mut right) = (vec![0.0f32; count], vec![0.0f32; count]);
    for (node, column) in nodes.iter().zip(column) {
        left[*column] = left[*column].max(node.x - node.left());
        right[*column] = right[*column].max(node.right() - node.x);
    }
    let mut xs = vec![0.0];
    for (right, left) in right.iter().zip(left.iter().skip(1)) {
        let x = xs[xs.len() - 1] + right + BLOCK_CLEARANCE + left;
        xs.push((x / U).ceil() * U);
    }
    xs.truncate(count);
    xs
}

fn row_of(order: &[usize], index: usize) -> usize {
    order.iter().position(|i| *i == index).unwrap_or(index)
}

fn input_y(arrangement: &Arrangement, slot: Slot) -> f32 {
    arrangement.tops[slot.node] + slot_offset(row_of(&arrangement.inputs[slot.node], slot.index))
}

fn output_y(arrangement: &Arrangement, slot: Slot) -> f32 {
    arrangement.tops[slot.node] + slot_offset(row_of(&arrangement.outputs[slot.node], slot.index))
}

/// Tops lining each linked slot of `node` up with its peer, on the inputs when `down`, else
/// on the outputs.
fn peer_tops(layered: &Layered, arrangement: &Arrangement, node: usize, down: bool) -> Vec<f32> {
    let (peers, order, peer_y): (_, _, fn(&Arrangement, Slot) -> f32) = if down {
        (
            layered.input_peers(node),
            &arrangement.inputs[node],
            output_y,
        )
    } else {
        (
            layered.output_peers(node),
            &arrangement.outputs[node],
            input_y,
        )
    };
    peers
        .iter()
        .enumerate()
        .filter_map(|(index, peer)| {
            Some(peer_y(arrangement, (*peer)?) - slot_offset(row_of(order, index)))
        })
        .collect()
}

fn median(mut values: Vec<f32>) -> Option<f32> {
    values.sort_by(f32::total_cmp);
    let count = values.len();
    (count > 0).then(|| (values[(count - 1) / 2] + values[count / 2]) / 2.0)
}

/// Span a block at `x` must keep clear of `link`, drawn straight between its slots.
fn passage(layered: &Layered, arrangement: &Arrangement, link: &Link, x: f32) -> (f32, f32) {
    let (x0, y0) = (
        layered.nodes()[link.from.node].x + BLOCK_WIDTH,
        output_y(arrangement, link.from),
    );
    let (x1, y1) = (
        layered.nodes()[link.to.node].x,
        input_y(arrangement, link.to),
    );
    let y_at = |x: f32| y0 + (y1 - y0) * (x - x0) / (x1 - x0);
    let (a, b) = (y_at(x), y_at(x + BLOCK_WIDTH));
    (a.min(b) - PASS_CLEARANCE, a.max(b) + PASS_CLEARANCE)
}

/// Top on the grid nearest to `wanted`, the upper one on a tie, where a block `height` tall
/// meets none of the `taken` spans.
fn nearest_free(taken: &[(f32, f32)], height: f32, wanted: f32) -> f32 {
    let free = |top: f32| taken.iter().all(|(a, b)| top + height <= *a || top >= *b);
    let lowest = taken
        .iter()
        .map(|(_, b)| (b / U).ceil() * U)
        .fold((wanted / U).round() * U, f32::max);
    taken
        .iter()
        .flat_map(|(a, b)| [((a - height) / U).floor() * U, (b / U).ceil() * U])
        .chain([(wanted / U).round() * U])
        .filter(|top| free(*top))
        .fold(lowest, |best, top| {
            let (d_top, d_best) = ((top - wanted).abs(), (best - wanted).abs());
            if d_top < d_best || (d_top == d_best && top < best) {
                top
            } else {
                best
            }
        })
}

/// Moves the nodes of every column, left to right when `down`, to the median of their peers
/// before them (after them when not `down`), the nodes with most peers first, else to the
/// nearest free top. A node never meets a block of its column or a link drawn across it.
fn pass(layered: &Layered, columns: &Columns, arrangement: &mut Arrangement, down: bool) {
    let order: Vec<usize> = if down {
        (1..columns.xs.len()).collect()
    } else {
        (0..columns.xs.len().saturating_sub(1)).rev().collect()
    };
    for column in order {
        let mut taken: Vec<(f32, f32)> = columns.passing[column]
            .iter()
            .map(|link| {
                passage(
                    layered,
                    arrangement,
                    &layered.links()[*link],
                    columns.xs[column],
                )
            })
            .collect();
        let mut nodes: Vec<(usize, Vec<f32>)> = columns.nodes[column]
            .iter()
            .map(|node| (*node, peer_tops(layered, arrangement, *node, down)))
            .collect();
        nodes.sort_by_key(|(node, peers)| (Reverse(peers.len()), *node));
        for (node, peers) in nodes {
            let height = layered.nodes()[node].height();
            let wanted = median(peers).unwrap_or(arrangement.tops[node]);
            let top = nearest_free(&taken, height, wanted);
            arrangement.tops[node] = top;
            taken.push((top - BLOCK_CLEARANCE, top + height + BLOCK_CLEARANCE));
        }
    }
}

/// What the placement cuts: the crossings, then the height.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Score {
    crossings: usize,
    height: f32,
}

impl Score {
    fn beats(&self, other: &Score) -> bool {
        (self.crossings, self.height) < (other.crossings, other.height)
    }
}

fn score(layered: &Layered, arrangement: &Arrangement) -> Score {
    let top = arrangement.tops.iter().copied().fold(f32::MAX, f32::min);
    let bottom = layered
        .nodes()
        .iter()
        .zip(&arrangement.tops)
        .map(|(node, top)| top + node.height())
        .fold(f32::MIN, f32::max);
    Score {
        crossings: crossings(layered, arrangement).total(),
        height: bottom - top,
    }
}

/// Orders the linked slots of every node by the height of their peer, the slots without a
/// link after them in their true order.
fn order_slots(layered: &Layered, arrangement: &mut Arrangement) {
    let count = layered.nodes().len();
    let inputs = (0..count)
        .map(|node| linked_first(layered.input_peers(node), |p| output_y(arrangement, p)))
        .collect();
    let outputs = (0..count)
        .map(|node| linked_first(layered.output_peers(node), |p| input_y(arrangement, p)))
        .collect();
    arrangement.inputs = inputs;
    arrangement.outputs = outputs;
}

/// The nodes of `layered` in one lane, each one a column right of its right-most parent, and
/// their columns.
fn in_columns(layered: &Layered) -> (Layered, Columns) {
    let column = columns(layered);
    let xs = column_xs(layered.nodes(), &column);
    let nodes = layered
        .nodes()
        .iter()
        .zip(&column)
        .map(|(node, column)| Node {
            lane: 0,
            x: xs[*column],
            ..node.clone()
        })
        .collect();
    let placed = Layered::new(1, nodes, layered.links().to_vec());
    let columns = Columns::new(placed.links(), &column, xs);
    (placed, columns)
}

/// The best placement found by rounds of passes from `start`, never worse than `start`.
fn refine(placed: &Layered, columns: &Columns, start: Arrangement, reorder: bool) -> Arrangement {
    let mut best = (score(placed, &start), start);
    for _ in 0..ROUNDS {
        let mut arrangement = best.1.clone();
        let mut improved = false;
        for index in 0..PASSES {
            pass(placed, columns, &mut arrangement, index % 2 == 1);
            if reorder {
                order_slots(placed, &mut arrangement);
            }
            let found = score(placed, &arrangement);
            if found.beats(&best.0) {
                best = (found, arrangement.clone());
                improved = true;
            }
        }
        if !improved {
            break;
        }
    }
    best.1
}

/// `arrangement` moved up, its highest top at 0.
fn raised(mut arrangement: Arrangement) -> Arrangement {
    let highest = arrangement.tops.iter().copied().fold(f32::MAX, f32::min);
    for top in &mut arrangement.tops {
        *top -= highest;
    }
    arrangement
}

/// The nodes of `layered` placed by structure, their lanes and x left out: each one a column
/// right of its right-most parent, then moved up or down to line its slots up with its peers
/// clear of the blocks and links around it. The nodes start at the tops of `start`, a column
/// whose blocks would meet packed in their order; the slots keep the orders of `start` unless
/// `reorder`. The result never has more crossings than its start. Returns the nodes with their
/// x, all in one lane, and their tops, the highest at 0.
pub fn untangle_global(
    layered: &Layered,
    start: &Arrangement,
    reorder: bool,
) -> (Layered, Arrangement) {
    let (placed, columns) = in_columns(layered);
    let mut arrangement = Arrangement {
        tops: columns.settled(&placed, &start.tops),
        ..start.clone()
    };
    if reorder {
        order_slots(&placed, &mut arrangement);
    }
    let arrangement = refine(&placed, &columns, arrangement, reorder);
    (placed, raised(arrangement))
}

/// `start` with the nodes of `placed`, all linked together, on the rows of their `Spine`, the
/// slots ordered again when `reorder`.
fn on_spine(placed: &Layered, start: &Arrangement, reorder: bool) -> Arrangement {
    let all: Vec<usize> = (0..placed.nodes().len()).collect();
    let spine = Spine::new(placed, &all);
    let mut arrangement = start.clone();
    for (nodes, top) in spine.rows.iter().zip(row_tops(placed, &spine.rows)) {
        for node in nodes {
            arrangement.tops[*node] = top;
        }
    }
    if reorder {
        order_slots(placed, &mut arrangement);
    }
    arrangement
}

/// The nodes of `layered`, all linked together, in columns as `untangle_global` places them
/// and starting on the rows of their `Spine`, then moved by passes as `untangle_global` does.
fn untangle_cluster(
    layered: &Layered,
    start: &Arrangement,
    reorder: bool,
) -> (Layered, Arrangement) {
    let (placed, columns) = in_columns(layered);
    let arrangement = on_spine(&placed, start, reorder);
    let arrangement = refine(&placed, &columns, arrangement, reorder);
    (placed, raised(arrangement))
}

/// The nodes `ids` of `layered` in one lane, numbered in their order, the links between them
/// and their tops and slot orders in `arrangement`.
pub fn part(layered: &Layered, arrangement: &Arrangement, ids: &[usize]) -> (Layered, Arrangement) {
    let index: HashMap<usize, usize> = ids.iter().enumerate().map(|(k, id)| (*id, k)).collect();
    let slot = |slot: Slot| {
        Some(Slot {
            node: *index.get(&slot.node)?,
            index: slot.index,
        })
    };
    let links = layered
        .links()
        .iter()
        .filter_map(|link| {
            Some(Link {
                from: slot(link.from)?,
                to: slot(link.to)?,
            })
        })
        .collect();
    let nodes = ids.iter().map(|id| layered.nodes()[*id].clone()).collect();
    let part = Arrangement {
        tops: ids.iter().map(|id| arrangement.tops[*id]).collect(),
        inputs: ids
            .iter()
            .map(|id| arrangement.inputs[*id].clone())
            .collect(),
        outputs: ids
            .iter()
            .map(|id| arrangement.outputs[*id].clone())
            .collect(),
    };
    (Layered::new(1, nodes, links), part)
}

/// Area a placement in one lane covers, leaves included, its left edge and size on the grid.
fn cover(layered: &Layered, arrangement: &Arrangement) -> Rectangle {
    let nodes = layered.nodes().iter().zip(&arrangement.tops);
    let left = nodes
        .clone()
        .map(|(n, _)| n.left())
        .fold(f32::MAX, f32::min);
    let right = nodes
        .clone()
        .map(|(n, _)| n.right())
        .fold(f32::MIN, f32::max);
    let top = nodes.clone().map(|(_, t)| *t).fold(f32::MAX, f32::min);
    let bottom = nodes.map(|(n, t)| t + n.height()).fold(f32::MIN, f32::max);
    let left = (left / U).floor() * U;
    Rectangle::new(
        Point::new(left, top),
        Size::new(
            ((right - left) / U).ceil() * U,
            ((bottom - top) / U).ceil() * U,
        ),
    )
}

/// Width a row of clusters fills before the next row starts: the widest cluster, at least
/// as wide as a square holding them all.
fn shelf_width(sizes: &[Size]) -> f32 {
    let area: f32 = sizes.iter().map(|s| s.width * s.height).sum();
    sizes.iter().map(|s| s.width).fold(area.sqrt(), f32::max)
}

/// Top left corner of each of `sizes` packed in rows left to right, top to bottom,
/// `CLUSTER_GAP` apart, a row ending before it gets wider than `width`.
fn shelves(sizes: &[Size], width: f32) -> Vec<Point> {
    let (mut x, mut y, mut row_height) = (0.0, 0.0, 0.0f32);
    sizes
        .iter()
        .map(|size| {
            if x > 0.0 && x + size.width > width {
                (x, y, row_height) = (0.0, y + row_height + CLUSTER_GAP, 0.0);
            }
            let corner = Point::new(x, y);
            x += size.width + CLUSTER_GAP;
            row_height = row_height.max(size.height);
            corner
        })
        .collect()
}

/// The nodes of `layered` in one lane, cluster by cluster: each cluster of nodes linked
/// together, across lanes, placed alone by its `Spine`, then the clusters packed in rows, the
/// largest first.
pub fn untangle_clusters(
    layered: &Layered,
    start: &Arrangement,
    reorder: bool,
) -> (Layered, Arrangement) {
    let one_lane: Vec<Node> = layered
        .nodes()
        .iter()
        .map(|node| Node {
            lane: 0,
            ..node.clone()
        })
        .collect();
    let flat = Layered::new(1, one_lane, layered.links().to_vec());
    let parts: Vec<(Vec<usize>, Layered, Arrangement)> = clusters(&flat)
        .into_iter()
        .map(|ids| {
            let (part, part_start) = part(&flat, start, &ids);
            let (placed, arrangement) = untangle_cluster(&part, &part_start, reorder);
            (ids, placed, arrangement)
        })
        .collect();
    let covers: Vec<Rectangle> = parts.iter().map(|(_, p, a)| cover(p, a)).collect();
    let sizes: Vec<Size> = covers.iter().map(Rectangle::size).collect();
    let corners = shelves(&sizes, shelf_width(&sizes));
    let mut nodes = flat.nodes().to_vec();
    let mut arrangement = start.clone();
    for ((ids, placed, part), (cover, corner)) in parts.iter().zip(covers.iter().zip(corners)) {
        let shift = corner - cover.position();
        for (k, id) in ids.iter().enumerate() {
            nodes[*id] = Node {
                x: placed.nodes()[k].x + shift.x,
                ..placed.nodes()[k].clone()
            };
            arrangement.tops[*id] = part.tops[k] + shift.y;
            arrangement.inputs[*id] = part.inputs[k].clone();
            arrangement.outputs[*id] = part.outputs[k].clone();
        }
    }
    (Layered::new(1, nodes, flat.links().to_vec()), arrangement)
}

/// Positions with the lanes off, slot display orders and wallet offsets.
#[derive(Debug, Clone, PartialEq)]
pub struct Global {
    pub positions: HashMap<ItemId, Point>,
    pub orders: Orders,
    pub offsets: Offsets,
}

/// The items of `wallets` placed by structure, mixed in one placement or, with `clusters`, as
/// `untangle_clusters` packs them. Every wallet offset is zero: its own positions are the map
/// positions. The transactions start at their height at `start`, the slot orders from
/// `orders`, ordered again when `reorder`.
pub fn place(
    graph: &TxGraph,
    wallets: &[WalletKey],
    start: &HashMap<ItemId, Point>,
    orders: &Orders,
    clusters: bool,
    reorder: bool,
) -> Global {
    let txs: Vec<LaneTx> = (0..graph.txs().len())
        .filter(|tx| wallets.contains(graph.txs()[*tx].primary()))
        .map(|tx| {
            let at = start.get(&graph.tx_item(tx)).copied();
            (tx, 0, at.unwrap_or(Point::ORIGIN))
        })
        .collect();
    let layered = lanes::layered(graph, 1, &txs);
    let start = lanes::arrangement(graph, &txs, orders);
    let (placed, arrangement) = if clusters {
        untangle_clusters(&layered, &start, reorder)
    } else {
        untangle_global(&layered, &start, reorder)
    };
    let blocks: Vec<Point> = placed
        .nodes()
        .iter()
        .zip(&arrangement.tops)
        .map(|(node, top)| Point::new(node.x, *top))
        .collect();
    let (positions, orders) = lanes::placed_txs(graph, &txs, &blocks, &arrangement);
    let mut offsets = Offsets::default();
    for wallet in wallets {
        offsets.set(wallet.clone(), Vector::ZERO);
    }
    Global {
        positions,
        orders,
        offsets,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use iced::{Point, Rectangle, Vector};
    use liana_ui::widget::graph_view::ItemId;

    use crate::app::state::map::{
        fixture,
        global::{
            cover, in_columns, on_spine, part, place, untangle_cluster, untangle_clusters, Global,
            CLUSTER_GAP,
        },
        graph::{LeafKind, TxGraph},
        lanes::{self, LaneTx},
        layout::{self, clearance, item_size, overlaps},
        untangle::{clusters, crossings, Arrangement, Crossings, Layered, Link, Node, Slot},
        wallets::WalletKey,
        Orders,
    };

    fn two_wallets() -> (WalletKey, TxGraph) {
        let two = fixture::two_wallets("a", "b");
        (two.b, TxGraph::new(two.wallets))
    }

    fn reset(graph: &TxGraph, wallets: &[WalletKey], clusters: bool) -> Global {
        place(
            graph,
            wallets,
            &HashMap::new(),
            &Orders::new(),
            clusters,
            true,
        )
    }

    /// The transactions of `wallets` in one lane, at the origin, and their slots in order.
    fn flat(graph: &TxGraph, wallets: &[WalletKey]) -> (Layered, Arrangement) {
        let txs: Vec<LaneTx> = (0..graph.txs().len())
            .filter(|tx| wallets.contains(graph.txs()[*tx].primary()))
            .map(|tx| (tx, 0, Point::ORIGIN))
            .collect();
        (
            lanes::layered(graph, 1, &txs),
            lanes::arrangement(graph, &txs, &Orders::new()),
        )
    }

    /// Chains of one input one output nodes, of `lengths` nodes each, in turn over two lanes.
    fn chains(lengths: &[usize]) -> (Layered, Arrangement) {
        let mut nodes = Vec::new();
        let mut links = Vec::new();
        for (index, length) in lengths.iter().enumerate() {
            for k in 0..*length {
                if k > 0 {
                    links.push(Link {
                        from: Slot {
                            node: nodes.len() - 1,
                            index: 0,
                        },
                        to: Slot {
                            node: nodes.len(),
                            index: 0,
                        },
                    });
                }
                nodes.push(Node {
                    lane: (index + k) % 2,
                    x: 0.0,
                    inputs: 1,
                    outputs: 1,
                    input_leaves: true,
                    output_leaves: true,
                });
            }
        }
        let arrangement = Arrangement {
            tops: vec![0.0; nodes.len()],
            inputs: vec![vec![0]; nodes.len()],
            outputs: vec![vec![0]; nodes.len()],
        };
        (Layered::new(2, nodes, links), arrangement)
    }

    /// Crossings of the transactions of `wallets` at `positions`, their slots in `orders`.
    fn map_crossings(
        graph: &TxGraph,
        wallets: &[WalletKey],
        positions: &HashMap<ItemId, Point>,
        orders: &Orders,
    ) -> Crossings {
        let txs: Vec<LaneTx> = (0..graph.txs().len())
            .filter(|tx| wallets.contains(graph.txs()[*tx].primary()))
            .map(|tx| (tx, 0, positions[&graph.tx_item(tx)]))
            .collect();
        crossings(
            &lanes::layered(graph, 1, &txs),
            &lanes::arrangement(graph, &txs, orders),
        )
    }

    /// The global placement before untangling: each wallet laid out alone, the current one at
    /// its own coordinates, the others below in their order.
    fn legacy(graph: &TxGraph, wallets: &[WalletKey]) -> HashMap<ItemId, Point> {
        let mut placed = HashMap::new();
        for wallet in wallets {
            let local = layout::reset(graph, wallet);
            let offset = match wallet {
                WalletKey::Current => Vector::ZERO,
                _ => layout::new_offset(graph, &placed, &local),
            };
            placed.extend(local.into_iter().map(|(id, p)| (id, p + offset)));
        }
        placed
    }

    fn assert_no_overlap(graph: &TxGraph, positions: &HashMap<ItemId, Point>) {
        let ids: Vec<ItemId> = positions.keys().copied().collect();
        for (i, a) in ids.iter().enumerate() {
            for b in &ids[i + 1..] {
                let rect = |id: &ItemId| Rectangle::new(positions[id], item_size(graph, *id));
                assert!(
                    !overlaps(rect(a), rect(b), clearance(graph, *a, *b)),
                    "{:?} and {:?} overlap",
                    a,
                    b
                );
            }
        }
    }

    #[test]
    fn a_child_lands_right_of_its_parents() {
        let graph = fixture::graph();
        let (b, two) = two_wallets();
        for (graph, wallets) in [
            (&graph, vec![WalletKey::Current]),
            (&two, vec![WalletKey::Current, b]),
        ] {
            for clusters in [false, true] {
                let placed = reset(graph, &wallets, clusters);
                let x = |tx: usize| placed.positions[&graph.tx_item(tx)].x;
                for edge in graph.coin_edges() {
                    assert!(x(edge.to.tx) > x(edge.from.tx));
                }
            }
        }
    }

    #[test]
    fn the_sample_wallet_is_laid_out_in_columns() {
        let f = fixture::sample_wallet();
        let graph = fixture::graph();
        let mixed = reset(&graph, &[WalletKey::Current], false);
        let at = |txid| mixed.positions[&graph.tx_item(graph.tx_index(txid).unwrap())];
        // Roots in the first column, the rent chain one column further each time: a block, its
        // output leaves, the clearance and the input leaves of the next column.
        for txid in [&f.ids.salary, &f.ids.incoming_change, &f.ids.incoming_four] {
            assert_eq!(at(txid).x, 0.0);
        }
        let rent: Vec<f32> = f.ids.rent.iter().map(|txid| at(txid).x).collect();
        assert_eq!(rent, vec![1008.0, 2016.0, 3024.0, 4032.0]);
        assert_eq!(at(&f.ids.payjoin).x, 5328.0);
        // The salary chain stays on one row.
        assert!(f
            .ids
            .rent
            .iter()
            .all(|txid| at(txid).y == at(&f.ids.salary).y));
    }

    #[test]
    fn global_placements_have_no_overlap() {
        let graph = fixture::graph();
        let (b, two) = two_wallets();
        for clusters in [false, true] {
            let sample = reset(&graph, &[WalletKey::Current], clusters);
            assert_eq!(sample.positions.len(), graph.item_ids().count());
            assert_no_overlap(&graph, &sample.positions);
            let wallets = [WalletKey::Current, b.clone()];
            let both = reset(&two, &wallets, clusters);
            assert_eq!(both.positions.len(), two.item_ids().count());
            assert_no_overlap(&two, &both.positions);
        }
    }

    #[test]
    fn global_placements_are_deterministic() {
        let (b, two) = two_wallets();
        let wallets = [WalletKey::Current, b];
        let graph = fixture::graph();
        for clusters in [false, true] {
            assert_eq!(
                reset(&two, &wallets, clusters),
                reset(&two, &wallets, clusters)
            );
            assert_eq!(
                reset(&graph, &[WalletKey::Current], clusters),
                reset(&graph, &[WalletKey::Current], clusters)
            );
        }
        let (layered, start) = chains(&[2, 1, 3, 1, 2]);
        let (placed, arrangement) = untangle_clusters(&layered, &start, true);
        let (again, again_arrangement) = untangle_clusters(&layered, &start, true);
        assert_eq!(placed.nodes(), again.nodes());
        assert_eq!(arrangement, again_arrangement);
    }

    #[test]
    fn untangling_never_crosses_more_than_the_legacy_placement() {
        let graph = fixture::graph();
        let (b, two) = two_wallets();
        for (graph, wallets) in [
            (&graph, vec![WalletKey::Current]),
            (&two, vec![WalletKey::Current, b]),
        ] {
            let before = legacy(graph, &wallets);
            let mixed = reset(graph, &wallets, false);
            let found = map_crossings(graph, &wallets, &mixed.positions, &mixed.orders);
            let legacy_found = map_crossings(graph, &wallets, &before, &Orders::new());
            assert!(found.total() <= legacy_found.total());
        }
    }

    #[test]
    fn clusters_link_transactions_across_wallets() {
        let f = fixture::sample_wallet();
        let graph = fixture::graph();
        let (layered, _) = flat(&graph, &[WalletKey::Current]);
        let node = |txid| graph.tx_index(txid).unwrap();
        let rent: Vec<usize> = f.ids.rent.iter().map(node).collect();
        let mut salary = vec![node(&f.ids.salary), node(&f.ids.payjoin)];
        salary.extend(rent);
        salary.push(node(&f.ids.unconfirmed));
        salary.sort_unstable();
        let mut incoming = vec![
            node(&f.ids.incoming_four),
            node(&f.ids.consolidation),
            node(&f.ids.batch),
        ];
        incoming.sort_unstable();
        let mut change = vec![node(&f.ids.incoming_change), node(&f.ids.self_transfer)];
        change.sort_unstable();
        assert_eq!(clusters(&layered), vec![salary, incoming, change]);

        // A pays B: the payment of A and the spend of B are one cluster.
        let (b, two) = two_wallets();
        let (layered, _) = flat(&two, &[WalletKey::Current, b]);
        assert_eq!(clusters(&layered), vec![vec![0, 1, 2]]);

        // The largest first, then the oldest; isolated transactions last.
        let (layered, start) = chains(&[1, 3, 1, 2]);
        let (placed, _) = untangle_clusters(&layered, &start, true);
        assert_eq!(
            clusters(&placed),
            vec![vec![1, 2, 3], vec![5, 6], vec![0], vec![4]]
        );
    }

    #[test]
    fn clusters_never_overlap() {
        let (layered, start) = chains(&[3, 1, 2, 1, 4, 1, 1, 2]);
        let (placed, arrangement) = untangle_clusters(&layered, &start, true);
        let covers: Vec<Rectangle> = clusters(&placed)
            .iter()
            .map(|ids| {
                let (part, part_arrangement) = part(&placed, &arrangement, ids);
                cover(&part, &part_arrangement)
            })
            .collect();
        for (i, a) in covers.iter().enumerate() {
            for b in &covers[i + 1..] {
                let apart = a.x + a.width + CLUSTER_GAP <= b.x
                    || b.x + b.width + CLUSTER_GAP <= a.x
                    || a.y + a.height + CLUSTER_GAP <= b.y
                    || b.y + b.height + CLUSTER_GAP <= a.y;
                assert!(apart, "{:?} and {:?} overlap", a, b);
            }
        }
    }

    #[test]
    fn a_cluster_is_laid_out_as_alone() {
        let graph = fixture::graph();
        let (two_b, two) = two_wallets();
        for (layered, start) in [
            flat(&graph, &[WalletKey::Current]),
            flat(&two, &[WalletKey::Current, two_b]),
            chains(&[3, 1, 2, 1, 4]),
        ] {
            let (placed, arrangement) = untangle_clusters(&layered, &start, true);
            for ids in clusters(&placed) {
                let (alone, alone_arrangement) = {
                    let (part, part_start) = part(&layered, &start, &ids);
                    untangle_cluster(&part, &part_start, true)
                };
                let (packed, packed_arrangement) = part(&placed, &arrangement, &ids);
                let shift = cover(&packed, &packed_arrangement).position()
                    - cover(&alone, &alone_arrangement).position();
                for (k, node) in alone.nodes().iter().enumerate() {
                    assert_eq!(packed.nodes()[k].x, node.x + shift.x);
                    assert_eq!(
                        packed_arrangement.tops[k],
                        alone_arrangement.tops[k] + shift.y
                    );
                }
                assert_eq!(packed_arrangement.inputs, alone_arrangement.inputs);
                assert_eq!(packed_arrangement.outputs, alone_arrangement.outputs);
            }
        }
    }

    #[test]
    fn a_cluster_starts_on_its_spine_and_never_crosses_more() {
        let f = fixture::sample_wallet();
        let graph = fixture::graph();
        let (layered, start) = flat(&graph, &[WalletKey::Current]);
        let salary = clusters(&layered)[0].clone();
        let (part_layered, part_start) = part(&layered, &start, &salary);
        let (placed, _) = in_columns(&part_layered);
        let spine_start = on_spine(&placed, &part_start, true);
        // The salary chain, the longest one, starts on one row.
        let node = |txid| {
            salary
                .binary_search(&graph.tx_index(txid).unwrap())
                .unwrap()
        };
        let top = spine_start.tops[node(&f.ids.salary)];
        for txid in f
            .ids
            .rent
            .iter()
            .chain([&f.ids.payjoin, &f.ids.unconfirmed])
        {
            assert_eq!(spine_start.tops[node(txid)], top);
        }
        let (b, two) = two_wallets();
        for (layered, start) in [
            flat(&graph, &[WalletKey::Current]),
            flat(&two, &[WalletKey::Current, b.clone()]),
            chains(&[3, 1, 2, 1, 4]),
        ] {
            for ids in clusters(&untangle_clusters(&layered, &start, true).0) {
                let (part_layered, part_start) = part(&layered, &start, &ids);
                let (placed, _) = in_columns(&part_layered);
                let before = crossings(&placed, &on_spine(&placed, &part_start, true));
                let (placed, after) = untangle_cluster(&part_layered, &part_start, true);
                assert!(crossings(&placed, &after).total() <= before.total());
            }
        }
        let placed = reset(&graph, &[WalletKey::Current], true);
        assert_eq!(placed.offsets.get(&WalletKey::Current), Some(Vector::ZERO));
        let both = reset(&two, &[WalletKey::Current, b.clone()], true);
        assert_eq!(both.offsets.get(&b), Some(Vector::ZERO));
    }

    #[test]
    fn slot_orders_are_kept_unless_ordered_again() {
        let f = fixture::sample_wallet();
        let graph = fixture::graph();
        let txid = f.ids.incoming_four;
        let orders = Orders::from([(txid, (None, Some(vec![4, 3, 2, 1, 0])))]);
        let kept = place(
            &graph,
            &[WalletKey::Current],
            &HashMap::new(),
            &orders,
            false,
            false,
        );
        assert_eq!(kept.orders, orders);
        let leaf = *graph.txs()[graph.tx_index(&txid).unwrap()]
            .leaves
            .iter()
            .find(|leaf| graph.leaves()[**leaf].kind == LeafKind::CounterpartyOutput)
            .unwrap();
        let block = kept.positions[&graph.tx_item(graph.tx_index(&txid).unwrap())];
        // The counterparty output, shown first, has its leaf on the first row.
        assert_eq!(
            kept.positions[&graph.leaf_item(leaf)],
            Point::new(block.x + 768.0, block.y + 12.0)
        );
        let ordered = place(
            &graph,
            &[WalletKey::Current],
            &HashMap::new(),
            &orders,
            false,
            true,
        );
        assert_ne!(ordered.orders.get(&txid), orders.get(&txid));
    }
}
