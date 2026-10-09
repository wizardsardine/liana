use std::collections::HashMap;

use iced::{Point, Vector};
use liana_ui::widget::graph_view::ItemId;
use lianad::commands::GraphWallet;

use crate::app::state::map::{
    display_row,
    graph::{LeafKind, TxGraph, WalletTxs},
    history::Order,
    layout,
    offsets::{Offsets, WalletLayout},
    untangle::{untangle, Arrangement, Layered, Link, Node, Slot},
    wallets::{WalletKey, WalletStore},
    MapWallet, Orders,
};

/// A wallet loaded on the map, in lane order.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WalletLane {
    pub wallet: WalletKey,
    /// A hidden wallet is left out of the graph.
    pub displayed: bool,
}

/// Vertical span of a lane, map px.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Band {
    pub top: f32,
    pub height: f32,
}

/// The loaded wallets in lane order, only the displayed ones drawn.
#[derive(Debug, Default)]
pub struct LoadedWallets {
    pub lanes: Vec<WalletLane>,
    pub stores: HashMap<WalletKey, WalletStore>,
    pub txs: Vec<WalletTxs>,
    pub layouts: Vec<(WalletKey, WalletLayout)>,
    /// Height of each resized lane.
    pub heights: HashMap<WalletKey, f32>,
}

/// Puts the loaded wallets in lane order: the stored lanes first in their order, then the
/// current wallet, then the others by row key.
pub fn split_loaded(mut wallets: Vec<MapWallet>) -> LoadedWallets {
    wallets.sort_by_key(|wallet| {
        (
            wallet.lane.is_none(),
            wallet.lane,
            wallet.txs.key != WalletKey::Current,
            wallet.txs.key.row(),
        )
    });
    let mut loaded = LoadedWallets::default();
    for wallet in wallets {
        let key = wallet.txs.key.clone();
        loaded.lanes.push(WalletLane {
            wallet: key.clone(),
            displayed: wallet.displayed,
        });
        if let Some(store) = wallet.store {
            loaded.stores.insert(key.clone(), store);
        }
        if let Some(height) = wallet.lane_height {
            loaded.heights.insert(key.clone(), height);
        }
        if wallet.displayed {
            loaded.layouts.push((key, wallet.layout));
            loaded.txs.push(wallet.txs);
        }
    }
    loaded
}

/// Least height of the lane of `wallet`: its items at `lane_layout`, the clearance of a row
/// and the gap below them.
pub fn content_height(
    graph: &TxGraph,
    lane_layout: &HashMap<ItemId, Point>,
    wallet: &WalletKey,
) -> f32 {
    let items = graph.wallet_items(wallet);
    let positions = items.iter().filter_map(|id| lane_layout.get_key_value(id));
    layout::bounds(graph, positions).map_or(0.0, |rect| {
        rect.y + rect.height + layout::BLOCK_CLEARANCE + layout::LANE_GAP
    })
}

/// Bands of the lanes of `wallets` stacked down from 0 in their order. A lane is as tall as
/// its resized height (`resized`), else its default content (`defaults`), but never shorter
/// than its items at `lane_layout`.
pub fn stacked_bands(
    graph: &TxGraph,
    lane_layout: &HashMap<ItemId, Point>,
    defaults: &HashMap<WalletKey, f32>,
    resized: &HashMap<WalletKey, f32>,
    wallets: &[WalletKey],
) -> Vec<(WalletKey, Band)> {
    let heights: Vec<f32> = wallets
        .iter()
        .map(|wallet| {
            let height = resized
                .get(wallet)
                .or_else(|| defaults.get(wallet))
                .copied()
                .unwrap_or(0.0);
            height.max(content_height(graph, lane_layout, wallet))
        })
        .collect();
    let tops = stacked_tops(&heights);
    wallets
        .iter()
        .zip(tops.into_iter().zip(heights))
        .map(|(wallet, (top, height))| (wallet.clone(), Band { top, height }))
        .collect()
}

/// Map positions of the items at `lane_layout`, each shifted down to the top of its lane.
pub fn shown(
    graph: &TxGraph,
    lane_layout: &HashMap<ItemId, Point>,
    bands: &[(WalletKey, Band)],
) -> HashMap<ItemId, Point> {
    let tops: HashMap<&WalletKey, f32> = bands
        .iter()
        .map(|(wallet, band)| (wallet, band.top))
        .collect();
    lane_layout
        .iter()
        .filter_map(|(id, p)| {
            let top = tops.get(graph.item_wallet(*id)?)?;
            Some((*id, *p + Vector::new(0.0, *top)))
        })
        .collect()
}

