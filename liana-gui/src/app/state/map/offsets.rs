use std::collections::{BTreeSet, HashMap};

use iced::{Point, Vector};
use liana_ui::widget::graph_view::ItemId;
use lianad::commands::{GraphItem, GraphLayoutEntry};

use crate::app::{
    settings::WalletId,
    state::map::{graph::TxGraph, layout, split_stored, wallets::WalletKey, Orders},
};

/// Offsets of the other wallets from the current wallet's origin.
#[derive(Debug, Clone, Default)]
pub struct Offsets(HashMap<WalletId, Vector>);

impl Offsets {
    /// `None` for another wallet not placed yet.
    pub fn get(&self, wallet: &WalletKey) -> Option<Vector> {
        match wallet {
            WalletKey::Current => Some(Vector::ZERO),
            WalletKey::Other(id) => self.0.get(id).copied(),
        }
    }

    pub fn set(&mut self, wallet: WalletId, offset: Vector) {
        self.0.insert(wallet, offset);
    }

    /// Every placed wallet, the current one first.
    pub fn iter(&self) -> impl Iterator<Item = (WalletKey, Vector)> + '_ {
        std::iter::once((WalletKey::Current, Vector::ZERO)).chain(
            self.0
                .iter()
                .map(|(id, offset)| (WalletKey::Other(id.clone()), *offset)),
        )
    }
}

/// The stored layout of a wallet, in its own coordinates.
#[derive(Debug, Default)]
pub struct WalletLayout {
    pub entries: Vec<GraphLayoutEntry>,
    /// `None` for the current wallet and for another wallet not placed yet.
    pub offset: Option<Vector>,
}

/// The map positions of every wallet's items and what loading them leaves to save.
#[derive(Debug, Default)]
pub struct Placement {
    pub layout: HashMap<ItemId, Point>,
    pub orders: Orders,
    pub offsets: Offsets,
    /// Items placed by default or whose stored order was dropped.
    pub save: BTreeSet<ItemId>,
    /// Entries whose item is not on the map anymore, per wallet.
    pub remove: HashMap<WalletKey, Vec<GraphItem>>,
    /// Wallets given an offset.
    pub placed: Vec<WalletId>,
}

/// Places each wallet in its own coordinates and shifts it by its offset. A wallet without
/// offset lands below the wallets placed before it.
pub fn place_wallets(graph: &TxGraph, wallets: Vec<(WalletKey, WalletLayout)>) -> Placement {
    let (known, new): (Vec<_>, Vec<_>) = wallets
        .into_iter()
        .partition(|(key, wallet)| *key == WalletKey::Current || wallet.offset.is_some());
    let mut placement = Placement::default();
    for (key, wallet) in known.into_iter().chain(new) {
        let stored = split_stored(graph, &key, wallet.entries);
        let mut local = stored.positions;
        let placed = layout::place(graph, &key, &local);
        placement
            .save
            .extend(placed.keys().copied().chain(stored.resave));
        local.extend(placed);
        let offset = match &key {
            WalletKey::Current => Vector::ZERO,
            WalletKey::Other(id) => {
                let offset = match wallet.offset {
                    Some(offset) => offset,
                    None => {
                        placement.placed.push(id.clone());
                        layout::new_offset(graph, &placement.layout, &local)
                    }
                };
                placement.offsets.set(id.clone(), offset);
                offset
            }
        };
        placement
            .layout
            .extend(local.into_iter().map(|(id, p)| (id, p + offset)));
        placement.orders.extend(stored.orders);
        if !stored.remove.is_empty() {
            placement.remove.insert(key, stored.remove);
        }
    }
    placement
}

/// Layout entries of `items` in their owning wallet's coordinates, per wallet.
pub fn local_entries(
    graph: &TxGraph,
    layout: &HashMap<ItemId, Point>,
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
        entries
            .entry(wallet.clone())
            .or_default()
            .push(GraphLayoutEntry {
                item,
                position,
                input_order,
                output_order,
                lane_position: None,
            });
    }
    entries
}

