use std::{
    cmp::{Ordering, Reverse},
    collections::{BTreeMap, BinaryHeap, HashMap},
};

use liana_ui::{
    component::panels::map::{BLOCK_WIDTH, LEAF_WIDTH, SLOT_HEIGHT},
    widget::graph_view::Shape,
};

use crate::app::state::map::layout::{BLOCK_CLEARANCE, LANE_GAP, LEAF_OFFSET};

/// Sweeps refining the rows of the spines, left to right then right to left.
const SWEEPS: usize = 8;

/// A transaction of the lanes placement: its lane, its x and its slots.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub lane: usize,
    pub x: f32,
    pub inputs: usize,
    pub outputs: usize,
    /// Counterparty coins drawn left of the inputs.
    pub input_leaves: bool,
    /// Payments and counterparty outputs drawn right of the outputs.
    pub output_leaves: bool,
}

impl Node {
    fn height(&self) -> f32 {
        Shape::Block {
            inputs: self.inputs,
            outputs: self.outputs,
        }
        .size()
        .height
    }

    fn left(&self) -> f32 {
        if self.input_leaves {
            self.x - LEAF_OFFSET - LEAF_WIDTH
        } else {
            self.x
        }
    }

    fn right(&self) -> f32 {
        let leaves = if self.output_leaves {
            LEAF_OFFSET + LEAF_WIDTH
        } else {
            0.0
        };
        self.x + BLOCK_WIDTH + leaves
    }

    /// Too close to share a row with `other`, leaves included.
    fn clashes(&self, other: &Node) -> bool {
        self.left() < other.right() + BLOCK_CLEARANCE
            && other.left() < self.right() + BLOCK_CLEARANCE
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    pub node: usize,
    /// Index in the true transaction order.
    pub index: usize,
}

/// A coin link from an output to the input spending it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Link {
    pub from: Slot,
    pub to: Slot,
}

/// The transactions of the lanes, in time order, and their coin links.
#[derive(Debug, Clone)]
pub struct Layered {
    lanes: usize,
    nodes: Vec<Node>,
    links: Vec<Link>,
    /// Peer of each input slot.
    input_peers: Vec<Vec<Option<Slot>>>,
    /// Peer of each output slot.
    output_peers: Vec<Vec<Option<Slot>>>,
    /// Links of each node.
    node_links: Vec<Vec<usize>>,
}

impl Layered {
    pub fn new(lanes: usize, nodes: Vec<Node>, links: Vec<Link>) -> Self {
        let mut input_peers: Vec<Vec<Option<Slot>>> =
            nodes.iter().map(|node| vec![None; node.inputs]).collect();
        let mut output_peers: Vec<Vec<Option<Slot>>> =
            nodes.iter().map(|node| vec![None; node.outputs]).collect();
        let mut node_links = vec![Vec::new(); nodes.len()];
        for (index, link) in links.iter().enumerate() {
            input_peers[link.to.node][link.to.index] = Some(link.from);
            output_peers[link.from.node][link.from.index] = Some(link.to);
            node_links[link.from.node].push(index);
            node_links[link.to.node].push(index);
        }
        Self {
            lanes,
            nodes,
            links,
            input_peers,
            output_peers,
            node_links,
        }
    }

    /// Nodes linked to `node`, either way.
    fn peers(&self, node: usize) -> impl Iterator<Item = usize> + '_ {
        self.node_links[node].iter().map(move |link| {
            let link = &self.links[*link];
            if link.from.node == node {
                link.to.node
            } else {
                link.from.node
            }
        })
    }

    fn parents(&self, node: usize) -> impl Iterator<Item = usize> + '_ {
        self.input_peers[node]
            .iter()
            .flatten()
            .map(|slot| slot.node)
    }

    fn children(&self, node: usize) -> impl Iterator<Item = usize> + '_ {
        self.output_peers[node]
            .iter()
            .flatten()
            .map(|slot| slot.node)
    }

    /// Nodes left to right, the oldest first at the same x.
    fn by_x(&self) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.nodes.len()).collect();
        order.sort_by(|a, b| self.nodes[*a].x.total_cmp(&self.nodes[*b].x).then(a.cmp(b)));
        order
    }
}

/// Top of each node in its lane and the display order of its slots, `order[row]` being the
/// true index of the slot shown at `row`.
#[derive(Debug, Clone, PartialEq)]
pub struct Arrangement {
    pub tops: Vec<f32>,
    pub inputs: Vec<Vec<usize>>,
    pub outputs: Vec<Vec<usize>>,
}

impl Arrangement {
    /// Height of each lane: its lowest block, the clearance below and the lane gap.
    pub fn lane_heights(&self, layered: &Layered) -> Vec<f32> {
        let mut bottoms = vec![0.0f32; layered.lanes];
        for (node, top) in layered.nodes.iter().zip(&self.tops) {
            bottoms[node.lane] = bottoms[node.lane].max(top + node.height() + BLOCK_CLEARANCE);
        }
        bottoms
            .into_iter()
            .map(|bottom| bottom + LANE_GAP)
            .collect()
    }

    /// Top of each lane, stacked in their order.
    fn lane_tops(&self, layered: &Layered) -> Vec<f32> {
        let mut top = 0.0;
        self.lane_heights(layered)
            .into_iter()
            .map(|height| {
                let lane_top = top;
                top += height;
                lane_top
            })
            .collect()
    }
}

/// Display row of each slot: the inverse of a display order.
fn display_rows(order: &[usize]) -> Vec<usize> {
    let mut rows = vec![0; order.len()];
    for (row, index) in order.iter().enumerate() {
        rows[*index] = row;
    }
    rows
}