/// Positions in their lanes, slot display orders and lane heights of untangled lanes.
#[derive(Debug, Clone, PartialEq)]
pub struct Untangled {
    pub positions: HashMap<ItemId, Point>,
    pub orders: Orders,
    pub heights: HashMap<WalletKey, f32>,
}

/// Display order of a column, `None` for the true order.
fn column_order(order: &[usize]) -> Order {
    let identity = order.iter().copied().eq(0..order.len());
    (!identity).then(|| order.iter().map(|index| *index as u32).collect())
}

/// A transaction of a displayed wallet: its index, its lane and its position in the lane.
pub type LaneTx = (usize, usize, Point);

/// The transactions at `txs` and their coin links between them.
pub fn layered(graph: &TxGraph, lanes: usize, txs: &[LaneTx]) -> Layered {
    let node_of: HashMap<usize, usize> = txs
        .iter()
        .enumerate()
        .map(|(node, (tx, ..))| (*tx, node))
        .collect();
    let nodes = txs
        .iter()
        .map(|(tx, lane, at)| {
            let map_tx = &graph.txs()[*tx];
            let input_leaf =
                |leaf: &usize| graph.leaves()[*leaf].kind == LeafKind::CounterpartyCoin;
            Node {
                lane: *lane,
                x: at.x,
                inputs: map_tx.inputs.len(),
                outputs: map_tx.outputs.len(),
                input_leaves: map_tx.leaves.iter().any(input_leaf),
                output_leaves: !map_tx.leaves.iter().all(input_leaf),
            }
        })
        .collect();
    let links = graph
        .coin_edges()
        .iter()
        .filter_map(|edge| {
            let slot = |tx: usize, index: usize| {
                Some(Slot {
                    node: *node_of.get(&tx)?,
                    index,
                })
            };
            Some(Link {
                from: slot(edge.from.tx, edge.from.index)?,
                to: slot(edge.to.tx, edge.to.index)?,
            })
        })
        .collect();
    Layered::new(lanes, nodes, links)
}

/// The transactions at `txs` with their slot display `orders`.
pub fn arrangement(graph: &TxGraph, txs: &[LaneTx], orders: &Orders) -> Arrangement {
    let display_order = |order: &Order, len: usize| match order {
        Some(order) => order.iter().map(|index| *index as usize).collect(),
        None => (0..len).collect(),
    };
    let (mut inputs, mut outputs) = (Vec::new(), Vec::new());
    for (tx, ..) in txs {
        let map_tx = &graph.txs()[*tx];
        let (input_order, output_order) = orders
            .get(&map_tx.history().txid)
            .cloned()
            .unwrap_or_default();
        inputs.push(display_order(&input_order, map_tx.inputs.len()));
        outputs.push(display_order(&output_order, map_tx.outputs.len()));
    }
    Arrangement {
        tops: txs.iter().map(|(.., at)| at.y).collect(),
        inputs,
        outputs,
    }
}

/// The items of `wallets` untangled from their positions in the lanes `start` and the slot
/// `orders`: each transaction keeps its lane and x and changes row, its leaves land next to
/// their slots. A transaction missing from `start` is left out.
pub fn untangled(
    graph: &TxGraph,
    wallets: &[WalletKey],
    start: &HashMap<ItemId, Point>,
    orders: &Orders,
) -> Untangled {
    let txs: Vec<LaneTx> = (0..graph.txs().len())
        .filter_map(|tx| {
            let lane = wallets
                .iter()
                .position(|wallet| wallet == graph.txs()[tx].primary())?;
            Some((tx, lane, *start.get(&graph.tx_item(tx))?))
        })
        .collect();
    let layered = layered(graph, wallets.len(), &txs);
    let arrangement = untangle(&layered, &arrangement(graph, &txs, orders));
    let blocks: Vec<Point> = txs
        .iter()
        .zip(&arrangement.tops)
        .map(|((.., at), top)| Point::new(at.x, *top))
        .collect();
    let (positions, orders) = placed_txs(graph, &txs, &blocks, &arrangement);
    let heights = wallets
        .iter()
        .cloned()
        .zip(arrangement.lane_heights(&layered))
        .collect();
    Untangled {
        positions,
        orders,
        heights,
    }
}

