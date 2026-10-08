pub mod coin_ui;
pub mod display;
pub mod edit;
#[cfg(test)]
pub mod fixture;
pub mod graph;
pub mod history;
pub mod layout;
pub mod selection;

use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
};

use iced::{advanced::widget::Id, keyboard::Modifiers, Point, Subscription, Task};
use liana::miniscript::bitcoin::Txid;
use liana_ui::{
    component::panels::map::header::HeaderAction,
    widget::{
        graph_view::{self, geometry::ZOOM_STEP, GraphEvent, ItemId, Side, Target},
        Element,
    },
};
use lianad::commands::{GraphItem as LayoutItem, GraphLayoutEntry};

use crate::{
    app::{
        cache::Cache,
        error::Error,
        menu::{MapFocus, Menu},
        message::Message,
        state::{
            map::{
                coin_ui::CoinUi,
                display::{click_action, display_state, ClickAction},
                graph::{MapItem, TxGraph},
                history::{Change, History},
                selection::{Selection, TagHighlight},
            },
            State,
        },
        view::{self, MapMessage},
        wallet::Wallet,
    },
    daemon::{
        model::{Coin, HistoryTransaction},
        Daemon,
    },
};

/// Display orders (inputs, outputs) per transaction, `None` = true order.
pub type Orders = HashMap<Txid, (history::Order, history::Order)>;

#[derive(Debug)]
pub struct MapData {
    pub txs: Vec<HistoryTransaction>,
    pub coins: Vec<Coin>,
    pub layout: Vec<GraphLayoutEntry>,
}

/// The stored layout, checked against the current graph.
#[derive(Debug, Default)]
pub struct StoredLayout {
    pub positions: HashMap<ItemId, Point>,
    pub orders: Orders,
    /// Entries whose item is not on the map anymore.
    pub remove: Vec<LayoutItem>,
    /// Transactions whose stored order was dropped.
    pub resave: Vec<ItemId>,
}

/// View toggles of the header, never recorded.
#[derive(Debug, Clone, Copy, Default)]
pub struct Toggles {
    pub area: bool,
    pub unspent: bool,
    pub snap: bool,
}

/// Slot drag in progress, view only and never recorded.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LiveReorder {
    pub item: ItemId,
    pub side: Side,
    pub from: usize,
    pub to: usize,
    pub offset_y: f32,
}

pub struct MapPanel {
    graph_id: Id,
    graph: Option<TxGraph>,
    layout: HashMap<ItemId, Point>,
    orders: Orders,
    coin_ui: CoinUi,
    selection: Selection,
    hover: Option<Target>,
    tag_highlight: Option<TagHighlight>,
    toggles: Toggles,
    history: History,
    reorder: Option<LiveReorder>,
    zoom: f32,
    loading: bool,
    pending_focus: Option<MapFocus>,
    warning: Option<Error>,
}

/// Display row of the slot at true index `index` (`order[row]` is the true index).
pub fn display_row(order: Option<&[u32]>, index: usize) -> usize {
    order
        .and_then(|order| {
            order
                .iter()
                .position(|&true_index| true_index as usize == index)
        })
        .unwrap_or(index)
}

/// A stored order is usable when it lists every slot exactly once.
fn is_valid_order(order: &[u32], len: usize) -> bool {
    let mut seen = vec![false; len];
    order.len() == len
        && order.iter().all(|&index| {
            let index = index as usize;
            index < len && !std::mem::replace(&mut seen[index], true)
        })
}

pub fn split_stored(graph: &TxGraph, entries: Vec<GraphLayoutEntry>) -> StoredLayout {
    let mut stored = StoredLayout::default();
    for entry in entries {
        let Some(id) = graph.item_id(&entry.item) else {
            stored.remove.push(entry.item);
            continue;
        };
        if let Some((x, y)) = entry.position {
            stored.positions.insert(id, Point::new(x as f32, y as f32));
        }
        let LayoutItem::Tx(txid) = &entry.item else {
            continue;
        };
        let Some(tx) = graph.tx_index(txid) else {
            continue;
        };
        let tx = &graph.txs()[tx];
        let keep = |order: Option<Vec<u32>>, len: usize| order.filter(|o| is_valid_order(o, len));
        let input_order = keep(entry.input_order.clone(), tx.inputs.len());
        let output_order = keep(entry.output_order.clone(), tx.outputs.len());
        if input_order != entry.input_order || output_order != entry.output_order {
            stored.resave.push(id);
        }
        if input_order.is_some() || output_order.is_some() {
            stored
                .orders
                .insert(tx.history.txid, (input_order, output_order));
        }
    }
    stored
}