/// Offset of the middle of the slot shown at `row` below the top of its block.
fn slot_offset(row: usize) -> f32 {
    row as f32 * SLOT_HEIGHT + SLOT_HEIGHT / 2.0
}

/// Map heights of the blocks and slot anchors.
struct Anchors {
    lane_tops: Vec<f32>,
    /// Top of each node on the map.
    tops: Vec<f32>,
    input_rows: Vec<Vec<usize>>,
    output_rows: Vec<Vec<usize>>,
}

impl Anchors {
    fn new(layered: &Layered, arrangement: &Arrangement) -> Self {
        let lane_tops = arrangement.lane_tops(layered);
        Self {
            tops: layered
                .nodes
                .iter()
                .zip(&arrangement.tops)
                .map(|(node, top)| lane_tops[node.lane] + top)
                .collect(),
            lane_tops,
            input_rows: arrangement.inputs.iter().map(|o| display_rows(o)).collect(),
            output_rows: arrangement
                .outputs
                .iter()
                .map(|o| display_rows(o))
                .collect(),
        }
    }

    fn input_y(&self, slot: Slot) -> f32 {
        self.tops[slot.node] + slot_offset(self.input_rows[slot.node][slot.index])
    }

    fn output_y(&self, slot: Slot) -> f32 {
        self.tops[slot.node] + slot_offset(self.output_rows[slot.node][slot.index])
    }
}

/// A link drawn as a straight segment, its left end first.
struct Segment {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    ends: [usize; 2],
}

impl Segment {
    fn new(layered: &Layered, anchors: &Anchors, link: &Link) -> Self {
        let from = (
            layered.nodes[link.from.node].x + BLOCK_WIDTH,
            anchors.output_y(link.from),
        );
        let to = (layered.nodes[link.to.node].x, anchors.input_y(link.to));
        let ((x0, y0), (x1, y1)) = if from.0 <= to.0 {
            (from, to)
        } else {
            (to, from)
        };
        Self {
            x0,
            y0,
            x1,
            y1,
            ends: [link.from.node, link.to.node],
        }
    }

    fn y_at(&self, x: f32) -> f32 {
        self.y0 + (self.y1 - self.y0) * (x - self.x0) / (self.x1 - self.x0)
    }

    /// Over the span both links share, their order flips between both ends.
    fn crosses(&self, other: &Segment) -> bool {
        let (lo, hi) = (self.x0.max(other.x0), self.x1.min(other.x1));
        lo < hi && (self.y_at(lo) - other.y_at(lo)) * (self.y_at(hi) - other.y_at(hi)) < 0.0
    }

    /// Passes through the block of `node`, its top at `top`, between its ends.
    fn hits(&self, layered: &Layered, node: usize, top: f32) -> bool {
        let x = layered.nodes[node].x;
        let (lo, hi) = (x.max(self.x0), (x + BLOCK_WIDTH).min(self.x1));
        if self.ends.contains(&node) || lo >= hi {
            return false;
        }
        let (y_lo, y_hi) = (self.y_at(lo), self.y_at(hi));
        y_lo.max(y_hi) > top && y_lo.min(y_hi) < top + layered.nodes[node].height()
    }
}

/// Crossings of a placement: link pairs crossing each other and links drawn through a block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Crossings {
    pub links: usize,
    pub blocks: usize,
}

impl Crossings {
    pub fn total(&self) -> usize {
        self.links + self.blocks
    }
}

/// What the untangling cuts: the crossings, then the height the links drop.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Score {
    crossings: Crossings,
    drop: f32,
}

impl Score {
    fn beats(&self, other: &Score) -> bool {
        match self.crossings.total().cmp(&other.crossings.total()) {
            Ordering::Less => true,
            Ordering::Equal => self.drop < other.drop,
            Ordering::Greater => false,
        }
    }
}

/// Counts the crossings of the links at `arrangement`. Two links cross when their order flips
/// over the span they share, as with the virtual nodes of long links at every x.
pub fn crossings(layered: &Layered, arrangement: &Arrangement) -> Crossings {
    score(layered, arrangement).crossings
}

fn score(layered: &Layered, arrangement: &Arrangement) -> Score {
    let anchors = Anchors::new(layered, arrangement);
    let mut segments: Vec<Segment> = layered
        .links
        .iter()
        .map(|link| Segment::new(layered, &anchors, link))
        .collect();
    segments.sort_by(|a, b| a.x0.total_cmp(&b.x0));

    let mut links = 0;
    for (i, a) in segments.iter().enumerate() {
        links += segments[i + 1..]
            .iter()
            .take_while(|b| b.x0 < a.x1)
            .filter(|b| a.crosses(b))
            .count();
    }
    let by_x = layered.by_x();
    let mut blocks = 0;
    for segment in &segments {
        let first = by_x.partition_point(|n| layered.nodes[*n].x + BLOCK_WIDTH <= segment.x0);
        blocks += by_x[first..]
            .iter()
            .take_while(|n| layered.nodes[**n].x < segment.x1)
            .filter(|n| segment.hits(layered, **n, anchors.tops[**n]))
            .count();
    }
    Score {
        crossings: Crossings { links, blocks },
        drop: segments.iter().map(|s| (s.y1 - s.y0).abs()).sum(),
    }
}

/// A node, the nodes it links to and their links: what moving the node changes, as their
/// slots are ordered again.
struct Neighborhood {
    node: usize,
    /// The node and its peers, their slots ordered again for each row tried.
    nodes: Vec<usize>,
    links: Vec<usize>,
    /// The other links, left as they are.
    others: Vec<Segment>,
}

/// Slot orders of the nodes of a neighborhood, `(node, inputs, outputs)`.
type NodeOrders = Vec<(usize, Vec<usize>, Vec<usize>)>;