/// Positions of the transactions at `txs`, their blocks at `blocks` and their leaves next to
/// their slots, and the slot display orders of `arrangement`.
pub fn placed_txs(
    graph: &TxGraph,
    txs: &[LaneTx],
    blocks: &[Point],
    arrangement: &Arrangement,
) -> (HashMap<ItemId, Point>, Orders) {
    let mut positions = HashMap::new();
    let mut orders = Orders::new();
    for (node, ((tx, ..), block)) in txs.iter().zip(blocks).enumerate() {
        let map_tx = &graph.txs()[*tx];
        let order = (
            column_order(&arrangement.inputs[node]),
            column_order(&arrangement.outputs[node]),
        );
        positions.insert(graph.tx_item(*tx), *block);
        for leaf in &map_tx.leaves {
            let leaf_ref = &graph.leaves()[*leaf];
            let column = match leaf_ref.kind {
                LeafKind::CounterpartyCoin => &order.0,
                LeafKind::Payment | LeafKind::CounterpartyOutput => &order.1,
            };
            let row = display_row(column.as_deref(), leaf_ref.index);
            positions.insert(
                graph.leaf_item(*leaf),
                layout::leaf_position(leaf_ref.kind, *block, row),
            );
        }
        if order != (None, None) {
            orders.insert(map_tx.history().txid, order);
        }
    }
    (positions, orders)
}

/// Default positions in their lanes of the items of `wallets`, untangled.
pub fn reset(graph: &TxGraph, wallets: &[WalletKey]) -> Untangled {
    let defaults: HashMap<ItemId, Point> = wallets
        .iter()
        .flat_map(|wallet| layout::reset_lane(graph, wallet))
        .collect();
    untangled(graph, wallets, &defaults, &Orders::new())
}

/// Tops of lanes `heights` tall stacked down from 0 in their order.
pub fn stacked_tops(heights: &[f32]) -> Vec<f32> {
    let mut top = 0.0;
    heights
        .iter()
        .map(|height| {
            let lane_top = top;
            top += height;
            lane_top
        })
        .collect()
}

/// The displayed wallets in lane order.
pub fn displayed(lanes: &[WalletLane]) -> Vec<WalletKey> {
    lanes
        .iter()
        .filter(|lane| lane.displayed)
        .map(|lane| lane.wallet.clone())
        .collect()
}

/// `lanes` with the lane of `wallet` moved just before the lane of `before`, at the end for
/// `None`.
pub fn moved(
    lanes: &[WalletLane],
    wallet: &WalletKey,
    before: Option<&WalletKey>,
) -> Vec<WalletLane> {
    let Some(lane) = lanes.iter().find(|lane| lane.wallet == *wallet) else {
        return lanes.to_vec();
    };
    let mut moved: Vec<WalletLane> = lanes
        .iter()
        .filter(|other| other.wallet != *wallet)
        .cloned()
        .collect();
    let at = before
        .and_then(|before| moved.iter().position(|other| other.wallet == *before))
        .unwrap_or(moved.len());
    moved.insert(at, lane.clone());
    moved
}