impl MapPanel {
    pub fn new() -> Self {
        Self {
            graph_id: Id::unique(),
            graph: None,
            layout: HashMap::new(),
            orders: HashMap::new(),
            coin_ui: CoinUi::default(),
            selection: Selection::default(),
            hover: None,
            tag_highlight: None,
            toggles: Toggles::default(),
            history: History::default(),
            reorder: None,
            zoom: 1.0,
            loading: false,
            pending_focus: None,
            warning: None,
        }
    }

    pub fn set_focus(&mut self, focus: Option<MapFocus>) {
        self.pending_focus = focus;
    }

    fn on_click(&mut self, target: &Target, modifiers: Modifiers) {
        let Some(graph) = &self.graph else {
            return;
        };
        match click_action(graph, &self.orders, target, modifiers) {
            ClickAction::None => return,
            ClickAction::Select(id) => self.selection.click(id),
            ClickAction::Toggle(id) => self.selection.command_click(graph, id),
            ClickAction::Range(id) => self.selection.shift_click(graph, id),
            ClickAction::Chain(id) => self.selection.command_shift_click(graph, id),
            ClickAction::TagHighlight(slot) => {
                self.tag_highlight = graph.slot_coin(slot).and_then(|coin| {
                    TagHighlight::new(slot, self.coin_ui.coin_tags(&coin).to_vec())
                });
                return;
            }
        }
        self.tag_highlight = None;
    }

    /// Records a move of items, applies it and persists the touched items.
    fn commit_move(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        moves: Vec<(ItemId, Point, Point)>,
    ) -> Task<Message> {
        let Some(graph) = &self.graph else {
            return Task::none();
        };
        let change = Change::Move(
            moves
                .iter()
                .filter_map(|(id, before, after)| Some((graph.graph_item(*id)?, *before, *after)))
                .collect(),
        );
        let touched = edit::apply_layout_change(graph, &mut self.layout, &mut self.orders, &change);
        if touched.is_empty() {
            return Task::none();
        }
        self.history.record(change);
        self.save_layout(daemon, touched, vec![])
    }

    /// Applies the change returned by `History::undo` or `History::redo`.
    fn apply_history(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        change: Option<Change>,
    ) -> Task<Message> {
        let (Some(change), Some(graph)) = (change, &self.graph) else {
            return Task::none();
        };
        if self.coin_ui.apply(&change) {
            return Task::none();
        }
        let touched = edit::apply_layout_change(graph, &mut self.layout, &mut self.orders, &change);
        self.save_layout(daemon, touched, vec![])
    }

    fn save_layout(
        &self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        items: impl IntoIterator<Item = ItemId>,
        remove: Vec<LayoutItem>,
    ) -> Task<Message> {
        let Some(graph) = &self.graph else {
            return Task::none();
        };
        let set: Vec<GraphLayoutEntry> = items
            .into_iter()
            .filter_map(|id| {
                let item = graph.graph_item(id)?;
                let (input_order, output_order) = match &item {
                    LayoutItem::Tx(txid) => self.orders.get(txid).cloned().unwrap_or_default(),
                    _ => (None, None),
                };
                Some(GraphLayoutEntry {
                    item,
                    position: self
                        .layout
                        .get(&id)
                        .map(|p| (f64::from(p.x), f64::from(p.y))),
                    input_order,
                    output_order,
                })
            })
            .collect();
        if set.is_empty() && remove.is_empty() {
            return Task::none();
        }
        Task::perform(
            async move {
                daemon
                    .update_graph_layout(&set, &remove)
                    .await
                    .map_err(Into::into)
            },
            Message::MapLayoutSaved,
        )
    }
}

