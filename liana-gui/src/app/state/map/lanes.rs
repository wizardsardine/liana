use std::collections::HashMap;

use iced::{Point, Vector};
use liana_ui::widget::graph_view::ItemId;
use lianad::commands::GraphWallet;

use crate::app::state::map::{
    graph::{TxGraph, WalletTxs},
    layout,
    offsets::{Offsets, WalletLayout},
    wallets::{WalletKey, WalletStore},
    MapWallet,
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

/// Default positions in their lanes of the items of `wallets`.
pub fn reset(graph: &TxGraph, wallets: &[WalletKey]) -> HashMap<ItemId, Point> {
    wallets
        .iter()
        .flat_map(|wallet| layout::reset_lane(graph, wallet))
        .collect()
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
                Band, WalletLane,
            },
            layout::{self, item_size, lane_height, BLOCK_CLEARANCE, LANE_GAP},
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
        let wallets = [WalletKey::Current, two.b.clone()];
        let heights = wallets
            .iter()
            .map(|wallet| (wallet.clone(), lane_height(graph, wallet)))
            .collect();
        (reset(graph, &wallets), heights)
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
    fn reset_places_the_lanes_by_default() {
        let (two, graph) = two_wallets();
        let mut expected = layout::reset_lane(&graph, &WalletKey::Current);
        expected.extend(layout::reset_lane(&graph, &two.b));
        assert_eq!(
            reset(&graph, &[WalletKey::Current, two.b.clone()]),
            expected
        );
        assert_eq!(
            reset(&graph, &[two.b.clone()]),
            layout::reset_lane(&graph, &two.b)
        );
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