impl Neighborhood {
    fn new(layered: &Layered, anchors: &Anchors, node: usize) -> Self {
        let mut nodes: Vec<usize> = layered.node_links[node]
            .iter()
            .flat_map(|link| [layered.links[*link].from.node, layered.links[*link].to.node])
            .chain([node])
            .collect();
        nodes.sort_unstable();
        nodes.dedup();
        let mut links: Vec<usize> = nodes
            .iter()
            .flat_map(|n| layered.node_links[*n].iter().copied())
            .collect();
        links.sort_unstable();
        links.dedup();
        let others = layered
            .links
            .iter()
            .enumerate()
            .filter(|(index, _)| links.binary_search(index).is_err())
            .map(|(_, link)| Segment::new(layered, anchors, link))
            .collect();
        Self {
            node,
            nodes,
            links,
            others,
        }
    }

    /// Crossings of the links of the neighborhood and of the other links through the block
    /// of the node, its top on the map at `top` and the slots of the neighborhood ordered
    /// again, with these orders.
    fn crossings(&self, layered: &Layered, anchors: &mut Anchors, top: f32) -> (usize, NodeOrders) {
        let current_top = std::mem::replace(&mut anchors.tops[self.node], top);
        let mut orders = Vec::with_capacity(self.nodes.len());
        let mut saved = Vec::with_capacity(self.nodes.len());
        for node in &self.nodes {
            let inputs = linked_first(&layered.input_peers[*node], |peer| anchors.output_y(peer));
            let outputs = linked_first(&layered.output_peers[*node], |peer| anchors.input_y(peer));
            saved.push((
                std::mem::replace(&mut anchors.input_rows[*node], display_rows(&inputs)),
                std::mem::replace(&mut anchors.output_rows[*node], display_rows(&outputs)),
            ));
            orders.push((*node, inputs, outputs));
        }
        let own: Vec<Segment> = self
            .links
            .iter()
            .map(|link| Segment::new(layered, anchors, &layered.links[*link]))
            .collect();
        let mut count = 0;
        for (i, segment) in own.iter().enumerate() {
            count += own[i + 1..]
                .iter()
                .chain(&self.others)
                .filter(|other| segment.crosses(other))
                .count();
            count += (0..layered.nodes.len())
                .filter(|n| segment.hits(layered, *n, anchors.tops[*n]))
                .count();
        }
        count += self
            .others
            .iter()
            .filter(|other| other.hits(layered, self.node, top))
            .count();
        anchors.tops[self.node] = current_top;
        for (node, (inputs, outputs)) in self.nodes.iter().zip(saved) {
            anchors.input_rows[*node] = inputs;
            anchors.output_rows[*node] = outputs;
        }
        (count, orders)
    }
}

/// Where a node goes: one of the rows of its lane or a new row below them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Row(usize),
    NewRow,
}

/// The rows of every lane, top to bottom, and the slot display orders.
#[derive(Debug, Clone)]
struct State {
    rows: Vec<Vec<Vec<usize>>>,
    inputs: Vec<Vec<usize>>,
    outputs: Vec<Vec<usize>>,
}

impl State {
    /// One row per distinct top in each lane; a node clashing with its row takes the next free
    /// row below.
    fn new(layered: &Layered, arrangement: &Arrangement) -> Self {
        let mut rows = vec![Vec::new(); layered.lanes];
        for (lane, lane_rows) in rows.iter_mut().enumerate() {
            let mut nodes: Vec<usize> = (0..layered.nodes.len())
                .filter(|n| layered.nodes[*n].lane == lane)
                .collect();
            nodes.sort_by(|a, b| {
                let (top_a, top_b) = (arrangement.tops[*a], arrangement.tops[*b]);
                top_a
                    .total_cmp(&top_b)
                    .then(layered.nodes[*a].x.total_cmp(&layered.nodes[*b].x))
                    .then(a.cmp(b))
            });
            let mut last_top = None;
            let mut first = 0;
            for node in nodes {
                let top = arrangement.tops[node];
                if last_top != Some(top) {
                    last_top = Some(top);
                    lane_rows.push(Vec::new());
                    first = lane_rows.len() - 1;
                }
                let free = (first..lane_rows.len())
                    .find(|row| !clashes_with(layered, &lane_rows[*row], node));
                match free {
                    Some(row) => lane_rows[row].push(node),
                    None => lane_rows.push(vec![node]),
                }
            }
        }
        Self {
            rows,
            inputs: arrangement.inputs.clone(),
            outputs: arrangement.outputs.clone(),
        }
    }

    /// Rows packed from the top of their lane, each as tall as its tallest block and the
    /// clearance below.
    fn arrangement(&self, layered: &Layered) -> Arrangement {
        let mut tops = vec![0.0; layered.nodes.len()];
        for lane_rows in &self.rows {
            for (row, row_top) in lane_rows.iter().zip(row_tops(layered, lane_rows)) {
                for node in row {
                    tops[*node] = row_top;
                }
            }
        }
        Arrangement {
            tops,
            inputs: self.inputs.clone(),
            outputs: self.outputs.clone(),
        }
    }

    fn row_of(&self, layered: &Layered, node: usize) -> usize {
        self.rows[layered.nodes[node].lane]
            .iter()
            .position(|row| row.contains(&node))
            .expect("every node has a row")
    }

    /// Rows of its lane `node` fits in, left alone.
    fn free_rows(&self, layered: &Layered, node: usize) -> Vec<Target> {
        let lane_rows = &self.rows[layered.nodes[node].lane];
        (0..lane_rows.len())
            .filter(|row| {
                let others: Vec<usize> = lane_rows[*row]
                    .iter()
                    .copied()
                    .filter(|other| *other != node)
                    .collect();
                !clashes_with(layered, &others, node)
            })
            .map(Target::Row)
            .chain([Target::NewRow])
            .collect()
    }