impl Default for MapPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl State for MapPanel {
    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        let display = self.graph.as_ref().map(|graph| {
            display_state(
                graph,
                &self.layout,
                &self.orders,
                &self.selection,
                self.hover.as_ref(),
                self.tag_highlight.as_ref(),
                &self.coin_ui,
                self.toggles.unspent,
            )
        });
        let align_count = self
            .graph
            .as_ref()
            .and_then(|graph| layout::align_targets(graph, &self.selection.selected_txs(graph)))
            .map_or(0, |targets| targets.len());
        view::full_dashboard(
            &Menu::Map(None),
            cache,
            self.warning.as_ref(),
            view::map::map_view(
                self.graph.as_ref(),
                &self.layout,
                &self.orders,
                &self.coin_ui,
                display.as_ref(),
                self.selection.items(),
                self.tag_highlight.as_ref(),
                self.toggles,
                self.history.can_undo(),
                self.history.can_redo(),
                align_count,
                self.reorder,
                self.zoom,
                self.loading,
                &self.graph_id,
            ),
        )
    }

    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        _cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        match message {
            Message::MapLoaded(Err(e)) => {
                self.loading = false;
                self.warning = Some(e);
            }
            Message::MapLoaded(Ok(data)) => {
                let graph = TxGraph::new(data.txs, &data.coins);
                let stored = split_stored(&graph, data.layout);
                let placed = layout::place(&graph, &stored.positions);
                let to_save: BTreeSet<ItemId> = placed
                    .keys()
                    .copied()
                    .chain(stored.resave.iter().copied())
                    .collect();
                self.layout = stored.positions;
                self.layout.extend(placed);
                self.orders = stored.orders;
                let fit = if graph.is_empty() {
                    Task::none()
                } else {
                    graph_view::fit(self.graph_id.clone())
                };
                self.hover = None;
                self.tag_highlight = None;
                self.reorder = None;
                self.selection.retain(|id| graph.item(id).is_some());
                self.graph = Some(graph);
                self.loading = false;
                let save = self.save_layout(daemon, to_save, stored.remove);
                return Task::batch([save, fit]);
            }
            Message::MapLayoutSaved(Err(e)) => self.warning = Some(e),
            Message::View(view::Message::Map(message)) => match message {
                MapMessage::Header(HeaderAction::ZoomIn) => {
                    return graph_view::zoom_by(self.graph_id.clone(), ZOOM_STEP);
                }
                MapMessage::Header(HeaderAction::ZoomOut) => {
                    return graph_view::zoom_by(self.graph_id.clone(), 1.0 / ZOOM_STEP);
                }
                MapMessage::Header(HeaderAction::Fit) => {
                    return graph_view::fit(self.graph_id.clone());
                }
                MapMessage::Header(HeaderAction::ToggleArea) => {
                    self.toggles.area = !self.toggles.area;
                }
                MapMessage::Header(HeaderAction::ToggleUnspent) => {
                    self.toggles.unspent = !self.toggles.unspent;
                }
                MapMessage::Header(HeaderAction::ToggleSnap) => {
                    self.toggles.snap = !self.toggles.snap;
                }
                MapMessage::Header(HeaderAction::Undo) if self.reorder.is_none() => {
                    let change = self.history.undo();
                    return self.apply_history(daemon, change);
                }
                MapMessage::Header(HeaderAction::Redo) if self.reorder.is_none() => {
                    let change = self.history.redo();
                    return self.apply_history(daemon, change);
                }
                MapMessage::Header(
                    action @ (HeaderAction::AlignHorizontal | HeaderAction::AlignVertical),
                ) => {
                    let Some(graph) = &self.graph else {
                        return Task::none();
                    };
                    let Some(targets) =
                        layout::align_targets(graph, &self.selection.selected_txs(graph))
                    else {
                        return Task::none();
                    };
                    let align = if action == HeaderAction::AlignHorizontal {
                        layout::align_horizontal
                    } else {
                        layout::align_vertical
                    };
                    let moves = align(graph, &self.layout, &targets, self.toggles.snap)
                        .into_iter()
                        .filter_map(|(id, after)| Some((id, *self.layout.get(&id)?, after)))
                        .collect();
                    return self.commit_move(daemon, moves);
                }
                MapMessage::Header(HeaderAction::ResetLayout) => {
                    let Some(graph) = &self.graph else {
                        return Task::none();
                    };
                    let change = Change::Layout {
                        before: edit::layout_state(graph, &self.layout, &self.orders),
                        after: edit::layout_state(graph, &layout::reset(graph), &Orders::new()),
                    };
                    let touched = edit::apply_layout_change(
                        graph,
                        &mut self.layout,
                        &mut self.orders,
                        &change,
                    );
                    self.history.record(change);
                    let save = self.save_layout(daemon, touched, vec![]);
                    return Task::batch([save, graph_view::fit(self.graph_id.clone())]);
                }
                MapMessage::Graph(event) => match event {
                    GraphEvent::Moved { items, delta } => {
                        let moves =
                            edit::moved_positions(&self.layout, &items, delta, self.toggles.snap);
                        return self.commit_move(daemon, moves);
                    }
                    GraphEvent::SlotDrag {
                        item,
                        side,
                        from,
                        to_display_index,
                        offset_y,
                    } => {
                        self.reorder = Some(LiveReorder {
                            item,
                            side,
                            from,
                            to: to_display_index,
                            offset_y,
                        });
                    }
                    GraphEvent::SlotDropped {
                        item,
                        side,
                        from,
                        to,
                    } => {
                        self.reorder = None;
                        let Some(graph) = &self.graph else {
                            return Task::none();
                        };
                        let Some(MapItem::Tx(tx)) = graph.item(item) else {
                            return Task::none();
                        };
                        if from == to {
                            return Task::none();
                        }
                        let tx = &graph.txs()[tx];
                        let txid = tx.history.txid;
                        let stored = self.orders.get(&txid).cloned().unwrap_or_default();
                        let (len, before) = match side {
                            Side::Input => (tx.inputs.len(), stored.0),
                            Side::Output => (tx.outputs.len(), stored.1),
                        };
                        let after = edit::reorder_column(before.as_deref(), len, from, to);
                        let change = Change::Reorder {
                            tx: txid,
                            side,
                            before,
                            after,
                        };
                        let touched = edit::apply_layout_change(
                            graph,
                            &mut self.layout,
                            &mut self.orders,
                            &change,
                        );
                        self.history.record(change);
                        return self.save_layout(daemon, touched, vec![]);
                    }
                    GraphEvent::Zoom(zoom) => self.zoom = zoom,
                    GraphEvent::Click { target, modifiers } => self.on_click(&target, modifiers),
                    GraphEvent::EmptyClick => {
                        self.selection.clear();
                        self.tag_highlight = None;
                    }
                    GraphEvent::Hover(target) => self.hover = target,
                    GraphEvent::AreaSelected {
                        items, additive, ..
                    } => {
                        self.selection.area(items, additive);
                        self.tag_highlight = None;
                    }
                    GraphEvent::SlotWheel { steps, .. } => {
                        if let Some(tag) = &mut self.tag_highlight {
                            tag.cycle(steps);
                        }
                    }
                    _ => {}
                },
                _ => {}
            },
            _ => {}
        }
        Task::none()
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::none()
    }

    fn reload(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        _wallet: Arc<Wallet>,
    ) -> Task<Message> {
        self.loading = true;
        self.warning = None;
        Task::perform(
            async move {
                let coins = daemon.list_all_coins().await?;
                let txs = daemon.get_all_history_txs(&coins).await?;
                let layout = daemon.get_graph_layout().await?;
                Ok(MapData { txs, coins, layout })
            },
            Message::MapLoaded,
        )
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use liana::miniscript::bitcoin::{OutPoint, Txid};
    use lianad::commands::{GraphItem as LayoutItem, GraphLayoutEntry};

    use crate::app::state::map::{display_row, fixture, graph::TxGraph, split_stored};

    fn entry(item: LayoutItem) -> GraphLayoutEntry {
        GraphLayoutEntry {
            item,
            position: Some((10.0, 20.0)),
            input_order: None,
            output_order: None,
        }
    }

    fn unknown_outpoint() -> OutPoint {
        OutPoint::new(
            Txid::from_str("00000000000000000000000000000000000000000000000000000000000000ff")
                .unwrap(),
            0,
        )
    }

    #[test]
    fn stale_entries_are_removed() {
        let graph = fixture::graph();
        let unknown_tx = LayoutItem::Tx(unknown_outpoint().txid);
        let orphan = LayoutItem::OutputLeaf(unknown_outpoint());
        let stored = split_stored(&graph, vec![entry(unknown_tx), entry(orphan)]);
        assert_eq!(stored.remove, vec![unknown_tx, orphan]);
        assert!(stored.positions.is_empty());
    }

    #[test]
    fn wrong_order_length_is_ignored() {
        let f = fixture::sample_wallet();
        let graph = TxGraph::new(f.txs, &f.coins);
        let txid = f.ids.batch;
        let mut bad = entry(LayoutItem::Tx(txid));
        bad.output_order = Some(vec![0]);
        let stored = split_stored(&graph, vec![bad]);
        let id = graph.tx_item(graph.tx_index(&txid).unwrap());
        assert_eq!(stored.resave, vec![id]);
        assert!(stored.orders.is_empty());
        assert!(stored.positions.contains_key(&id));
    }

    #[test]
    fn known_entries_are_kept() {
        let f = fixture::sample_wallet();
        let graph = TxGraph::new(f.txs, &f.coins);
        let txid = f.ids.batch;
        let slots = graph.txs()[graph.tx_index(&txid).unwrap()].outputs.len() as u32;
        let order: Vec<u32> = (0..slots).rev().collect();
        let mut kept = entry(LayoutItem::Tx(txid));
        kept.output_order = Some(order.clone());
        let stored = split_stored(&graph, vec![kept]);
        assert!(stored.remove.is_empty());
        assert!(stored.resave.is_empty());
        assert_eq!(stored.orders[&txid], (None, Some(order)));
        assert_eq!(stored.positions.len(), 1);
    }

    #[test]
    fn display_row_inverts_order() {
        let order = [2, 0, 1];
        assert_eq!(display_row(Some(&order), 2), 0);
        assert_eq!(display_row(Some(&order), 0), 1);
        assert_eq!(display_row(Some(&order), 1), 2);
        assert_eq!(display_row(None, 2), 2);
    }
}