/// The `graph_wallets` row of each lane. The offset of a displayed wallet comes from `offsets`,
/// a hidden wallet's is filled from the stored row. `heights` holds the resized lanes.
pub fn rows(
    lanes: &[WalletLane],
    offsets: &Offsets,
    heights: &HashMap<WalletKey, f32>,
) -> Vec<GraphWallet> {
    lanes
        .iter()
        .enumerate()
        .map(|(index, lane)| GraphWallet {
            wallet: lane.wallet.row(),
            selected: true,
            offset: offsets
                .get(&lane.wallet)
                .map(|offset| (f64::from(offset.x), f64::from(offset.y))),
            lane: Some(index as u32),
            displayed: lane.displayed,
            lane_height: heights.get(&lane.wallet).map(|height| f64::from(*height)),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use iced::{Point, Vector};
    use liana_ui::widget::graph_view::ItemId;
    use lianad::commands::GraphWallet;

    use crate::app::{
        settings::WalletId,
        state::map::{
            fixture::{self, TwoWallets},
            graph::{OutputSlot, TxGraph, WalletTxs},
            lanes::{
                displayed, moved, reset, rows, shown, split_loaded, stacked_bands, stacked_tops,
                untangled, Band, WalletLane,
            },
            layout::{self, item_size, BLOCK_CLEARANCE, LANE_GAP},
            offsets::{Offsets, WalletLayout},
            wallets::WalletKey,
            MapWallet,
        },
    };

    fn lane(wallet: &WalletKey, displayed: bool) -> WalletLane {
        WalletLane {
            wallet: wallet.clone(),
            displayed,
        }
    }

    fn other(name: &str) -> WalletKey {
        WalletKey::Other(WalletId::new(name.to_string(), None))
    }

    fn two_wallets() -> (TwoWallets, TxGraph) {
        let mut two = fixture::two_wallets("a", "b");
        let graph = TxGraph::new(std::mem::take(&mut two.wallets));
        (two, graph)
    }

    #[test]
    fn stacked_tops_follow_the_order() {
        assert_eq!(
            stacked_tops(&[300.0, 240.0, 120.0]),
            vec![0.0, 300.0, 540.0]
        );
        assert!(stacked_tops(&[]).is_empty());
    }

    fn band_at(top: f32, height: f32) -> Band {
        Band { top, height }
    }

    /// Default lane positions of the current wallet and b, and the default height of each lane.
    fn default_lanes(
        two: &TwoWallets,
        graph: &TxGraph,
    ) -> (HashMap<ItemId, Point>, HashMap<WalletKey, f32>) {
        let reset = reset(graph, &[WalletKey::Current, two.b.clone()]);
        (reset.positions, reset.heights)
    }

    #[test]
    fn lane_tops_follow_the_order() {
        let (two, graph) = two_wallets();
        let (lane_layout, heights) = default_lanes(&two, &graph);
        let (current, b) = (heights[&WalletKey::Current], heights[&two.b]);

        let bands = stacked_bands(
            &graph,
            &lane_layout,
            &heights,
            &HashMap::new(),
            &[WalletKey::Current, two.b.clone()],
        );
        assert_eq!(
            bands,
            vec![
                (WalletKey::Current, band_at(0.0, current)),
                (two.b.clone(), band_at(current, b)),
            ]
        );

        let reordered = stacked_bands(
            &graph,
            &lane_layout,
            &heights,
            &HashMap::new(),
            &[two.b.clone(), WalletKey::Current],
        );
        assert_eq!(
            reordered,
            vec![
                (two.b.clone(), band_at(0.0, b)),
                (WalletKey::Current, band_at(b, current)),
            ]
        );

        // b hidden: its lane takes no space anymore.
        let hidden = stacked_bands(
            &graph,
            &lane_layout,
            &heights,
            &HashMap::new(),
            &[two.b.clone()],
        );
        assert_eq!(hidden, vec![(two.b.clone(), band_at(0.0, b))]);
    }

    #[test]
    fn shown_positions_follow_the_lane_tops() {
        let (two, graph) = two_wallets();
        let (lane_layout, heights) = default_lanes(&two, &graph);
        let current = heights[&WalletKey::Current];
        let spend = graph.tx_item(graph.tx_index(&two.spend).unwrap());
        let funding = graph.tx_item(graph.tx_index(&two.funding).unwrap());

        let bands = stacked_bands(
            &graph,
            &lane_layout,
            &heights,
            &HashMap::new(),
            &[WalletKey::Current, two.b.clone()],
        );
        let at = shown(&graph, &lane_layout, &bands);
        assert_eq!(at[&funding], lane_layout[&funding]);
        assert_eq!(at[&spend], lane_layout[&spend] + Vector::new(0.0, current));

        let bands = stacked_bands(
            &graph,
            &lane_layout,
            &heights,
            &HashMap::new(),
            &[two.b.clone(), WalletKey::Current],
        );
        let at = shown(&graph, &lane_layout, &bands);
        assert_eq!(at[&spend], lane_layout[&spend]);
        assert_eq!(
            at[&funding],
            lane_layout[&funding] + Vector::new(0.0, heights[&two.b])
        );

        // Only the items of a displayed lane are shown.
        let bands = stacked_bands(
            &graph,
            &lane_layout,
            &heights,
            &HashMap::new(),
            &[WalletKey::Current],
        );
        let at = shown(&graph, &lane_layout, &bands);
        assert!(!at.contains_key(&spend));
        assert_eq!(at[&funding], lane_layout[&funding]);
    }

    #[test]
    fn a_lane_grows_with_its_items() {
        let (two, graph) = two_wallets();
        let (mut lane_layout, heights) = default_lanes(&two, &graph);
        let current = heights[&WalletKey::Current];
        let funding = graph.tx_item(graph.tx_index(&two.funding).unwrap());
        let low = Point::new(0.0, current + 200.0);
        lane_layout.insert(funding, low);

        let bands = stacked_bands(
            &graph,
            &lane_layout,
            &heights,
            &HashMap::new(),
            &[WalletKey::Current, two.b.clone()],
        );
        let height = low.y + item_size(&graph, funding).height + BLOCK_CLEARANCE + LANE_GAP;
        assert_eq!(bands[0], (WalletKey::Current, band_at(0.0, height)));
        assert_eq!(bands[1], (two.b.clone(), band_at(height, heights[&two.b])));
    }

    #[test]
    fn reset_untangles_the_default_lanes() {
        let (two, graph) = two_wallets();
        let wallets = [WalletKey::Current, two.b.clone()];
        let lanes = reset(&graph, &wallets);
        let item = |txid| graph.tx_item(graph.tx_index(&txid).unwrap());
        // By default the payment is one row below its funding parent: untangled, it lines its
        // input up with the funding output.
        assert_eq!(
            layout::reset_lane(&graph, &WalletKey::Current)[&item(two.payment)],
            Point::new(1392.0, 180.0)
        );
        assert_eq!(
            [two.funding, two.payment, two.spend].map(|txid| lanes.positions[&item(txid)]),
            [
                Point::new(0.0, 0.0),
                Point::new(1392.0, 0.0),
                Point::new(2784.0, 0.0)
            ]
        );
        assert_eq!(lanes.positions.len(), graph.item_ids().count());
        assert!(lanes.orders.is_empty());
        assert_eq!(
            lanes.heights,
            HashMap::from([(WalletKey::Current, 252.0), (two.b.clone(), 252.0)])
        );

        let alone = reset(&graph, &[two.b.clone()]);
        let mut ids: Vec<ItemId> = alone.positions.keys().copied().collect();
        ids.sort();
        assert_eq!(ids, graph.wallet_items(&two.b));
    }

    #[test]
    fn untangled_keeps_the_x_and_the_lanes() {
        let (two, graph) = two_wallets();
        let wallets = [WalletKey::Current, two.b.clone()];
        let mut start = reset(&graph, &wallets).positions;
        for (k, tx) in (0..graph.txs().len()).enumerate() {
            let id = graph.tx_item(tx);
            start.insert(id, start[&id] + Vector::new(24.0 * k as f32, 600.0));
        }
        let lanes = untangled(&graph, &wallets, &start, &HashMap::new());
        for tx in 0..graph.txs().len() {
            let id = graph.tx_item(tx);
            assert_eq!(lanes.positions[&id].x, start[&id].x);
            assert_eq!(lanes.positions[&id].y, 0.0);
        }
        assert_eq!(lanes, untangled(&graph, &wallets, &start, &HashMap::new()));
    }

    #[test]
    fn hidden_lane_reorder_moves_no_displayed_lane() {
        let (b, c, d) = (other("b"), other("c"), other("d"));
        let lanes = vec![
            lane(&WalletKey::Current, true),
            lane(&b, false),
            lane(&c, true),
            lane(&d, false),
        ];
        let reordered = moved(&lanes, &d, Some(&b));
        assert_eq!(
            reordered,
            vec![
                lane(&WalletKey::Current, true),
                lane(&d, false),
                lane(&b, false),
                lane(&c, true),
            ]
        );
        assert_eq!(displayed(&reordered), displayed(&lanes));
        assert_eq!(displayed(&lanes), vec![WalletKey::Current, c.clone()]);
    }

    #[test]
    fn moved_goes_before_its_target() {
        let (b, c, d) = (other("b"), other("c"), other("d"));
        let lanes = vec![
            lane(&WalletKey::Current, true),
            lane(&b, false),
            lane(&c, true),
            lane(&d, true),
        ];
        assert_eq!(
            moved(&lanes, &d, Some(&WalletKey::Current)),
            vec![
                lane(&d, true),
                lane(&WalletKey::Current, true),
                lane(&b, false),
                lane(&c, true),
            ]
        );
        assert_eq!(
            moved(&lanes, &WalletKey::Current, None)[3],
            lane(&WalletKey::Current, true)
        );
        assert_eq!(moved(&lanes, &b, Some(&c)), lanes);
        assert_eq!(moved(&lanes, &other("e"), None), lanes);
    }

    #[test]
    fn rows_hold_lane_and_displayed() {
        let (b, c) = (other("b"), other("c"));
        let mut offsets = Offsets::default();
        offsets.set(c.clone(), Vector::new(0.0, 480.0));
        offsets.set(WalletKey::Current, Vector::new(0.0, 240.0));
        let lanes = vec![
            lane(&c, true),
            lane(&WalletKey::Current, true),
            lane(&b, false),
        ];
        assert_eq!(
            rows(&lanes, &offsets, &HashMap::from([(c.clone(), 420.0)])),
            vec![
                GraphWallet {
                    wallet: c.row(),
                    selected: true,
                    offset: Some((0.0, 480.0)),
                    lane: Some(0),
                    displayed: true,
                    lane_height: Some(420.0),
                },
                GraphWallet {
                    wallet: "current".to_string(),
                    selected: true,
                    offset: Some((0.0, 240.0)),
                    lane: Some(1),
                    displayed: true,
                    lane_height: None,
                },
                GraphWallet {
                    wallet: b.row(),
                    selected: true,
                    offset: None,
                    lane: Some(2),
                    displayed: false,
                    lane_height: None,
                },
            ]
        );
    }

    #[test]
    fn hidden_wallet_is_left_out_of_the_graph() {
        let two = fixture::two_wallets("a", "b");
        let load = |b_displayed: bool| {
            let wallets = fixture::two_wallets("a", "b")
                .wallets
                .into_iter()
                .map(|txs| MapWallet {
                    displayed: txs.key == WalletKey::Current || b_displayed,
                    txs,
                    layout: WalletLayout::default(),
                    store: None,
                    lane: None,
                    lane_height: None,
                })
                .collect();
            split_loaded(wallets)
        };
        let payment_output = |graph: &TxGraph| {
            let payment = graph.tx_index(&two.payment).unwrap();
            graph.txs()[payment].outputs[0].clone()
        };

        let shown = load(true);
        assert_eq!(
            shown.lanes,
            vec![lane(&WalletKey::Current, true), lane(&two.b, true)]
        );
        let graph = TxGraph::new(shown.txs);
        assert!(matches!(payment_output(&graph), OutputSlot::OurCoin { .. }));

        let hidden = load(false);
        assert_eq!(
            hidden.lanes,
            vec![lane(&WalletKey::Current, true), lane(&two.b, false)]
        );
        assert_eq!(hidden.layouts.len(), 1);
        assert!(hidden.txs.iter().all(|txs| txs.key == WalletKey::Current));
        let graph = TxGraph::new(hidden.txs);
        assert!(graph.tx_index(&two.spend).is_none());
        assert!(matches!(payment_output(&graph), OutputSlot::Payment { .. }));
    }

    #[test]
    fn stored_lanes_come_first() {
        let wallets = vec![
            (other("c"), None),
            (WalletKey::Current, None),
            (other("b"), None),
            (other("d"), Some(0)),
        ]
        .into_iter()
        .map(|(key, lane)| MapWallet {
            txs: WalletTxs {
                checksum: key.row(),
                key,
                txs: Vec::new(),
                coins: Vec::new(),
            },
            layout: WalletLayout::default(),
            store: None,
            lane,
            displayed: true,
            lane_height: lane.map(|_| 300.0),
        })
        .collect();
        let loaded = split_loaded(wallets);
        assert_eq!(loaded.heights, HashMap::from([(other("d"), 300.0)]));
        let order: Vec<WalletKey> = loaded.lanes.into_iter().map(|lane| lane.wallet).collect();
        assert_eq!(
            order,
            vec![other("d"), WalletKey::Current, other("b"), other("c")]
        );
    }
}
