pub mod coin_ui;
pub mod display;
pub mod edit;
pub mod external;
#[cfg(test)]
pub mod fixture;
pub mod focus;
pub mod global;
pub mod graph;
pub mod history;
pub mod import;
pub mod lanes;
pub mod layout;
pub mod offsets;
pub mod selection;
pub mod topology;
pub mod untangle;
pub mod wallets;

use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    mem,
    sync::Arc,
};

use iced::{
    advanced::widget::Id,
    event,
    keyboard::{self, key::Named, Modifiers},
    window, Color, Event, Point, Rectangle, Size, Subscription, Task, Vector,
};
use liana::miniscript::bitcoin::{bip32::Fingerprint, Address, Network, OutPoint, Txid};
use liana_ui::{
    component::panels::map::{header::HeaderAction, modals::ImportMode, wallet_color},
    theme::Theme,
    widget::{
        graph_view::{
            self,
            geometry::{LEAF_HEIGHT, LEAF_WIDTH, ZOOM_STEP},
            GraphEvent, Handle, ItemId, Lane, Side, Target,
        },
        modal::Modal,
        text_input, Element,
    },
};
use lianad::commands::{GraphItem as LayoutItem, GraphLayoutEntry, GraphWallet};

use crate::{
    app::{
        cache::Cache,
        error::Error,
        menu::{MapFocus, Menu},
        message::Message,
        settings::WalletId,
        state::{
            label::{label_item_from_str, LabelsEdited},
            map::{
                coin_ui::CoinUi,
                display::{
                    click_action, display_state, label_key, label_wallet, slot_ref, ClickAction,
                },
                external::{
                    external_wallets, parse_descriptor, remembered_electrum, remove,
                    save_external_labels, save_layout, ExternalWallet,
                },
                focus::{resolve_focus, FocusLanding, ShowOnMap},
                graph::{MapItem, SlotRef, TxGraph, WalletTxs},
                history::{Change, History, PlacementKind},
                import::{
                    daemon_electrum, import, parse_account, rescan, ImportFailure, ImportForm,
                    ImportSource,
                },
                lanes::{Band, WalletLane},
                offsets::{Offsets, WalletLayout},
                selection::{linked_txs, Link, Selection, TagHighlight},
                wallets::{
                    load_selected, other_wallets, save_wallet_labels, save_wallet_layout,
                    stored_offset, ListedWallets, OtherWallet, WalletKey, WalletStore,
                },
            },
            State,
        },
        view::{self, LabelMessage, MapKey, MapMessage},
        wallet::Wallet,
    },
    daemon::{
        model::{LabelItem, LabelsLoader},
        Daemon,
    },
    dir::{LianaDirectory, NetworkDirectory},
    export,
    hw::{HardwareWallet, HardwareWallets},
};

const TOPOLOGY_FILE: &str = "map-topology.json";

/// Display orders (inputs, outputs) per transaction, `None` = true order.
pub type Orders = HashMap<Txid, (history::Order, history::Order)>;

/// A wallet loaded for the map.
#[derive(Debug)]
pub struct MapWallet {
    pub txs: WalletTxs,
    pub layout: WalletLayout,
    /// `None` for the current wallet.
    pub store: Option<WalletStore>,
    /// Stored position of its lane.
    pub lane: Option<u32>,
    pub displayed: bool,
    /// Height its lane was resized to, `None` for automatic.
    pub lane_height: Option<f32>,
}

/// The stored layout, checked against the current graph.
#[derive(Debug, Default)]
pub struct StoredLayout {
    pub positions: HashMap<ItemId, Point>,
    /// Positions in the lanes, relative to the lane top.
    pub lane_positions: HashMap<ItemId, Point>,
    pub orders: Orders,
    /// Entries whose item is not on the map anymore.
    pub remove: Vec<LayoutItem>,
    /// Transactions whose stored order was dropped.
    pub resave: Vec<ItemId>,
}

/// View toggles of the header, never recorded.
#[derive(Debug, Clone, Copy)]
pub struct Toggles {
    pub area: bool,
    pub unspent: bool,
    pub snap: bool,
    pub lanes: bool,
    /// With the lanes off, Reset and Tidy up lay each cluster of linked transactions out alone.
    pub clusters: bool,
}