    fn move_to(&mut self, layered: &Layered, node: usize, target: Target) {
        let from = self.row_of(layered, node);
        let lane_rows = &mut self.rows[layered.nodes[node].lane];
        match target {
            Target::Row(row) if row == from => return,
            Target::Row(row) => lane_rows[row].push(node),
            Target::NewRow => lane_rows.push(vec![node]),
        }
        lane_rows[from].retain(|other| *other != node);
        lane_rows.retain(|row| !row.is_empty());
    }

    /// Moves `node` to the free row of its lane with the fewest crossings around it, the
    /// closest to `wanted` among them, the upper one on a tie, and orders the slots around it
    /// again.
    fn settle(&mut self, layered: &Layered, anchors: &mut Anchors, node: usize, wanted: f32) {
        let lane = layered.nodes[node].lane;
        let lane_rows = &self.rows[lane];
        let tops = row_tops(layered, lane_rows);
        let bottom = lane_rows
            .last()
            .map_or(0.0, |row| tops[tops.len() - 1] + row_height(layered, row));
        let top = |target: &Target| match target {
            Target::Row(row) => tops[*row],
            Target::NewRow => bottom,
        };
        let neighborhood = Neighborhood::new(layered, anchors, node);
        let best = self
            .free_rows(layered, node)
            .into_iter()
            .map(|target| {
                let map_top = anchors.lane_tops[lane] + top(&target);
                let (crossings, orders) = neighborhood.crossings(layered, anchors, map_top);
                (target, crossings, (top(&target) - wanted).abs(), orders)
            })
            .min_by(|a, b| a.1.cmp(&b.1).then(a.2.total_cmp(&b.2)));
        if let Some((target, _, _, orders)) = best {
            self.move_to(layered, node, target);
            for (node, inputs, outputs) in orders {
                self.inputs[node] = inputs;
                self.outputs[node] = outputs;
            }
        }
    }

    /// Moves every node, left to right when `forward`, to its best row.
    fn sweep(&mut self, layered: &Layered, forward: bool) {
        let mut order = layered.by_x();
        if !forward {
            order.reverse();
        }
        for node in order {
            let arrangement = self.arrangement(layered);
            let mut anchors = Anchors::new(layered, &arrangement);
            let wanted =
                wanted_top(layered, &anchors, node, forward).unwrap_or(arrangement.tops[node]);
            self.settle(layered, &mut anchors, node, wanted);
        }
    }

    /// Orders the linked slots of every node by the height of their peer, the slots without
    /// a link after them in their true order.
    fn order_slots(&mut self, layered: &Layered) {
        let anchors = Anchors::new(layered, &self.arrangement(layered));
        self.inputs = layered
            .input_peers
            .iter()
            .map(|peers| linked_first(peers, |peer| anchors.output_y(peer)))
            .collect();
        self.outputs = layered
            .output_peers
            .iter()
            .map(|peers| linked_first(peers, |peer| anchors.input_y(peer)))
            .collect();
    }
}

/// Top in its lane `node` would like: the mean of the tops lining each linked slot up with its
/// peer, on the inputs when `forward`, else on the outputs. A node linked on one side only
/// uses that side.
fn wanted_top(layered: &Layered, anchors: &Anchors, node: usize, forward: bool) -> Option<f32> {
    let inputs: Vec<f32> = layered.input_peers[node]
        .iter()
        .enumerate()
        .filter_map(|(index, peer)| {
            Some(anchors.output_y((*peer)?) - slot_offset(anchors.input_rows[node][index]))
        })
        .collect();
    let outputs: Vec<f32> = layered.output_peers[node]
        .iter()
        .enumerate()
        .filter_map(|(index, peer)| {
            Some(anchors.input_y((*peer)?) - slot_offset(anchors.output_rows[node][index]))
        })
        .collect();
    let (first, second) = if forward {
        (inputs, outputs)
    } else {
        (outputs, inputs)
    };
    let wanted = if first.is_empty() { second } else { first };
    if wanted.is_empty() {
        return None;
    }
    let lane_top = anchors.lane_tops[layered.nodes[node].lane];
    Some(wanted.iter().sum::<f32>() / wanted.len() as f32 - lane_top)
}

fn clashes_with(layered: &Layered, row: &[usize], node: usize) -> bool {
    row.iter()
        .any(|other| layered.nodes[*other].clashes(&layered.nodes[node]))
}

fn row_height(layered: &Layered, row: &[usize]) -> f32 {
    row.iter()
        .map(|node| layered.nodes[*node].height())
        .fold(0.0, f32::max)
        + BLOCK_CLEARANCE
}

/// Top of each row of a lane, the first one at 0.
fn row_tops(layered: &Layered, rows: &[Vec<usize>]) -> Vec<f32> {
    let mut top = 0.0;
    rows.iter()
        .map(|row| {
            let row_top = top;
            top += row_height(layered, row);
            row_top
        })
        .collect()
}

/// Display order of a column: the linked slots by the height of their peer, then the others.
fn linked_first(peers: &[Option<Slot>], peer_y: impl Fn(Slot) -> f32) -> Vec<usize> {
    let mut linked: Vec<(usize, f32)> = peers
        .iter()
        .enumerate()
        .filter_map(|(index, peer)| Some((index, peer_y((*peer)?))))
        .collect();
    linked.sort_by(|(a, ya), (b, yb)| ya.total_cmp(yb).then(a.cmp(b)));
    let unlinked = peers
        .iter()
        .enumerate()
        .filter(|(_, peer)| peer.is_none())
        .map(|(index, _)| index);
    linked
        .into_iter()
        .map(|(index, _)| index)
        .chain(unlinked)
        .collect()
}

