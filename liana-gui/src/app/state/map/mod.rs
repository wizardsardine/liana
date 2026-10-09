pub mod coin_ui;
pub mod display;
pub mod edit;
#[cfg(test)]
pub mod fixture;
pub mod focus;
pub mod graph;
pub mod history;
pub mod layout;
pub mod selection;
pub mod wallets;

use std::{
    collections::{BTreeSet, HashMap},
    mem,
    sync::Arc,
};

use iced::{
    advanced::widget::Id,
    event,
    keyboard::{self, key::Named, Modifiers},
    window, Event, Point, Rectangle, Size, Subscription, Task,
};
use liana::miniscript::bitcoin::{Address, OutPoint, Txid};
use liana_ui::{
    component::panels::map::header::HeaderAction,
    widget::{
        graph_view::{
            self,
            geometry::{LEAF_HEIGHT, LEAF_WIDTH, ZOOM_STEP},
            GraphEvent, ItemId, Side, Target,
        },
        modal::Modal,
        text_input, Element,
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
            label::{label_item_from_str, LabelsEdited},
            map::{
                coin_ui::CoinUi,
                display::{click_action, display_state, label_key, slot_ref, ClickAction},
                focus::{resolve_focus, FocusLanding, ShowOnMap},
                graph::{MapItem, SlotRef, TxGraph},
                history::{Change, History},
                selection::{Selection, TagHighlight},
            },
            State,
        },
        view::{self, LabelMessage, MapKey, MapMessage},
        wallet::Wallet,
    },
    daemon::{
        model::{Coin, HistoryTransaction, LabelsLoader},
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

/// Transaction and leaf indices of the current graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelTarget {
    Tx(usize),
    Slot(SlotRef),
    Leaf(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MapModal {
    Label(LabelTarget),
    Reuse(Address),
}

/// What Esc does, first match wins (spec 14).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EscapeAction {
    CloseHelp,
    ClosePopover,
    CloseModal,
    ClearSelection,
}

pub fn escape_action(help: bool, popover: bool, modal: bool) -> EscapeAction {
    if help {
        EscapeAction::CloseHelp
    } else if popover {
        EscapeAction::ClosePopover
    } else if modal {
        EscapeAction::CloseModal
    } else {
        EscapeAction::ClearSelection
    }
}

/// Keys are only taken when no widget handled them, except Esc captured by a text input.
pub fn key_action(
    key: &keyboard::Key,
    modifiers: Modifiers,
    status: event::Status,
) -> Option<MapKey> {
    let ignored = status == event::Status::Ignored;
    let command = modifiers.command();
    match key {
        keyboard::Key::Named(Named::Escape) => match status {
            event::Status::Ignored if !command => Some(MapKey::Escape),
            event::Status::Captured => Some(MapKey::EscapeInInput),
            _ => None,
        },
        keyboard::Key::Character(c) if ignored => match c.as_str() {
            "z" | "Z" if command => Some(if modifiers.shift() {
                MapKey::Redo
            } else {
                MapKey::Undo
            }),
            "y" | "Y" if command => Some(MapKey::Redo),
            "?" if !command => Some(MapKey::Shortcuts),
            "u" | "U" if !command => Some(MapKey::Unspent),
            _ => None,
        },
        _ => None,
    }
}

fn map_event(event: Event, status: event::Status, _: window::Id) -> Option<Message> {
    let key = match event {
        Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => {
            key_action(&key, modifiers, status)?
        }
        Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) => {
            MapKey::Command(modifiers.command())
        }
        _ => return None,
    };
    Some(Message::View(view::Message::Map(MapMessage::Key(key))))
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
    show_on_map: Option<ShowOnMap>,
    toggles: Toggles,
    history: History,
    reorder: Option<LiveReorder>,
    zoom: f32,
    loading: bool,
    pending_focus: Option<MapFocus>,
    warning: Option<Error>,
    modal: Option<MapModal>,
    /// The help sits above any other modal.
    shortcuts_open: bool,
    command_held: bool,
    /// Filter text of the open tag popover.
    tag_popover: Option<String>,
    tag_input_id: text_input::Id,
    labels_edited: LabelsEdited,
    /// Item key and its own label before the save in flight.
    pending_label: Option<(String, Option<String>)>,
    /// Labels saved since the map was entered, forwarded to the origin panel on Back.
    label_changes: HashMap<String, Option<String>>,
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
            show_on_map: None,
            toggles: Toggles::default(),
            history: History::default(),
            reorder: None,
            zoom: 1.0,
            loading: false,
            pending_focus: None,
            warning: None,
            modal: None,
            shortcuts_open: false,
            command_held: false,
            tag_popover: None,
            tag_input_id: text_input::Id::unique(),
            labels_edited: LabelsEdited::default(),
            pending_label: None,
            label_changes: HashMap::new(),
        }
    }

    pub fn take_label_changes(&mut self) -> HashMap<String, Option<String>> {
        mem::take(&mut self.label_changes)
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
                self.show_on_map = None;
                return;
            }
        }
        self.tag_highlight = None;
        self.show_on_map = None;
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
        if let Change::Label { item, after, .. } = change {
            return Task::perform(
                async move {
                    daemon
                        .update_labels(&HashMap::from([(item.clone(), after.clone())]))
                        .await?;
                    Ok(HashMap::from([(item.to_string(), after)]))
                },
                Message::LabelsUpdated,
            );
        }
        if self.coin_ui.apply(&change) {
            return Task::none();
        }
        let touched = edit::apply_layout_change(graph, &mut self.layout, &mut self.orders, &change);
        self.save_layout(daemon, touched, vec![])
    }

    /// Applies the selection or highlights of a resolved focus and returns its target rect.
    fn land(&mut self, landing: FocusLanding) -> Rectangle {
        self.selection.clear();
        if let Some(block) = landing.select {
            self.selection.click(block);
        }
        self.show_on_map = landing.highlight;
        landing.rect
    }

    fn clear_selection(&mut self) {
        self.selection.clear();
        self.tag_highlight = None;
        self.show_on_map = None;
    }

    /// Cancels the unsaved edit of the open label modal.
    fn cancel_label_edit(&mut self, daemon: Arc<dyn Daemon + Sync + Send>) -> Task<Message> {
        let key = match (&self.graph, &self.modal) {
            (Some(graph), Some(MapModal::Label(target))) => label_key(graph, target),
            _ => None,
        };
        match key {
            Some(key) => {
                let cancel = view::Message::Label(vec![key], LabelMessage::Cancel);
                self.forward_label(daemon, Message::View(cancel))
            }
            None => Task::none(),
        }
    }

    /// Closes the help, else the popover and the modal.
    fn close_modal(&mut self, daemon: Arc<dyn Daemon + Sync + Send>) -> Task<Message> {
        if mem::take(&mut self.shortcuts_open) {
            return Task::none();
        }
        self.tag_popover = None;
        let cancel = self.cancel_label_edit(daemon);
        self.modal = None;
        cancel
    }

    /// The unspent coin of ours held by the slot of the open label modal.
    fn modal_coin(&self) -> Option<OutPoint> {
        let (Some(graph), Some(MapModal::Label(LabelTarget::Slot(slot)))) =
            (&self.graph, &self.modal)
        else {
            return None;
        };
        graph.slot_coin(*slot).filter(|coin| graph.is_unspent(coin))
    }

    /// Applies a coin action to the modal coin and records it.
    fn coin_action(&mut self, action: impl FnOnce(&mut CoinUi, OutPoint) -> Option<Change>) {
        let Some(coin) = self.modal_coin() else {
            return;
        };
        if let Some(change) = action(&mut self.coin_ui, coin) {
            self.history.record(change);
        }
    }

    /// Own label of the item `key` in the transaction owning the open label modal.
    fn own_label(&self, key: &str) -> Option<String> {
        let (Some(graph), Some(MapModal::Label(target))) = (&self.graph, &self.modal) else {
            return None;
        };
        let tx = match *target {
            LabelTarget::Tx(tx) => tx,
            LabelTarget::Slot(slot) => slot.tx,
            LabelTarget::Leaf(leaf) => graph.leaves().get(leaf)?.tx,
        };
        graph.txs().get(tx)?.history.labels.get(key).cloned()
    }

    fn forward_label(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        message: Message,
    ) -> Task<Message> {
        let targets = self
            .graph
            .iter_mut()
            .map(|graph| graph as &mut dyn LabelsLoader);
        match self.labels_edited.update(daemon, message, targets) {
            Ok(task) => task,
            Err(e) => {
                self.warning = Some(e);
                self.pending_label = None;
                Task::none()
            }
        }
    }

    /// Records the user save matching `saved` and keeps the labels for the origin panel.
    fn labels_saved(&mut self, saved: HashMap<String, Option<String>>) {
        let pending = self
            .pending_label
            .take_if(|(key, _)| saved.contains_key(key));
        if let Some((key, before)) = pending {
            let after = saved.get(&key).cloned().flatten();
            self.history.record(Change::Label {
                item: label_item_from_str(&key),
                before,
                after,
            });
        }
        self.label_changes.extend(saved);
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
                    lane_position: None,
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
                match &self.modal {
                    Some(MapModal::Reuse(address)) => Some(address),
                    _ => None,
                },
                self.show_on_map.as_ref(),
            )
        });
        let align_count = self
            .graph
            .as_ref()
            .and_then(|graph| layout::align_targets(graph, &self.selection.selected_txs(graph)))
            .map_or(0, |targets| targets.len());
        let dashboard = view::full_dashboard(
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
                self.command_held,
                self.history.can_undo(),
                self.history.can_redo(),
                align_count,
                self.reorder,
                self.zoom,
                self.loading,
                &self.graph_id,
            ),
        );
        let modal = match (&self.graph, &self.modal) {
            (Some(graph), Some(MapModal::Label(target))) => view::map::label_modal(
                graph,
                *target,
                self.labels_edited.cache(),
                &self.coin_ui,
                self.tag_popover.as_deref(),
                &self.tag_input_id,
            ),
            (Some(graph), Some(MapModal::Reuse(address))) => view::map::reuse_modal(graph, address),
            _ => None,
        };
        let close = Some(view::Message::Map(MapMessage::CloseModal));
        // Always the same tree, so the graph view keeps its camera when a modal opens or closes.
        let base = Modal::with_optional(dashboard, modal).on_blur(close.clone());
        Modal::with_optional(base, self.shortcuts_open.then(view::map::shortcuts_modal))
            .on_blur(close)
            .into()
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
                self.hover = None;
                self.tag_highlight = None;
                self.show_on_map = None;
                self.reorder = None;
                self.selection.retain(|id| graph.item(id).is_some());
                let landing = self
                    .pending_focus
                    .take()
                    .and_then(|focus| resolve_focus(&graph, &self.layout, &self.orders, &focus));
                let fit = if graph.is_empty() {
                    Task::none()
                } else {
                    graph_view::fit(self.graph_id.clone())
                };
                self.graph = Some(graph);
                self.loading = false;
                let save = self.save_layout(daemon, to_save, stored.remove);
                return match landing {
                    Some(landing) => {
                        let target = self.land(landing);
                        Task::batch([
                            save,
                            fit.chain(graph_view::focus(self.graph_id.clone(), target)),
                        ])
                    }
                    None => Task::batch([save, fit]),
                };
            }
            Message::MapLayoutSaved(Err(e)) => self.warning = Some(e),
            Message::View(view::Message::Label(ref items, LabelMessage::Confirm)) => {
                self.pending_label = items.first().map(|key| (key.clone(), self.own_label(key)));
                return self.forward_label(daemon, message);
            }
            Message::View(view::Message::Label(..)) => return self.forward_label(daemon, message),
            Message::LabelsUpdated(res) => {
                let saved = res.as_ref().ok().cloned();
                let task = self.forward_label(daemon, Message::LabelsUpdated(res));
                if let Some(saved) = saved {
                    self.labels_saved(saved);
                }
                return task;
            }
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
                MapMessage::Header(HeaderAction::ToggleUnspent)
                | MapMessage::Key(MapKey::Unspent) => {
                    self.toggles.unspent = !self.toggles.unspent;
                }
                MapMessage::Header(HeaderAction::ToggleSnap) => {
                    self.toggles.snap = !self.toggles.snap;
                }
                MapMessage::Header(HeaderAction::Undo) | MapMessage::Key(MapKey::Undo)
                    if self.reorder.is_none() =>
                {
                    let change = self.history.undo();
                    return self.apply_history(daemon, change);
                }
                MapMessage::Header(HeaderAction::Redo) | MapMessage::Key(MapKey::Redo)
                    if self.reorder.is_none() =>
                {
                    let change = self.history.redo();
                    return self.apply_history(daemon, change);
                }
                MapMessage::Header(HeaderAction::Shortcuts)
                | MapMessage::Key(MapKey::Shortcuts) => {
                    self.shortcuts_open = !self.shortcuts_open;
                }
                MapMessage::Key(MapKey::Command(held)) => self.command_held = held,
                MapMessage::Key(MapKey::Escape) => {
                    match escape_action(
                        self.shortcuts_open,
                        self.tag_popover.is_some(),
                        self.modal.is_some(),
                    ) {
                        EscapeAction::CloseHelp | EscapeAction::CloseModal => {
                            return self.close_modal(daemon);
                        }
                        EscapeAction::ClosePopover => self.tag_popover = None,
                        EscapeAction::ClearSelection => self.clear_selection(),
                    }
                }
                MapMessage::Key(MapKey::EscapeInInput) => {
                    if self.tag_popover.take().is_none() {
                        return self.cancel_label_edit(daemon);
                    }
                }
                MapMessage::ReuseRowSelected(leaf) => {
                    self.modal = None;
                    self.selection.click(leaf);
                    self.tag_highlight = None;
                    self.show_on_map = None;
                    let Some(at) = self.layout.get(&leaf) else {
                        return Task::none();
                    };
                    let target = Rectangle::new(*at, Size::new(LEAF_WIDTH, LEAF_HEIGHT));
                    return graph_view::focus(self.graph_id.clone(), target);
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
                    GraphEvent::DoubleClick { target } => {
                        let Some(graph) = &self.graph else {
                            return Task::none();
                        };
                        let label = match target {
                            Target::Item(id) => match graph.item(id) {
                                Some(MapItem::Tx(tx)) => Some(LabelTarget::Tx(tx)),
                                Some(MapItem::Leaf(index)) => {
                                    let leaf = &graph.leaves()[index];
                                    match (leaf.reused, &leaf.address) {
                                        (true, Some(address)) => {
                                            self.modal = Some(MapModal::Reuse(address.clone()));
                                            None
                                        }
                                        _ => Some(LabelTarget::Leaf(index)),
                                    }
                                }
                                None => None,
                            },
                            Target::Slot(item, side, row) => {
                                slot_ref(graph, &self.orders, item, side, row)
                                    .map(LabelTarget::Slot)
                            }
                            Target::Edge(_) | Target::Frame => None,
                        };
                        if let Some(label) = label {
                            self.modal = Some(MapModal::Label(label));
                        }
                    }
                    GraphEvent::EmptyClick => self.clear_selection(),
                    GraphEvent::Hover(target) => self.hover = target,
                    GraphEvent::AreaSelected {
                        items, additive, ..
                    } => {
                        self.selection.area(items, additive);
                        self.tag_highlight = None;
                        self.show_on_map = None;
                    }
                    GraphEvent::SlotWheel { steps, .. } => {
                        if let Some(tag) = &mut self.tag_highlight {
                            tag.cycle(steps);
                        }
                    }
                },
                MapMessage::CloseModal => return self.close_modal(daemon),
                MapMessage::ToggleCoinSelected => self.coin_action(CoinUi::toggle_selected),
                MapMessage::ToggleFrozen => {
                    self.coin_action(|coin_ui, coin| Some(coin_ui.toggle_frozen(coin)));
                }
                MapMessage::ToggleTagPopover => {
                    if self.tag_popover.take().is_none() {
                        self.tag_popover = Some(String::new());
                        return text_input::focus(self.tag_input_id.clone());
                    }
                }
                MapMessage::TagFilterEdited(text) => {
                    if let Some(filter) = &mut self.tag_popover {
                        *filter = text;
                    }
                }
                MapMessage::TagToggled(tag) => {
                    self.coin_action(|coin_ui, coin| Some(coin_ui.toggle_tag(coin, tag)));
                }
                MapMessage::TagCreate => {
                    if let Some(name) = self.tag_popover.as_mut().map(mem::take) {
                        self.coin_action(|coin_ui, coin| coin_ui.add_tag_by_name(coin, &name));
                    }
                }
                MapMessage::ClearCoinSelection => self.coin_ui.clear_selected(),
                _ => {}
            },
            _ => {}
        }
        Task::none()
    }

    fn interrupt(&mut self) {
        self.modal = None;
        self.shortcuts_open = false;
        self.command_held = false;
        self.tag_popover = None;
    }

    fn subscription(&self) -> Subscription<Message> {
        event::listen_with(map_event)
    }

    fn reload(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        _wallet: Arc<Wallet>,
    ) -> Task<Message> {
        self.loading = true;
        self.warning = None;
        self.modal = None;
        self.shortcuts_open = false;
        self.tag_popover = None;
        self.labels_edited = LabelsEdited::default();
        self.pending_label = None;
        self.label_changes.clear();
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

    use iced::{
        event::Status,
        keyboard::{key::Named, Key, Modifiers},
    };

    use liana_ui::widget::graph_view::Target;

    use crate::app::{
        state::map::{
            display_row, escape_action, fixture, focus::ShowOnMap, graph::TxGraph, key_action,
            split_stored, EscapeAction, MapPanel,
        },
        view::MapKey,
    };

    fn character(c: &str) -> Key {
        Key::Character(c.into())
    }

    fn entry(item: LayoutItem) -> GraphLayoutEntry {
        GraphLayoutEntry {
            item,
            position: Some((10.0, 20.0)),
            input_order: None,
            output_order: None,
            lane_position: None,
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
    fn click_clears_show_on_map() {
        let mut panel = MapPanel::new();
        let graph = fixture::graph();
        let id = graph.tx_item(0);
        panel.graph = Some(graph);
        panel.show_on_map = Some(ShowOnMap {
            slots: Vec::new(),
            leaf: None,
        });
        panel.on_click(&Target::Frame, Modifiers::empty());
        assert!(panel.show_on_map.is_some());
        panel.on_click(&Target::Item(id), Modifiers::empty());
        assert!(panel.show_on_map.is_none());
    }

    #[test]
    fn display_row_inverts_order() {
        let order = [2, 0, 1];
        assert_eq!(display_row(Some(&order), 2), 0);
        assert_eq!(display_row(Some(&order), 0), 1);
        assert_eq!(display_row(Some(&order), 1), 2);
        assert_eq!(display_row(None, 2), 2);
    }

    #[test]
    fn key_action_undo_redo() {
        let ctrl = Modifiers::CTRL;
        let undo = key_action(&character("z"), ctrl, Status::Ignored);
        assert_eq!(undo, Some(MapKey::Undo));
        let redo = key_action(&character("Z"), ctrl | Modifiers::SHIFT, Status::Ignored);
        assert_eq!(redo, Some(MapKey::Redo));
        let redo = key_action(&character("y"), ctrl, Status::Ignored);
        assert_eq!(redo, Some(MapKey::Redo));
    }

    #[test]
    fn key_action_plain_keys() {
        let none = Modifiers::empty();
        let help = key_action(&character("?"), Modifiers::SHIFT, Status::Ignored);
        assert_eq!(help, Some(MapKey::Shortcuts));
        let unspent = key_action(&character("u"), none, Status::Ignored);
        assert_eq!(unspent, Some(MapKey::Unspent));
        assert_eq!(key_action(&character("z"), none, Status::Ignored), None);
    }

    #[test]
    fn key_action_leaves_text_inputs_alone() {
        let undo = key_action(&character("z"), Modifiers::CTRL, Status::Captured);
        assert_eq!(undo, None);
        let esc = Key::Named(Named::Escape);
        let none = Modifiers::empty();
        assert_eq!(
            key_action(&esc, none, Status::Ignored),
            Some(MapKey::Escape)
        );
        assert_eq!(
            key_action(&esc, none, Status::Captured),
            Some(MapKey::EscapeInInput)
        );
        assert_eq!(key_action(&esc, Modifiers::CTRL, Status::Ignored), None);
    }

    #[test]
    fn escape_action_precedence() {
        assert_eq!(escape_action(true, true, true), EscapeAction::CloseHelp);
        assert_eq!(escape_action(false, true, true), EscapeAction::ClosePopover);
        assert_eq!(escape_action(false, false, true), EscapeAction::CloseModal);
        assert_eq!(
            escape_action(false, false, false),
            EscapeAction::ClearSelection
        );
    }
}