/// Default positions of every wallet's items, each wallet keeping its offset.
pub fn reset_layout(graph: &TxGraph, offsets: &Offsets) -> HashMap<ItemId, Point> {
    offsets
        .iter()
        .flat_map(|(wallet, offset)| {
            layout::reset(graph, &wallet)
                .into_iter()
                .map(move |(id, p)| (id, p + offset))
        })
        .collect()
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
            graph::TxGraph,
            layout::{item_size, new_offset, reset},
            offsets::{local_entries, place_wallets, reset_layout, Offsets, WalletLayout},
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
        let wallets = b_layout(
            vec![entry(GraphItem::Tx(two.spend), 10.0, 20.0)],
            Some(Vector::new(100.0, 1000.0)),
        );
        let placement = place_wallets(&graph, wallets);
        assert_eq!(placement.layout[&spend], Point::new(110.0, 1020.0));
        assert!(!placement.save.contains(&spend));
        assert!(placement.placed.is_empty());
        assert_eq!(
            placement.offsets.get(&two.b),
            Some(Vector::new(100.0, 1000.0))
        );
        assert_eq!(placement.layout.len(), graph.item_ids().count());
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
        assert_eq!(
            placement.layout[&payment],
            reset(&graph, &WalletKey::Current)[&payment]
        );
        assert!(placement.remove.is_empty());
    }

    #[test]
    fn new_wallet_lands_below_everything() {
        let (two, graph) = two_wallets();
        let placement = place_wallets(&graph, b_layout(Vec::new(), None));
        assert_eq!(placement.placed, vec![b_id()]);

        let current = reset(&graph, &WalletKey::Current);
        let bottom = current
            .iter()
            .map(|(id, p)| p.y + item_size(&graph, *id).height)
            .fold(f32::MIN, f32::max);
        let local = reset(&graph, &two.b);
        let offset = new_offset(&graph, &current, &local);
        assert_eq!(placement.offsets.get(&two.b), Some(offset));
        let b_items = graph.wallet_items(&two.b);
        let left = b_items
            .iter()
            .map(|id| placement.layout[id].x)
            .fold(f32::MAX, f32::min);
        let top = b_items
            .iter()
            .map(|id| placement.layout[id].y)
            .fold(f32::MAX, f32::min);
        assert_eq!((left, top), (0.0, bottom + 120.0));
        for (id, p) in current {
            assert_eq!(placement.layout[&id], p);
        }
        assert_eq!(placement.save.len(), graph.item_ids().count());
    }

    #[test]
    fn local_entries_go_to_the_owner() {
        let (two, graph) = two_wallets();
        let spend = graph.tx_item(graph.tx_index(&two.spend).unwrap());
        let funding = graph.tx_item(graph.tx_index(&two.funding).unwrap());
        let mut offsets = Offsets::default();
        offsets.set(b_id(), Vector::new(24.0, 600.0));
        let layout = HashMap::from([
            (spend, Point::new(48.0, 660.0)),
            (funding, Point::new(12.0, 36.0)),
        ]);
        let entries = local_entries(&graph, &layout, &Orders::new(), &offsets, [spend, funding]);
        assert_eq!(
            entries[&two.b],
            vec![entry(GraphItem::Tx(two.spend), 24.0, 60.0)]
        );
        assert_eq!(
            entries[&WalletKey::Current],
            vec![entry(GraphItem::Tx(two.funding), 12.0, 36.0)]
        );

        let unplaced = local_entries(
            &graph,
            &layout,
            &Orders::new(),
            &Offsets::default(),
            [spend],
        );
        assert!(unplaced.is_empty());
    }

    #[test]
    fn reset_keeps_the_offsets() {
        let (two, graph) = two_wallets();
        let mut offsets = Offsets::default();
        offsets.set(b_id(), Vector::new(24.0, 600.0));
        let layout = reset_layout(&graph, &offsets);
        let mut expected: HashMap<ItemId, Point> = reset(&graph, &WalletKey::Current);
        expected.extend(
            reset(&graph, &two.b)
                .into_iter()
                .map(|(id, p)| (id, p + Vector::new(24.0, 600.0))),
        );
        assert_eq!(layout, expected);
    }
}