impl Default for Toggles {
    fn default() -> Self {
        Self {
            area: false,
            unspent: false,
            snap: false,
            lanes: true,
            clusters: false,
        }
    }
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

#[derive(Debug)]
pub enum MapModal {
    Label(LabelTarget),
    Reuse(Address),
    Wallets,
    Import(Box<ImportForm>),
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
            "e" | "E" if command && modifiers.shift() => Some(MapKey::ExportTopology),
            "?" if !command => Some(MapKey::Shortcuts),
            "u" | "U" if !command => Some(MapKey::Unspent),
            _ => None,
        },
        keyboard::Key::Named(named) if ignored && !command => match named {
            Named::ArrowRight => Some(MapKey::Right),
            Named::ArrowLeft => Some(MapKey::Left),
            Named::ArrowUp => Some(MapKey::Up),
            Named::ArrowDown => Some(MapKey::Down),
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
    data_dir: LianaDirectory,
    network_dir: NetworkDirectory,
    network: Network,
    wallet_id: WalletId,
    /// A Liana Connect wallet: no other wallet can be added to its map.
    remote: bool,
    graph_id: Id,
    graph: Option<TxGraph>,
    /// Map positions with the lanes off: each item's position in its owning wallet's layout
    /// plus that wallet's offset.
    layout: HashMap<ItemId, Point>,
    /// Positions with the lanes on, relative to the top of the item's lane.
    lane_layout: HashMap<ItemId, Point>,
    orders: Orders,
    offsets: Offsets,
    /// The wallets added to the map, edits written to their own store.
    others: HashMap<WalletKey, WalletStore>,
    /// The loaded wallets, the current one included, in lane order.
    lanes: Vec<WalletLane>,
    /// Height of each drawn wallet's lane laid out by default.
    lane_heights: HashMap<WalletKey, f32>,
    /// Height of each lane resized by its bottom edge.
    resized_heights: HashMap<WalletKey, f32>,
    /// Alias or name of the current wallet.
    current_name: String,
    /// The other wallets listed by the wallets modal.
    listed: Vec<OtherWallet>,
    /// The external wallets listed by the wallets modal.
    listed_externals: Vec<ExternalWallet>,
    /// The external wallets being scanned again, by id.
    rescanning: HashSet<String>,
    /// A wallet is being added to or removed from the map.
    switching: bool,
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

/// The stored layout of `wallet`. Its entries of items another wallet owns on this map are
/// left alone.
pub fn split_stored(
    graph: &TxGraph,
    wallet: &WalletKey,
    entries: Vec<GraphLayoutEntry>,
) -> StoredLayout {
    let mut stored = StoredLayout::default();
    for entry in entries {
        let Some(id) = graph.item_id(&entry.item) else {
            stored.remove.push(entry.item);
            continue;
        };
        if graph.item_wallet(id) != Some(wallet) {
            continue;
        }
        if let Some((x, y)) = entry.position {
            stored.positions.insert(id, Point::new(x as f32, y as f32));
        }
        if let Some((x, y)) = entry.lane_position {
            stored
                .lane_positions
                .insert(id, Point::new(x as f32, y as f32));
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
                .insert(tx.history().txid, (input_order, output_order));
        }
    }
    stored
}

/// Loads the current wallet `current` and the other wallets selected on its map.
async fn load_map(
    daemon: Arc<dyn Daemon + Sync + Send>,
    network_dir: NetworkDirectory,
    network: Network,
    current: WalletId,
) -> Result<Vec<MapWallet>, Error> {
    let coins = daemon.list_all_coins().await?;
    let txs = daemon.get_all_history_txs(&coins).await?;
    let layout = daemon.get_graph_layout().await?;
    let rows = daemon.get_graph_wallets().await?;
    let current_row = rows
        .iter()
        .find(|row| row.wallet == WalletKey::Current.row());
    let (lane, displayed, lane_height) = current_row.map_or((None, true, None), |row| {
        (
            row.lane,
            row.displayed,
            row.lane_height.map(|height| height as f32),
        )
    });
    let offset = current_row.and_then(stored_offset);
    let checksum = current.descriptor_checksum.clone();
    let others =
        tokio::task::spawn_blocking(move || load_selected(&network_dir, network, &current, &rows))
            .await
            .map_err(|e| Error::Unexpected(e.to_string()))??;
    let wallet = MapWallet {
        txs: WalletTxs {
            key: WalletKey::Current,
            checksum,
            txs,
            coins,
        },
        layout: WalletLayout {
            entries: layout,
            offset,
        },
        store: None,
        lane,
        displayed,
        lane_height,
    };
    Ok(std::iter::once(wallet).chain(others).collect())
}

/// The Electrum server of the current wallet, `None` when it uses bitcoind.
fn current_electrum(daemon: &(dyn Daemon + Sync + Send)) -> Option<String> {
    daemon_electrum(
        daemon
            .config()
            .and_then(|config| config.bitcoin_backend.as_ref()),
    )
}

impl MapPanel {
    pub fn new(
        data_dir: LianaDirectory,
        network: Network,
        wallet_id: WalletId,
        remote: bool,
    ) -> Self {
        Self {
            network_dir: data_dir.network_directory(network),
            data_dir,
            network,
            wallet_id,
            remote,
            graph_id: Id::unique(),
            graph: None,
            layout: HashMap::new(),
            lane_layout: HashMap::new(),
            orders: HashMap::new(),
            offsets: Offsets::default(),
            others: HashMap::new(),
            lanes: Vec::new(),
            lane_heights: HashMap::new(),
            resized_heights: HashMap::new(),
            current_name: String::new(),
            listed: Vec::new(),
            listed_externals: Vec::new(),
            rescanning: HashSet::new(),
            switching: false,
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
            ClickAction::SelectWallet(id) => {
                if let Some(wallet) = graph.item_wallet(id) {
                    self.selection.area(graph.wallet_items(wallet), false);
                }
            }
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

    /// The placement edited and shown: the lanes while they are on.
    fn placement(&self) -> PlacementKind {
        if self.toggles.lanes {
            PlacementKind::Lanes
        } else {
            PlacementKind::Global
        }
    }

    /// Positions of the items in `placement`.
    fn positions(&self, placement: PlacementKind) -> &HashMap<ItemId, Point> {
        match placement {
            PlacementKind::Global => &self.layout,
            PlacementKind::Lanes => &self.lane_layout,
        }
    }

    /// Map positions of the items as drawn.
    fn shown_layout(&self) -> Cow<'_, HashMap<ItemId, Point>> {
        match (&self.graph, self.placement()) {
            (Some(graph), PlacementKind::Lanes) => {
                Cow::Owned(lanes::shown(graph, &self.lane_layout, &self.bands()))
            }
            _ => Cow::Borrowed(&self.layout),
        }
    }

    /// Sets the `after` state of a layout change on the positions of its placement and returns
    /// the items to persist.
    fn apply_layout_change(&mut self, change: &Change) -> Vec<ItemId> {
        let Some(graph) = &self.graph else {
            return Vec::new();
        };
        let layout = match change.placement() {
            Some(PlacementKind::Lanes) => &mut self.lane_layout,
            _ => &mut self.layout,
        };
        edit::apply_layout_change(graph, layout, &mut self.orders, &mut self.offsets, change)
    }

    /// Records a move of items in `placement`, applies it and persists the touched items.
    fn commit_move(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        placement: PlacementKind,
        moves: Vec<(ItemId, Point, Point)>,
    ) -> Task<Message> {
        let Some(graph) = &self.graph else {
            return Task::none();
        };
        let change = Change::Move {
            placement,
            moves: moves
                .iter()
                .filter_map(|(id, before, after)| Some((graph.graph_item(*id)?, *before, *after)))
                .collect(),
        };
        let touched = self.apply_layout_change(&change);
        if touched.is_empty() {
            return Task::none();
        }
        self.history.record(change);
        self.save_layout(daemon, touched, HashMap::new())
    }

    /// Records a reset of the lanes and of the slot orders, applies it and persists it.
    fn commit_lanes_reset(&mut self, daemon: Arc<dyn Daemon + Sync + Send>) -> Task<Message> {
        let Some(graph) = &self.graph else {
            return Task::none();
        };
        let reset = lanes::reset(graph, &lanes::displayed(&self.lanes));
        self.lane_heights.extend(reset.heights);
        self.resized_heights.clear();
        let none = Offsets::default();
        let change = Change::Layout {
            placement: PlacementKind::Lanes,
            before: edit::layout_state(graph, &self.lane_layout, &self.orders, &none),
            after: edit::layout_state(graph, &reset.positions, &reset.orders, &none),
        };
        let rows = self.save_rows(daemon.clone());
        Task::batch([self.commit_layout(daemon, change), rows])
    }

    /// Places the displayed wallets by structure, mixed or by cluster as toggled, as one change
    /// of the positions, slot orders and offsets with the lanes off. The transactions start at
    /// their height at `start`, the slots ordered again from `orders`.
    fn commit_global(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        start: &HashMap<ItemId, Point>,
        orders: &Orders,
    ) -> Task<Message> {
        let Some(graph) = &self.graph else {
            return Task::none();
        };
        let displayed = lanes::displayed(&self.lanes);
        let placed = global::place(
            graph,
            &displayed,
            start,
            orders,
            self.toggles.clusters,
            true,
        );
        let change = Change::Layout {
            placement: PlacementKind::Global,
            before: edit::layout_state(graph, &self.layout, &self.orders, &self.offsets),
            after: edit::layout_state(graph, &placed.positions, &placed.orders, &placed.offsets),
        };
        let layout = self.commit_layout(daemon.clone(), change);
        Task::batch([layout, self.save_rows(daemon)])
    }

    /// Untangles the lanes from their current positions and slot orders as one change.
    fn commit_tidy_up(&mut self, daemon: Arc<dyn Daemon + Sync + Send>) -> Task<Message> {
        let Some(graph) = &self.graph else {
            return Task::none();
        };
        let displayed = lanes::displayed(&self.lanes);
        let untangled = lanes::untangled(graph, &displayed, &self.lane_layout, &self.orders);
        let none = Offsets::default();
        let change = Change::Layout {
            placement: PlacementKind::Lanes,
            before: edit::layout_state(graph, &self.lane_layout, &self.orders, &none),
            after: edit::layout_state(graph, &untangled.positions, &untangled.orders, &none),
        };
        self.lane_heights.extend(untangled.heights);
        self.commit_layout(daemon, change)
    }

    /// Records a layout change, applies it and persists the touched items.
    fn commit_layout(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        change: Change,
    ) -> Task<Message> {
        let touched = self.apply_layout_change(&change);
        self.history.record(change);
        self.save_layout(daemon, touched, HashMap::new())
    }

    /// Records a drag of every item of a wallet as a move of its offset.
    fn commit_offset(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        wallet: WalletKey,
        delta: Vector,
    ) -> Task<Message> {
        let Some(before) = self.offsets.get(&wallet) else {
            return Task::none();
        };
        let after = edit::moved_offset(before, delta, self.toggles.snap);
        self.history.record(Change::Offset {
            wallet: wallet.clone(),
            before,
            after,
        });
        self.set_offset(daemon, wallet, after)
    }

    fn set_offset(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        wallet: WalletKey,
        offset: Vector,
    ) -> Task<Message> {
        let Some(graph) = &self.graph else {
            return Task::none();
        };
        if !edit::apply_offset(graph, &mut self.layout, &mut self.offsets, &wallet, offset) {
            return Task::none();
        }
        self.save_rows(daemon)
    }

    /// Applies the change returned by `History::undo` or `History::redo`.
    fn apply_history(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        change: Option<Change>,
    ) -> Task<Message> {
        let (Some(change), Some(_)) = (change, &self.graph) else {
            return Task::none();
        };
        if let Change::Label {
            wallet,
            item,
            after,
            ..
        } = change
        {
            let labels = HashMap::from([(item, after)]);
            return match wallet {
                WalletKey::Current => Task::perform(
                    async move {
                        daemon.update_labels(&labels).await?;
                        Ok(labels
                            .into_iter()
                            .map(|(item, label)| (item.to_string(), label))
                            .collect())
                    },
                    Message::LabelsUpdated,
                ),
                wallet => self.save_other_labels(wallet, labels),
            };
        }
        if let Change::Offset { wallet, after, .. } = change {
            return self.set_offset(daemon, wallet, after);
        }
        if self.coin_ui.apply(&change) {
            return Task::none();
        }
        let touched = self.apply_layout_change(&change);
        let save = self.save_layout(daemon.clone(), touched, HashMap::new());
        match change {
            Change::Layout {
                placement: PlacementKind::Global,
                ..
            } => Task::batch([save, self.save_rows(daemon)]),
            _ => save,
        }
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

    /// Selects the topmost child or parent of the single selected transaction.
    fn select_linked(&mut self, link: Link) -> Task<Message> {
        let Some(graph) = &self.graph else {
            return Task::none();
        };
        let Some(tx) = self.selection.single_tx(graph) else {
            return Task::none();
        };
        let txs = linked_txs(graph, &self.shown_layout(), tx, link);
        let selected = self.selection.select_linked(graph, txs);
        self.pan_to_selected(selected)
    }

    /// Selects another child or parent `steps` below the selected one.
    fn step_linked(&mut self, steps: isize) -> Task<Message> {
        let Some(graph) = &self.graph else {
            return Task::none();
        };
        let selected = self.selection.step_linked(graph, steps);
        self.pan_to_selected(selected)
    }

    /// Brings a block selected with the arrow keys into view.
    fn pan_to_selected(&mut self, selected: Option<ItemId>) -> Task<Message> {
        let (Some(graph), Some(id)) = (&self.graph, selected) else {
            return Task::none();
        };
        self.tag_highlight = None;
        self.show_on_map = None;
        let Some(at) = self.shown_layout().get(&id).copied() else {
            return Task::none();
        };
        let target = Rectangle::new(at, layout::item_size(graph, id));
        graph_view::pan_to(self.graph_id.clone(), target)
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

    /// The wallet the label of the open label modal is saved to.
    fn label_owner(&self) -> Option<WalletKey> {
        let (Some(graph), Some(MapModal::Label(target))) = (&self.graph, &self.modal) else {
            return None;
        };
        label_wallet(graph, target).cloned()
    }

    /// Own label of the item `key` in the owner's history of the open label modal.
    fn own_label(&self, key: &str) -> Option<String> {
        let (Some(graph), Some(MapModal::Label(target))) = (&self.graph, &self.modal) else {
            return None;
        };
        let tx = match *target {
            LabelTarget::Tx(tx) => tx,
            LabelTarget::Slot(slot) => slot.tx,
            LabelTarget::Leaf(leaf) => graph.leaves().get(leaf)?.tx,
        };
        let wallet = label_wallet(graph, target)?;
        let history = graph.txs().get(tx)?.wallet_history(wallet)?;
        history.labels.get(key).cloned()
    }

    /// Saves the edited labels of `keys` to an added wallet.
    fn confirm_other_labels(&mut self, wallet: WalletKey, keys: &[String]) -> Task<Message> {
        let labels = keys
            .iter()
            .filter_map(|key| {
                let label = self.labels_edited.cache().get(key)?;
                let value = (!label.value.is_empty()).then(|| label.value.clone());
                Some((label_item_from_str(key), value))
            })
            .collect();
        self.save_other_labels(wallet, labels)
    }

    fn save_other_labels(
        &self,
        wallet: WalletKey,
        labels: HashMap<LabelItem, Option<String>>,
    ) -> Task<Message> {
        let Some(store) = self.others.get(&wallet).cloned() else {
            return Task::done(Message::MapWalletLabelsSaved(
                wallet.clone(),
                Err(Error::Unexpected(format!(
                    "wallet {wallet:?} is not on the map"
                ))),
            ));
        };
        let (network_dir, network) = (self.network_dir.clone(), self.network);
        Task::perform(
            async move {
                let saved = labels
                    .iter()
                    .map(|(item, label)| (item.to_string(), label.clone()))
                    .collect();
                tokio::task::spawn_blocking(move || -> Result<(), Error> {
                    match store {
                        WalletStore::Other(other) => {
                            Ok(save_wallet_labels(&other, network, &labels)?)
                        }
                        WalletStore::External(external) => Ok(save_external_labels(
                            &external,
                            &network_dir,
                            network,
                            &labels,
                        )?),
                    }
                })
                .await
                .map_err(|e| Error::Unexpected(e.to_string()))??;
                Ok(saved)
            },
            move |res| Message::MapWalletLabelsSaved(wallet.clone(), res),
        )
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

    /// Records the user save matching `saved` and keeps the labels of the current wallet for
    /// the origin panel.
    fn labels_saved(&mut self, wallet: WalletKey, saved: HashMap<String, Option<String>>) {
        let pending = self
            .pending_label
            .take_if(|(key, _)| saved.contains_key(key));
        if let Some((key, before)) = pending {
            let after = saved.get(&key).cloned().flatten();
            self.history.record(Change::Label {
                wallet: wallet.clone(),
                item: label_item_from_str(&key),
                before,
                after,
            });
        }
        if wallet == WalletKey::Current {
            self.label_changes.extend(saved);
        }
    }

    /// Selects or unselects an added wallet, keeping its offset, and loads the map again.
    fn toggle_wallet(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        wallet: WalletKey,
    ) -> Task<Message> {
        if self.switching || wallet == WalletKey::Current {
            return Task::none();
        }
        let selected = !self.others.contains_key(&wallet);
        let wallet = wallet.row();
        self.switching = true;
        let (network_dir, network, current) = (
            self.network_dir.clone(),
            self.network,
            self.wallet_id.clone(),
        );
        Task::perform(
            async move {
                let stored = daemon
                    .get_graph_wallets()
                    .await?
                    .into_iter()
                    .find(|row| row.wallet == wallet);
                daemon
                    .update_graph_wallets(&[GraphWallet {
                        wallet,
                        selected,
                        offset: stored.as_ref().and_then(|row| row.offset),
                        lane: stored.as_ref().and_then(|row| row.lane),
                        displayed: stored.as_ref().is_none_or(|row| row.displayed),
                        lane_height: stored.and_then(|row| row.lane_height),
                    }])
                    .await?;
                load_map(daemon, network_dir, network, current).await
            },
            Message::MapLoaded,
        )
    }

    /// Loads the map again after a change of the added wallets.
    fn load_again(&mut self, daemon: Arc<dyn Daemon + Sync + Send>) -> Task<Message> {
        self.switching = true;
        Task::perform(
            load_map(
                daemon,
                self.network_dir.clone(),
                self.network,
                self.wallet_id.clone(),
            ),
            Message::MapLoaded,
        )
    }

    fn open_import(&mut self, daemon: &(dyn Daemon + Sync + Send)) {
        let (electrum, error) = match current_electrum(daemon) {
            Some(_) => (None, None),
            None => match remembered_electrum(&self.network_dir) {
                Ok(addr) => (Some(addr.unwrap_or_default()), None),
                Err(e) => (Some(String::new()), Some(e.to_string())),
            },
        };
        let hws = HardwareWallets::new(self.data_dir.clone(), self.network);
        let mut form = ImportForm::new(hws, electrum);
        form.error = error;
        self.modal = Some(MapModal::Import(Box::new(form)));
    }

    /// The open import form, `None` while it scans.
    fn import_form(&mut self) -> Option<&mut ImportForm> {
        match &mut self.modal {
            Some(MapModal::Import(form)) if !form.scanning => Some(form),
            _ => None,
        }
    }

    fn import_descriptor(&mut self, daemon: &(dyn Daemon + Sync + Send)) -> Task<Message> {
        let network = self.network;
        let Some(form) = self.import_form().filter(|form| form.can_import()) else {
            return Task::none();
        };
        match parse_descriptor(&form.descriptor.value, network) {
            Ok(descriptor) => {
                self.start_import(daemon, ImportSource::Descriptor(Box::new(descriptor)))
            }
            Err(e) => {
                form.error = Some(e.to_string());
                Task::none()
            }
        }
    }

    fn import_device(
        &mut self,
        daemon: &(dyn Daemon + Sync + Send),
        fingerprint: Fingerprint,
    ) -> Task<Message> {
        let Some(form) = self.import_form().filter(|form| form.ready()) else {
            return Task::none();
        };
        let account = match parse_account(&form.account.value) {
            Ok(account) => account,
            Err(e) => {
                form.error = Some(e.to_string());
                return Task::none();
            }
        };
        let device = form.hws.list.iter().find_map(|hw| match hw {
            HardwareWallet::Supported {
                device,
                fingerprint: device_fingerprint,
                ..
            } if *device_fingerprint == fingerprint => Some(device.clone()),
            _ => None,
        });
        let Some(device) = device else {
            return Task::none();
        };
        form.device = Some(fingerprint);
        self.start_import(
            daemon,
            ImportSource::Device {
                device,
                fingerprint,
                account,
            },
        )
    }

    /// Scans and saves the wallet of the import form.
    fn start_import(
        &mut self,
        daemon: &(dyn Daemon + Sync + Send),
        source: ImportSource,
    ) -> Task<Message> {
        let Some(MapModal::Import(form)) = &mut self.modal else {
            return Task::none();
        };
        let asked = form
            .electrum
            .as_ref()
            .map(|electrum| electrum.value.trim().to_string());
        let Some(electrum) = asked.clone().or_else(|| current_electrum(daemon)) else {
            form.device = None;
            form.error = Some(ImportFailure::NoElectrum.to_string());
            return Task::none();
        };
        form.scanning = true;
        form.error = None;
        Task::perform(
            import(
                self.network_dir.clone(),
                self.network,
                form.name.value.trim().to_string(),
                source,
                electrum,
                asked.is_some(),
            ),
            Message::MapWalletImported,
        )
    }

    fn rescan_external(
        &mut self,
        daemon: &(dyn Daemon + Sync + Send),
        id: String,
    ) -> Task<Message> {
        let Some(wallet) = self
            .listed_externals
            .iter()
            .find(|wallet| wallet.id == id)
            .cloned()
        else {
            return Task::none();
        };
        let Some(electrum) = wallet.electrum.clone().or_else(|| current_electrum(daemon)) else {
            self.warning = Some(ImportFailure::NoElectrum.into());
            return Task::none();
        };
        if !self.rescanning.insert(id.clone()) {
            return Task::none();
        }
        let (network_dir, network) = (self.network_dir.clone(), self.network);
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    rescan(wallet, &network_dir, network, &electrum)
                })
                .await?
            },
            move |res| Message::MapWalletRescanned(id.clone(), res),
        )
    }

    /// Keeps the new scan of `wallet` and loads the map again when it is on it.
    fn rescanned(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        wallet: ExternalWallet,
    ) -> Task<Message> {
        let key = WalletKey::External(wallet.id.clone());
        if let Some(listed) = self
            .listed_externals
            .iter_mut()
            .find(|listed| listed.id == wallet.id)
        {
            *listed = wallet.clone();
        }
        if !self.others.contains_key(&key) {
            return Task::none();
        }
        self.others.insert(key, WalletStore::External(wallet));
        self.load_again(daemon)
    }

    /// Saves the anonymized topology of the displayed graph to a file the user picks.
    fn export_topology(&self) -> Task<Message> {
        let Some(topology) = self
            .graph
            .as_ref()
            .and_then(|graph| topology::topology(graph, &lanes::displayed(&self.lanes)))
        else {
            return Task::none();
        };
        Task::perform(
            async move {
                let Some(path) = export::get_path(TOPOLOGY_FILE.to_string(), true).await else {
                    return Ok(());
                };
                tokio::task::spawn_blocking(move || -> Result<(), export::Error> {
                    let file = std::fs::File::create(path)?;
                    serde_json::to_writer_pretty(file, &topology)
                        .map_err(|e| export::Error::Io(e.to_string()))
                })
                .await?
            },
            Message::MapTopologyExported,
        )
    }

    /// Deletes an external wallet and unselects it.
    fn remove_external(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        id: String,
    ) -> Task<Message> {
        if self.switching || self.loading || self.rescanning.contains(&id) {
            return Task::none();
        }
        let wallet = WalletKey::External(id.clone()).row();
        self.switching = true;
        let network_dir = self.network_dir.clone();
        let removed = id.clone();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || remove(&network_dir, &removed))
                    .await
                    .map_err(|e| Error::Unexpected(e.to_string()))??;
                daemon
                    .update_graph_wallets(&[GraphWallet {
                        wallet,
                        selected: false,
                        offset: None,
                        lane: None,
                        displayed: true,
                        lane_height: None,
                    }])
                    .await?;
                Ok(())
            },
            move |res| Message::MapExternalRemoved(id.clone(), res),
        )
    }

    fn coin_wallet(&self, coin: &OutPoint) -> Option<&WalletKey> {
        let graph = self.graph.as_ref()?;
        graph.slot_wallet(graph.output_slot(coin)?)
    }

    /// Writes the items to their owning wallet's layout, with the stale entries of `remove`.
    fn save_layout(
        &self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        items: impl IntoIterator<Item = ItemId>,
        mut remove: HashMap<WalletKey, Vec<LayoutItem>>,
    ) -> Task<Message> {
        let Some(graph) = &self.graph else {
            return Task::none();
        };
        let mut sets = offsets::local_entries(
            graph,
            &self.layout,
            &self.lane_layout,
            &self.orders,
            &self.offsets,
            items,
        );
        let wallets: HashSet<WalletKey> = sets.keys().chain(remove.keys()).cloned().collect();
        Task::batch(wallets.into_iter().map(|wallet| {
            let set = sets.remove(&wallet).unwrap_or_default();
            let remove = remove.remove(&wallet).unwrap_or_default();
            self.write_layout(daemon.clone(), wallet, set, remove)
        }))
    }

    fn write_layout(
        &self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        wallet: WalletKey,
        set: Vec<GraphLayoutEntry>,
        remove: Vec<LayoutItem>,
    ) -> Task<Message> {
        if set.is_empty() && remove.is_empty() {
            return Task::none();
        }
        match wallet {
            WalletKey::Current => Task::perform(
                async move {
                    daemon
                        .update_graph_layout(&set, &remove)
                        .await
                        .map_err(Into::into)
                },
                Message::MapLayoutSaved,
            ),
            wallet => {
                let Some(store) = self.others.get(&wallet).cloned() else {
                    return Task::done(Message::MapLayoutSaved(Err(Error::Unexpected(format!(
                        "wallet {wallet:?} is not on the map"
                    )))));
                };
                let (network_dir, network) = (self.network_dir.clone(), self.network);
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || -> Result<(), Error> {
                            match store {
                                WalletStore::Other(other) => {
                                    Ok(save_wallet_layout(&other, network, &set, &remove)?)
                                }
                                WalletStore::External(external) => {
                                    Ok(save_layout(&external.dir(&network_dir), &set, &remove)?)
                                }
                            }
                        })
                        .await
                        .map_err(|e| Error::Unexpected(e.to_string()))?
                    },
                    Message::MapLayoutSaved,
                )
            }
        }
    }

    /// Lanes of the displayed wallets stacked in their order.
    fn bands(&self) -> Vec<(WalletKey, Band)> {
        let Some(graph) = &self.graph else {
            return Vec::new();
        };
        lanes::stacked_bands(
            graph,
            &self.lane_layout,
            &self.lane_heights,
            &self.resized_heights,
            &lanes::displayed(&self.lanes),
        )
    }

    /// Writes the row of every loaded wallet: its lane, whether it is displayed and its offset.
    fn save_rows(&self, daemon: Arc<dyn Daemon + Sync + Send>) -> Task<Message> {
        let rows = lanes::rows(&self.lanes, &self.offsets, &self.resized_heights);
        Task::perform(write_rows(daemon, rows), Message::MapLayoutSaved)
    }

    /// Shows or hides the wallet of lane `index` and draws the map again without the hidden
    /// wallets.
    fn toggle_lane(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        index: usize,
    ) -> Task<Message> {
        if self.switching || index >= self.lanes.len() {
            return Task::none();
        }
        let mut lanes = self.lanes.clone();
        lanes[index].displayed = !lanes[index].displayed;
        let rows = lanes::rows(&lanes, &self.offsets, &self.resized_heights);
        self.switching = true;
        let (network_dir, network, current) = (
            self.network_dir.clone(),
            self.network,
            self.wallet_id.clone(),
        );
        Task::perform(
            async move {
                write_rows(daemon.clone(), rows).await?;
                load_map(daemon, network_dir, network, current).await
            },
            Message::MapLoaded,
        )
    }

    /// Moves the lane `index` just before the lane `before`, at the end for `None`.
    fn move_lane(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        index: usize,
        before: Option<usize>,
    ) -> Task<Message> {
        let Some(wallet) = self.lanes.get(index).map(|lane| lane.wallet.clone()) else {
            return Task::none();
        };
        let before = match before.map(|before| self.lanes.get(before)) {
            Some(Some(lane)) => Some(lane.wallet.clone()),
            Some(None) => return Task::none(),
            None => None,
        };
        self.lanes = lanes::moved(&self.lanes, &wallet, before.as_ref());
        self.save_rows(daemon)
    }

    /// Lane `index` dragged by its items: it moves just before the lane `before` when that
    /// changes the lanes drawn, and its items move `dx` across in the lanes.
    fn drag_lane(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        index: usize,
        before: Option<usize>,
        dx: f32,
    ) -> Task<Message> {
        let Some(wallet) = self.lanes.get(index).map(|lane| lane.wallet.clone()) else {
            return Task::none();
        };
        let target = before
            .and_then(|before| self.lanes.get(before))
            .map(|lane| lane.wallet.clone());
        let reordered = lanes::moved(&self.lanes, &wallet, target.as_ref());
        let rows = if lanes::displayed(&reordered) != lanes::displayed(&self.lanes) {
            self.move_lane(daemon.clone(), index, before)
        } else {
            Task::none()
        };
        let moves = match &self.graph {
            Some(graph) if dx != 0.0 => edit::moved_positions(
                &self.lane_layout,
                &graph.wallet_items(&wallet),
                Vector::new(dx, 0.0),
                false,
            ),
            _ => Vec::new(),
        };
        let shift = self.commit_move(daemon, PlacementKind::Lanes, moves);
        Task::batch([rows, shift])
    }

    /// Sets the height of the lane `index`, kept until the lanes are reset.
    fn resize_lane(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        index: usize,
        height: f32,
    ) -> Task<Message> {
        let Some(lane) = self.lanes.get(index) else {
            return Task::none();
        };
        self.resized_heights.insert(lane.wallet.clone(), height);
        self.save_rows(daemon)
    }
}