/// The state with the best score seen.
struct Best {
    state: State,
    score: Score,
}

impl Best {
    fn new(layered: &Layered, state: State) -> Self {
        let score = score(layered, &state.arrangement(layered));
        Self { state, score }
    }

    fn offer(&mut self, layered: &Layered, state: &State) {
        let found = score(layered, &state.arrangement(layered));
        if found.beats(&self.score) {
            self.state = state.clone();
            self.score = found;
        }
    }
}

/// Rows and slot orders cutting the crossings of the lanes, then straightening the links.
/// Each lane starts cluster by cluster on the rows of its `Spine`, then sweeps move each node
/// in turn to its best row. Each node keeps its lane and x; the rows are packed again. The
/// result never has more crossings than `start` packed.
pub fn untangle(layered: &Layered, start: &Arrangement) -> Arrangement {
    let mut best = Best::new(layered, State::new(layered, start));
    let mut state = State {
        rows: spine_rows(layered),
        inputs: start.inputs.clone(),
        outputs: start.outputs.clone(),
    };
    state.order_slots(layered);
    best.offer(layered, &state);
    for sweep in 0..SWEEPS {
        state.sweep(layered, sweep % 2 == 0);
        best.offer(layered, &state);
        state.order_slots(layered);
        best.offer(layered, &state);
    }
    best.state.arrangement(layered)
}

/// Groups of nodes linked to each other within their lane, the largest first, then the one
/// with the oldest node, each in time order.
fn clusters(layered: &Layered) -> Vec<Vec<usize>> {
    let mut seen = vec![false; layered.nodes.len()];
    let mut found = Vec::new();
    for first in 0..layered.nodes.len() {
        if seen[first] {
            continue;
        }
        seen[first] = true;
        let mut cluster = vec![first];
        let mut next = 0;
        while let Some(node) = cluster.get(next).copied() {
            next += 1;
            for peer in layered.peers(node) {
                if !seen[peer] && layered.nodes[peer].lane == layered.nodes[node].lane {
                    seen[peer] = true;
                    cluster.push(peer);
                }
            }
        }
        cluster.sort_unstable();
        found.push(cluster);
    }
    found.sort_by_key(|cluster| (Reverse(cluster.len()), cluster[0]));
    found
}

/// Nodes of a cluster in an order putting every parent before its children, the oldest first
/// when free to choose.
fn parents_first(layered: &Layered, cluster: &[usize]) -> Vec<usize> {
    let mut parents: HashMap<usize, usize> = cluster.iter().map(|node| (*node, 0)).collect();
    for node in cluster {
        for child in layered.children(*node) {
            if let Some(count) = parents.get_mut(&child) {
                *count += 1;
            }
        }
    }
    let mut ready: BinaryHeap<Reverse<usize>> = parents
        .iter()
        .filter(|(_, count)| **count == 0)
        .map(|(node, _)| Reverse(*node))
        .collect();
    let mut order = Vec::with_capacity(cluster.len());
    while let Some(Reverse(node)) = ready.pop() {
        order.push(node);
        for child in layered.children(node) {
            if let Some(count) = parents.get_mut(&child) {
                *count -= 1;
                if *count == 0 {
                    ready.push(Reverse(child));
                }
            }
        }
    }
    order
}

/// A chain ending at a node: its length, the links of its nodes, its first node and the end
/// of the chain before it.
#[derive(Debug, Clone, Copy)]
struct ChainEnd {
    len: usize,
    links: usize,
    first: usize,
    before: Option<(usize, bool)>,
}

impl ChainEnd {
    /// Longer, then with more links, then starting with an older node.
    fn beats(&self, other: &ChainEnd) -> bool {
        (self.len, self.links, Reverse(self.first)) > (other.len, other.links, Reverse(other.first))
    }
}

/// The rows of a cluster: its longest chain alone on one row, then the longest chain of the
/// nodes left with a node linked to a placed one, on the nearest free row above or below that
/// placed node, the sides taken in turn.
#[derive(Debug, Clone, PartialEq)]
struct Spine {
    /// Rows, top to bottom.
    rows: Vec<Vec<usize>>,
    /// Chains in the order they were placed, the longest first.
    chains: Vec<Vec<usize>>,
}

impl Spine {
    fn new(layered: &Layered, cluster: &[usize]) -> Self {
        let order = parents_first(layered, cluster);
        let mut row_of: HashMap<usize, i64> = HashMap::new();
        let mut rows: BTreeMap<i64, Vec<usize>> = BTreeMap::new();
        let mut chains = Vec::new();
        let mut below = true;
        while row_of.len() < cluster.len() {
            let chain = longest_chain(layered, &order, &row_of);
            let row = match attachment(layered, &chain, &row_of) {
                Some(attached) => {
                    let (row, side_below) = free_row(layered, &rows, &chain, attached, below);
                    below = !side_below;
                    row
                }
                None => 0,
            };
            for node in &chain {
                row_of.insert(*node, row);
                rows.entry(row).or_default().push(*node);
            }
            chains.push(chain);
        }
        Self {
            rows: rows.into_values().collect(),
            chains,
        }
    }
}

/// Longest chain of the nodes of `order` not `placed` yet with a node linked to a placed one,
/// any chain while none is placed.
fn longest_chain(layered: &Layered, order: &[usize], placed: &HashMap<usize, i64>) -> Vec<usize> {
    let attached = |node: usize| {
        placed.is_empty() || layered.peers(node).any(|peer| placed.contains_key(&peer))
    };
    // The best chain ending at each node, without then with an attached node.
    let mut ends: HashMap<(usize, bool), ChainEnd> = HashMap::new();
    let mut best: Option<(usize, ChainEnd)> = None;
    for node in order.iter().copied().filter(|n| !placed.contains_key(n)) {
        let own = attached(node);
        let links = layered.node_links[node].len();
        let mut found = vec![(
            own,
            ChainEnd {
                len: 1,
                links,
                first: node,
                before: None,
            },
        )];
        for parent in layered.parents(node) {
            for parent_attached in [false, true] {
                if let Some(end) = ends.get(&(parent, parent_attached)) {
                    let chain = ChainEnd {
                        len: end.len + 1,
                        links: end.links + links,
                        first: end.first,
                        before: Some((parent, parent_attached)),
                    };
                    found.push((parent_attached || own, chain));
                }
            }
        }
        for (with_attached, chain) in found {
            let key = (node, with_attached);
            if ends.get(&key).is_none_or(|end| chain.beats(end)) {
                ends.insert(key, chain);
            }
        }
        if let Some(end) = ends.get(&(node, true)) {
            if best.is_none_or(|(_, best)| end.beats(&best)) {
                best = Some((node, *end));
            }
        }
    }
    let mut chain = Vec::new();
    let mut at = best.map(|(node, _)| (node, true));
    while let Some(key) = at {
        chain.push(key.0);
        at = ends[&key].before;
    }
    chain.reverse();
    chain
}

/// Row of the first placed node linked to `chain`, `None` while none is placed.
fn attachment(layered: &Layered, chain: &[usize], placed: &HashMap<usize, i64>) -> Option<i64> {
    chain
        .iter()
        .flat_map(|node| layered.peers(*node))
        .find_map(|peer| placed.get(&peer).copied())
}

/// Nearest row above or below `attached` where `chain` meets no node, the side below first
/// when `below`, and whether it is below.
fn free_row(
    layered: &Layered,
    rows: &BTreeMap<i64, Vec<usize>>,
    chain: &[usize],
    attached: i64,
    below: bool,
) -> (i64, bool) {
    let free = |row: i64| {
        rows.get(&row)
            .is_none_or(|nodes| !chain.iter().any(|node| clashes_with(layered, nodes, *node)))
    };
    (1..)
        .flat_map(|distance| [(distance, below), (distance, !below)])
        .map(|(distance, side_below)| {
            let row = if side_below {
                attached + distance
            } else {
                attached - distance
            };
            (row, side_below)
        })
        .find(|(row, _)| free(*row))
        .expect("rows past the last one are free")
}