/// Writes `rows`, a row without offset keeping the stored one.
async fn write_rows(
    daemon: Arc<dyn Daemon + Sync + Send>,
    mut rows: Vec<GraphWallet>,
) -> Result<(), Error> {
    let stored = daemon.get_graph_wallets().await?;
    for row in rows.iter_mut().filter(|row| row.offset.is_none()) {
        row.offset = stored
            .iter()
            .find(|stored| stored.wallet == row.wallet)
            .and_then(|stored| stored.offset);
    }
    daemon.update_graph_wallets(&rows).await?;
    Ok(())
}

impl State for MapPanel {
    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        let layout = self.shown_layout();
        let display = self.graph.as_ref().map(|graph| {
            display_state(
                graph,
                &layout,
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
        let theme = Theme::default();
        let wallet_colors: HashMap<WalletKey, Color> = self
            .others
            .iter()
            .map(|(key, store)| (key.clone(), wallet_color(&theme, store.checksum())))
            .collect();
        let wallet_names: HashMap<WalletKey, String> = self
            .others
            .iter()
            .map(|(key, store)| (key.clone(), store.name().to_string()))
            .chain([(WalletKey::Current, self.current_name.clone())])
            .collect();
        let lane_color = |wallet: &WalletKey| {
            wallet_colors
                .get(wallet)
                .copied()
                .unwrap_or(theme.colors.general.accent)
        };
        let lane_ids: HashMap<&WalletKey, u64> = self
            .lanes
            .iter()
            .enumerate()
            .map(|(index, lane)| (&lane.wallet, index as u64))
            .collect();
        let lanes: Vec<Lane> = match &self.graph {
            Some(graph) if self.toggles.lanes => self
                .bands()
                .into_iter()
                .map(|(wallet, band)| Lane {
                    id: lane_ids[&wallet],
                    top: band.top,
                    height: band.height,
                    min_height: lanes::content_height(graph, &self.lane_layout, &wallet),
                    items: graph.wallet_items(&wallet),
                    color: lane_color(&wallet),
                })
                .collect(),
            _ => Vec::new(),
        };
        let handles = self
            .lanes
            .iter()
            .enumerate()
            .map(|(index, lane)| Handle {
                id: index as u64,
                name: wallet_names.get(&lane.wallet).cloned().unwrap_or_default(),
                color: lane_color(&lane.wallet),
                displayed: lane.displayed,
                lane: (lane.displayed && self.toggles.lanes).then_some(index as u64),
            })
            .collect();
        let dashboard = view::full_dashboard(
            &Menu::Map(None),
            cache,
            self.warning.as_ref(),
            view::map::map_view(
                self.graph.as_ref(),
                &layout,
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
                !self.remote,
                &wallet_colors,
                &wallet_names,
                lanes,
                handles,
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
            (_, Some(MapModal::Wallets)) => Some(view::map::wallets_modal(
                &self.listed,
                &self.listed_externals,
                &self.others,
                &self.rescanning,
                self.switching,
            )),
            (_, Some(MapModal::Import(form))) => Some(view::map::import_modal(form)),
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
                self.switching = false;
                self.warning = Some(e);
            }
            Message::MapLoaded(Ok(wallets)) => {
                let loaded = lanes::split_loaded(wallets);
                let before: HashSet<WalletLane> = self.lanes.drain(..).collect();
                self.others = loaded.stores;
                self.lanes = loaded.lanes;
                let graph = TxGraph::new(loaded.txs);
                if self.lanes.iter().cloned().collect::<HashSet<_>>() != before {
                    // Item ids shift and a recorded change may belong to another owner now.
                    self.history.clear();
                    self.selection.clear();
                }
                if self
                    .coin_ui
                    .selected()
                    .iter()
                    .any(|coin| graph.coin(coin).is_none())
                {
                    self.coin_ui.clear_selected();
                }
                let placement = offsets::place_wallets(&graph, loaded.layouts);
                self.layout = placement.layout;
                self.lane_layout = placement.lane_layout;
                self.orders = placement.orders;
                self.offsets = placement.offsets;
                self.lane_heights = placement.heights;
                self.resized_heights = loaded.heights;
                self.hover = None;
                self.tag_highlight = None;
                self.show_on_map = None;
                self.reorder = None;
                self.selection.retain(|id| graph.item(id).is_some());
                // Adding or removing a wallet keeps the camera.
                let fit = if !self.loading || graph.is_empty() {
                    Task::none()
                } else {
                    graph_view::fit(self.graph_id.clone())
                };
                self.graph = Some(graph);
                self.loading = false;
                self.switching = false;
                let landing = self.pending_focus.take().and_then(|focus| {
                    let graph = self.graph.as_ref()?;
                    resolve_focus(graph, &self.shown_layout(), &self.orders, &focus)
                });
                let rows = if placement.placed.is_empty() {
                    Task::none()
                } else {
                    self.save_rows(daemon.clone())
                };
                let save = Task::batch([
                    self.save_layout(daemon, placement.save, placement.remove),
                    rows,
                ]);
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
                if let Some(wallet) = self
                    .label_owner()
                    .filter(|wallet| *wallet != WalletKey::Current)
                {
                    return self.confirm_other_labels(wallet, items);
                }
                return self.forward_label(daemon, message);
            }
            Message::View(view::Message::Label(..)) => return self.forward_label(daemon, message),
            Message::LabelsUpdated(res) => {
                let saved = res.as_ref().ok().cloned();
                let task = self.forward_label(daemon, Message::LabelsUpdated(res));
                if let Some(saved) = saved {
                    self.labels_saved(WalletKey::Current, saved);
                }
                return task;
            }
            Message::MapWalletLabelsSaved(_, Err(e)) => {
                self.warning = Some(e);
                self.pending_label = None;
            }
            Message::MapWalletLabelsSaved(wallet, Ok(saved)) => {
                if let Some(graph) = &mut self.graph {
                    graph.load_wallet_labels(&wallet, &saved);
                }
                let saved_keys = saved.keys().cloned().collect();
                let clear = view::Message::Label(saved_keys, LabelMessage::Cancel);
                let task = self.forward_label(daemon, Message::View(clear));
                self.labels_saved(wallet, saved);
                return task;
            }
            Message::MapWalletsListed(Err(e)) => self.warning = Some(e),
            Message::MapWalletsListed(Ok(wallets)) => {
                self.listed = wallets.others;
                self.listed_externals = wallets.externals;
                self.modal = Some(MapModal::Wallets);
            }
            Message::MapWalletImported(Ok(wallet)) => {
                self.modal = None;
                return self.toggle_wallet(daemon, WalletKey::External(wallet.id));
            }
            Message::MapWalletImported(Err(e)) => match &mut self.modal {
                Some(MapModal::Import(form)) => {
                    form.scanning = false;
                    form.device = None;
                    form.error = Some(e.to_string());
                }
                _ => self.warning = Some(e.into()),
            },
            Message::MapWalletRescanned(id, res) => {
                self.rescanning.remove(&id);
                match res {
                    Ok(wallet) => return self.rescanned(daemon, wallet),
                    Err(e) => self.warning = Some(e.into()),
                }
            }
            Message::MapExternalRemoved(id, res) => match res {
                Ok(()) => {
                    self.listed_externals.retain(|wallet| wallet.id != id);
                    if self.others.contains_key(&WalletKey::External(id)) {
                        return self.load_again(daemon);
                    }
                    self.switching = false;
                }
                Err(e) => {
                    self.switching = false;
                    self.warning = Some(e);
                }
            },
            Message::MapTopologyExported(Err(e)) => self.warning = Some(Error::ImportExport(e)),
            Message::HardwareWallets(message) => {
                if let Some(MapModal::Import(form)) = &mut self.modal {
                    match form.hws.update(message) {
                        Ok(task) => return task.map(Message::HardwareWallets),
                        Err(e) => form.error = Some(e.to_string()),
                    }
                }
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
                MapMessage::Header(HeaderAction::ToggleLanes) => {
                    self.toggles.lanes = !self.toggles.lanes;
                }
                MapMessage::Header(HeaderAction::ToggleGroupClusters) if !self.toggles.lanes => {
                    self.toggles.clusters = !self.toggles.clusters;
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
                MapMessage::Header(HeaderAction::OtherWallets) if !self.remote => {
                    let (network_dir, network, current) = (
                        self.network_dir.clone(),
                        self.network,
                        self.wallet_id.clone(),
                    );
                    return Task::perform(
                        async move {
                            tokio::task::spawn_blocking(move || {
                                Ok(ListedWallets {
                                    others: other_wallets(&network_dir, network, &current)?,
                                    externals: external_wallets(&network_dir),
                                })
                            })
                            .await
                            .map_err(|e| Error::Unexpected(e.to_string()))?
                        },
                        Message::MapWalletsListed,
                    );
                }
                MapMessage::WalletToggled(wallet) => return self.toggle_wallet(daemon, wallet),
                MapMessage::ImportWallet => self.open_import(daemon.as_ref()),
                MapMessage::ImportMode(mode) => {
                    if let Some(form) = self.import_form() {
                        form.mode = mode;
                        form.error = None;
                    }
                }
                MapMessage::ImportName(name) => {
                    if let Some(form) = self.import_form() {
                        form.name.value = name;
                    }
                }
                MapMessage::ImportDescriptor(descriptor) => {
                    if let Some(form) = self.import_form() {
                        form.descriptor.value = descriptor;
                        form.error = None;
                    }
                }
                MapMessage::ImportAccount(account) => {
                    if let Some(form) = self.import_form() {
                        form.account.value = account;
                        form.error = None;
                    }
                }
                MapMessage::ImportElectrum(electrum) => {
                    if let Some(electrum_field) =
                        self.import_form().and_then(|form| form.electrum.as_mut())
                    {
                        electrum_field.value = electrum;
                    }
                }
                MapMessage::ImportConfirm => return self.import_descriptor(daemon.as_ref()),
                MapMessage::ImportDevice(fingerprint) => {
                    return self.import_device(daemon.as_ref(), fingerprint);
                }
                MapMessage::ExternalRescan(id) => {
                    return self.rescan_external(daemon.as_ref(), id);
                }
                MapMessage::ExternalRemove(id) => return self.remove_external(daemon, id),
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
                MapMessage::Key(MapKey::Right) => return self.select_linked(Link::Children),
                MapMessage::Key(MapKey::Left) => return self.select_linked(Link::Parents),
                MapMessage::Key(MapKey::Up) => return self.step_linked(-1),
                MapMessage::Key(MapKey::Down) => return self.step_linked(1),
                MapMessage::Key(MapKey::ExportTopology) => return self.export_topology(),
                MapMessage::ReuseRowSelected(leaf) => {
                    self.modal = None;
                    self.selection.click(leaf);
                    self.tag_highlight = None;
                    self.show_on_map = None;
                    let Some(at) = self.shown_layout().get(&leaf).copied() else {
                        return Task::none();
                    };
                    let target = Rectangle::new(at, Size::new(LEAF_WIDTH, LEAF_HEIGHT));
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
                    let placement = self.placement();
                    let positions = self.positions(placement);
                    let moves = align(graph, positions, &targets, self.toggles.snap)
                        .into_iter()
                        .filter_map(|(id, after)| Some((id, *positions.get(&id)?, after)))
                        .collect();
                    return self.commit_move(daemon, placement, moves);
                }
                MapMessage::Header(HeaderAction::TidyUp) => {
                    return match self.placement() {
                        PlacementKind::Lanes => self.commit_tidy_up(daemon),
                        PlacementKind::Global => {
                            let (start, orders) = (self.layout.clone(), self.orders.clone());
                            self.commit_global(daemon, &start, &orders)
                        }
                    };
                }
                MapMessage::Header(HeaderAction::ResetLayout) => {
                    let reset = match self.placement() {
                        PlacementKind::Lanes => self.commit_lanes_reset(daemon),
                        PlacementKind::Global => {
                            self.commit_global(daemon, &HashMap::new(), &Orders::new())
                        }
                    };
                    return Task::batch([reset, graph_view::fit(self.graph_id.clone())]);
                }
                MapMessage::Graph(event) => match event {
                    GraphEvent::Moved { items, delta } => {
                        let placement = self.placement();
                        // In the lanes a whole wallet moves as its items, kept in its lane.
                        let wallet = self
                            .graph
                            .as_ref()
                            .filter(|_| placement == PlacementKind::Global)
                            .and_then(|graph| edit::dragged_wallet(graph, &items));
                        if let Some(wallet) = wallet {
                            return self.commit_offset(daemon, wallet, delta);
                        }
                        let moves = edit::moved_positions(
                            self.positions(placement),
                            &items,
                            delta,
                            self.toggles.snap,
                        );
                        return self.commit_move(daemon, placement, moves);
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
                        let txid = tx.history().txid;
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
                        let touched = self.apply_layout_change(&change);
                        self.history.record(change);
                        return self.save_layout(daemon, touched, HashMap::new());
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
                    GraphEvent::HandleToggled(id) => return self.toggle_lane(daemon, id as usize),
                    GraphEvent::HandleMoved { id, before } => {
                        return self.move_lane(daemon, id as usize, before.map(|id| id as usize));
                    }
                    GraphEvent::LaneMoved { id, before, dx } => {
                        return self.drag_lane(
                            daemon,
                            id as usize,
                            before.map(|id| id as usize),
                            dx,
                        );
                    }
                    GraphEvent::LaneResized { id, height } => {
                        return self.resize_lane(daemon, id as usize, height);
                    }
                    GraphEvent::SpaceShifted { from_x, dx } => {
                        let placement = self.placement();
                        let moves = edit::space_moves(self.positions(placement), from_x, dx);
                        return self.commit_move(daemon, placement, moves);
                    }
                },
                MapMessage::CloseModal => return self.close_modal(daemon),
                MapMessage::ToggleCoinSelected => {
                    // The selection holds the coins of a single wallet.
                    let other_wallet = self.modal_coin().is_some_and(|coin| {
                        self.coin_ui
                            .selected()
                            .iter()
                            .any(|selected| self.coin_wallet(selected) != self.coin_wallet(&coin))
                    });
                    if other_wallet {
                        self.coin_ui.clear_selected();
                    }
                    self.coin_action(CoinUi::toggle_selected);
                }
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
        let events = event::listen_with(map_event);
        match &self.modal {
            Some(MapModal::Import(form)) if form.mode == ImportMode::SigningDevice => {
                Subscription::batch([events, form.hws.refresh().map(Message::HardwareWallets)])
            }
            _ => events,
        }
    }

    fn reload(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        wallet: Arc<Wallet>,
    ) -> Task<Message> {
        self.current_name = wallet
            .alias
            .clone()
            .filter(|alias| !alias.is_empty())
            .unwrap_or_else(|| wallet.name.clone());
        self.loading = true;
        self.warning = None;
        self.modal = None;
        self.shortcuts_open = false;
        self.tag_popover = None;
        self.labels_edited = LabelsEdited::default();
        self.pending_label = None;
        self.label_changes.clear();
        Task::perform(
            load_map(
                daemon,
                self.network_dir.clone(),
                self.network,
                self.wallet_id.clone(),
            ),
            Message::MapLoaded,
        )
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, path::PathBuf, str::FromStr, sync::Arc};

    use liana::miniscript::bitcoin::{Network, OutPoint, Txid};
    use lianad::commands::{GraphItem as LayoutItem, GraphLayoutEntry};

    use iced::{
        event::Status,
        keyboard::{key::Named, Key, Modifiers},
        Point, Vector,
    };

    use liana_ui::{
        component::panels::map::header::HeaderAction,
        widget::graph_view::{GraphEvent, ItemId, Target},
    };

    use crate::{
        app::{
            cache::Cache,
            message::Message,
            settings::WalletId,
            state::{
                map::{
                    display_row, escape_action,
                    fixture::{self, TwoWallets},
                    focus::ShowOnMap,
                    global, key_action, lanes,
                    lanes::Band,
                    offsets::{self, WalletLayout},
                    split_stored,
                    wallets::WalletKey,
                    EscapeAction, LabelTarget, MapModal, MapPanel, MapWallet, Orders,
                },
                State,
            },
            view::{self, MapKey, MapMessage},
        },
        daemon::{client::Lianad, Daemon},
        dir::LianaDirectory,
        utils::mock,
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
        let stored = split_stored(
            &graph,
            &WalletKey::Current,
            vec![entry(unknown_tx), entry(orphan)],
        );
        assert_eq!(stored.remove, vec![unknown_tx, orphan]);
        assert!(stored.positions.is_empty());
    }

    #[test]
    fn wrong_order_length_is_ignored() {
        let f = fixture::sample_wallet();
        let graph = fixture::current_graph(f.txs, f.coins);
        let txid = f.ids.batch;
        let mut bad = entry(LayoutItem::Tx(txid));
        bad.output_order = Some(vec![0]);
        let stored = split_stored(&graph, &WalletKey::Current, vec![bad]);
        let id = graph.tx_item(graph.tx_index(&txid).unwrap());
        assert_eq!(stored.resave, vec![id]);
        assert!(stored.orders.is_empty());
        assert!(stored.positions.contains_key(&id));
    }

    #[test]
    fn known_entries_are_kept() {
        let f = fixture::sample_wallet();
        let graph = fixture::current_graph(f.txs, f.coins);
        let txid = f.ids.batch;
        let slots = graph.txs()[graph.tx_index(&txid).unwrap()].outputs.len() as u32;
        let order: Vec<u32> = (0..slots).rev().collect();
        let mut kept = entry(LayoutItem::Tx(txid));
        kept.output_order = Some(order.clone());
        let stored = split_stored(&graph, &WalletKey::Current, vec![kept]);
        assert!(stored.remove.is_empty());
        assert!(stored.resave.is_empty());
        assert_eq!(stored.orders[&txid], (None, Some(order)));
        assert_eq!(stored.positions.len(), 1);
    }

    #[test]
    fn click_clears_show_on_map() {
        let mut panel = MapPanel::new(
            LianaDirectory::new(PathBuf::new()),
            Network::Bitcoin,
            WalletId::new("current".to_string(), None),
            false,
        );
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
    fn key_action_export_topology() {
        let ctrl_shift = Modifiers::CTRL | Modifiers::SHIFT;
        for c in ["e", "E"] {
            let export = key_action(&character(c), ctrl_shift, Status::Ignored);
            assert_eq!(export, Some(MapKey::ExportTopology));
            assert_eq!(
                key_action(&character(c), ctrl_shift, Status::Captured),
                None
            );
            assert_eq!(
                key_action(&character(c), Modifiers::CTRL, Status::Ignored),
                None
            );
        }
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
    fn key_action_plain_arrows() {
        let none = Modifiers::empty();
        let arrows = [
            (Named::ArrowRight, MapKey::Right),
            (Named::ArrowLeft, MapKey::Left),
            (Named::ArrowUp, MapKey::Up),
            (Named::ArrowDown, MapKey::Down),
        ];
        for (named, key) in arrows {
            let arrow = Key::Named(named);
            assert_eq!(key_action(&arrow, none, Status::Ignored), Some(key));
            assert_eq!(key_action(&arrow, Modifiers::CTRL, Status::Ignored), None);
            assert_eq!(key_action(&arrow, none, Status::Captured), None);
        }
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

    fn panel() -> MapPanel {
        MapPanel::new(
            LianaDirectory::new(PathBuf::new()),
            Network::Bitcoin,
            WalletId::new("current".to_string(), None),
            false,
        )
    }

    /// A daemon receiving no request: the tasks of the panel are never run.
    fn daemon() -> Arc<dyn Daemon + Sync + Send> {
        Arc::new(Lianad::new(mock::Daemon::new(Vec::new()).run()))
    }

    fn send(panel: &mut MapPanel, message: MapMessage) {
        let _ = panel.update(
            daemon(),
            &Cache::default(),
            Message::View(view::Message::Map(message)),
        );
    }

    fn load(panel: &mut MapPanel, wallets: Vec<MapWallet>) {
        let _ = panel.update(daemon(), &Cache::default(), Message::MapLoaded(Ok(wallets)));
    }

    fn drag(panel: &mut MapPanel, items: Vec<ItemId>, delta: Vector) {
        send(panel, MapMessage::Graph(GraphEvent::Moved { items, delta }));
    }

    /// A panel showing the current wallet and b, laid out by default with the lanes on.
    fn two_wallet_panel() -> (TwoWallets, MapPanel) {
        let mut two = fixture::two_wallets("a", "b");
        let wallets = std::mem::take(&mut two.wallets)
            .into_iter()
            .map(|txs| MapWallet {
                txs,
                layout: WalletLayout::default(),
                store: None,
                lane: None,
                displayed: true,
                lane_height: None,
            })
            .collect();
        let mut panel = panel();
        load(&mut panel, wallets);
        assert!(panel.toggles.lanes);
        (two, panel)
    }

    fn item(panel: &MapPanel, txid: &Txid) -> ItemId {
        let graph = panel.graph.as_ref().unwrap();
        graph.tx_item(graph.tx_index(txid).unwrap())
    }

    #[test]
    fn placements_are_moved_and_undone_independently() {
        let (two, mut panel) = two_wallet_panel();
        let funding = item(&panel, &two.funding);
        let (global, lane) = (panel.layout.clone(), panel.lane_layout.clone());
        let delta = Vector::new(48.0, 24.0);

        drag(&mut panel, vec![funding], delta);
        assert_eq!(panel.lane_layout[&funding], lane[&funding] + delta);
        assert_eq!(panel.layout, global);
        let lane_moved = panel.lane_layout.clone();

        // Switching the lanes off shows the global positions, untouched.
        send(&mut panel, MapMessage::Header(HeaderAction::ToggleLanes));
        assert_eq!(panel.layout, global);
        assert_eq!(panel.lane_layout, lane_moved);
        assert_eq!(*panel.shown_layout(), global);

        drag(&mut panel, vec![funding], delta * 2.0);
        assert_eq!(panel.layout[&funding], global[&funding] + delta * 2.0);
        assert_eq!(panel.lane_layout, lane_moved);
        let global_moved = panel.layout.clone();

        send(&mut panel, MapMessage::Header(HeaderAction::Undo));
        assert_eq!(panel.layout, global);
        assert_eq!(panel.lane_layout, lane_moved);

        // The history is kept: the lane move is undone with the lanes off too.
        send(&mut panel, MapMessage::Header(HeaderAction::Undo));
        assert_eq!(panel.lane_layout, lane);
        assert_eq!(panel.layout, global);

        send(&mut panel, MapMessage::Header(HeaderAction::ToggleLanes));
        send(&mut panel, MapMessage::Header(HeaderAction::Redo));
        assert_eq!(panel.lane_layout, lane_moved);
        assert_eq!(panel.layout, global);
        send(&mut panel, MapMessage::Header(HeaderAction::Redo));
        assert_eq!(panel.layout, global_moved);
        assert_eq!(panel.lane_layout, lane_moved);
    }

    #[test]
    fn whole_wallet_drag_moves_the_offset_only_with_the_lanes_off() {
        let (two, mut panel) = two_wallet_panel();
        let graph = panel.graph.as_ref().unwrap();
        let b_items = graph.wallet_items(&two.b);
        let (global, lane, offsets) = (
            panel.layout.clone(),
            panel.lane_layout.clone(),
            panel.offsets.clone(),
        );
        let delta = Vector::new(48.0, 24.0);

        drag(&mut panel, b_items.clone(), delta);
        assert_eq!(panel.offsets, offsets);
        assert_eq!(panel.layout, global);
        for id in &b_items {
            assert_eq!(panel.lane_layout[id], lane[id] + delta);
        }

        send(&mut panel, MapMessage::Header(HeaderAction::ToggleLanes));
        let lane_moved = panel.lane_layout.clone();
        drag(&mut panel, b_items.clone(), delta);
        assert_eq!(
            panel.offsets.get(&two.b),
            Some(offsets.get(&two.b).unwrap() + delta)
        );
        for id in &b_items {
            assert_eq!(panel.layout[id], global[id] + delta);
        }
        assert_eq!(panel.lane_layout, lane_moved);
    }

    #[test]
    fn handle_moved_down_reorders_the_lanes() {
        let (two, mut panel) = two_wallet_panel();
        send(
            &mut panel,
            MapMessage::Graph(GraphEvent::HandleMoved {
                id: 0,
                before: Some(1),
            }),
        );
        assert_eq!(
            lanes::displayed(&panel.lanes),
            vec![WalletKey::Current, two.b.clone()]
        );

        send(
            &mut panel,
            MapMessage::Graph(GraphEvent::HandleMoved {
                id: 0,
                before: None,
            }),
        );
        assert_eq!(
            lanes::displayed(&panel.lanes),
            vec![two.b.clone(), WalletKey::Current]
        );
        let b_height = panel.lane_heights[&two.b];
        assert_eq!(panel.bands()[1].1.top, b_height);
    }

    #[test]
    fn lane_order_and_hiding_only_move_the_lane_tops() {
        let (two, mut panel) = two_wallet_panel();
        let (global, lane, offsets) = (
            panel.layout.clone(),
            panel.lane_layout.clone(),
            panel.offsets.clone(),
        );
        let current_height = panel.lane_heights[&WalletKey::Current];
        let b_height = panel.lane_heights[&two.b];
        assert_eq!(
            panel.bands(),
            vec![
                (
                    WalletKey::Current,
                    Band {
                        top: 0.0,
                        height: current_height
                    }
                ),
                (
                    two.b.clone(),
                    Band {
                        top: current_height,
                        height: b_height
                    }
                ),
            ]
        );

        send(
            &mut panel,
            MapMessage::Graph(GraphEvent::HandleMoved {
                id: 1,
                before: Some(0),
            }),
        );
        assert_eq!(
            lanes::displayed(&panel.lanes),
            vec![two.b.clone(), WalletKey::Current]
        );
        assert_eq!(
            panel.bands(),
            vec![
                (
                    two.b.clone(),
                    Band {
                        top: 0.0,
                        height: b_height
                    }
                ),
                (
                    WalletKey::Current,
                    Band {
                        top: b_height,
                        height: current_height
                    }
                ),
            ]
        );
        assert_eq!(panel.layout, global);
        assert_eq!(panel.lane_layout, lane);
        assert_eq!(panel.offsets, offsets);
        assert!(!panel.history.can_undo());

        // b hidden: the map is loaded again from the stored layout without it.
        let graph = panel.graph.as_ref().unwrap();
        let current_items = graph.wallet_items(&WalletKey::Current);
        let by_item = |positions: &HashMap<ItemId, Point>| -> Vec<(LayoutItem, Point)> {
            current_items
                .iter()
                .map(|id| (graph.graph_item(*id).unwrap(), positions[id]))
                .collect()
        };
        let (global, lane) = (by_item(&global), by_item(&lane));
        let mut entries = offsets::local_entries(
            graph,
            &panel.layout,
            &panel.lane_layout,
            &panel.orders,
            &panel.offsets,
            current_items.clone(),
        );
        let wallets = std::mem::take(&mut fixture::two_wallets("a", "b").wallets)
            .into_iter()
            .map(|txs| {
                let current = txs.key == WalletKey::Current;
                MapWallet {
                    layout: WalletLayout {
                        entries: entries.remove(&txs.key).unwrap_or_default(),
                        offset: panel.offsets.get(&txs.key),
                    },
                    lane: Some(if current { 1 } else { 0 }),
                    displayed: current,
                    lane_height: None,
                    store: None,
                    txs,
                }
            })
            .collect();
        load(&mut panel, wallets);
        let bands = panel.bands();
        assert_eq!(bands.len(), 1);
        assert_eq!((&bands[0].0, bands[0].1.top), (&WalletKey::Current, 0.0));
        let graph = panel.graph.as_ref().unwrap();
        let at = |positions: &HashMap<ItemId, Point>, item: &LayoutItem| {
            positions[&graph.item_id(item).unwrap()]
        };
        for (item, p) in &global {
            assert_eq!(at(&panel.layout, item), *p);
        }
        for (item, p) in &lane {
            assert_eq!(at(&panel.lane_layout, item), *p);
        }
        assert_eq!(
            panel.offsets.get(&WalletKey::Current),
            offsets.get(&WalletKey::Current)
        );
    }

    #[test]
    fn leaf_label_opens_on_double_click_only() {
        let (_, mut panel) = two_wallet_panel();
        let graph = panel.graph.as_ref().unwrap();
        let index = graph.leaves().iter().position(|leaf| !leaf.reused).unwrap();
        let leaf = graph.leaf_item(index);

        send(
            &mut panel,
            MapMessage::Graph(GraphEvent::Click {
                target: Target::Item(leaf),
                modifiers: Modifiers::empty(),
            }),
        );
        assert!(panel.modal.is_none());
        assert!(panel.selection.items().contains(&leaf));

        send(
            &mut panel,
            MapMessage::Graph(GraphEvent::DoubleClick {
                target: Target::Item(leaf),
            }),
        );
        assert!(matches!(
            panel.modal,
            Some(MapModal::Label(LabelTarget::Leaf(opened))) if opened == index
        ));
    }

    #[test]
    fn lane_drag_reorders_the_lanes_and_moves_across() {
        let (two, mut panel) = two_wallet_panel();
        let (global, lane, offsets) = (
            panel.layout.clone(),
            panel.lane_layout.clone(),
            panel.offsets.clone(),
        );
        let b_items = panel.graph.as_ref().unwrap().wallet_items(&two.b);
        let across = Vector::new(48.0, 0.0);

        send(
            &mut panel,
            MapMessage::Graph(GraphEvent::LaneMoved {
                id: 1,
                before: Some(0),
                dx: across.x,
            }),
        );
        assert_eq!(
            lanes::displayed(&panel.lanes),
            vec![two.b.clone(), WalletKey::Current]
        );
        for (id, p) in &lane {
            let expected = if b_items.contains(id) {
                *p + across
            } else {
                *p
            };
            assert_eq!(panel.lane_layout[id], expected);
        }
        assert_eq!(panel.layout, global);
        assert_eq!(panel.offsets, offsets);

        // Undo moves the lane back across, its slot is display state.
        send(&mut panel, MapMessage::Header(HeaderAction::Undo));
        assert_eq!(panel.lane_layout, lane);
        assert_eq!(
            lanes::displayed(&panel.lanes),
            vec![two.b.clone(), WalletKey::Current]
        );

        // Dropped in its own slot, the lane only moves across.
        send(
            &mut panel,
            MapMessage::Graph(GraphEvent::LaneMoved {
                id: 0,
                before: Some(1),
                dx: -across.x,
            }),
        );
        assert_eq!(
            lanes::displayed(&panel.lanes),
            vec![two.b.clone(), WalletKey::Current]
        );
        for id in &b_items {
            assert_eq!(panel.lane_layout[id], lane[id] - across);
        }
        assert_eq!(panel.layout, global);
    }

    #[test]
    fn lane_resize_moves_the_lanes_below_until_reset() {
        let (two, mut panel) = two_wallet_panel();
        let lane = panel.lane_layout.clone();
        let current_height = panel.lane_heights[&WalletKey::Current];
        let b_height = panel.lane_heights[&two.b];
        let resized = current_height + 240.0;

        send(
            &mut panel,
            MapMessage::Graph(GraphEvent::LaneResized {
                id: 0,
                height: resized,
            }),
        );
        assert_eq!(
            panel.bands(),
            vec![
                (
                    WalletKey::Current,
                    Band {
                        top: 0.0,
                        height: resized
                    }
                ),
                (
                    two.b.clone(),
                    Band {
                        top: resized,
                        height: b_height
                    }
                ),
            ]
        );
        let rows = lanes::rows(&panel.lanes, &panel.offsets, &panel.resized_heights);
        assert_eq!(rows[0].lane_height, Some(f64::from(resized)));
        assert_eq!(rows[1].lane_height, None);
        assert_eq!(panel.lane_layout, lane);
        assert!(!panel.history.can_undo());

        send(&mut panel, MapMessage::Header(HeaderAction::ResetLayout));
        assert!(panel.resized_heights.is_empty());
        assert_eq!(panel.bands()[1].1.top, current_height);
    }

    #[test]
    fn space_shift_moves_the_items_from_its_line() {
        let (two, mut panel) = two_wallet_panel();
        let (global, lane) = (panel.layout.clone(), panel.lane_layout.clone());
        let from_x = lane[&item(&panel, &two.payment)].x;
        let across = Vector::new(120.0, 0.0);

        send(
            &mut panel,
            MapMessage::Graph(GraphEvent::SpaceShifted {
                from_x,
                dx: across.x,
            }),
        );
        assert!(lane.values().any(|p| p.x < from_x));
        for (id, p) in &lane {
            let expected = if p.x >= from_x { *p + across } else { *p };
            assert_eq!(panel.lane_layout[id], expected);
        }
        assert_eq!(panel.layout, global);

        send(&mut panel, MapMessage::Header(HeaderAction::Undo));
        assert_eq!(panel.lane_layout, lane);
    }

    #[test]
    fn reset_only_touches_the_shown_placement() {
        let (two, mut panel) = two_wallet_panel();
        let funding = item(&panel, &two.funding);
        let wallets = [WalletKey::Current, two.b.clone()];
        let delta = Vector::new(48.0, 24.0);
        drag(&mut panel, vec![funding], delta);
        send(&mut panel, MapMessage::Header(HeaderAction::ToggleLanes));
        drag(&mut panel, vec![funding], delta);
        let b_items = panel.graph.as_ref().unwrap().wallet_items(&two.b);
        drag(&mut panel, b_items, delta);
        send(&mut panel, MapMessage::Header(HeaderAction::ToggleLanes));
        let (global, offsets) = (panel.layout.clone(), panel.offsets.clone());

        send(&mut panel, MapMessage::Header(HeaderAction::ResetLayout));
        let graph = panel.graph.as_ref().unwrap();
        let lane_reset = lanes::reset(graph, &wallets).positions;
        assert_eq!(panel.lane_layout, lane_reset);
        assert_eq!(panel.layout, global);
        assert_eq!(panel.offsets, offsets);

        send(&mut panel, MapMessage::Header(HeaderAction::ToggleLanes));
        send(&mut panel, MapMessage::Header(HeaderAction::ResetLayout));
        let graph = panel.graph.as_ref().unwrap();
        let global_reset = global::place(
            graph,
            &wallets,
            &HashMap::new(),
            &Orders::new(),
            false,
            true,
        );
        assert_eq!(panel.layout, global_reset.positions);
        assert_eq!(panel.offsets, global_reset.offsets);
        assert_eq!(panel.lane_layout, lane_reset);

        // Undoing the global reset leaves the lanes alone.
        send(&mut panel, MapMessage::Header(HeaderAction::Undo));
        assert_eq!(panel.layout, global);
        assert_eq!(panel.offsets, offsets);
        assert_eq!(panel.lane_layout, lane_reset);
    }

    #[test]
    fn tidy_up_is_one_change_of_the_lanes() {
        let (two, mut panel) = two_wallet_panel();
        let payment = item(&panel, &two.payment);
        assert_eq!(panel.lane_layout[&payment], Point::new(1392.0, 0.0));
        drag(&mut panel, vec![payment], Vector::new(48.0, 360.0));
        let dragged = panel.lane_layout.clone();

        send(&mut panel, MapMessage::Header(HeaderAction::TidyUp));
        // Back on the row of its funding parent, at the x it was dragged to.
        assert_eq!(panel.lane_layout[&payment], Point::new(1440.0, 0.0));
        assert_eq!(panel.lane_heights[&WalletKey::Current], 252.0);
        let tidy = panel.lane_layout.clone();

        send(&mut panel, MapMessage::Header(HeaderAction::Undo));
        assert_eq!(panel.lane_layout, dragged);
        send(&mut panel, MapMessage::Header(HeaderAction::Redo));
        assert_eq!(panel.lane_layout, tidy);
    }

    #[test]
    fn tidy_up_with_the_lanes_off_is_one_global_change() {
        let (two, mut panel) = two_wallet_panel();
        let wallets = [WalletKey::Current, two.b.clone()];
        let payment = item(&panel, &two.payment);
        send(&mut panel, MapMessage::Header(HeaderAction::ToggleLanes));
        drag(&mut panel, vec![payment], Vector::new(48.0, 360.0));
        let (dragged, offsets, lane) = (
            panel.layout.clone(),
            panel.offsets.clone(),
            panel.lane_layout.clone(),
        );

        send(&mut panel, MapMessage::Header(HeaderAction::TidyUp));
        let graph = panel.graph.as_ref().unwrap();
        let tidy = global::place(graph, &wallets, &dragged, &Orders::new(), false, true);
        assert_eq!(panel.layout, tidy.positions);
        assert_eq!(panel.offsets, tidy.offsets);
        // The payment lines its input up with the funding output again, one column right: no
        // leaf between them, only the clearance.
        assert_eq!(panel.layout[&payment], Point::new(720.0, 0.0));
        assert_eq!(panel.lane_layout, lane);

        send(&mut panel, MapMessage::Header(HeaderAction::Undo));
        assert_eq!(panel.layout, dragged);
        assert_eq!(panel.offsets, offsets);
        send(&mut panel, MapMessage::Header(HeaderAction::Redo));
        assert_eq!(panel.layout, tidy.positions);
    }

    #[test]
    fn group_by_cluster_toggles_only_with_the_lanes_off() {
        let (_, mut panel) = two_wallet_panel();
        let toggle = MapMessage::Header(HeaderAction::ToggleGroupClusters);
        send(&mut panel, toggle.clone());
        assert!(!panel.toggles.clusters);

        send(&mut panel, MapMessage::Header(HeaderAction::ToggleLanes));
        send(&mut panel, toggle.clone());
        assert!(panel.toggles.clusters);
        assert!(!panel.history.can_undo());
        send(&mut panel, toggle);
        assert!(!panel.toggles.clusters);
    }

    #[test]
    fn reset_and_tidy_up_by_cluster_are_one_change_each() {
        let (two, mut panel) = two_wallet_panel();
        let wallets = [WalletKey::Current, two.b.clone()];
        let payment = item(&panel, &two.payment);
        send(&mut panel, MapMessage::Header(HeaderAction::ToggleLanes));
        send(
            &mut panel,
            MapMessage::Header(HeaderAction::ToggleGroupClusters),
        );
        drag(&mut panel, vec![payment], Vector::new(48.0, 360.0));
        let (dragged, offsets) = (panel.layout.clone(), panel.offsets.clone());

        send(&mut panel, MapMessage::Header(HeaderAction::ResetLayout));
        let graph = panel.graph.as_ref().unwrap();
        let reset = global::place(graph, &wallets, &HashMap::new(), &Orders::new(), true, true);
        assert_eq!(panel.layout, reset.positions);
        assert_eq!(panel.offsets, reset.offsets);
        assert_eq!(panel.offsets.get(&two.b), Some(Vector::ZERO));
        send(&mut panel, MapMessage::Header(HeaderAction::Undo));
        assert_eq!(panel.layout, dragged);
        assert_eq!(panel.offsets, offsets);

        send(&mut panel, MapMessage::Header(HeaderAction::TidyUp));
        let graph = panel.graph.as_ref().unwrap();
        let tidy = global::place(graph, &wallets, &dragged, &Orders::new(), true, true);
        assert_eq!(panel.layout, tidy.positions);
        assert_eq!(panel.offsets, tidy.offsets);
        send(&mut panel, MapMessage::Header(HeaderAction::Undo));
        assert_eq!(panel.layout, dragged);
        assert_eq!(panel.offsets, offsets);
        send(&mut panel, MapMessage::Header(HeaderAction::Redo));
        assert_eq!(panel.layout, tidy.positions);
    }
}