/// Rows of every lane: the clusters of the lane on the rows of their `Spine`, the largest
/// first, each on the highest rows where it meets no cluster placed before.
fn spine_rows(layered: &Layered) -> Vec<Vec<Vec<usize>>> {
    let mut rows = vec![Vec::new(); layered.lanes];
    // Rows and x span each cluster placed in a lane takes.
    let mut taken: Vec<Vec<(usize, usize, f32, f32)>> = vec![Vec::new(); layered.lanes];
    for cluster in clusters(layered) {
        let lane = layered.nodes[cluster[0]].lane;
        let spine = Spine::new(layered, &cluster);
        let left = cluster
            .iter()
            .map(|n| layered.nodes[*n].left())
            .fold(f32::MAX, f32::min);
        let right = cluster
            .iter()
            .map(|n| layered.nodes[*n].right())
            .fold(f32::MIN, f32::max);
        let height = spine.rows.len();
        let meets = |first: usize| {
            taken[lane].iter().any(|(top, rows, l, r)| {
                first < top + rows
                    && *top < first + height
                    && left < r + BLOCK_CLEARANCE
                    && *l < right + BLOCK_CLEARANCE
            })
        };
        let first = std::iter::once(0)
            .chain(taken[lane].iter().map(|(top, rows, ..)| top + rows))
            .filter(|first| !meets(*first))
            .min()
            .expect("rows below every cluster are free");
        let lane_rows = &mut rows[lane];
        if lane_rows.len() < first + height {
            lane_rows.resize(first + height, Vec::new());
        }
        for (row, nodes) in spine.rows.into_iter().enumerate() {
            lane_rows[first + row].extend(nodes);
        }
        taken[lane].push((first, height, left, right));
    }
    rows
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use liana_ui::component::panels::map::U;

    use crate::app::state::map::{
        layout::{BLOCK_CLEARANCE, CROSS_STEP, LANE_PITCH},
        topology::{TopoInput, TopoOutput, Topology},
        untangle::{
            clusters, crossings, spine_rows, untangle, Arrangement, Crossings, Layered, Link, Node,
            Slot, Spine, State,
        },
    };

    /// A node without leaves, its x set by `laid_out`.
    fn node(lane: usize, inputs: usize, outputs: usize) -> Node {
        Node {
            lane,
            x: 0.0,
            inputs,
            outputs,
            input_leaves: false,
            output_leaves: false,
        }
    }

    fn slot(node: usize, index: usize) -> Slot {
        Slot { node, index }
    }

    fn link(from: (usize, usize), to: (usize, usize)) -> Link {
        Link {
            from: slot(from.0, from.1),
            to: slot(to.0, to.1),
        }
    }

    /// The nodes laid out as the lanes placement does by default: each one `CROSS_STEP` right
    /// of the one before and `LANE_PITCH` right of its lane and parents, one row below its
    /// lowest parent of its lane, the slots in their true order.
    fn laid_out(lanes: usize, mut nodes: Vec<Node>, links: Vec<Link>) -> (Layered, Arrangement) {
        let parents = |node: usize| {
            links
                .iter()
                .filter(move |link| link.to.node == node)
                .map(|link| link.from.node)
        };
        let mut rows: Vec<usize> = Vec::new();
        let mut heights: Vec<Vec<f32>> = vec![Vec::new(); lanes];
        for index in 0..nodes.len() {
            let lane = nodes[index].lane;
            let after_lane = nodes[..index]
                .iter()
                .filter(|other| other.lane == lane)
                .map(|other| other.x + LANE_PITCH);
            let after_parents = parents(index).map(|parent| nodes[parent].x + LANE_PITCH);
            let x = index
                .checked_sub(1)
                .map(|before| nodes[before].x + CROSS_STEP)
                .into_iter()
                .chain(after_lane)
                .chain(after_parents)
                .reduce(f32::max)
                .map_or(0.0, |x| (x / U).ceil() * U);
            nodes[index].x = x;
            let row = parents(index)
                .filter(|parent| nodes[*parent].lane == lane)
                .map(|parent| rows[parent] + 1)
                .max()
                .unwrap_or(0);
            rows.push(row);
            let lane_heights = &mut heights[lane];
            if lane_heights.len() <= row {
                lane_heights.resize(row + 1, 0.0);
            }
            lane_heights[row] = lane_heights[row].max(nodes[index].height() + BLOCK_CLEARANCE);
        }
        let start = Arrangement {
            tops: nodes
                .iter()
                .zip(&rows)
                .map(|(node, row)| heights[node.lane][..*row].iter().sum())
                .collect(),
            inputs: nodes.iter().map(|n| (0..n.inputs).collect()).collect(),
            outputs: nodes.iter().map(|n| (0..n.outputs).collect()).collect(),
        };
        (Layered::new(lanes, nodes, links), start)
    }

    /// A deterministic tangle: `count` nodes over `lanes`, about half of the inputs spending an
    /// output of an older node.
    fn tangle(seed: u64, lanes: usize, count: usize) -> (Layered, Arrangement) {
        let mut state = seed;
        let mut next = |n: usize| {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((state >> 33) % n as u64) as usize
        };
        let mut nodes = Vec::new();
        let mut free: Vec<(usize, usize)> = Vec::new();
        let mut links = Vec::new();
        for index in 0..count {
            let (lane, inputs, outputs) = (next(lanes), 1 + next(3), 1 + next(4));
            let mut external = false;
            for input in 0..inputs {
                if free.is_empty() || next(2) == 0 {
                    external = true;
                    continue;
                }
                let from = free.swap_remove(next(free.len()));
                links.push(link(from, (index, input)));
            }
            free.extend((0..outputs).map(|output| (index, output)));
            nodes.push(Node {
                input_leaves: external,
                output_leaves: next(3) == 0,
                ..node(lane, inputs, outputs)
            });
        }
        laid_out(lanes, nodes, links)
    }

    /// Rows of a lane never overlap: blocks whose rows meet are far enough apart.
    fn assert_no_overlap(layered: &Layered, arrangement: &Arrangement) {
        for (a, node_a) in layered.nodes.iter().enumerate() {
            for (b, node_b) in layered.nodes.iter().enumerate().skip(a + 1) {
                let (top_a, top_b) = (arrangement.tops[a], arrangement.tops[b]);
                let meet = top_a < top_b + node_b.height() + BLOCK_CLEARANCE
                    && top_b < top_a + node_a.height() + BLOCK_CLEARANCE;
                assert!(
                    node_a.lane != node_b.lane || !meet || !node_a.clashes(node_b),
                    "nodes {} and {} overlap",
                    a,
                    b
                );
            }
        }
    }

    #[test]
    fn ordering_the_outputs_uncrosses_two_links() {
        // A pays B one row below from its first output and C on its row from its second one.
        let nodes = vec![node(0, 1, 2), node(0, 1, 1), node(0, 1, 1)];
        let links = vec![link((0, 0), (1, 0)), link((0, 1), (2, 0))];
        let (layered, mut start) = laid_out(1, nodes, links);
        start.tops = vec![0.0, 180.0, 0.0];
        assert_eq!(
            crossings(&layered, &start),
            Crossings {
                links: 1,
                blocks: 0
            }
        );
        let untangled = untangle(&layered, &start);
        assert_eq!(
            crossings(&layered, &untangled),
            Crossings {
                links: 0,
                blocks: 0
            }
        );
    }

    #[test]
    fn a_link_leaves_the_block_in_its_way() {
        // A funds C two lane pitches later, B sits on their row in between.
        let nodes = vec![node(0, 1, 1), node(0, 1, 1), node(0, 1, 1)];
        let (layered, start) = laid_out(1, nodes, vec![link((0, 0), (2, 0))]);
        assert_eq!(start.tops, vec![0.0, 0.0, 180.0]);
        let mut through = start.clone();
        through.tops[2] = 0.0;
        assert_eq!(
            crossings(&layered, &through),
            Crossings {
                links: 0,
                blocks: 1
            }
        );
        let untangled = untangle(&layered, &through);
        assert_eq!(crossings(&layered, &untangled).total(), 0);
        // C lines up with A, B gives way.
        assert_eq!(untangled.tops, vec![0.0, 180.0, 0.0]);
    }

    #[test]
    fn untangling_never_adds_crossings() {
        for seed in 0..20 {
            let (layered, start) = tangle(seed, 3, 40);
            let before = crossings(&layered, &start);
            let untangled = untangle(&layered, &start);
            let after = crossings(&layered, &untangled);
            assert!(after.total() <= before.total(), "seed {}", seed);
            assert_no_overlap(&layered, &untangled);
            assert_eq!(untangled, untangle(&layered, &start), "seed {seed}");
            for (node, (order, slots)) in untangled
                .inputs
                .iter()
                .zip(layered.nodes.iter().map(|n| n.inputs))
                .enumerate()
            {
                let mut sorted = order.clone();
                sorted.sort();
                assert_eq!(sorted, (0..slots).collect::<Vec<_>>(), "node {node}");
            }
        }
    }

    #[test]
    fn untangling_cuts_the_crossings_of_tangles() {
        let (mut before, mut after) = (0, 0);
        for seed in 0..20 {
            let (layered, start) = tangle(seed, 3, 40);
            before += crossings(&layered, &start).total();
            after += crossings(&layered, &untangle(&layered, &start)).total();
        }
        assert!(
            after * 2 < before,
            "{} crossings before, {} after",
            before,
            after
        );
    }

    /// A chain 0 to 3, the chain 4 and 5 spending the second output of 0 and 6 spending the
    /// second output of 1, in one lane.
    fn spined() -> (Layered, Arrangement) {
        let nodes = vec![
            node(0, 1, 2),
            node(0, 1, 2),
            node(0, 1, 1),
            node(0, 1, 1),
            node(0, 1, 1),
            node(0, 1, 1),
            node(0, 1, 1),
        ];
        let links = vec![
            link((0, 0), (1, 0)),
            link((1, 0), (2, 0)),
            link((2, 0), (3, 0)),
            link((0, 1), (4, 0)),
            link((4, 0), (5, 0)),
            link((1, 1), (6, 0)),
        ];
        laid_out(1, nodes, links)
    }

    #[test]
    fn the_spine_puts_the_longest_chain_on_one_row_and_the_others_around() {
        let (layered, _) = spined();
        let spine = Spine::new(&layered, &[0, 1, 2, 3, 4, 5, 6]);
        assert_eq!(spine.chains, vec![vec![0, 1, 2, 3], vec![4, 5], vec![6]]);
        // The first satellite below, the next one above.
        assert_eq!(spine.rows, vec![vec![6], vec![0, 1, 2, 3], vec![4, 5]]);
    }

    #[test]
    fn lane_clusters_only_follow_links_within_a_lane() {
        let nodes = vec![node(0, 1, 1), node(1, 1, 1), node(0, 1, 1), node(0, 1, 1)];
        let links = vec![link((0, 0), (1, 0)), link((0, 0), (2, 0))];
        let (layered, _) = laid_out(2, nodes, links);
        assert_eq!(clusters(&layered), vec![vec![0, 2], vec![1], vec![3]]);
    }

    #[test]
    fn untangling_starts_on_the_spines_and_never_crosses_more() {
        // The chain 0, 2, 4 and 5, 1 spending the second output of 0 and 3 the second one of 2.
        let nodes = vec![
            node(0, 1, 2),
            node(0, 1, 1),
            node(0, 1, 2),
            node(0, 1, 1),
            node(0, 1, 1),
            node(0, 1, 1),
        ];
        let links = vec![
            link((0, 0), (2, 0)),
            link((0, 1), (1, 0)),
            link((2, 0), (4, 0)),
            link((2, 1), (3, 0)),
            link((4, 0), (5, 0)),
        ];
        let (layered, start) = laid_out(1, nodes, links);
        // The first satellite below the chain, the next one above.
        let rows = spine_rows(&layered);
        assert_eq!(rows, vec![vec![vec![3], vec![0, 2, 4, 5], vec![1]]]);
        let spine_start = State {
            rows,
            inputs: start.inputs.clone(),
            outputs: start.outputs.clone(),
        };
        let before = crossings(&layered, &spine_start.arrangement(&layered));
        let untangled = untangle(&layered, &start);
        assert!(crossings(&layered, &untangled).total() <= before.total());
    }

    /// Lays out the topology exported from the map, as the lanes placement does by default.
    fn from_topology(topology: &Topology) -> (Layered, Arrangement) {
        let nodes = topology
            .txs
            .iter()
            .map(|tx| Node {
                input_leaves: tx.inputs.contains(&TopoInput::External),
                output_leaves: tx
                    .outputs
                    .iter()
                    .any(|o| matches!(o, TopoOutput::Payment | TopoOutput::External)),
                ..node(tx.wallet, tx.inputs.len(), tx.outputs.len())
            })
            .collect();
        let links = topology
            .txs
            .iter()
            .flat_map(|tx| {
                tx.outputs
                    .iter()
                    .enumerate()
                    .filter_map(move |(index, output)| match output {
                        TopoOutput::Own { to: Some(to), .. } => {
                            Some(link((tx.order, index), (to.tx, to.slot)))
                        }
                        _ => None,
                    })
            })
            .collect();
        laid_out(topology.wallets, nodes, links)
    }

    /// Prints the crossings of the topology exported at `MAP_TOPOLOGY`, laid out by default
    /// then untangled.
    #[test]
    #[ignore]
    fn untangle_exported_topology() {
        let path = std::env::var("MAP_TOPOLOGY").expect("MAP_TOPOLOGY is the exported topology");
        let json = std::fs::read_to_string(path).expect("readable topology");
        let topology: Topology = serde_json::from_str(&json).expect("topology json");
        let (layered, start) = from_topology(&topology);
        let before = crossings(&layered, &start);
        let started = Instant::now();
        let untangled = untangle(&layered, &start);
        let elapsed = started.elapsed();
        let after = crossings(&layered, &untangled);
        let height = |a: &Arrangement| a.lane_heights(&layered).iter().sum::<f32>();
        println!(
            "{} txs, {} links: default {:?} total {} height {}, untangled {:?} total {} height {} in {:?}",
            layered.nodes.len(),
            layered.links.len(),
            before,
            before.total(),
            height(&start),
            after,
            after.total(),
            height(&untangled),
            elapsed
        );
        assert_no_overlap(&layered, &untangled);
    }
}
