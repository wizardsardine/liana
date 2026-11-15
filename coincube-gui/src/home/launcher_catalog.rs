//! The Launcher's production List body: every local Cube of the selected
//! network in one column, the Cubes registered in Connect below them, and a
//! read-only details panel for the selected local Cube.
//!
//! This is the R05a List fallback, not the full Launcher. It renders only what
//! Home already knows before unlock: no retained-open state, wallet inventory,
//! PIN presence or balances are inferred. Opening, renaming and deleting go
//! through the existing chain-bound catalog messages, so `Home::update`'s
//! normalization and the GUI unlock lifecycle stay the only route.
//!
//! Selection and search are transient presentation state owned by this Home.
//! They are never persisted and never change the authoritative lists, quotas
//! or recoverability.

use crate::app::view::responsive;
use coincube_ui::locale::copy as localized_copy;
use coincube_ui::localized_format;
use iced::{
    advanced::widget::Id,
    event,
    keyboard::{self, key},
    mouse,
    widget::{button as iced_button, container, rule, scrollable, text::Wrapping, Space},
    Alignment, Length, Size, Task,
};
use std::time::{Duration, Instant};

use coincube_ui::{
    component::{card, text::*},
    icon, theme,
    widget::{action::Action, text_input, Column, Container, Element, Row, TextInput},
};

use super::{
    catalog_display_name, vault_subtitle, CubeCatalogKey, DeleteCubeMessage, Home, HomeSection,
    Message, RemoteCube, State, ViewMessage,
};
use crate::{
    app::settings::{self, CubeConnectState, CubeSettings},
    chain::{ChainId, ChainIdExt},
    feature_flags,
};

/// The selected Cube's details panel width when there is room beside the list.
pub(super) const DETAILS_WIDTH: f32 = 440.0;
/// Narrowest list kept beside the details panel; below this the panel stacks
/// above the list instead of squeezing it. With the body's 24px gutters the
/// switch happens at a 1024px body (a 1072px window): 1024 stays stacked and
/// 1440 is side by side.
const LIST_MIN_WIDTH: f32 = 560.0;
const PANEL_GAP: f32 = 24.0;
/// Height of the compact Launcher top bar.
#[cfg(test)]
const TOP_BAR_HEIGHT: f32 = 64.0;
/// Horizontal gutter of the top bar, the body and the dock.
const GUTTER: f32 = 24.0;
/// At the compact window size the columns become labeled, stacked facts.
const TABLE_MIN_VIEW_WIDTH: f32 = 1100.0;
/// Fixed slots plus gaps and row padding use 660px; reserve 180px for the
/// Cube name when details occupy the right side of the body.
const TABLE_MIN_LIST_WIDTH: f32 = 840.0;
const AVATAR_WIDTH: f32 = 44.0;
const NETWORK_WIDTH: f32 = 145.0;
const WALLETS_WIDTH: f32 = 160.0;
const BACKUP_WIDTH: f32 = 145.0;
const ACTION_WIDTH: f32 = 72.0;
const CELL_GAP: f32 = 14.0;

/// The one scrollable that owns the visible List's vertical overflow: the
/// Fill-height body between the top bar and the dock. Neither of those
/// scrolls. The ID names that viewport for a later shell reveal-on-focus
/// integration; reveal itself is not implemented here.
pub(super) const CATALOG_SCROLL_ID: &str = "tenshu.launcher.catalog.scroll";

/// Whether `Home::view` renders the Launcher catalog, including its empty state.
pub(super) fn shows_catalog(home: &Home) -> bool {
    match &home.state {
        State::Cubes { create_cube, .. } => !*create_cube,
        State::NoCube { .. } => !home.showing_create_form(),
        _ => false,
    }
}

/// Whether the List is what the user actually sees: the Cubes section, an
/// available network, and a list body rather than a form or another screen.
pub(super) fn catalog_visible(home: &Home) -> bool {
    matches!(home.active_section, HomeSection::Cubes)
        && home.chain_availability(home.network).is_available()
        && shows_catalog(home)
}

/// Transient, in-memory presentation state for the catalog. Boxed in `Home`
/// so the Tab state stays small.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LauncherMode {
    #[default]
    Cubes,
    List,
}

pub(super) const FLOOR_PAGE_SIZE: usize = 4;
/// Six Cubes a page once the stage is wide enough to hold them at full
/// size with the page arrows clear of the outer slots: 6 x 232 + 5 x 40 =
/// 1592px of row plus the 80px arrow gutter each side, rounded up.
pub(super) const FLOOR_WIDE_PAGE_SIZE: usize = 6;
pub(super) const FLOOR_WIDE_MIN_WIDTH: f32 = 1760.0;

/// Cubes per floor page for a stage `width` pixels wide.
pub(super) fn floor_page_size(width: f32) -> usize {
    if width >= FLOOR_WIDE_MIN_WIDTH {
        FLOOR_WIDE_PAGE_SIZE
    } else {
        FLOOR_PAGE_SIZE
    }
}

/// Pages for `cubes` Cubes at `per_page` a page; an empty floor is one page.
pub(super) fn floor_pages(cubes: usize, per_page: usize) -> usize {
    cubes.div_ceil(per_page.max(1)).max(1)
}

/// The page that shows the Cube at `anchor` when pages hold `per_page`.
pub(super) fn floor_page_keeping(anchor: usize, per_page: usize, cubes: usize) -> usize {
    (anchor / per_page.max(1)).min(floor_pages(cubes, per_page) - 1)
}

/// Only presentation data from the page that contained the selected Cube.
/// The unlock state must not retain Home, Connect credentials, or wallet data.
#[derive(Debug, Clone)]
pub(crate) struct LauncherSceneContext {
    pub selected: CubeCatalogKey,
    pub page: usize,
    pub selected_slot: usize,
    pub backup: &'static str,
    pub connect: String,
    pub wallets: String,
    pub peers: Vec<LauncherScenePeer>,
    /// Floor pages, for the arrows and dots that stay up under the face.
    pub pages: usize,
    /// The Launcher's bars, redrawn quiet behind the unlock face.
    pub chrome: super::launcher_chrome::ChromeSnapshot,
}

#[derive(Debug, Clone)]
pub(crate) struct LauncherScenePeer {
    pub key: CubeCatalogKey,
    pub name: String,
    pub status: &'static str,
    pub marks: super::launcher_scene::CubeMarks,
}

/// The prototype's Key card: hover peeks, a click pins, a second click or
/// Escape shuts it until the pointer leaves, so it does not reopen at once.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum KeyMode {
    #[default]
    Closed,
    Peek,
    Pinned,
    Shut,
}

/// The two hover regions that make up the prototype's one `.keywrap`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyPart {
    Button,
    /// The card and the 8px gap under it.
    Card,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct KeyCard {
    pub mode: KeyMode,
    /// Over the button or the card. They are separate Iced areas, and one
    /// pointer move reports entering one before leaving the other, so each
    /// keeps its own flag and the wrap counts as hovered while either is.
    pub over_button: bool,
    pub over_card: bool,
}

impl KeyCard {
    fn hovered(&self) -> bool {
        self.over_button || self.over_card
    }

    fn hover(&mut self, part: KeyPart, over: bool) {
        match part {
            KeyPart::Button => self.over_button = over,
            KeyPart::Card => self.over_card = over,
        }
        let hovered = self.hovered();
        self.mode = match (self.mode, hovered) {
            (KeyMode::Closed, true) => KeyMode::Peek,
            (KeyMode::Peek | KeyMode::Shut, false) => KeyMode::Closed,
            (mode, _) => mode,
        };
        self.settle();
    }

    fn shut(&mut self) {
        self.mode = if self.hovered() {
            KeyMode::Shut
        } else {
            KeyMode::Closed
        };
        self.settle();
    }

    fn click(&mut self) {
        if self.mode == KeyMode::Pinned {
            self.shut();
        } else {
            self.mode = KeyMode::Pinned;
        }
    }

    /// A hidden card is no longer under the pointer, and Iced sends no exit
    /// for an area it stops drawing. Leaving the wrap that way clears a shut
    /// card too, as the prototype's `keyLeave` does.
    fn settle(&mut self) {
        if !matches!(self.mode, KeyMode::Peek | KeyMode::Pinned) {
            self.over_card = false;
        }
        if self.mode == KeyMode::Shut && !self.hovered() {
            self.mode = KeyMode::Closed;
        }
    }
}

impl CatalogState {
    /// Leaving the Launcher screen closes its popovers; Iced sends no hover
    /// exit for areas that are no longer drawn.
    pub(super) fn dismiss_popovers(&mut self) {
        self.key = KeyCard::default();
        self.network_menu = false;
        self.plan_menu = false;
    }

    pub(super) fn key_open(&self) -> bool {
        matches!(self.key.mode, KeyMode::Peek | KeyMode::Pinned)
    }
}

#[derive(Debug, Clone, Default)]
pub(super) struct CatalogState {
    /// The selected local Cube by persisted identity, never by list position.
    /// It is re-resolved against the current list on every render and update.
    pub(super) selected: Option<CubeCatalogKey>,
    /// Bounded animation state. The scene reads wall time only while this is
    /// present, and the subscription stops after its final frame.
    pub(super) motion: Option<super::launcher_scene::Motion>,
    /// Set only by the actual floor face's Open action, then consumed by the
    /// tab handoff. A List fallback can never inherit floor provenance.
    pub(super) open_from_floor: bool,
    /// The dock's Key card: open while hovered, or pinned by a click.
    pub(super) key: KeyCard,
    /// The top bar's network menu is open.
    pub(super) network_menu: bool,
    /// The dock's plan menu is open.
    pub(super) plan_menu: bool,
    /// Which networks' Cubes the Launcher shows (P-Launcher's chips and
    /// Network menu). Presentation only; it never changes a Cube's network.
    pub(super) filter: NetworkFilter,
    /// A Cube picked or opened on another network, replayed once that
    /// network is current and its list is checked.
    pub(super) pending_select: Option<(CubeCatalogKey, PendingIntent)>,
    /// Each floor Open gets a distinct origin carried by its queued Run.
    /// Clearing open_from_floor invalidates that origin even if the selected
    /// identity is later selected again.
    pub(super) floor_open_generation: u64,
    /// Name filter over the currently listed Cubes.
    pub(super) query: String,
    /// The floor pages only visible local Cubes, never Connect-only records.
    pub(super) page: usize,
    pub(super) mode: LauncherMode,
    /// Trackpad events arrive as many small deltas. A turn consumes one
    /// gesture, without making a short sideways touch flip the floor.
    pub(super) wheel_pixels: f32,
    pub(super) last_wheel_input: Option<Instant>,
    pub(super) last_wheel_page: Option<Instant>,
    pub(super) wheel_consumed: bool,
    /// The floor's width as last laid out, reported by its sensor. Paging
    /// messages use it so they count the same Cubes a page as the screen.
    pub(super) stage_width: Option<f32>,
    /// Resting Cubes under the pointer, or settling after it left.
    pub(super) hover: Vec<FloorHover>,
    /// Network activity in the sky above the Cubes, and its dock menu.
    pub(super) activity: super::launcher_activity::ActivityState,
}

/// A resting Cube floats while the pointer is over it (see
/// [`super::launcher_scene::bob`]) and settles once the pointer leaves.
#[derive(Debug, Clone)]
pub(super) struct FloorHover {
    pub(super) key: CubeCatalogKey,
    entered: Instant,
    /// When the pointer left, and the lift at that moment.
    left: Option<(Instant, f32)>,
}

impl FloorHover {
    pub(super) fn lift(&self, now: Instant) -> f32 {
        match self.left {
            None => super::launcher_scene::bob(now.saturating_duration_since(self.entered)),
            Some((at, from)) => {
                super::launcher_scene::settle(from, now.saturating_duration_since(at))
            }
        }
    }

    fn settled(&self, now: Instant) -> bool {
        self.left.is_some_and(|(at, _)| {
            now.saturating_duration_since(at) >= super::launcher_scene::BOB_SETTLE
        })
    }
}

impl CatalogState {
    /// Cubes per floor page at the measured width; four until measured.
    pub(super) fn page_size(&self) -> usize {
        self.stage_width.map_or(FLOOR_PAGE_SIZE, floor_page_size)
    }

    /// The scene needs frames: a turn is running or a Cube is floating.
    pub(super) fn animating(&self) -> bool {
        self.motion.is_some() || !self.hover.is_empty()
    }

    /// How far a resting Cube floats at `now`.
    pub(super) fn lift(&self, key: &CubeCatalogKey, now: Instant) -> f32 {
        self.hover
            .iter()
            .find(|hover| &hover.key == key)
            .map_or(0.0, |hover| hover.lift(now))
    }

    pub(super) fn hover(&mut self, key: CubeCatalogKey, over: bool, now: Instant) {
        let lift = self.lift(&key, now);
        self.hover.retain(|hover| hover.key != key);
        if over {
            // Re-entering a settling Cube picks the float up where it is.
            let phase = (1.0 - 2.0 * lift / super::launcher_scene::BOB_HEIGHT)
                .clamp(-1.0, 1.0)
                .acos();
            let into = super::launcher_scene::BOB_PERIOD.mul_f32(phase / std::f32::consts::TAU);
            self.hover.push(FloorHover {
                key,
                entered: now.checked_sub(into).unwrap_or(now),
                left: None,
            });
        } else if lift > 0.0 {
            self.hover.push(FloorHover {
                key,
                entered: now,
                left: Some((now, lift)),
            });
        }
    }
}

const WHEEL_PAGE_THRESHOLD: f32 = 24.0;
const WHEEL_GESTURE_GAP: Duration = Duration::from_millis(250);
const WHEEL_PAGE_COOLDOWN: Duration = Duration::from_millis(600);

/// What a pick on another network asked for, replayed after the switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingIntent {
    Select,
    /// List or floor Open: unlock once the checked list confirms the Cube.
    Open,
}

/// P-Launcher's network filter: every listed network, the live or test
/// ones (Developer mode only), or one network from the Network menu.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum NetworkFilter {
    #[default]
    All,
    Live,
    Test,
    Network(ChainId),
}

impl NetworkFilter {
    pub(super) fn matches(self, chain: ChainId) -> bool {
        let test = super::launcher_coin::is_test_network(chain);
        match self {
            NetworkFilter::All => true,
            NetworkFilter::Live => !test,
            NetworkFilter::Test => test,
            NetworkFilter::Network(only) => chain == only,
        }
    }
}

/// The canvas's network order: the two mainnets, then the test networks.
pub(super) fn network_rank(chain: ChainId) -> u8 {
    match chain {
        ChainId::Bitcoin => 0,
        ChainId::BitcoinBlake2b => 1,
        ChainId::Testnet4 => 2,
        ChainId::Testnet => 3,
        ChainId::Signet => 4,
        ChainId::Regtest => 5,
        ChainId::BitcoinBlake2bTestnet4 => 6,
    }
}

impl Home {
    /// The filter as it applies now. Without Developer mode there are no
    /// test networks, so Live, Test and a test network fall back to All, as
    /// do networks no longer listed.
    pub(super) fn effective_filter(&self) -> NetworkFilter {
        match self.catalog.filter {
            NetworkFilter::Live | NetworkFilter::Test if !self.developer_mode => NetworkFilter::All,
            NetworkFilter::Network(chain) if !self.displayed_networks.contains(&chain) => {
                NetworkFilter::All
            }
            filter => filter,
        }
    }

    /// The network a filter makes current, if it must change: the one
    /// network it names, or for Live and Test the first listed network they
    /// show when the current one is hidden. `None` keeps the current one.
    pub(super) fn filter_target(&self, filter: NetworkFilter) -> Option<ChainId> {
        let target = match filter {
            NetworkFilter::All => return None,
            NetworkFilter::Network(chain) => chain,
            NetworkFilter::Live | NetworkFilter::Test if filter.matches(self.network) => {
                return None
            }
            NetworkFilter::Live | NetworkFilter::Test => self
                .ordered_networks()
                .into_iter()
                .find(|chain| filter.matches(*chain))?,
        };
        (target != self.network).then_some(target)
    }

    /// Listed networks in the canvas's order, plus the network of the
    /// checked list when it is not listed (it is always shown).
    pub(super) fn ordered_networks(&self) -> Vec<ChainId> {
        let mut networks = self.displayed_networks.clone();
        if let State::Cubes { source, .. } = &self.state {
            if !networks.contains(source) {
                networks.push(*source);
            }
        }
        networks.sort_by_key(|chain| network_rank(*chain));
        networks
    }

    pub(super) fn developer_mode(&self) -> bool {
        self.developer_mode
    }

    /// A Cube shown from another network's read-only list cannot be acted on
    /// in place: make its network current, and select it once that network's
    /// list is checked (see `Message::Checked`).
    pub(super) fn select_on_other_network(
        &mut self,
        key: &CubeCatalogKey,
        intent: PendingIntent,
    ) -> Option<Task<Message>> {
        if key.chain == self.network {
            // While the switch to this network is still being checked, the
            // latest pick or Open wins over the one that started the switch.
            let switching = self.catalog.pending_select.is_some()
                && !matches!(&self.state, State::Cubes { source, .. } if *source == self.network);
            if switching && !key.cube_id.is_empty() {
                self.catalog.pending_select = Some((key.clone(), intent));
                return Some(Task::none());
            }
            return None;
        }
        // A local Cube of that network, or one of its ghosts on the floor.
        if key.cube_id.is_empty()
            || !(self
                .network_catalog(key.chain)
                .iter()
                .any(|cube| cube.id == key.cube_id)
                || (intent == PendingIntent::Select
                    && project(self)
                        .ghosts
                        .iter()
                        .any(|cube| ghost_key(cube) == *key)))
        {
            return None;
        }
        self.catalog.pending_select = Some((key.clone(), intent));
        self.catalog.selected = None;
        self.catalog.motion = None;
        self.catalog.open_from_floor = false;
        Some(Task::done(Message::View(
            super::ViewMessage::SelectNetwork(key.chain),
        )))
    }
}

/// The floor is inside a vertical scrollable. It does not forward wheel events
/// to children, and can mark even a sideways delta captured when it first
/// announces its viewport. Keys must remain unhandled by focused widgets;
/// horizontal wheel input is filtered by the reducer instead of that status.
/// `listen_with` has no hit target, so native QA must check sideways scroll
/// over Launcher chrome and Connect rows before treating this as floor-only.
pub(super) fn floor_event(
    event: iced::Event,
    status: event::Status,
    _: iced::window::Id,
) -> Option<Message> {
    let intent = match event {
        iced::Event::Keyboard(keyboard::Event::KeyPressed {
            key: keyboard::Key::Named(key),
            modifiers,
            ..
        }) if status == event::Status::Ignored && !modifiers.command() && !modifiers.alt() => {
            match key {
                key::Named::ArrowLeft | key::Named::PageUp => CatalogMessage::Step(-1),
                key::Named::ArrowRight | key::Named::PageDown => CatalogMessage::Step(1),
                _ => return None,
            }
        }
        iced::Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
            let x = match delta {
                mouse::ScrollDelta::Lines { x, .. } | mouse::ScrollDelta::Pixels { x, .. } => x,
            };
            if !x.is_finite() || x == 0.0 {
                return None;
            }
            CatalogMessage::Wheel(delta)
        }
        _ => return None,
    };
    Some(Message::View(ViewMessage::Catalog(intent)))
}

pub(super) fn load_launcher_mode(
    root: &crate::dir::CoincubeDirectory,
    system_reduces_motion: Option<bool>,
) -> LauncherMode {
    use crate::app::settings::{global::GlobalSettings, preferences::LauncherStyle};
    let path = GlobalSettings::path(root);
    let preference = GlobalSettings::load_preferences(&path).unwrap_or_default();
    match preference
        .resolve(None, system_reduces_motion)
        .launcher_style
    {
        LauncherStyle::Cubes => LauncherMode::Cubes,
        LauncherStyle::List => LauncherMode::List,
    }
}

/// Presentation-only catalog intents. None of them opens, unlocks, renames,
/// deletes or starts a task other than a focus operation.
#[derive(Debug, Clone)]
pub enum CatalogMessage {
    Select(CubeCatalogKey),
    SelectFloor(CubeCatalogKey),
    MotionTick(Instant),
    /// The pointer entered or left the Key button or its card.
    KeyHover(KeyPart, bool),
    /// A click pins or unpins the Key card.
    KeyToggle,
    /// Escape shuts it.
    KeyClose,
    /// Opens or closes the top bar's network menu.
    NetworkMenu(bool),
    /// Opens or closes the dock's plan menu.
    PlanMenu(bool),
    /// All / Live / Test networks, or one network from the menu.
    Filter(NetworkFilter),
    ClearSelection,
    Query(String),
    ClearQuery,
    Page(usize),
    /// Relative floor navigation from keyboard. Shares Page's selection reset.
    Step(i8),
    /// Horizontal wheel/trackpad movement in Iced's native scroll direction.
    Wheel(mouse::ScrollDelta),
    /// The floor's laid-out width, from its sensor, which sets the page size.
    StageWidth(f32),
    /// The pointer entered or left a resting Cube's slot.
    CubeHover(CubeCatalogKey, bool),
    /// A shared Action's pointer-down request: apply whole-tree focus.
    Focus(Id),
    /// Network activity reads, its arrival frames and the dock's source menu.
    Activity(super::launcher_activity::ActivityMessage),
}

impl Home {
    pub(crate) fn floor_open_matches_queued(&self, generation: u64, cube: &CubeSettings) -> bool {
        self.catalog.open_from_floor
            && self.catalog.floor_open_generation == generation
            && self.floor_open_matches_current_cube(cube)
    }
    pub(crate) fn launcher_floor_open_armed(&self) -> bool {
        self.catalog.open_from_floor
    }

    /// A floor Open is queued with a clone of the selected Cube. Reject that
    /// clone if a catalog reload changed any persisted field before the GUI
    /// receives Run, even when the same chain and ID still resolve. Comparing
    /// JSON values avoids making CubeSettings' numerous nested fields part of
    /// an ad hoc presentation-only equality definition.
    pub(crate) fn floor_open_matches_current_cube(&self, queued: &CubeSettings) -> bool {
        if !catalog_visible(self) {
            return false;
        }
        let projection = project(self);
        if !projection.floor_eligible(self.catalog.mode) {
            return false;
        }
        let Some(index) = projection.selected else {
            return false;
        };
        let Some(current) = projection.locals.get(index) else {
            return false;
        };
        let Some(first) = self.catalog.page.checked_mul(self.catalog.page_size()) else {
            return false;
        };
        if index
            .checked_sub(first)
            .is_none_or(|slot| slot >= self.catalog.page_size())
        {
            return false;
        }
        if current.key.cube_id != queued.id || current.key.chain != queued.network {
            return false;
        }
        matches!(
            (
                serde_json::to_value(current.cube),
                serde_json::to_value(queued)
            ),
            (Ok(current), Ok(queued)) if current == queued
        )
    }

    /// Only a selected floor Cube has a details face for the tab to turn away
    /// from. Reduced motion resolves to List before this point.
    pub(crate) fn take_launcher_details_face(
        &mut self,
        cube: &crate::app::settings::CubeSettings,
    ) -> bool {
        let opened_on_floor = std::mem::take(&mut self.catalog.open_from_floor);
        opened_on_floor
            && catalog_visible(self)
            && project(self).floor_eligible(self.catalog.mode)
            && self
                .catalog
                .selected
                .as_ref()
                .is_some_and(|key| key.cube_id == cube.id && key.chain == cube.network)
    }

    /// Recheck the live filtered page at the admitted handoff. An earlier
    /// floor Open intent is provenance, not permission to revive a stale row.
    pub(crate) fn launcher_scene_context(
        &self,
        cube: &crate::app::settings::CubeSettings,
    ) -> Option<LauncherSceneContext> {
        if !self.floor_open_matches_current_cube(cube) {
            return None;
        }
        let projection = project(self);
        if !projection.floor_eligible(self.catalog.mode) {
            return None;
        }
        let index = projection.selected?;
        let selected = projection.locals.get(index)?;
        if selected.key.cube_id != cube.id || selected.key.chain != cube.network {
            return None;
        }
        let page = self.catalog.page;
        let first = page.checked_mul(self.catalog.page_size())?;
        let selected_slot = index.checked_sub(first)?;
        if selected_slot >= self.catalog.page_size() {
            return None;
        }
        let peers = projection
            .floor_entries()
            .into_iter()
            .skip(first)
            .take(self.catalog.page_size())
            .map(|entry| match entry {
                FloorEntry::Local(row) => LauncherScenePeer {
                    key: row.key.clone(),
                    name: row.name.clone(),
                    status: if row.actionable {
                        super::launcher_presenter::unlock_prompt(row.cube)
                    } else {
                        "Unavailable on this network"
                    },
                    marks: super::launcher_gpu::marks(row.cube),
                },
                FloorEntry::Ghost(cube) => LauncherScenePeer {
                    key: ghost_key(cube),
                    name: catalog_display_name(&cube.name),
                    status: ghost_status(cube, projection.connect_trouble),
                    marks: ghost_marks(cube),
                },
            })
            .collect::<Vec<_>>();
        if peers.get(selected_slot)?.key != selected.key {
            return None;
        }
        Some(LauncherSceneContext {
            selected: selected.key.clone(),
            page,
            selected_slot,
            backup: super::launcher_presenter::backup(cube).0,
            connect: selected.connect.1.clone(),
            wallets: selected.wallets.clone(),
            peers,
            pages: floor_pages(projection.floor_len(), self.catalog.page_size()),
            chrome: super::launcher_chrome::snapshot(self, &projection),
        })
    }

    /// The same identity check the catalog messages use, plus a nonempty ID.
    pub(super) fn catalog_resolves(&self, key: &CubeCatalogKey) -> bool {
        !key.cube_id.is_empty() && self.catalog_index(key).is_some()
    }

    /// A key can be selected only while the List is visible and the key names
    /// exactly one record on it.
    fn catalog_selectable(&self, key: &CubeCatalogKey) -> bool {
        catalog_visible(self) && (self.catalog_resolves(key) || self.ghost_resolves(key))
    }

    /// A ghost is selectable on the floor only: its key is the current
    /// network and a Connect registration this device does not hold.
    fn ghost_resolves(&self, key: &CubeCatalogKey) -> bool {
        if self.catalog.mode != LauncherMode::Cubes
            || key.cube_id.is_empty()
            || key.chain != self.network
        {
            return false;
        }
        // Only a floor draws a ghost's face; a List fallback has none.
        let projection = project(self);
        projection.floor_eligible(self.catalog.mode)
            && projection.ghosts.iter().any(|cube| ghost_key(cube) == *key)
    }

    pub(super) fn update_catalog(&mut self, message: CatalogMessage) -> Task<Message> {
        match message {
            CatalogMessage::Activity(message) => self.update_activity(message),
            CatalogMessage::Select(key) => {
                if let Some(task) = self.select_on_other_network(&key, PendingIntent::Select) {
                    return task;
                }
                self.catalog.selected = self.catalog_selectable(&key).then_some(key.clone());
                self.catalog.motion = None;
                self.catalog.open_from_floor = false;
                if let Some(index) = project(self).floor_position(&key) {
                    self.catalog.page = index / self.catalog.page_size();
                }
                Task::none()
            }
            CatalogMessage::SelectFloor(key) => {
                if let Some(task) = self.select_on_other_network(&key, PendingIntent::Select) {
                    return task;
                }
                let previous = self.catalog.selected.clone();
                self.catalog.selected = self.catalog_selectable(&key).then_some(key.clone());
                self.catalog.motion = None;
                self.catalog.hover.clear();
                self.catalog.open_from_floor = false;
                if self.catalog.mode == LauncherMode::Cubes
                    && self.catalog.selected.as_ref() == Some(&key)
                    && previous.as_ref() != Some(&key)
                {
                    self.catalog.motion = Some(super::launcher_scene::Motion::new(previous));
                }
                if let Some(index) = project(self).floor_position(&key) {
                    self.catalog.page = index / self.catalog.page_size();
                }
                if self.catalog.selected.as_ref() == Some(&key) {
                    iced::widget::operation::focus(select_id(&key))
                } else {
                    Task::none()
                }
            }
            CatalogMessage::KeyHover(part, over) => {
                self.catalog.key.hover(part, over);
                Task::none()
            }
            CatalogMessage::KeyToggle => {
                // One dock popover at a time.
                self.catalog.key.click();
                if self.catalog.key_open() {
                    self.catalog.plan_menu = false;
                    self.catalog.activity.menu = false;
                }
                Task::none()
            }
            CatalogMessage::KeyClose => {
                self.catalog.key.shut();
                Task::none()
            }
            CatalogMessage::NetworkMenu(open) => {
                self.catalog.network_menu = open;
                Task::none()
            }
            CatalogMessage::PlanMenu(open) => {
                // One dock menu at a time.
                self.catalog.plan_menu = open;
                if open {
                    self.catalog.activity.menu = false;
                    self.catalog.key = KeyCard::default();
                }
                Task::none()
            }
            CatalogMessage::Filter(filter) => {
                let from_menu = std::mem::take(&mut self.catalog.network_menu);
                // Create and Import act on the current network, so it must
                // be one the filter shows: one network becomes current, and
                // Live or Test move off a network they hide.
                let target = self.filter_target(filter);
                self.catalog.filter = filter;
                self.catalog.page = 0;
                // A selection the filter hides falls back to browsing.
                if self
                    .catalog
                    .selected
                    .as_ref()
                    .is_some_and(|key| !self.effective_filter().matches(key.chain))
                {
                    self.catalog.selected = None;
                    self.catalog.motion = None;
                    self.catalog.open_from_floor = false;
                }
                let switch = target.map_or_else(Task::none, |chain| {
                    Task::done(Message::View(super::ViewMessage::SelectNetwork(chain)))
                });
                // A menu choice closes the menu; keep keyboard focus on its chip.
                if from_menu {
                    Task::batch([switch, iced::widget::operation::focus(network_id())])
                } else {
                    switch
                }
            }
            CatalogMessage::MotionTick(now) => {
                // Settled floats, and Cubes paged or selected away, stop.
                let browsing = self.catalog.selected.is_none();
                let on_page: Vec<_> = if browsing && !self.catalog.hover.is_empty() {
                    let projection = project(self);
                    if projection.shows_create_placeholder(self.catalog.mode) {
                        vec![create_placeholder_key()]
                    } else {
                        projection
                            .floor_keys()
                            .into_iter()
                            .skip(self.catalog.page * self.catalog.page_size())
                            .take(self.catalog.page_size())
                            .collect()
                    }
                } else {
                    Vec::new()
                };
                self.catalog
                    .hover
                    .retain(|hover| !hover.settled(now) && on_page.contains(&hover.key));
                if self
                    .catalog
                    .motion
                    .as_ref()
                    .is_some_and(|motion| motion.done(now))
                {
                    self.catalog.motion = None;
                    if let Some(key) = &self.catalog.selected {
                        return iced::widget::operation::focus(details_open_id(key));
                    }
                }
                Task::none()
            }
            CatalogMessage::ClearSelection => {
                // Back spins the Cube to rest and brightens its peers, as the
                // prototype's `toBrowse` reverses the same transitions.
                let returning = self.catalog.selected.take();
                self.catalog.motion = returning
                    .filter(|_| {
                        self.catalog.mode == LauncherMode::Cubes
                            && project(self).floor_eligible(self.catalog.mode)
                    })
                    .map(super::launcher_scene::Motion::returning);
                self.catalog.open_from_floor = false;
                Task::none()
            }
            CatalogMessage::Query(query) => {
                self.catalog.query = query;
                self.catalog.page = 0;
                self.catalog.open_from_floor = false;
                if self.catalog.mode == LauncherMode::Cubes {
                    self.catalog.selected = None;
                    self.catalog.motion = None;
                }
                self.catalog.wheel_pixels = 0.0;
                self.catalog.last_wheel_input = None;
                self.catalog.wheel_consumed = false;
                Task::none()
            }
            CatalogMessage::ClearQuery => {
                self.catalog.query.clear();
                self.catalog.page = 0;
                self.catalog.open_from_floor = false;
                if self.catalog.mode == LauncherMode::Cubes {
                    self.catalog.selected = None;
                    self.catalog.motion = None;
                }
                self.catalog.wheel_pixels = 0.0;
                self.catalog.last_wheel_input = None;
                self.catalog.wheel_consumed = false;
                Task::none()
            }
            CatalogMessage::Page(page) => {
                let pages = floor_pages(project(self).floor_len(), self.catalog.page_size());
                if page < pages {
                    self.catalog.page = page;
                    self.catalog.selected = None;
                    self.catalog.motion = None;
                    self.catalog.open_from_floor = false;
                    self.catalog.wheel_pixels = 0.0;
                    self.catalog.last_wheel_input = None;
                    self.catalog.last_wheel_page = None;
                    self.catalog.wheel_consumed = false;
                }
                Task::none()
            }
            CatalogMessage::Step(direction) => self.step_floor_page(direction),
            CatalogMessage::Wheel(delta) => self.wheel_floor_page(delta, Instant::now()),
            CatalogMessage::StageWidth(width) => {
                let before = self.catalog.page_size();
                self.catalog.stage_width = Some(width);
                let after = self.catalog.page_size();
                if after != before {
                    // Keep the selected Cube, else the page's first, on screen.
                    let projection = project(self);
                    let anchor = projection
                        .floor_selected()
                        .unwrap_or(self.catalog.page.saturating_mul(before));
                    let page = floor_page_keeping(anchor, after, projection.floor_len());
                    self.catalog.page = page;
                }
                Task::none()
            }
            CatalogMessage::CubeHover(key, over) => {
                if over && self.catalog.selected.is_some() {
                    return Task::none();
                }
                self.catalog.hover(key, over, Instant::now());
                Task::none()
            }
            CatalogMessage::Focus(id) => iced::widget::operation::focus(id),
        }
    }

    fn step_floor_page(&mut self, direction: i8) -> Task<Message> {
        let projection = project(self);
        if !catalog_visible(self) || !projection.floor_eligible(self.catalog.mode) {
            return Task::none();
        }
        let pages = floor_pages(projection.floor_len(), self.catalog.page_size());
        let target = if direction < 0 {
            self.catalog.page.checked_sub(1)
        } else if direction > 0 {
            self.catalog
                .page
                .checked_add(1)
                .filter(|page| *page < pages)
        } else {
            None
        };
        if let Some(target) = target {
            self.update_catalog(CatalogMessage::Page(target))
        } else {
            Task::none()
        }
    }

    fn wheel_floor_page(&mut self, delta: mouse::ScrollDelta, now: Instant) -> Task<Message> {
        if !catalog_visible(self) || !project(self).floor_eligible(self.catalog.mode) {
            self.catalog.wheel_pixels = 0.0;
            self.catalog.last_wheel_input = None;
            self.catalog.wheel_consumed = false;
            return Task::none();
        }
        let x = match delta {
            mouse::ScrollDelta::Lines { x, .. } => x * 60.0,
            mouse::ScrollDelta::Pixels { x, .. } => x,
        };
        if !x.is_finite() || x == 0.0 {
            return Task::none();
        }
        if self
            .catalog
            .last_wheel_input
            .is_some_and(|last| now.saturating_duration_since(last) > WHEEL_GESTURE_GAP)
        {
            self.catalog.wheel_pixels = 0.0;
            self.catalog.wheel_consumed = false;
        }
        self.catalog.last_wheel_input = Some(now);
        if self.catalog.wheel_consumed {
            return Task::none();
        }
        if self
            .catalog
            .last_wheel_page
            .is_some_and(|last| now.saturating_duration_since(last) < WHEEL_PAGE_COOLDOWN)
        {
            // A new gesture inside the cooldown is spent as a whole. Its
            // later deltas must not turn a second page when 600ms elapses.
            self.catalog.wheel_consumed = true;
            return Task::none();
        }
        if self.catalog.wheel_pixels.signum() != x.signum() {
            self.catalog.wheel_pixels = 0.0;
        }
        self.catalog.wheel_pixels += x;
        if self.catalog.wheel_pixels.abs() < WHEEL_PAGE_THRESHOLD {
            return Task::none();
        }
        let direction = if self.catalog.wheel_pixels < 0.0 {
            1
        } else {
            -1
        };
        self.catalog.wheel_pixels = 0.0;
        let page_before = self.catalog.page;
        let task = self.step_floor_page(direction);
        self.catalog.last_wheel_input = Some(now);
        self.catalog.wheel_consumed = true;
        if self.catalog.page != page_before {
            self.catalog.last_wheel_page = Some(now);
        }
        task
    }

    /// Drop a selection once the List is no longer visible (another section,
    /// the create form, an unavailable network) or the key no longer names
    /// exactly one listed Cube (removed, duplicated, reloaded from another
    /// chain). Never moves it to another Cube. Runs after every message.
    pub(super) fn reconcile_catalog(&mut self) {
        // A pick or Open on another network belongs to the Launcher. Leaving
        // it by any route (a nav click, a banner CTA, a handoff) drops it, so
        // its replay cannot start an unlock from another section.
        if !matches!(self.active_section, HomeSection::Cubes) {
            self.catalog.pending_select = None;
        }
        // The Key card belongs to the Key button, which needs a Cube the
        // filter shows.
        if project(self).filtered_total == 0 {
            self.catalog.key = KeyCard::default();
        }
        // A Bitcoin Blake2b activity choice lapses when the menu stops
        // offering it, so it cannot return unseen when Blake2b is listed again.
        if !self.activity_blake2b_offered() {
            self.catalog.activity.blake2b = false;
        }
        let pages = floor_pages(project(self).floor_len(), self.catalog.page_size());
        self.catalog.page = self.catalog.page.min(pages - 1);
        if let Some(key) = &self.catalog.selected {
            let hidden_on_floor = self.catalog.mode == LauncherMode::Cubes
                && project(self)
                    .floor_selected()
                    .is_some_and(|index| index / self.catalog.page_size() != self.catalog.page);
            if !self.catalog_selectable(key) || hidden_on_floor {
                self.catalog.selected = None;
                self.catalog.motion = None;
                self.catalog.open_from_floor = false;
            }
        }
        // Back leaves nothing selected while its Cube spins home, so a Return
        // motion survives the empty selection until it finishes or its Cube
        // goes away.
        let returning = self
            .catalog
            .motion
            .as_ref()
            .and_then(|motion| motion.returning_key())
            .is_some_and(|key| self.catalog_selectable(key));
        if (self.catalog.selected.is_none() && !returning)
            || self.catalog.mode != LauncherMode::Cubes
        {
            self.catalog.motion = None;
            self.catalog.open_from_floor = false;
        }
    }
}

/// How the Connect state reads, and how strongly to tint it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Tone {
    Good,
    Muted,
    Warn,
}

/// The local Cube's Recovery Kit line: whether Connect holds its encrypted
/// Kit. The tones are the former Connect caption's, which also tint the
/// avatar; a registration refusal still reads as the server's reason.
pub(super) fn recovery_kit_caption(
    cube: &CubeSettings,
    signed_in: bool,
    sync_error: Option<String>,
) -> (Tone, String) {
    match cube.connect_state() {
        CubeConnectState::BackedUp => (Tone::Good, "On Connect".to_string()),
        CubeConnectState::Registered => (
            Tone::Muted,
            if cube.has_imported_bitcoin() && cube.has_recovery_kit() {
                "Needs updating for imported wallet".to_string()
            } else {
                "None yet".to_string()
            },
        ),
        CubeConnectState::Sovereign if signed_in => match sync_error {
            Some(reason) => (Tone::Warn, reason),
            None => (Tone::Muted, "None yet — syncing to Connect".to_string()),
        },
        CubeConnectState::Sovereign => (Tone::Warn, "None — local only".to_string()),
    }
}

/// What a remote Cube can be restored with from this screen, and the exact
/// existing message for it. Password needs a kit holding the encrypted seed;
/// phone needs a sealed envelope and the owner-keychain flag.
#[derive(Debug, Clone)]
pub(super) struct RemoteRestore {
    pub(super) status: &'static str,
    pub(super) action: Option<(ViewMessage, &'static str)>,
}

pub(super) fn remote_restore(cube: &RemoteCube) -> RemoteRestore {
    let (password_recoverable, phone_recoverable) = remote_recovery(cube);
    let status = match (password_recoverable, phone_recoverable) {
        (true, true) => localized_copy("Recovery Kit + Phone recovery available"),
        (true, false) => localized_copy("Recovery Kit available"),
        (false, true) => localized_copy("Phone recovery available"),
        (false, false) if cube.has_recovery_kit => localized_copy(
            "Recovery Kit has no Master Seed — restore from the device where it was backed up",
        ),
        (false, false) => localized_copy("Registered in Connect (no Recovery Kit backed up)"),
    };
    let action = match (password_recoverable, phone_recoverable) {
        (true, true) => Some((
            ViewMessage::ShowRecoveryMethodPicker(cube.uuid.clone()),
            "Recover…",
        )),
        (true, false) => Some((
            ViewMessage::RestoreFromRecoveryKit(cube.uuid.clone()),
            "Restore",
        )),
        (false, true) => Some((
            ViewMessage::RestoreWithPhone {
                cube_id: cube.id,
                full_cube: cube.phone_full_cube,
            },
            "Recover with phone",
        )),
        (false, false) => None,
    };
    RemoteRestore { status, action }
}

/// The ways Connect holds to restore `cube` (password, phone). Deleting it
/// from Connect takes these.
pub(super) fn remote_recovery(cube: &RemoteCube) -> (bool, bool) {
    (
        cube.has_recovery_kit && cube.has_encrypted_seed,
        cube.phone_recoverable && feature_flags::OWNER_KEYCHAIN_RECOVERY_ENABLED,
    )
}

/// Length-prefixed so no Cube ID can collide with another purpose or chain.
fn local_action_id(purpose: &str, key: &CubeCatalogKey) -> Id {
    let chain = key.chain.label();
    Id::from(format!(
        "tenshu.launcher.catalog:local:{}:{}:{}:{}:{}:{}",
        purpose.len(),
        purpose,
        chain.len(),
        chain,
        key.cube_id.len(),
        key.cube_id,
    ))
}

fn remote_action_id(purpose: &str, uuid: &str) -> Id {
    Id::from(format!(
        "tenshu.launcher.catalog:remote:{}:{}:{}:{}",
        purpose.len(),
        purpose,
        uuid.len(),
        uuid,
    ))
}

fn control_id(purpose: &str) -> Id {
    Id::from(format!("tenshu.launcher.catalog:control:{}", purpose))
}

pub(super) fn select_id(key: &CubeCatalogKey) -> Id {
    local_action_id("select", key)
}
pub(super) fn open_id(key: &CubeCatalogKey) -> Id {
    local_action_id("open", key)
}
pub(super) fn details_open_id(key: &CubeCatalogKey) -> Id {
    local_action_id("details-open", key)
}
pub(super) fn rename_id(key: &CubeCatalogKey) -> Id {
    local_action_id("rename", key)
}
pub(super) fn delete_id(key: &CubeCatalogKey) -> Id {
    local_action_id("delete", key)
}
pub(super) fn close_details_id() -> Id {
    control_id("close-details")
}
pub(super) fn restore_id(uuid: &str) -> Id {
    remote_action_id("restore", uuid)
}
pub(super) fn remote_delete_id(uuid: &str) -> Id {
    remote_action_id("delete", uuid)
}
fn search_input_id() -> text_input::Id {
    text_input::Id::new("tenshu.launcher.catalog:control:search")
}

#[cfg(test)]
pub(super) fn search_id() -> Id {
    search_input_id().into()
}
pub(super) fn network_id() -> Id {
    control_id("network")
}
pub(super) fn connect_id() -> Id {
    control_id("connect")
}
pub(super) fn sign_in_id() -> Id {
    control_id("sign-in")
}

pub(super) fn key_id() -> Id {
    control_id("key")
}

pub(super) fn settings_id() -> Id {
    control_id("settings")
}
#[cfg(test)]
pub(super) fn theme_id() -> Id {
    control_id("theme")
}
pub(super) fn recover_vault_id() -> Id {
    control_id("recover-vault")
}
#[cfg(test)]
pub(super) fn split_wallet_id() -> Id {
    control_id("split-wallet")
}
pub(super) fn clear_query_id() -> Id {
    control_id("clear-query")
}
/// An All / Live / Test networks chip.
pub(super) fn filter_id(name: &str) -> Id {
    control_id(&format!("filter-{name}"))
}
/// The top bar's "Clear filters" link.
pub(super) fn clear_filters_id() -> Id {
    control_id("clear-filters")
}
/// "Clear filters" in the empty state when every Cube is filtered out.
pub(super) fn clear_filters_empty_id() -> Id {
    control_id("clear-filters-empty")
}
pub(super) fn create_id() -> Id {
    control_id("create")
}
pub(super) fn import_id() -> Id {
    control_id("import")
}

fn focus(id: Id) -> ViewMessage {
    ViewMessage::Catalog(CatalogMessage::Focus(id))
}

/// A row in the local section.
#[derive(Debug, Clone)]
pub(super) struct LocalRow<'a> {
    /// The record's own chain and ID. Never the list position.
    pub(super) key: CubeCatalogKey,
    /// Position in Home's list, used only to match the exact record the
    /// protected `catalog_index` resolves. Never part of an action ID.
    pub(super) index: usize,
    pub(super) cube: &'a CubeSettings,
    pub(super) name: String,
    pub(super) vault: String,
    pub(super) connect: (Tone, String),
    /// A short, visible registration refusal; full guidance stays in details.
    pub(super) sync_warning: Option<String>,
    /// This process holds the Cube's unlocked session.
    pub(super) open: bool,
    /// The wallets the Cube holds, as the details list them.
    pub(super) wallets: String,
    /// The key is nonempty, the record's chain is the current network and the
    /// list's source, and `catalog_index` resolves to this very record.
    /// Otherwise the row is shown but its controls have no press target.
    pub(super) actionable: bool,
}

/// Everything the List body shows, derived from Home without side effects.
#[derive(Debug, Clone)]
pub(super) struct Projection<'a> {
    pub(super) locals: Vec<LocalRow<'a>>,
    pub(super) remotes: Vec<&'a RemoteCube>,
    /// Unfiltered counts, so a search never changes what the plan shows.
    pub(super) local_total: usize,
    pub(super) remote_total: usize,
    /// Connect-only Cubes on every network the filter shows: the floor's
    /// ghosts, after the local Cubes, in network order. `remotes` stays the
    /// current network's, as the List acts on those in place.
    pub(super) ghosts: Vec<&'a RemoteCube>,
    /// The selected local Cube when it is listed exactly once and visible.
    pub(super) selected: Option<usize>,
    /// The selected ghost on the floor: its index in `remotes`.
    pub(super) selected_ghost: Option<usize>,
    pub(super) query: String,
    /// Why the stored Connect session couldn't be checked, while it waits:
    /// ghosts then say they can't be restored yet (L5).
    pub(super) connect_trouble: Option<crate::services::connect::grpc::trouble::Trouble>,
    pub(super) network: ChainId,
    pub(super) total_count: usize,
    pub(super) cube_limit: usize,
    pub(super) tier: &'static str,
    /// Local Cubes the network filter hides (after any search).
    pub(super) hidden: usize,
    /// Connect-only Cubes on listed networks the filter hides from the
    /// floor (after any search). The List never shows other networks'
    /// ghosts, so only the floor counts these.
    pub(super) hidden_ghosts: usize,
    /// Local Cubes the network filter shows, before any search: what the
    /// floor would hold, so the 20-Cube cap counts only these.
    pub(super) filtered_total: usize,
    /// Listed networks whose Cube list could not be read.
    pub(super) unread: Vec<ChainId>,
    /// Every listed network with its local Cube count, in the canvas order.
    pub(super) network_counts: Vec<(ChainId, usize)>,
    /// Each network's use of the plan's per-network Cube limit, in the canvas
    /// order: the current network always, others only when they hold a Cube
    /// here or in Connect, listed or not.
    pub(super) usage: Vec<NetworkUsage>,
    pub(super) filter: NetworkFilter,
}

/// One network's share of the plan's Cube limit. The limit is per network and
/// counts every Cube registered in Connect, so Cubes on another device use a
/// slot here even though this device's lists never show them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct NetworkUsage {
    pub(super) chain: ChainId,
    /// Cubes on this device; `None` when this device's list for the network
    /// cannot be counted (see `Home::cube_usage`).
    pub(super) local: Option<usize>,
    /// Cubes registered in Connect with no copy on this device.
    pub(super) remote: usize,
}

impl NetworkUsage {
    /// The slots in use, when this device's share is known.
    pub(super) fn total(&self) -> Option<usize> {
        self.local.map(|local| local + self.remote)
    }
}

/// One Cube on the floor, in floor order: the local Cubes, then the ghosts.
#[derive(Debug, Clone, Copy)]
pub(super) enum FloorEntry<'p, 'a> {
    Local(&'p LocalRow<'a>),
    /// A Cube registered in Connect with no copy on this device.
    Ghost(&'a RemoteCube),
}

impl<'p> FloorEntry<'p, '_> {
    pub(super) fn key(&self) -> CubeCatalogKey {
        match self {
            FloorEntry::Local(row) => row.key.clone(),
            FloorEntry::Ghost(cube) => ghost_key(cube),
        }
    }
}

/// A ghost's selection key: its network and the registration's UUID. A
/// local Cube's ID is its registration UUID, and a ghost is a registration
/// with no local Cube, so the two never collide.
pub(super) fn ghost_key(cube: &RemoteCube) -> CubeCatalogKey {
    CubeCatalogKey {
        chain: ghost_chain(cube).unwrap_or(ChainId::Bitcoin),
        cube_id: cube.uuid.clone(),
    }
}

/// The chain a registration names. The floor stands only ghosts whose
/// network parses; acting on one first makes its network current.
/// The Create Cube placeholder's hover key. No Cube has this id: Cube ids
/// and Connect UUIDs never contain a colon.
pub(super) fn create_placeholder_key() -> CubeCatalogKey {
    CubeCatalogKey {
        chain: ChainId::Bitcoin,
        cube_id: "tenshu.launcher.empty:create".into(),
    }
}

pub(super) fn ghost_chain(cube: &RemoteCube) -> Option<ChainId> {
    ChainId::from_api_str(&cube.network)
}

/// A ghost looks restorable only when this device can restore it: a kit
/// holding the seed, or phone recovery. A kit without its seed reads as no
/// Recovery Kit (Robert, 2026-10-29).
pub(super) fn ghost_kind(cube: &RemoteCube) -> super::launcher_scene::Ghost {
    if remote_restore(cube).action.is_some() {
        super::launcher_scene::Ghost::Restorable
    } else {
        super::launcher_scene::Ghost::Unrestorable
    }
}

/// A ghost's face marks: its chain, its kind and the wallets Connect knows.
pub(super) fn ghost_marks(cube: &RemoteCube) -> super::launcher_scene::CubeMarks {
    super::launcher_scene::CubeMarks::ghost(ghost_key(cube).chain, ghost_kind(cube))
        .with_vault(cube.has_vault == Some(true))
}

/// The state line under a ghost on the floor. While the session waits to be
/// checked, nothing can be restored: it says why instead.
pub(super) fn ghost_status(
    cube: &RemoteCube,
    trouble: Option<crate::services::connect::grpc::trouble::Trouble>,
) -> &'static str {
    use crate::services::connect::grpc::trouble::Trouble;
    match trouble {
        Some(Trouble::Offline) => return "In Connect · Offline",
        Some(Trouble::Unreachable) => return "In Connect · Reconnecting…",
        None => {}
    }
    match ghost_kind(cube) {
        super::launcher_scene::Ghost::Restorable => "In Connect · Restore",
        super::launcher_scene::Ghost::Unrestorable => "In Connect · No Recovery Kit",
    }
}

impl Projection<'_> {
    /// The floor shows whenever the Launcher style is Cubes, however many
    /// Cubes there are: it pages, four or six to a page (Robert, 2026-10-30).
    /// Search belongs to the List; the floor has no query control, so a query
    /// typed there keeps the List until it is cleared. Cubes in Connect only
    /// stand on the floor as ghosts after the local Cubes, and are the whole
    /// row on a new device.
    pub(super) fn floor_eligible(&self, mode: LauncherMode) -> bool {
        mode == LauncherMode::Cubes && self.query.is_empty()
    }

    /// Every Cube the floor stands, local Cubes first.
    pub(super) fn floor_entries(&self) -> Vec<FloorEntry<'_, '_>> {
        self.locals
            .iter()
            .map(FloorEntry::Local)
            .chain(self.ghosts.iter().copied().map(FloorEntry::Ghost))
            .collect()
    }

    /// Cubes in Connect only that this view stands or lists: the floor's
    /// ghosts on every shown network, or the List's current-network rows.
    pub(super) fn connect_only(&self, mode: LauncherMode) -> usize {
        if self.floor_eligible(mode) {
            self.ghosts.len()
        } else {
            self.remotes.len()
        }
    }

    pub(super) fn floor_len(&self) -> usize {
        self.locals.len() + self.ghosts.len()
    }

    /// Every Cube the filter keeps off the floor, local or only in Connect.
    pub(super) fn floor_hidden(&self) -> usize {
        self.hidden + self.hidden_ghosts
    }

    /// The floor stands only the Create Cube placeholder: nothing to stand
    /// and nothing filtered out, local or in Connect.
    pub(super) fn shows_create_placeholder(&self, mode: LauncherMode) -> bool {
        self.floor_eligible(mode) && self.floor_len() == 0 && self.floor_hidden() == 0
    }

    pub(super) fn floor_keys(&self) -> Vec<CubeCatalogKey> {
        self.floor_entries().iter().map(FloorEntry::key).collect()
    }

    pub(super) fn floor_position(&self, key: &CubeCatalogKey) -> Option<usize> {
        self.locals
            .iter()
            .position(|row| &row.key == key)
            .or_else(|| {
                self.ghosts
                    .iter()
                    .position(|cube| ghost_key(cube) == *key)
                    .map(|index| self.locals.len() + index)
            })
    }

    /// The selected Cube's floor position, local or ghost.
    pub(super) fn floor_selected(&self) -> Option<usize> {
        self.selected
            .or_else(|| self.selected_ghost.map(|index| self.locals.len() + index))
    }

    pub(super) fn at_limit(&self) -> bool {
        self.total_count >= self.cube_limit
    }
    pub(super) fn no_matches(&self) -> bool {
        (!self.query.trim().is_empty() || self.hidden > 0)
            && self.locals.is_empty()
            && self.remotes.is_empty()
            && self.local_total + self.remote_total > 0
    }
}

fn matches_query(name: &str, query: &str) -> bool {
    let query = query.trim();
    query.is_empty() || name.to_lowercase().contains(&query.to_lowercase())
}

pub(super) fn project(home: &Home) -> Projection<'_> {
    let network = home.network;
    let signed_in = home.connect_account.authenticated_client().is_some();
    let query = home.catalog.query.clone();
    let checked: Option<(&[CubeSettings], ChainId)> = match &home.state {
        State::Cubes { cubes, source, .. } => Some((cubes.as_slice(), *source)),
        _ => None,
    };
    let open_cube = crate::app::session::current_cube_id();
    let filter = home.effective_filter();
    // Every listed network, in the canvas's order. The current network's
    // rows come from the checked list; the others from read-only copies,
    // which act by first making their network current.
    let mut network_counts = Vec::new();
    let mut local_total = 0;
    let mut filtered_total = 0;
    let mut hidden = 0;
    let mut locals: Vec<LocalRow<'_>> = Vec::new();
    for chain in home.ordered_networks() {
        let (cubes, current) = match checked {
            Some((cubes, source)) if source == chain => (cubes, true),
            _ => (home.network_catalog(chain), false),
        };
        network_counts.push((chain, cubes.len()));
        local_total += cubes.len();
        for (index, cube) in cubes.iter().enumerate() {
            let key = CubeCatalogKey {
                chain: cube.network,
                cube_id: cube.id.clone(),
            };
            let name = catalog_display_name(&cube.name);
            if filter.matches(chain) {
                filtered_total += 1;
            }
            if !matches_query(&name, &query) {
                continue;
            }
            if !filter.matches(chain) {
                hidden += 1;
                continue;
            }
            let connect = recovery_kit_caption(cube, signed_in, home.cube_sync_hint(cube));
            let sync_warning = (signed_in && connect.0 == Tone::Warn)
                .then(|| connect.1.lines().next().unwrap_or_default().to_string());
            locals.push(LocalRow {
                // `catalog_index` also requires key chain == source == network.
                // Another network's row is acted on by switching to it.
                actionable: !key.cube_id.is_empty()
                    && if current {
                        home.catalog_index(&key) == Some(index)
                    } else {
                        key.chain == chain
                    },
                index: if current { index } else { usize::MAX },
                connect,
                sync_warning,
                open: open_cube.as_deref() == Some(cube.id.as_str()),
                // The Liquid probe reads the current network's data only.
                wallets: super::launcher_presenter::local_wallets(
                    cube,
                    chain == network && home.liquid_on_disk,
                ),
                vault: vault_subtitle(cube),
                key,
                cube,
                name,
            });
        }
    }
    // Every network with a Cube, local or only in Connect, whatever the
    // filter shows and whether or not the network is listed here (Developer
    // mode off lists Bitcoin alone, yet Connect may hold a Testnet4 Cube).
    // Counts come from `Home::cube_usage`, the same source as the Create gate,
    // so the dock never marks a network full that the gate does not, or the
    // reverse.
    let mut usage: Vec<NetworkUsage> = Vec::new();
    for chain in home.ordered_networks().into_iter().chain(ChainId::LAUNCHER) {
        if usage.iter().any(|u| u.chain == chain) {
            continue;
        }
        let (local, remote) = home.cube_usage(chain);
        if chain == network || local.unwrap_or(0) + remote > 0 {
            usage.push(NetworkUsage {
                chain,
                local,
                remote,
            });
        }
    }
    usage.sort_by_key(|u| network_rank(u.chain));
    let network_str = settings::network_to_api_string(network);
    let all_remotes: Vec<&RemoteCube> = home
        .remote_cubes
        .iter()
        .filter(|rc| rc.network == network_str && filter.matches(network))
        .collect();
    let remote_total = all_remotes.len();
    let remotes: Vec<&RemoteCube> = all_remotes
        .into_iter()
        .filter(|rc| matches_query(&catalog_display_name(&rc.name), &query))
        .collect();
    // The floor's ghosts follow the filter across networks, as local Cubes
    // do, in the networks' listed order.
    let all_ghosts: Vec<&RemoteCube> = home
        .ordered_networks()
        .into_iter()
        .filter(|chain| filter.matches(*chain))
        .flat_map(|chain| {
            home.remote_cubes
                .iter()
                .filter(move |rc| ghost_chain(rc) == Some(chain))
        })
        .collect();
    let ghosts: Vec<&RemoteCube> = all_ghosts
        .into_iter()
        .filter(|rc| matches_query(&catalog_display_name(&rc.name), &query))
        .collect();
    let hidden_ghosts = home
        .ordered_networks()
        .into_iter()
        .filter(|chain| !filter.matches(*chain))
        .flat_map(|chain| {
            home.remote_cubes
                .iter()
                .filter(move |rc| ghost_chain(rc) == Some(chain))
        })
        .filter(|rc| matches_query(&catalog_display_name(&rc.name), &query))
        .count();
    // A ghost is selected on the floor only, where it has a details face.
    let selected_ghost = home.catalog.selected.as_ref().and_then(|key| {
        if !catalog_visible(home) || home.catalog.mode != LauncherMode::Cubes {
            return None;
        }
        ghosts.iter().position(|cube| ghost_key(cube) == *key)
    });
    // Effective selection: the List is visible, the key resolves to exactly
    // one record, and that very record is a visible, actionable row.
    let selected = home.catalog.selected.as_ref().and_then(|key| {
        if !catalog_visible(home) || key.cube_id.is_empty() {
            return None;
        }
        let record = home.catalog_index(key)?;
        locals
            .iter()
            .position(|row| row.index == record && row.actionable)
    });
    Projection {
        connect_trouble: home.connect_account.unverified.as_ref().map(|u| u.trouble),
        local_total,
        filtered_total,
        hidden,
        hidden_ghosts,
        network_counts,
        usage,
        unread: home.network_catalog_unread.clone(),
        filter,
        remote_total,
        ghosts,
        locals,
        remotes,
        selected,
        selected_ghost,
        query,
        network,
        total_count: home.total_cube_count(),
        cube_limit: home.cube_limit(),
        tier: home.account_tier.display_name(),
    }
}

fn row_style(selected: bool) -> impl Fn(&theme::Theme, iced_button::Status) -> iced_button::Style {
    move |theme, status| {
        let shell = theme.colors.shell;
        let background = if selected {
            shell.selected
        } else if matches!(
            status,
            iced_button::Status::Hovered | iced_button::Status::Pressed
        ) {
            shell.hover
        } else {
            iced::Color::TRANSPARENT
        };
        iced_button::Style {
            background: Some(iced::Background::Color(background)),
            text_color: shell.ink,
            border: iced::Border {
                radius: 6.0.into(),
                ..Default::default()
            },
            ..Default::default()
        }
    }
}

fn link_style(theme: &theme::Theme, status: iced_button::Status) -> iced_button::Style {
    let shell = theme.colors.shell;
    iced_button::Style {
        background: Some(iced::Background::Color(
            if matches!(
                status,
                iced_button::Status::Hovered | iced_button::Status::Pressed
            ) {
                shell.hover
            } else {
                iced::Color::TRANSPARENT
            },
        )),
        text_color: if matches!(status, iced_button::Status::Disabled) {
            shell.muted
        } else {
            shell.accent_ink
        },
        border: iced::Border {
            radius: 6.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn panel_style(theme: &theme::Theme) -> container::Style {
    container::Style {
        background: Some(iced::Background::Color(theme.colors.shell.panel)),
        border: iced::Border {
            color: theme.colors.shell.line,
            width: 1.0,
            radius: 10.0.into(),
        },
        ..Default::default()
    }
}

fn tone_style(tone: Tone) -> impl Fn(&theme::Theme) -> iced::widget::text::Style {
    move |theme| {
        let shell = theme.colors.shell;
        iced::widget::text::Style {
            color: Some(match tone {
                Tone::Good => shell.good,
                Tone::Muted => shell.muted,
                Tone::Warn => shell.warn,
            }),
        }
    }
}

fn muted(theme: &theme::Theme) -> iced::widget::text::Style {
    iced::widget::text::Style {
        color: Some(theme.colors.shell.muted),
    }
}

/// The Launcher presenter's Seed phrase line, in the List's tones.
fn backup_caption(cube: &CubeSettings) -> (Tone, &'static str) {
    let (text, tone) = super::launcher_presenter::backup(cube);
    let tone = match tone {
        coincube_ui::theme::tenshu::Tone::Good => Tone::Good,
        coincube_ui::theme::tenshu::Tone::Warn => Tone::Warn,
        _ => Tone::Muted,
    };
    (tone, text)
}

fn wallets_cell<'a>(row: &LocalRow<'_>) -> Element<'a, ViewMessage> {
    Row::new()
        .spacing(6)
        .align_y(Alignment::Center)
        .width(Length::Fixed(WALLETS_WIDTH))
        .push(
            p2_regular(row.wallets.clone())
                .style(muted)
                .wrapping(Wrapping::WordOrGlyph),
        )
        .into()
}

fn avatar_style(theme: &theme::Theme) -> container::Style {
    container::Style {
        background: Some(iced::Background::Color(theme.colors.shell.selected)),
        border: iced::Border {
            radius: 16.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn cube_avatar<'a>(tone: Tone) -> Element<'a, ViewMessage> {
    Container::new(icon::cube_icon().style(tone_style(tone)))
        .width(Length::Fixed(32.0))
        .height(Length::Fixed(32.0))
        .align_x(iced::alignment::Horizontal::Center)
        .align_y(iced::alignment::Vertical::Center)
        .style(avatar_style)
        .into()
}

fn label_action<'a>(
    id: Id,
    label: &'static str,
    press: Option<ViewMessage>,
    style: fn(&theme::Theme, iced_button::Status) -> iced_button::Style,
) -> Action<'a, ViewMessage> {
    Action::new(
        id,
        Container::new(p2_medium(localized_copy(label))).padding([6, 12]),
        focus,
    )
    .style(style)
    .on_press_maybe(press)
}

fn local_row<'a>(row: &LocalRow<'_>, selected: bool, table: bool) -> Element<'a, ViewMessage> {
    let (tone, _) = row.connect.clone();
    let (backup_tone, backup) = backup_caption(row.cube);
    let name = Column::new()
        .spacing(2)
        .width(Length::Fill)
        .push(
            p1_bold(row.name.clone())
                .wrapping(Wrapping::WordOrGlyph)
                .width(Length::Fill),
        )
        .push(p2_regular(super::launcher_presenter::lock_state(row.open)).style(muted))
        .push(row.sync_warning.as_ref().map(|warning| {
            p2_regular(warning.clone())
                .style(tone_style(Tone::Warn))
                .wrapping(Wrapping::WordOrGlyph)
        }));
    let facts: Element<'a, ViewMessage> = if table {
        Row::new()
            .spacing(CELL_GAP)
            .align_y(Alignment::Center)
            .push(Container::new(cube_avatar(tone)).width(Length::Fixed(AVATAR_WIDTH)))
            .push(name)
            .push(
                p2_regular(row.key.chain.label())
                    .style(muted)
                    .width(Length::Fixed(NETWORK_WIDTH)),
            )
            .push(wallets_cell(row))
            .push(
                p2_regular(backup)
                    .style(tone_style(backup_tone))
                    .wrapping(Wrapping::WordOrGlyph)
                    .width(Length::Fixed(BACKUP_WIDTH)),
            )
            .into()
    } else {
        Column::new()
            .spacing(8)
            .width(Length::Fill)
            .push(
                Row::new()
                    .spacing(12)
                    .align_y(Alignment::Center)
                    .push(cube_avatar(tone))
                    .push(name),
            )
            .push(
                p2_regular(localized_format!(
                    "Network: {}  ·  Wallets: {}  ·  Backup: {}",
                    "Red: {} · Carteras: {} · Copia de seguridad: {}",
                    row.key.chain.label(),
                    row.wallets,
                    backup,
                ))
                .style(muted)
                .wrapping(Wrapping::WordOrGlyph),
            )
            .into()
    };
    let select = Action::new(select_id(&row.key), facts, focus)
        .padding([10, 12])
        .width(Length::Fill)
        .style(row_style(selected))
        .on_press_maybe(
            row.actionable
                .then(|| ViewMessage::Catalog(CatalogMessage::Select(row.key.clone()))),
        );
    // Open is a sibling of the selection control, never nested inside it, so
    // one activation cannot both select and open.
    Row::new()
        .spacing(if table { CELL_GAP } else { 8.0 })
        .align_y(Alignment::Center)
        .push(select)
        .push(
            Container::new(label_action(
                open_id(&row.key),
                localized_copy("Open"),
                row.actionable
                    .then(|| ViewMessage::RunCatalog(row.key.clone())),
                link_style,
            ))
            .width(if table {
                Length::Fixed(ACTION_WIDTH)
            } else {
                Length::Shrink
            }),
        )
        .into()
}

fn remote_row<'a>(cube: &RemoteCube) -> Element<'a, ViewMessage> {
    let restore = remote_restore(cube);
    Row::new()
        .spacing(8)
        .align_y(Alignment::Center)
        .push(
            Row::new()
                .spacing(14)
                .align_y(Alignment::Center)
                .width(Length::Fill)
                .padding([10, 12])
                .push(icon::cloud_arrow_down_icon().style(muted))
                .push(
                    Column::new()
                        .spacing(2)
                        .width(Length::Fill)
                        .push(
                            p1_bold(catalog_display_name(&cube.name))
                                .style(muted)
                                .wrapping(Wrapping::WordOrGlyph),
                        )
                        .push(
                            p2_regular(restore.status)
                                .style(muted)
                                .wrapping(Wrapping::WordOrGlyph),
                        ),
                ),
        )
        .push(restore.action.map(|(message, label)| {
            label_action(restore_id(&cube.uuid), label, Some(message), link_style)
        }))
        .push(label_action(
            remote_delete_id(&cube.uuid),
            localized_copy("Delete…"),
            Some(ViewMessage::DeleteCube(DeleteCubeMessage::ShowRemoteModal(
                cube.uuid.clone(),
            ))),
            link_style,
        ))
        .into()
}

fn detail_line<'a>(
    label: &'static str,
    value: Element<'a, ViewMessage>,
) -> Element<'a, ViewMessage> {
    Column::new()
        .spacing(2)
        .push(p2_regular(label).style(muted))
        .push(value)
        .into()
}

fn details_panel<'a>(row: &LocalRow<'_>) -> Element<'a, ViewMessage> {
    let (tone, caption) = row.connect.clone();
    let (backup_tone, backup) = backup_caption(row.cube);
    Container::new(
        Column::new()
            .spacing(16)
            .push(
                Row::new()
                    .spacing(12)
                    .align_y(Alignment::Center)
                    .push(
                        h4_bold(row.name.clone())
                            .wrapping(Wrapping::WordOrGlyph)
                            .width(Length::Fill),
                    )
                    .push(label_action(
                        close_details_id(),
                        localized_copy("Close"),
                        Some(ViewMessage::Catalog(CatalogMessage::ClearSelection)),
                        link_style,
                    )),
            )
            .push(detail_line(
                localized_copy("Network"),
                p2_regular(row.key.chain.label()).into(),
            ))
            .push(detail_line(
                localized_copy("Stored"),
                p2_regular(localized_copy("On this device")).into(),
            ))
            .push(detail_line(
                localized_copy("Lock"),
                p2_regular(super::launcher_presenter::lock_state(row.open))
                    .style(muted)
                    .into(),
            ))
            .push(detail_line(
                localized_copy("Seed phrase"),
                p2_regular(backup).style(tone_style(backup_tone)).into(),
            ))
            .push(detail_line(
                "Vault",
                p2_regular(row.vault.clone())
                    .wrapping(Wrapping::WordOrGlyph)
                    .into(),
            ))
            .push(detail_line(
                localized_copy("Recovery Kit"),
                p2_regular(caption)
                    .style(tone_style(tone))
                    .wrapping(Wrapping::WordOrGlyph)
                    .into(),
            ))
            .push(detail_line(
                localized_copy("Wallets"),
                p2_regular(row.wallets.clone())
                    .style(muted)
                    .wrapping(Wrapping::WordOrGlyph)
                    .into(),
            ))
            // Passkey metadata is a stored fact; PIN presence is not known
            // before unlock, so nothing is claimed for other Cubes.
            .push(
                row.cube.is_passkey_cube().then(|| {
                    detail_line(localized_copy("Unlocks with"), p2_regular("Passkey").into())
                }),
            )
            .push(
                p2_regular(localized_copy(
                    "Opening asks for its PIN or passkey on the next screen.",
                ))
                .style(muted)
                .wrapping(Wrapping::WordOrGlyph),
            )
            .push(
                Row::new()
                    .spacing(8)
                    .push(label_action(
                        details_open_id(&row.key),
                        localized_copy("Open"),
                        Some(ViewMessage::RunCatalog(row.key.clone())),
                        theme::button::primary,
                    ))
                    .push(label_action(
                        rename_id(&row.key),
                        "Rename",
                        Some(ViewMessage::RenameCatalog(row.key.clone())),
                        link_style,
                    ))
                    .push(label_action(
                        delete_id(&row.key),
                        "Delete…",
                        Some(ViewMessage::DeleteCatalog(row.key.clone())),
                        link_style,
                    )),
            ),
    )
    .padding([24, 28])
    .width(Length::Fill)
    .max_width(DETAILS_WIDTH)
    .style(panel_style)
    .into()
}

fn section_heading<'a>(title: &'static str, count: usize) -> Element<'a, ViewMessage> {
    Row::new()
        .spacing(8)
        .align_y(Alignment::Center)
        .padding([0, 12])
        .push(p2_bold(title))
        .push(p2_regular(count.to_string()).style(muted))
        .into()
}

/// Mirrors the reference's six slots. The final slot is reserved for the
/// independent Open Action, so headings stay aligned with row content.
fn table_heading<'a>() -> Element<'a, ViewMessage> {
    Row::new()
        .spacing(CELL_GAP)
        .align_y(Alignment::Center)
        .padding([6, 12])
        .width(Length::Fill)
        .push(Space::new().width(Length::Fixed(AVATAR_WIDTH)))
        .push(p2_regular("Cube").style(muted).width(Length::Fill))
        .push(
            p2_regular(localized_copy("Network"))
                .style(muted)
                .width(Length::Fixed(NETWORK_WIDTH)),
        )
        .push(
            p2_regular(localized_copy("Wallets"))
                .style(muted)
                .width(Length::Fixed(WALLETS_WIDTH)),
        )
        .push(
            p2_regular(localized_copy("Seed phrase"))
                .style(muted)
                .width(Length::Fixed(BACKUP_WIDTH)),
        )
        .push(Space::new().width(Length::Fixed(ACTION_WIDTH)))
        .into()
}

fn body_style(theme: &theme::Theme) -> container::Style {
    container::Style {
        background: Some(iced::Background::Color(theme.colors.shell.background)),
        text_color: Some(theme.colors.shell.ink),
        ..Default::default()
    }
}

/// The visible Launcher: compact top bar, one scrolling floor or List body,
/// and the pinned dock. Both bodies consume the same catalog projection.
#[cfg(test)]
pub(super) fn launcher_view(home: &Home) -> Element<'_, ViewMessage> {
    launcher_view_with_worldview(home, Default::default())
}

pub(super) fn launcher_view_with_worldview(
    home: &Home,
    worldview: crate::app::settings::preferences::WorldviewMode,
) -> Element<'_, ViewMessage> {
    launcher_screen(home, true, worldview)
}

/// The Launcher under a wizard sheet: the same top bar and body, without
/// the dock, which the sheet would half cover. It is drawn inert.
pub(super) fn launcher_backdrop(
    home: &Home,
    worldview: crate::app::settings::preferences::WorldviewMode,
) -> Element<'_, ViewMessage> {
    launcher_screen(home, false, worldview)
}

fn launcher_screen(
    home: &Home,
    with_dock: bool,
    worldview: crate::app::settings::preferences::WorldviewMode,
) -> Element<'_, ViewMessage> {
    let projection = project(home);
    // `.quiet`: the bars mute while a Cube's face is up, as in every
    // non-browse phase of the prototype.
    let quiet = projection.selected.is_some();
    let dock = super::launcher_chrome::dock(home, &projection, quiet);
    let error = home.error.clone().filter(|_| !home.showing_create_form());
    let floor_visible = projection.floor_eligible(home.catalog.mode);
    // The floor's stage fills the visible body, so it is laid out against
    // the body's own height rather than the scrolled content's.
    let body = responsive(move |size: Size| {
        let body_content = Column::new()
            .spacing(if floor_visible { 0 } else { 16 })
            .width(Length::Fill)
            .push(error.clone().map(|error| card::simple(p1_regular(error))))
            .push(if floor_visible {
                super::launcher_presenter::floor_body_with_worldview(home, size.height, worldview)
            } else {
                super::launcher_presenter::list_body_with_worldview(home, worldview)
            });
        let body_content = if floor_visible {
            body_content
        } else {
            body_content.padding([20.0, GUTTER])
        };
        scrollable(body_content)
            .id(CATALOG_SCROLL_ID)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    })
    .width(Length::Fill)
    .height(Length::Fill);
    let screen = Column::new()
        .width(Length::Fill)
        .height(Length::Fill)
        .push(super::launcher_chrome::top_bar(home, &projection, quiet))
        .push(rule::horizontal(1))
        .push(body);
    let screen = if with_dock {
        screen.push(rule::horizontal(1)).push(dock)
    } else {
        screen
    };
    Container::new(
        iced::widget::Stack::new()
            .width(Length::Fill)
            .height(Length::Fill)
            .push(screen)
            .push(super::launcher_chrome::key_layer(home, &projection))
            .push(super::launcher_chrome::activity_layer(home))
            .push(super::launcher_chrome::plan_layer(home, &projection))
            .push(super::launcher_chrome::network_layer(home, &projection)),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .style(body_style)
    .into()
}

/// The List body. Pure: it reads Home and builds widgets, nothing else.
pub(super) fn catalog_view(home: &Home) -> Element<'_, ViewMessage> {
    let projection = project(home);
    responsive(move |size: Size| layout(&projection, size.width))
        .height(Length::Shrink)
        .into()
}

/// Owns everything it renders, so the element does not borrow the Home the
/// projection was read from.
pub(super) fn layout<'a>(projection: &Projection<'_>, width: f32) -> Element<'a, ViewMessage> {
    let list_width =
        if projection.selected.is_some() && width >= LIST_MIN_WIDTH + DETAILS_WIDTH + PANEL_GAP {
            width - DETAILS_WIDTH - PANEL_GAP
        } else {
            width
        };
    let table = width >= TABLE_MIN_VIEW_WIDTH && list_width >= TABLE_MIN_LIST_WIDTH;
    let search = Row::new()
        .spacing(8)
        .align_y(Alignment::Center)
        .push(
            TextInput::new(localized_copy("Search Cubes by name"), &projection.query)
                .id(search_input_id())
                .on_input(|query| ViewMessage::Catalog(CatalogMessage::Query(query)))
                .padding(10)
                .width(Length::Fill),
        )
        .push((!projection.query.is_empty()).then(|| {
            label_action(
                clear_query_id(),
                localized_copy("Clear"),
                Some(ViewMessage::Catalog(CatalogMessage::ClearQuery)),
                link_style,
            )
        }));

    let mut list = Column::new().spacing(4).width(Length::Fill).push(search);
    if projection.no_matches() {
        list = list.push(
            Column::new()
                .spacing(8)
                .padding([16, 12])
                .push(p1_regular(localized_format!(
                    "No Cubes match “{}”.",
                    "No Cubes coinciden con “{}”.",
                    projection.query.trim()
                )))
                .push(label_action(
                    control_id("clear-query-empty"),
                    localized_copy("Clear search"),
                    Some(ViewMessage::Catalog(CatalogMessage::ClearQuery)),
                    link_style,
                )),
        );
    }
    if !projection.locals.is_empty() {
        list = list
            .push(Space::new().height(Length::Fixed(8.0)))
            .push(section_heading("On this device", projection.locals.len()))
            .push(rule::horizontal(1));
        if table {
            list = list.push(table_heading());
        }
        for (index, row) in projection.locals.iter().enumerate() {
            list = list.push(local_row(row, projection.selected == Some(index), table));
        }
    }
    if !projection.remotes.is_empty() {
        list = list
            .push(Space::new().height(Length::Fixed(16.0)))
            .push(section_heading(
                "Available in Connect",
                projection.remotes.len(),
            ))
            .push(rule::horizontal(1));
        for cube in projection.remotes.iter().copied() {
            list = list.push(remote_row(cube));
        }
    }
    let details = projection
        .selected
        .and_then(|index| projection.locals.get(index))
        .map(details_panel);
    match details {
        None => list.into(),
        Some(details) if width >= LIST_MIN_WIDTH + DETAILS_WIDTH + PANEL_GAP => Row::new()
            .spacing(PANEL_GAP)
            .align_y(Alignment::Start)
            .push(list)
            .push(Container::new(details).width(Length::Fixed(DETAILS_WIDTH)))
            .into(),
        // Compact: the details stack above the list, at full width, so they
        // stay reachable without overflowing beside it.
        Some(details) => Column::new()
            .spacing(16)
            .width(Length::Fill)
            .push(details)
            .push(list)
            .into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::view::shared::landing::capture;
    use crate::dir::CoincubeDirectory;
    use coincube_core::miniscript::bitcoin::Network;
    use coincube_ui::font;
    use iced::advanced::{
        layout as widget_layout,
        renderer::Headless,
        widget::{operation::Focusable, Operation, Tree},
        Layout,
    };
    use iced::Rectangle;
    use std::collections::HashSet;
    use std::path::PathBuf;

    const COMPACT: (u32, u32) = (1024, 768);
    const WIDE: (u32, u32) = (1440, 900);
    /// Tall enough that every pointer target is inside the viewport.
    const TALL: (u32, u32) = (1440, 4000);

    fn key(id: &str) -> CubeCatalogKey {
        CubeCatalogKey {
            chain: ChainId::Bitcoin,
            cube_id: id.to_string(),
        }
    }

    fn home_with(cubes: &[(&str, &str)]) -> Home {
        let mut home = Home::new(
            CoincubeDirectory::new(PathBuf::new()),
            Some(Network::Bitcoin),
        )
        .0;
        home.network = ChainId::Bitcoin;
        home.state = State::Cubes {
            cubes: cubes
                .iter()
                .map(|(id, name)| {
                    CubeSettings::new_with_raw_id(
                        id.to_string(),
                        name.to_string(),
                        ChainId::Bitcoin,
                    )
                })
                .collect(),
            create_cube: false,
            source: ChainId::Bitcoin,
        };
        // This fixture exercises the accessible List after the separate
        // first-launch firmware advisory has been dismissed. A live advisory
        // owns the Modal overlay and intentionally hides background controls.
        home.advisory_notice = None;
        home.catalog.mode = LauncherMode::List;
        home
    }

    fn remote(
        uuid: &str,
        name: &str,
        kit: bool,
        seed: bool,
        phone: bool,
        full: bool,
    ) -> RemoteCube {
        RemoteCube {
            id: 7,
            uuid: uuid.to_string(),
            name: name.to_string(),
            network: settings::network_to_api_string(ChainId::Bitcoin),
            has_recovery_kit: kit,
            has_encrypted_seed: seed,
            phone_recoverable: phone,
            phone_full_cube: full,
            encryption_pubkey: None,
            has_vault: None,
        }
    }

    fn catalog(home: &mut Home, message: CatalogMessage) {
        let _ = home.update(Message::View(ViewMessage::Catalog(message)));
    }

    /// Moves a real pointer over the mounted Launcher, feeding each batch of
    /// messages back through `Home::update` before the next move.
    fn pointer_path(home: &mut Home, points: &[iced::Point]) -> Vec<ViewMessage> {
        let mut all = Vec::new();
        for point in points {
            let cursor = iced::mouse::Cursor::Available(*point);
            let messages = {
                let mut mounted = mount_window(launcher_view(home), 1440.0, 900.0);
                // A fresh mount has no hover state: replay the previous point
                // (already applied to `home`) and keep only this move's output.
                let previous = all_last(&all, iced::Point::new(720.0, 450.0));
                let _ = mounted.send(
                    iced::Event::Mouse(iced::mouse::Event::CursorMoved { position: previous }),
                    iced::mouse::Cursor::Available(previous),
                );
                mounted.send(
                    iced::Event::Mouse(iced::mouse::Event::CursorMoved { position: *point }),
                    cursor,
                )
            };
            for message in &messages {
                let _ = home.update(Message::View(message.clone()));
            }
            all.push((*point, messages));
        }
        all.into_iter().flat_map(|(_, messages)| messages).collect()
    }

    fn all_last(path: &[(iced::Point, Vec<ViewMessage>)], fallback: iced::Point) -> iced::Point {
        path.last().map(|(point, _)| *point).unwrap_or(fallback)
    }

    #[test]
    fn a_peeked_key_card_survives_the_pointer_moving_up_onto_it() {
        let mut home = home_with(&[("a", "Alpha")]);
        let button = bounds_of(
            &focus_bounds_window(launcher_view(&home), 1440.0, 900.0),
            &key_id(),
        );
        let on_button = button.center();
        // 4px above the button: inside the 8px gap that belongs to the card.
        let in_gap = iced::Point::new(on_button.x, button.y - 4.0);
        let card = iced::Point::new(on_button.x, button.y - 40.0);
        let messages = pointer_path(&mut home, &[on_button, in_gap, card]);
        assert!(
            home.catalog.key_open(),
            "the peek closed on the way up: {:?} -> {:?}",
            messages,
            home.catalog.key
        );
        assert_eq!(home.catalog.key.mode, KeyMode::Peek);
    }

    #[test]
    fn hiding_the_key_card_under_the_pointer_lets_hover_peek_again() {
        let mut home = home_with(&[("a", "Alpha")]);
        let button = bounds_of(
            &focus_bounds_window(launcher_view(&home), 1440.0, 900.0),
            &key_id(),
        );
        let on_button = button.center();
        let card = iced::Point::new(on_button.x, button.y - 40.0);
        let away = iced::Point::new(720.0, 450.0);
        // Escape while resting on a peeked card.
        pointer_path(&mut home, &[on_button, card]);
        assert_eq!(home.catalog.key.mode, KeyMode::Peek);
        catalog(&mut home, CatalogMessage::KeyClose);
        assert!(!home.catalog.key_open());
        pointer_path(&mut home, &[away, on_button]);
        assert_eq!(
            home.catalog.key.mode,
            KeyMode::Peek,
            "hover must peek again: {:?}",
            home.catalog.key
        );
        // A keyboard toggle that hides a pinned card under the pointer.
        catalog(&mut home, CatalogMessage::KeyToggle);
        // Each call starts a fresh pointer, so the path begins on the button.
        pointer_path(&mut home, &[on_button, card]);
        assert_eq!(home.catalog.key.mode, KeyMode::Pinned);
        catalog(&mut home, CatalogMessage::KeyToggle);
        assert!(!home.catalog.key_open());
        pointer_path(&mut home, &[away, on_button]);
        assert_eq!(
            home.catalog.key.mode,
            KeyMode::Peek,
            "{:?}",
            home.catalog.key
        );
    }

    #[test]
    fn presses_inside_the_network_menu_do_not_close_it() {
        use super::super::launcher_chrome::network_option_id;
        let mut home = rows(2);
        catalog(&mut home, CatalogMessage::NetworkMenu(true));
        let row = bounds_of(
            &focus_bounds_window(launcher_view(&home), 1440.0, 900.0),
            &network_option_id(home.network),
        );
        // The filter note sits under the last row, inside the menu.
        let note = iced::Point::new(row.x + 20.0, row.y + row.height + 24.0);
        let mut mounted = mount_window(launcher_view(&home), 1440.0, 900.0);
        let cursor = iced::mouse::Cursor::Available(note);
        let mut messages = mounted.send(
            iced::Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left)),
            cursor,
        );
        messages.extend(mounted.send(
            iced::Event::Mouse(iced::mouse::Event::ButtonReleased(
                iced::mouse::Button::Left,
            )),
            cursor,
        ));
        assert!(
            !messages
                .iter()
                .any(|m| matches!(m, ViewMessage::Catalog(CatalogMessage::NetworkMenu(false)))),
            "{:?}",
            messages
        );
        // A press outside the menu still closes it.
        let outside = iced::mouse::Cursor::Available(iced::Point::new(720.0, 700.0));
        let messages = mounted.send(
            iced::Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left)),
            outside,
        );
        assert!(messages
            .iter()
            .any(|m| matches!(m, ViewMessage::Catalog(CatalogMessage::NetworkMenu(false)))));
    }

    #[test]
    fn leaving_the_launcher_closes_its_popovers() {
        let mut home = home_with(&[("a", "Alpha")]);
        catalog(&mut home, CatalogMessage::KeyHover(KeyPart::Button, true));
        catalog(&mut home, CatalogMessage::NetworkMenu(true));
        let _ = home.update(Message::View(ViewMessage::ShowCreateCube(true)));
        assert!(!home.catalog.key_open());
        assert!(!home.catalog.network_menu);
        assert_eq!(home.catalog.key, KeyCard::default());
    }

    #[test]
    fn the_key_peeks_on_hover_pins_on_click_and_shuts_until_the_pointer_leaves() {
        let mut home = home_with(&[("a", "Alpha")]);
        let closed = home_tree("key-card", &home, WIDE, "dark", theme::Theme::dark());
        assert!(closed.has(&key_id()));
        assert!(!closed.contains("Network coin"));

        catalog(&mut home, CatalogMessage::KeyHover(KeyPart::Button, true));
        assert_eq!(home.catalog.key.mode, KeyMode::Peek);
        assert!(
            home_tree("key-card", &home, WIDE, "dark", theme::Theme::dark())
                .contains("Network coin")
        );
        catalog(&mut home, CatalogMessage::KeyHover(KeyPart::Button, false));
        assert!(!home.catalog.key_open(), "a peek closes on hover-out");

        catalog(&mut home, CatalogMessage::KeyHover(KeyPart::Button, true));
        catalog(&mut home, CatalogMessage::KeyToggle);
        catalog(&mut home, CatalogMessage::KeyHover(KeyPart::Button, false));
        assert_eq!(home.catalog.key.mode, KeyMode::Pinned, "a click pins it");

        catalog(&mut home, CatalogMessage::KeyHover(KeyPart::Button, true));
        catalog(&mut home, CatalogMessage::KeyToggle);
        assert_eq!(
            home.catalog.key.mode,
            KeyMode::Shut,
            "a second click shuts it"
        );
        assert!(!home.catalog.key_open());
        catalog(&mut home, CatalogMessage::KeyHover(KeyPart::Button, false));
        catalog(&mut home, CatalogMessage::KeyHover(KeyPart::Button, true));
        assert_eq!(
            home.catalog.key.mode,
            KeyMode::Peek,
            "leaving clears the shut"
        );

        catalog(&mut home, CatalogMessage::KeyClose);
        assert_eq!(home.catalog.key.mode, KeyMode::Shut, "Escape shuts a peek");
        catalog(&mut home, CatalogMessage::KeyHover(KeyPart::Button, false));
        catalog(&mut home, CatalogMessage::KeyToggle);
        catalog(&mut home, CatalogMessage::KeyClose);
        assert_eq!(
            home.catalog.key.mode,
            KeyMode::Closed,
            "Escape away from the pointer closes, so hover can peek again"
        );
    }

    fn cubes_mut(home: &mut Home) -> &mut Vec<CubeSettings> {
        match &mut home.state {
            State::Cubes { cubes, .. } => cubes,
            _ => panic!("expected the Cubes catalog"),
        }
    }

    fn selected_id(home: &Home) -> Option<String> {
        let projection = project(home);
        projection
            .selected
            .map(|index| projection.locals[index].key.cube_id.clone())
    }

    #[test]
    fn floor_fallback_keeps_search_and_clear_visible_after_filter_or_mode_change() {
        let mut many = rows(21);
        many.catalog.mode = LauncherMode::Cubes;
        assert!(
            project(&many).floor_eligible(many.catalog.mode),
            "no Cube count takes the floor away"
        );
        catalog(&mut many, CatalogMessage::Query("Cube 20".into()));
        let filtered = project(&many);
        assert_eq!(filtered.local_total, 21);
        assert_eq!(filtered.locals.len(), 1);
        assert!(!filtered.floor_eligible(many.catalog.mode));
        let tree = home_tree(
            "v113-floor-filtered-21",
            &many,
            COMPACT,
            "dark",
            theme::Theme::dark(),
        );
        assert!(tree.has(&search_id()));
        assert!(tree.has(&Id::from("tenshu.launcher.catalog:control:clear-query")));
        assert!(tree.has(&select_id(&key("cube-20"))));

        let mut small = rows(4);
        catalog(&mut small, CatalogMessage::Query("no match".into()));
        small.catalog.mode = LauncherMode::Cubes;
        assert!(!project(&small).floor_eligible(small.catalog.mode));
        let tree = home_tree(
            "v113-floor-mode-change-with-query",
            &small,
            COMPACT,
            "dark",
            theme::Theme::dark(),
        );
        assert!(tree.has(&search_id()));
        assert!(tree.has(&Id::from("tenshu.launcher.catalog:control:clear-query")));
        assert!(tree.contains("No Cubes match"));
    }

    #[test]
    fn floor_search_clears_selection_but_list_search_restores_it() {
        let mut floor = rows(5);
        floor.catalog.mode = LauncherMode::Cubes;
        catalog(&mut floor, CatalogMessage::Select(key("cube-0")));
        assert_eq!(floor.catalog.selected, Some(key("cube-0")));
        catalog(&mut floor, CatalogMessage::Query("Cube 4".into()));
        assert_eq!(floor.catalog.selected, None);
        assert_eq!(floor.catalog.page, 0);

        let mut list = rows(5);
        catalog(&mut list, CatalogMessage::Select(key("cube-0")));
        catalog(&mut list, CatalogMessage::Query("Cube 4".into()));
        assert_eq!(list.catalog.selected, Some(key("cube-0")));
        assert_eq!(project(&list).selected, None);
        catalog(&mut list, CatalogMessage::ClearQuery);
        assert_eq!(selected_id(&list).as_deref(), Some("cube-0"));
    }

    #[test]
    fn floor_remote_only_cubes_stand_as_ghosts_in_the_first_viewport() {
        for (appearance, palette) in [
            ("dark", theme::Theme::dark()),
            ("light", theme::Theme::light()),
        ] {
            let mut home = home_with(&[]);
            home.state = State::NoCube { create_cube: false };
            home.catalog.mode = LauncherMode::Cubes;
            home.remote_cubes = vec![remote(
                "restore-me",
                "Connect backup",
                true,
                true,
                false,
                false,
            )];
            let projection = project(&home);
            assert_eq!(projection.local_total, 0);
            assert_eq!(projection.remote_total, 1);
            // A new device's floor stands the Connect-only Cube as a ghost.
            assert!(projection.floor_eligible(home.catalog.mode));
            let tree = home_tree(
                "v113-floor-remote-only",
                &home,
                COMPACT,
                appearance,
                palette,
            );
            let restore = tree.bounds(&select_id(&ghost(&home, "restore-me")));
            let (viewport, _, _) = tree.scroll(&Id::from(CATALOG_SCROLL_ID));
            assert!(restore.y >= viewport.y);
            assert!(restore.y + restore.height <= viewport.y + viewport.height);
            assert!(!tree.contains("Available in Connect"));
            assert!(tree.contains("In Connect · Restore"));
            assert!(tree.contains(super::super::launcher_presenter::GHOST_INTRO));
            assert!(!tree.contains("No Cubes on this network"));
        }
    }

    #[test]
    fn paging_clears_hidden_selection_and_clamps_after_refresh() {
        let mut home = home_with(&[
            ("a", "A"),
            ("b", "B"),
            ("c", "C"),
            ("d", "D"),
            ("e", "E"),
            ("f", "F"),
            ("g", "G"),
            ("h", "H"),
            ("i", "I"),
        ]);
        home.catalog.mode = LauncherMode::Cubes;
        catalog(&mut home, CatalogMessage::Select(key("b")));
        assert_eq!(home.catalog.selected, Some(key("b")));
        catalog(&mut home, CatalogMessage::Page(2));
        assert_eq!(home.catalog.page, 2);
        assert!(home.catalog.selected.is_none());
        catalog(&mut home, CatalogMessage::Select(key("i")));
        assert_eq!(home.catalog.page, 2);
        cubes_mut(&mut home).truncate(3);
        catalog(&mut home, CatalogMessage::ClearQuery);
        assert_eq!(home.catalog.page, 0);
        assert!(home.catalog.selected.is_none());
        catalog(&mut home, CatalogMessage::Page(9));
        assert_eq!(home.catalog.page, 0);
    }

    #[test]
    fn floor_keyboard_input_uses_page_intent_and_leaves_list_or_search_alone() {
        let mut home = rows(9);
        home.catalog.mode = LauncherMode::Cubes;
        catalog(&mut home, CatalogMessage::Select(key("cube-0")));
        for (key, expected) in [
            (key::Named::ArrowRight, 1),
            (key::Named::PageDown, 2),
            (key::Named::ArrowRight, 2),
            (key::Named::PageUp, 1),
            (key::Named::ArrowLeft, 0),
        ] {
            let named = keyboard::Key::Named(key);
            let event = iced::Event::Keyboard(keyboard::Event::KeyPressed {
                key: named.clone(),
                modified_key: named,
                physical_key: key::Physical::Unidentified(key::NativeCode::Unidentified),
                location: keyboard::Location::Standard,
                modifiers: keyboard::Modifiers::empty(),
                text: None,
                repeat: false,
            });
            let message = floor_event(
                event.clone(),
                event::Status::Ignored,
                iced::window::Id::unique(),
            )
            .expect("unhandled page key should route to Home");
            assert!(
                floor_event(event, event::Status::Captured, iced::window::Id::unique()).is_none()
            );
            let _ = home.update(message);
            assert_eq!(home.catalog.page, expected);
            assert!(home.catalog.selected.is_none());
        }
        home.catalog.mode = LauncherMode::List;
        catalog(&mut home, CatalogMessage::Step(1));
        assert_eq!(home.catalog.page, 0);
        home.catalog.mode = LauncherMode::Cubes;
        catalog(&mut home, CatalogMessage::Query("Cube 8".into()));
        catalog(&mut home, CatalogMessage::Step(1));
        assert_eq!(home.catalog.page, 0);
        assert_eq!(home.catalog.query, "Cube 8");
    }

    #[test]
    fn floor_horizontal_trackpad_accumulates_and_debounces_without_vertical_turns() {
        let mut home = rows(9);
        home.catalog.mode = LauncherMode::Cubes;
        let start = Instant::now();
        let pixels = |x, y| mouse::ScrollDelta::Pixels { x, y };
        let captured_sideways = floor_event(
            iced::Event::Mouse(mouse::Event::WheelScrolled {
                delta: pixels(-25.0, 0.0),
            }),
            event::Status::Captured,
            iced::window::Id::unique(),
        );
        assert!(matches!(
            captured_sideways,
            Some(Message::View(ViewMessage::Catalog(CatalogMessage::Wheel(
                _
            ))))
        ));
        assert!(floor_event(
            iced::Event::Mouse(mouse::Event::WheelScrolled {
                delta: pixels(0.0, -100.0),
            }),
            event::Status::Captured,
            iced::window::Id::unique(),
        )
        .is_none());
        let _ = home.wheel_floor_page(pixels(0.0, -100.0), start);
        assert_eq!(home.catalog.page, 0);
        let _ = home.wheel_floor_page(pixels(-10.0, 0.0), start);
        let _ = home.wheel_floor_page(pixels(-10.0, 0.0), start);
        assert_eq!(home.catalog.page, 0);
        let _ = home.wheel_floor_page(pixels(-5.0, 0.0), start);
        assert_eq!(home.catalog.page, 1);
        let _ = home.wheel_floor_page(pixels(-100.0, 0.0), start + Duration::from_millis(100));
        assert_eq!(home.catalog.page, 1);
        let _ = home.wheel_floor_page(pixels(-30.0, 0.0), start + Duration::from_millis(601));
        assert_eq!(home.catalog.page, 2);
        let _ = home.wheel_floor_page(
            mouse::ScrollDelta::Lines { x: 1.0, y: 0.0 },
            start + Duration::from_millis(1202),
        );
        assert_eq!(home.catalog.page, 1);
        home.catalog.mode = LauncherMode::List;
        let _ = home.wheel_floor_page(pixels(-50.0, 0.0), start + Duration::from_millis(1803));
        assert_eq!(home.catalog.page, 1);
    }

    #[test]
    fn separate_short_horizontal_gestures_do_not_combine_into_a_page_turn() {
        let mut home = rows(9);
        home.catalog.mode = LauncherMode::Cubes;
        let start = Instant::now();
        let pixels = |x| mouse::ScrollDelta::Pixels { x, y: 0.0 };

        let _ = home.wheel_floor_page(pixels(-10.0), start);
        assert_eq!(home.catalog.page, 0);
        let _ = home.wheel_floor_page(pixels(-15.0), start + Duration::from_secs(1));
        assert_eq!(home.catalog.page, 0);
        assert_eq!(home.catalog.wheel_pixels, -15.0);

        let _ = home.wheel_floor_page(pixels(-10.0), start + Duration::from_millis(1020));
        assert_eq!(home.catalog.page, 1);
        assert_eq!(home.catalog.wheel_pixels, 0.0);
    }

    #[test]
    fn continuous_sideways_gesture_turns_only_one_page_after_cooldown() {
        let mut home = rows(9);
        home.catalog.mode = LauncherMode::Cubes;
        let start = Instant::now();
        let pixels = mouse::ScrollDelta::Pixels { x: -30.0, y: 0.0 };

        let _ = home.wheel_floor_page(pixels, start);
        assert_eq!(home.catalog.page, 1);
        for offset in (100..=1200).step_by(100) {
            let _ = home.wheel_floor_page(pixels, start + Duration::from_millis(offset));
            assert_eq!(home.catalog.page, 1, "same gesture turned at {}ms", offset);
        }
        let _ = home.wheel_floor_page(pixels, start + Duration::from_millis(1500));
        assert_eq!(home.catalog.page, 2, "a later gesture can turn again");
    }

    #[test]
    fn only_the_visible_home_tab_subscribes_to_floor_input() {
        use crate::gui::{pane, tab};
        use std::hash::Hasher as _;

        // The floor listener and the network activity check: both belong
        // to the visible tab only.
        fn recipes_hash(subscription: iced::Subscription<pane::Message>) -> u64 {
            let recipes = iced::advanced::subscription::into_recipes(subscription);
            assert_eq!(recipes.len(), 2);
            let mut hasher = iced::advanced::subscription::Hasher::default();
            for recipe in recipes {
                recipe.hash(&mut hasher);
            }
            hasher.finish()
        }
        // A settled sign-in, so the Connect light has no pulse to tick.
        let settled = || crate::app::state::connect::ConnectFlowStep::Login {
            email: String::new(),
            loading: false,
        };
        let mut first = rows(9);
        first.catalog.mode = LauncherMode::Cubes;
        first.connect_account.step = settled();
        let mut second = rows(9);
        second.catalog.mode = LauncherMode::Cubes;
        second.connect_account.step = settled();
        let mut pane = pane::Pane::new_with_tab(tab::State::Home(first));
        pane.tabs.push(tab::Tab::new(2, tab::State::Home(second)));

        let first_hash = recipes_hash(pane.subscription(true));
        assert_eq!(
            iced::advanced::subscription::into_recipes(pane.subscription(false)).len(),
            0,
            "a hidden pane has no floor listener"
        );
        pane.focused_tab = 1;
        let second_hash = recipes_hash(pane.subscription(true));
        assert_ne!(first_hash, second_hash, "switching tabs moves the listener");
    }

    #[test]
    fn selection_follows_identity_through_reorder_and_is_dropped_on_removal() {
        let mut home = home_with(&[("a", "Alpha"), ("b", "Beta"), ("c", "Gamma")]);
        catalog(&mut home, CatalogMessage::Select(key("b")));
        assert_eq!(home.catalog.selected, Some(key("b")));
        assert_eq!(selected_id(&home).as_deref(), Some("b"));

        cubes_mut(&mut home).reverse();
        assert_eq!(selected_id(&home).as_deref(), Some("b"));
        assert_eq!(project(&home).selected, Some(1));

        // An async refresh removes it: the view stops showing details at once,
        // and the next message clears the stale key instead of moving it.
        cubes_mut(&mut home).retain(|cube| cube.id != "b");
        assert_eq!(selected_id(&home), None);
        catalog(&mut home, CatalogMessage::ClearQuery);
        assert_eq!(home.catalog.selected, None);

        catalog(&mut home, CatalogMessage::Select(key("missing")));
        assert_eq!(home.catalog.selected, None);
        catalog(&mut home, CatalogMessage::Select(key("a")));
        catalog(&mut home, CatalogMessage::ClearSelection);
        assert_eq!(home.catalog.selected, None);
    }

    #[test]
    fn duplicate_empty_and_foreign_identities_are_never_actionable() {
        let mut duplicate = home_with(&[("a", "First"), ("a", "Second")]);
        catalog(&mut duplicate, CatalogMessage::Select(key("a")));
        assert_eq!(duplicate.catalog.selected, None);
        assert!(project(&duplicate).locals.iter().all(|row| !row.actionable));

        let mut empty = home_with(&[("", "Nameless")]);
        catalog(&mut empty, CatalogMessage::Select(key("")));
        assert_eq!(empty.catalog.selected, None);
        assert!(!project(&empty).locals[0].actionable);

        // Records read from another chain's directory: shown, never opened.
        let mut foreign = home_with(&[("a", "Alpha")]);
        if let State::Cubes { source, .. } = &mut foreign.state {
            *source = ChainId::Testnet4;
        }
        catalog(&mut foreign, CatalogMessage::Select(key("a")));
        assert_eq!(foreign.catalog.selected, None);
        assert!(!project(&foreign).locals[0].actionable);

        for home in [&duplicate, &empty, &foreign] {
            for size in [COMPACT, WIDE] {
                let inspection = capture::inspect(
                    "catalog-unresolvable-identities",
                    catalog_view(home),
                    size,
                    "dark",
                    theme::Theme::dark(),
                );
                for row in project(home).locals {
                    assert!(!inspection.has_focus(select_id(&row.key)));
                    assert!(!inspection.has_focus(open_id(&row.key)));
                }
            }
        }
    }

    #[test]
    fn hovered_cube_floats_settles_and_stops_when_selected() {
        use super::super::launcher_scene::{BOB_HEIGHT, BOB_PERIOD, BOB_SETTLE};
        let mut home = home_with(&[("a", "Alpha"), ("b", "Beta")]);
        let start = Instant::now();
        assert!(!home.catalog.animating());
        home.catalog.hover(key("a"), true, start);
        assert!(home.catalog.animating(), "a floating Cube needs frames");
        let top = start + BOB_PERIOD / 2;
        assert!((home.catalog.lift(&key("a"), top) - BOB_HEIGHT).abs() < 1e-3);
        assert_eq!(home.catalog.lift(&key("b"), top), 0.0);

        // Leaving settles from the current height instead of dropping.
        home.catalog.hover(key("a"), false, top);
        assert!((home.catalog.lift(&key("a"), top) - BOB_HEIGHT).abs() < 1e-3);
        // Re-entering mid-settle picks the float up where it is.
        let back = top + BOB_SETTLE / 2;
        let height = home.catalog.lift(&key("a"), back);
        home.catalog.hover(key("a"), true, back);
        assert!((home.catalog.lift(&key("a"), back) - height).abs() < 0.05);
        home.catalog.hover(key("a"), false, back);
        catalog(&mut home, CatalogMessage::MotionTick(back + BOB_SETTLE));
        assert!(!home.catalog.animating(), "a settled Cube stops the frames");

        // A selection settles every Cube and ignores new hovers.
        catalog(&mut home, CatalogMessage::CubeHover(key("b"), true));
        assert!(home.catalog.animating());
        catalog(&mut home, CatalogMessage::SelectFloor(key("b")));
        home.catalog.motion = None;
        assert!(home.catalog.hover.is_empty());
        catalog(&mut home, CatalogMessage::CubeHover(key("a"), true));
        assert!(home.catalog.hover.is_empty());
    }

    #[test]
    fn list_facts_report_lock_backup_and_wallets_from_recorded_metadata() {
        use super::super::launcher_presenter::{local_wallets, lock_state};
        let wallets = |cube: &CubeSettings| local_wallets(cube, false);
        let mut home = home_with(&[("a", "Alpha")]);
        assert_eq!(lock_state(project(&home).locals[0].open), "Locked");
        let cube = &mut cubes_mut(&mut home)[0];
        // No master seed recorded for this Cube: this device cannot tell.
        assert_eq!(backup_caption(cube), (Tone::Muted, "Status unknown"));
        cube.master_signer_fingerprint = Some([1, 2, 3, 4].into());
        // Matches the open Cube's "Backup not confirmed" warning.
        assert_eq!(backup_caption(cube), (Tone::Warn, "Not confirmed"));
        // A Recovery Kit is shown on the Recovery Kit line, not here.
        cube.recovery_kit_password_backed_up = true;
        assert_eq!(backup_caption(cube), (Tone::Warn, "Not confirmed"));
        cube.backed_up = true;
        assert_eq!(backup_caption(cube), (Tone::Good, "Confirmed"));

        // Spark comes with every mainnet Cube; Liquid only behind its gate.
        assert_eq!(wallets(cube), "Spark");
        cube.liquid_granted = Some(true);
        assert_eq!(wallets(cube), "Spark");
        cube.liquid_granted = None;
        assert_eq!(local_wallets(cube, true), "Spark");
        cube.vault_wallet_id = Some(crate::app::settings::WalletId::new(
            "test".to_string(),
            None,
        ));
        assert_eq!(wallets(cube), "Spark, Vault");
        let mut testnet = cube.clone();
        testnet.network = ChainId::Testnet4;
        testnet.liquid_granted = Some(true);
        assert_eq!(local_wallets(&testnet, true), "Vault");
        testnet.vault_wallet_id = None;
        assert_eq!(local_wallets(&testnet, true), "No wallets yet");

        cube.vault_wallet_id = None;
        cube.bitcoin_import = Some(Box::new(settings::BitcoinImport::pending()));
        assert_eq!(wallets(cube), "Spark, Bitcoin (importing)");
        assert!(!cube.has_imported_bitcoin());

        use coincube_core::{
            miniscript::bitcoin::{
                bip32::{DerivationPath, Xpriv, Xpub},
                secp256k1::Secp256k1,
            },
            wallet_descriptor::ImportedWalletDescriptor,
        };
        use std::str::FromStr;
        let secp = Secp256k1::new();
        let master = Xpriv::new_master(Network::Bitcoin, &[29; 32]).unwrap();
        let account = master
            .derive_priv(&secp, &DerivationPath::from_str("m/84h/0h/0h").unwrap())
            .unwrap();
        let xpub = Xpub::from_priv(&secp, &account);
        let key = format!("[{}/84h/0h/0h]{xpub}", master.fingerprint(&secp));
        let descriptor = ImportedWalletDescriptor::parse(
            &format!("wpkh({key}/0/*)"),
            Some(&format!("wpkh({key}/1/*)")),
            ChainId::Bitcoin,
        )
        .unwrap();
        let wallet =
            settings::ImportedWalletAttachment::new(descriptor, ChainId::Bitcoin, 123, 200)
                .unwrap();
        cube.bitcoin_import.as_mut().unwrap().stage = settings::BitcoinImportStage::Installing {
            wallet: wallet.clone(),
        };
        assert_eq!(wallets(cube), "Spark, Bitcoin (importing)");
        assert!(!cube.has_imported_bitcoin());
        cube.bitcoin_import.as_mut().unwrap().stage =
            settings::BitcoinImportStage::Ready { wallet };
        assert_eq!(wallets(cube), "Spark, Bitcoin");
        assert!(cube.has_imported_bitcoin());
    }

    #[test]
    fn aligned_wide_rows_and_compact_facts_keep_select_and_open_separate() {
        let mut home = home_with(&[(
            "a",
            "A very long Cube name that must wrap inside its own cell without hiding actions",
        )]);
        catalog(&mut home, CatalogMessage::Select(key("a")));
        for (size, table) in [(COMPACT, false), (WIDE, true)] {
            let tree = home_tree(
                "catalog-columns-and-details",
                &home,
                size,
                "light",
                theme::Theme::light(),
            );
            assert!(tree.has(&select_id(&key("a"))));
            assert!(tree.has(&open_id(&key("a"))));
            assert!(tree.has(&details_open_id(&key("a"))));
            assert!(tree.contains("Locked"));
            assert!(tree.contains("Status unknown"));
            // The presenter draws these rows: the network is a coin mark and
            // label in both layouts, and the compact List folds Wallets and
            // Backup into one caption that the wide table splits into columns.
            let mounted_row = tree.bounds(&select_id(&key("a")));
            let in_row = |bounds: &iced::Rectangle| {
                bounds.x >= mounted_row.x
                    && bounds.x < mounted_row.x + mounted_row.width
                    && bounds.y >= mounted_row.y
                    && bounds.y < mounted_row.y + mounted_row.height
            };
            let network_in_row = tree
                .text
                .iter()
                .any(|(value, bounds)| value == "Bitcoin (BTC)" && in_row(bounds));
            assert!(
                network_in_row,
                "network label outside the row at {:?}",
                size
            );
            let folded_facts_in_row = tree.text.iter().any(|(value, bounds)| {
                value.starts_with("Wallets: ") && value.contains(" · Backup: ") && in_row(bounds)
            });
            assert_eq!(folded_facts_in_row, !table, "at {:?}", size);

            let width = size.0 as f32 - 2.0 * GUTTER;
            let bounds = focus_bounds(layout(&project(&home), width), width);
            let select = bounds_of(&bounds, &select_id(&key("a")));
            let open = bounds_of(&bounds, &open_id(&key("a")));
            assert!(open.x >= select.x + select.width - 1.0);
            assert!(open.x + open.width <= width + 1.0);
        }
    }

    #[test]
    fn selected_details_do_not_force_table_columns_into_a_narrow_list() {
        let mut home = home_with(&[(
            "a",
            "A very long Cube name that must stay within the selected List viewport",
        )]);
        catalog(&mut home, CatalogMessage::Select(key("a")));
        let projection = project(&home);
        // The 1024px window leaves a 976px body after its gutters.
        for (body_width, table) in [(976.0, false), (1100.0, false), (1392.0, true)] {
            let list_width = if body_width >= LIST_MIN_WIDTH + DETAILS_WIDTH + PANEL_GAP {
                body_width - DETAILS_WIDTH - PANEL_GAP
            } else {
                body_width
            };
            let inspection = capture::inspect(
                "catalog-selected-list-width",
                layout(&projection, body_width),
                (body_width as u32, 900),
                "light",
                theme::Theme::light(),
            );
            assert_eq!(inspection.contains("Network: Bitcoin"), !table);
            assert!(inspection.contains("Backup: Status unknown") || table);
            let bounds = focus_bounds(layout(&projection, body_width), body_width);
            let select = bounds_of(&bounds, &select_id(&key("a")));
            let long_name = inspection
                .text
                .iter()
                .filter(|(value, bounds)| {
                    value.starts_with("A very long Cube name")
                        && bounds.y >= select.y
                        && bounds.y < select.y + select.height
                })
                .map(|(_, bounds)| *bounds)
                .next()
                .expect("long name rendered in List row");
            assert!(long_name.x + long_name.width <= list_width + 1.0);
            for id in [select_id(&key("a")), open_id(&key("a"))] {
                let cell = bounds_of(&bounds, &id);
                assert!(
                    cell.x + cell.width <= list_width + 1.0,
                    "{:?}: {:?}",
                    id,
                    cell
                );
            }
            if body_width >= LIST_MIN_WIDTH + DETAILS_WIDTH + PANEL_GAP {
                assert!(bounds_of(&bounds, &details_open_id(&key("a"))).x >= list_width);
            }
        }
    }

    #[test]
    fn catalog_messages_touch_only_transient_presentation_state() {
        let mut home = home_with(&[("a", "Alpha"), ("b", "Beta")]);
        let section = home.active_section.clone();
        for message in [
            CatalogMessage::Select(key("a")),
            CatalogMessage::Query("alp".to_string()),
            CatalogMessage::ClearQuery,
            CatalogMessage::ClearSelection,
        ] {
            catalog(&mut home, message);
            assert_eq!(home.active_section, section);
            assert!(home.delete_cube_modal.is_none());
            assert!(home.rename_cube_modal.is_none());
            assert!(home.recovery_method_modal.is_none());
            assert!(matches!(
                home.state,
                State::Cubes {
                    create_cube: false,
                    ..
                }
            ));
            assert_eq!(cubes_mut(&mut home).len(), 2);
        }
    }

    #[test]
    fn search_filters_rows_but_never_the_quota_or_authoritative_lists() {
        let mut home = home_with(&[("a", "Family"), ("b", "Travel"), ("c", "Archive")]);
        home.remote_cubes = vec![
            remote("r1", "Family backup", true, true, false, false),
            remote("r2", "Other", false, false, false, false),
        ];
        home.server_cube_limit = Some(10);
        let before = project(&home);
        let (count, limit) = (before.total_count, before.cube_limit);
        assert_eq!((before.locals.len(), before.remotes.len()), (3, 2));

        catalog(&mut home, CatalogMessage::Select(key("b")));
        catalog(&mut home, CatalogMessage::Query("  FAM ".to_string()));
        let filtered = project(&home);
        let names: Vec<_> = filtered
            .locals
            .iter()
            .map(|row| row.name.as_str())
            .collect();
        assert_eq!(names, vec!["Family"]);
        assert_eq!(filtered.remotes.len(), 1);
        assert_eq!(filtered.remotes[0].uuid, "r1");
        assert_eq!((filtered.total_count, filtered.cube_limit), (count, limit));
        assert_eq!((filtered.local_total, filtered.remote_total), (3, 2));
        // The selected Cube is hidden, so are its details; the selection waits.
        assert_eq!(filtered.selected, None);
        assert_eq!(home.catalog.selected, Some(key("b")));
        assert!(!filtered.no_matches());

        catalog(&mut home, CatalogMessage::Query("zzz".to_string()));
        assert!(project(&home).no_matches());
        assert_eq!(home.remote_cubes.len(), 2);
        assert_eq!(cubes_mut(&mut home).len(), 3);

        catalog(&mut home, CatalogMessage::ClearQuery);
        let restored = project(&home);
        assert_eq!((restored.locals.len(), restored.remotes.len()), (3, 2));
        assert_eq!(selected_id(&home).as_deref(), Some("b"));
    }

    #[test]
    fn remote_restore_matrix_keeps_exact_eligibility_and_payloads() {
        for bits in 0..16u8 {
            let (kit, seed, phone, full) =
                (bits & 1 != 0, bits & 2 != 0, bits & 4 != 0, bits & 8 != 0);
            let cube = remote("uuid-x", "Remote", kit, seed, phone, full);
            let restore = remote_restore(&cube);
            let password = kit && seed;
            let phone_ok = phone && feature_flags::OWNER_KEYCHAIN_RECOVERY_ENABLED;
            match (password, phone_ok, &restore.action) {
                (true, true, Some((ViewMessage::ShowRecoveryMethodPicker(uuid), _)))
                | (true, false, Some((ViewMessage::RestoreFromRecoveryKit(uuid), _))) => {
                    assert_eq!(uuid, "uuid-x", "bits {}", bits)
                }
                (false, true, Some((ViewMessage::RestoreWithPhone { cube_id, full_cube }, _))) => {
                    assert_eq!(*cube_id, 7, "bits {}", bits);
                    assert_eq!(*full_cube, full, "bits {}", bits);
                }
                (false, false, None) => {
                    assert_eq!(
                        restore
                            .status
                            .starts_with("Recovery Kit has no Master Seed"),
                        kit,
                        "bits {}",
                        bits
                    );
                }
                (p, f, action) => panic!(
                    "bits {}: password {} phone {} gave {:?}",
                    bits, p, f, action
                ),
            }
        }
    }

    /// One non-focus message from the real tree; a pointer press must also
    /// publish exactly one focus intent for the same target.
    fn activate_once(content: Element<'_, ViewMessage>, target: Id, pointer: bool) -> ViewMessage {
        let messages = capture::activate(content, target.clone(), TALL, pointer);
        let (focus, rest): (Vec<_>, Vec<_>) = messages
            .into_iter()
            .partition(|message| matches!(message, ViewMessage::Catalog(CatalogMessage::Focus(_))));
        if pointer {
            assert!(
                matches!(
                    focus.as_slice(),
                    [ViewMessage::Catalog(CatalogMessage::Focus(id))] if id == &target
                ),
                "{:?}",
                focus
            );
        } else {
            assert!(focus.is_empty(), "{:?}", focus);
        }
        assert_eq!(rest.len(), 1, "{:?}", rest);
        rest.into_iter().next().unwrap()
    }

    #[test]
    fn real_tree_actions_emit_the_exact_existing_catalog_messages() {
        let mut home = home_with(&[("a", "Alpha"), ("b", "Beta")]);
        home.remote_cubes = vec![remote("r1", "Remote", true, true, false, false)];
        home.server_cube_limit = Some(10);
        catalog(&mut home, CatalogMessage::Select(key("b")));
        for pointer in [false, true] {
            assert!(matches!(
                activate_once(catalog_view(&home), open_id(&key("a")), pointer),
                ViewMessage::RunCatalog(k) if k == key("a")
            ));
            // Selecting a row neither opens it nor selects another.
            assert!(matches!(
                activate_once(catalog_view(&home), select_id(&key("a")), pointer),
                ViewMessage::Catalog(CatalogMessage::Select(k)) if k == key("a")
            ));
            assert!(matches!(
                activate_once(catalog_view(&home), details_open_id(&key("b")), pointer),
                ViewMessage::RunCatalog(k) if k == key("b")
            ));
            assert!(matches!(
                activate_once(catalog_view(&home), rename_id(&key("b")), pointer),
                ViewMessage::RenameCatalog(k) if k == key("b")
            ));
            assert!(matches!(
                activate_once(catalog_view(&home), delete_id(&key("b")), pointer),
                ViewMessage::DeleteCatalog(k) if k == key("b")
            ));
            assert!(matches!(
                activate_once(catalog_view(&home), close_details_id(), pointer),
                ViewMessage::Catalog(CatalogMessage::ClearSelection)
            ));
            assert!(matches!(
                activate_once(catalog_view(&home), restore_id("r1"), pointer),
                ViewMessage::RestoreFromRecoveryKit(uuid) if uuid == "r1"
            ));
            assert!(matches!(
                activate_once(catalog_view(&home), remote_delete_id("r1"), pointer),
                ViewMessage::DeleteCube(DeleteCubeMessage::ShowRemoteModal(uuid)) if uuid == "r1"
            ));
        }
    }

    fn ghost(home: &Home, uuid: &str) -> CubeCatalogKey {
        let cube = home
            .remote_cubes
            .iter()
            .find(|cube| cube.uuid == uuid)
            .expect("fixture ghost");
        ghost_key(cube)
    }

    #[test]
    fn no_cube_count_pushes_the_floor_into_the_list() {
        // Eight local Cubes and thirteen in Connect only: 21 in all, but
        // the floor keeps them, local Cubes first, ghosts paged after.
        let names: Vec<(String, String)> = (0..8)
            .map(|i| (format!("l{}", i), format!("Local {}", i)))
            .collect();
        let refs: Vec<(&str, &str)> = names
            .iter()
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect();
        let mut home = home_with(&refs);
        home.remote_cubes = (0..13)
            .map(|i| remote(&format!("g{}", i), "Ghost", true, true, false, false))
            .collect();
        home.catalog.mode = LauncherMode::Cubes;
        let projection = project(&home);
        assert!(projection.floor_eligible(LauncherMode::Cubes));
        assert_eq!(projection.floor_len(), 21);
        // Nor do many local Cubes: the floor pages them.
        let names: Vec<(String, String)> = (0..40)
            .map(|i| (format!("l{}", i), format!("Local {}", i)))
            .collect();
        let refs: Vec<(&str, &str)> = names
            .iter()
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect();
        let mut home = home_with(&refs);
        home.catalog.mode = LauncherMode::Cubes;
        let projection = project(&home);
        assert!(projection.floor_eligible(LauncherMode::Cubes));
        assert_eq!(projection.floor_len(), 40);
    }

    #[test]
    fn ghosts_follow_the_network_filter_across_networks() {
        let mut home = home_with(&[("a", "Alpha")]);
        home.developer_mode = true;
        home.displayed_networks = vec![ChainId::Bitcoin, ChainId::Testnet4, ChainId::Signet];
        let on = |uuid: &str, chain: ChainId| RemoteCube {
            network: settings::network_to_api_string(chain),
            ..remote(uuid, uuid, true, true, false, false)
        };
        home.remote_cubes = vec![
            on("signet", ChainId::Signet),
            on("main", ChainId::Bitcoin),
            on("t4", ChainId::Testnet4),
        ];
        home.catalog.mode = LauncherMode::Cubes;
        let chained = |uuid: &str, chain: ChainId| CubeCatalogKey {
            chain,
            cube_id: uuid.into(),
        };
        // All networks: every network's ghosts, in the networks' order,
        // each keyed and marked by its own chain.
        let projection = project(&home);
        assert_eq!(
            projection.floor_keys(),
            vec![
                key("a"),
                chained("main", ChainId::Bitcoin),
                chained("t4", ChainId::Testnet4),
                chained("signet", ChainId::Signet),
            ]
        );
        assert_eq!(projection.connect_only(LauncherMode::Cubes), 3);
        // The List still acts in place, so it keeps the current network's.
        assert_eq!(projection.remotes.len(), 1);
        assert_eq!(projection.connect_only(LauncherMode::List), 1);

        home.catalog.filter = NetworkFilter::Test;
        assert_eq!(
            project(&home).floor_keys(),
            vec![
                chained("t4", ChainId::Testnet4),
                chained("signet", ChainId::Signet),
            ]
        );
        home.catalog.filter = NetworkFilter::Network(ChainId::Bitcoin);
        assert_eq!(
            project(&home).floor_keys(),
            vec![key("a"), chained("main", ChainId::Bitcoin)]
        );

        // Picking another network's ghost makes its network current first;
        // its face waits for that network, where its actions apply.
        home.catalog.filter = NetworkFilter::All;
        let t4 = chained("t4", ChainId::Testnet4);
        let task = home.update(Message::View(ViewMessage::Catalog(
            CatalogMessage::SelectFloor(t4.clone()),
        )));
        assert_eq!(task.units(), 1);
        assert_eq!(home.catalog.selected, None);
        assert_eq!(
            home.catalog.pending_select,
            Some((t4.clone(), PendingIntent::Select))
        );
        assert!(!home.catalog_selectable(&t4), "not on the current network");
    }

    #[test]
    fn ghosts_stand_after_the_local_cubes_and_select_only_on_the_floor() {
        let mut home = home_with(&[("a", "Alpha"), ("b", "Beta")]);
        home.remote_cubes = vec![
            remote("kit", "Passkey", true, true, false, false),
            remote("nokit", "Old laptop", false, false, false, false),
            remote("seedless", "Half kit", true, false, false, false),
        ];
        home.catalog.mode = LauncherMode::Cubes;
        let projection = project(&home);
        assert!(projection.floor_eligible(LauncherMode::Cubes));
        assert_eq!(
            projection.floor_keys(),
            vec![
                key("a"),
                key("b"),
                ghost(&home, "kit"),
                ghost(&home, "nokit"),
                ghost(&home, "seedless"),
            ]
        );
        // Restorable means this device can restore it: a kit without its
        // seed reads as no Recovery Kit.
        let kinds: Vec<_> = home.remote_cubes.iter().map(ghost_kind).collect();
        use super::super::launcher_scene::Ghost;
        assert_eq!(
            kinds,
            [Ghost::Restorable, Ghost::Unrestorable, Ghost::Unrestorable]
        );
        assert_eq!(
            ghost_status(&home.remote_cubes[0], None),
            "In Connect · Restore"
        );
        assert_eq!(
            ghost_status(&home.remote_cubes[2], None),
            "In Connect · No Recovery Kit"
        );
        // While the stored session waits to be checked, nothing restores (L5).
        use crate::services::connect::grpc::trouble::Trouble;
        assert_eq!(
            ghost_status(&home.remote_cubes[0], Some(Trouble::Offline)),
            "In Connect · Offline"
        );
        assert_eq!(
            ghost_status(&home.remote_cubes[2], Some(Trouble::Unreachable)),
            "In Connect · Reconnecting…"
        );

        // A ghost turns to its face on the floor, and stays out of every
        // local-Cube gate: nothing can open, rename or delete it as a Cube.
        {
            let key = ghost(&home, "nokit");
            catalog(&mut home, CatalogMessage::SelectFloor(key));
        }
        let projection = project(&home);
        assert_eq!(home.catalog.selected, Some(ghost(&home, "nokit")));
        assert_eq!(projection.selected, None);
        assert_eq!(projection.selected_ghost, Some(1));
        assert_eq!(projection.floor_selected(), Some(3));
        assert!(home.catalog.motion.is_some(), "the ghost turns to its face");
        assert!(!home.catalog_resolves(&ghost(&home, "nokit")));
        assert_eq!(home.catalog.page, 0);

        // A ghost on another page moves the floor to it.
        home.catalog.stage_width = Some(900.0);
        assert_eq!(home.catalog.page_size(), 4);
        {
            let key = ghost(&home, "seedless");
            catalog(&mut home, CatalogMessage::SelectFloor(key));
        }
        assert_eq!(home.catalog.page, 1);

        // The List has no ghost faces; ghosts are rows with their own actions.
        home.catalog.mode = LauncherMode::List;
        {
            let key = ghost(&home, "kit");
            catalog(&mut home, CatalogMessage::Select(key));
        }
        assert_eq!(home.catalog.selected, None);
        assert_eq!(project(&home).selected_ghost, None);

        // Signing out takes the ghosts, and their selection, off the floor.
        home.catalog.mode = LauncherMode::Cubes;
        {
            let key = ghost(&home, "kit");
            catalog(&mut home, CatalogMessage::SelectFloor(key));
        }
        let key = ghost(&home, "kit");
        home.remote_cubes.clear();
        home.reconcile_catalog();
        assert_eq!(home.catalog.selected, None);
        assert!(!project(&home).floor_keys().contains(&key));
    }

    #[test]
    fn a_ghost_face_emits_the_existing_restore_delete_and_back_messages() {
        let mut home = home_with(&[("a", "Alpha")]);
        home.remote_cubes = vec![
            remote("kit", "Passkey", true, true, false, false),
            remote("nokit", "Old laptop", false, false, false, false),
        ];
        home.catalog.mode = LauncherMode::Cubes;
        for pointer in [false, true] {
            home.catalog.selected = Some(ghost(&home, "kit"));
            let floor = || super::super::launcher_presenter::floor_body(&home);
            assert!(matches!(
                activate_once(floor(), details_open_id(&ghost(&home, "kit")), pointer),
                ViewMessage::RestoreFromRecoveryKit(uuid) if uuid == "kit"
            ));
            assert!(matches!(
                activate_once(floor(), remote_delete_id("kit"), pointer),
                ViewMessage::DeleteCube(DeleteCubeMessage::ShowRemoteModal(uuid)) if uuid == "kit"
            ));
            assert!(matches!(
                activate_once(floor(), close_details_id(), pointer),
                ViewMessage::Catalog(CatalogMessage::ClearSelection)
            ));
            home.catalog.selected = Some(ghost(&home, "nokit"));
            let floor = || super::super::launcher_presenter::floor_body(&home);
            assert!(matches!(
                activate_once(floor(), details_open_id(&ghost(&home, "nokit")), pointer),
                ViewMessage::ImportGhostWithPhrase(uuid) if uuid == "nokit"
            ));
            assert!(matches!(
                activate_once(floor(), remote_delete_id("nokit"), pointer),
                ViewMessage::DeleteCube(DeleteCubeMessage::ShowRemoteModal(uuid)) if uuid == "nokit"
            ));
            // A browsing ghost's slot selects it.
            home.catalog.selected = None;
            let floor = || super::super::launcher_presenter::floor_body(&home);
            assert!(matches!(
                activate_once(floor(), select_id(&ghost(&home, "kit")), pointer),
                ViewMessage::Catalog(CatalogMessage::SelectFloor(k)) if k == ghost(&home, "kit")
            ));
        }
    }

    #[test]
    fn every_row_renders_with_unique_action_ids_in_both_themes_and_sizes() {
        let names: Vec<(String, String)> = (0..20)
            .map(|index| (format!("cube-{}", index), format!("Cube {}", index)))
            .collect();
        let pairs: Vec<(&str, &str)> = names
            .iter()
            .map(|(id, name)| (id.as_str(), name.as_str()))
            .collect();
        let mut home = home_with(&pairs);
        cubes_mut(&mut home)[3].name =
            "A very long Cube name that keeps going \u{202e}to test wrapping\n behaviour on a compact window"
                .to_string();
        home.remote_cubes = vec![remote("r1", "Remote", false, false, false, false)];
        catalog(&mut home, CatalogMessage::Select(key("cube-3")));
        for (appearance, theme) in [
            ("dark", theme::Theme::dark()),
            ("light", theme::Theme::light()),
        ] {
            for outer in [COMPACT, WIDE] {
                let inspection = capture::inspect(
                    "catalog-20-rows-selected",
                    catalog_view(&home),
                    outer,
                    appearance,
                    theme,
                );
                for (id, _) in &pairs {
                    assert!(inspection.has_focus(select_id(&key(id))), "{}", id);
                    assert!(inspection.has_focus(open_id(&key(id))), "{}", id);
                }
                assert!(inspection.has_focus(details_open_id(&key("cube-3"))));
                assert!(inspection.has_focus(remote_delete_id("r1")));
                // No restore control without an eligible method.
                assert!(!inspection.has_focus(restore_id("r1")));
                let ids: Vec<_> = inspection.focus.iter().map(|(id, _)| id.clone()).collect();
                let unique: HashSet<_> = ids.iter().cloned().collect();
                assert_eq!(unique.len(), ids.len(), "duplicate action IDs");
                // Sanitized name: no bidi override or newline is rendered.
                assert!(inspection.contains("A very long Cube name"));
                assert!(!inspection.contains("\u{202e}"));
                assert!(inspection.contains("Available in Connect"));
                assert!(inspection.contains("Registered in Connect (no Recovery Kit backed up)"));
            }
        }
    }

    /// The limit is per network and counts Cubes registered from other
    /// devices: five here plus two only in Connect fill a 7-Cube Bitcoin
    /// mainnet, while Testnet4's three stay well under it. The dock names
    /// the plan; its menu gives every network's count instead of one bare
    /// ratio.
    #[test]
    fn plan_usage_is_shown_per_network_and_counts_remote_cubes() {
        let mut home = home_with(&[
            ("a", "Alpha"),
            ("b", "Beta"),
            ("c", "Gamma"),
            ("d", "Delta"),
            ("e", "Epsilon"),
        ]);
        home.account_tier = crate::app::settings::global::AccountTier::Estate;
        home.server_cube_limit = Some(7);
        home.displayed_networks = vec![ChainId::Bitcoin, ChainId::Testnet4, ChainId::Signet];
        let cube = |id: &str| {
            CubeSettings::new_with_raw_id(id.to_string(), id.to_string(), ChainId::Testnet4)
        };
        home.network_catalog = vec![
            (ChainId::Testnet4, vec![cube("t1"), cube("t2"), cube("t3")]),
            (ChainId::Signet, Vec::new()),
        ];
        home.remote_cubes = vec![
            remote("r1", "Elsewhere", true, true, false, false),
            remote("r2", "Laptop", true, true, false, false),
        ];

        let projection = project(&home);
        assert_eq!(
            projection.usage,
            vec![
                NetworkUsage {
                    chain: ChainId::Bitcoin,
                    local: Some(5),
                    remote: 2,
                },
                NetworkUsage {
                    chain: ChainId::Testnet4,
                    local: Some(3),
                    remote: 0,
                },
            ],
            "Signet holds no Cube, so it is left out"
        );
        assert!(projection.at_limit());
        // The narrowest window the app allows, with every dock control the
        // multi-network state brings. The dock names the plan alone.
        for (scenario, window) in [("plan-usage", WIDE), ("plan-usage-1024", (1024, 768))] {
            let tree = home_tree(scenario, &home, window, "dark", theme::Theme::dark());
            assert!(tree.contains("Estate plan"));
            assert!(!tree.contains("per network"));
            assert!(!tree.contains("New Cubes"));
            assert!(!tree.has(&create_id()));
            assert!(!tree.has(&import_id()));
        }
        // Its menu holds each network's use of the limit.
        home.catalog.plan_menu = true;
        for (scenario, window) in [
            ("plan-usage-menu", WIDE),
            ("plan-usage-menu-1024", (1024, 768)),
        ] {
            let tree = home_tree(scenario, &home, window, "dark", theme::Theme::dark());
            for line in [
                "Estate plan · 7 Cubes per network",
                "Bitcoin mainnet",
                "7 of 7",
                "Full",
                "2 only in Connect",
                "Testnet4",
                "3 of 7",
                "Upgrade, or delete a Bitcoin mainnet Cube, to add one.",
            ] {
                assert!(tree.contains(line), "missing {:?}", line);
            }
        }

        // Developer mode off lists Bitcoin alone, but a Signet Cube held only
        // in Connect still uses a Signet slot and is named. This device's
        // Signet list is not read, so no ratio is claimed for it.
        home.displayed_networks = vec![ChainId::Bitcoin];
        home.network_catalog.clear();
        let mut signet = remote("s1", "Lab", true, true, false, false);
        signet.network = settings::network_to_api_string(ChainId::Signet);
        home.remote_cubes.push(signet);
        let projection = project(&home);
        assert!(projection.usage.contains(&NetworkUsage {
            chain: ChainId::Signet,
            local: None,
            remote: 1,
        }));
        let tree = home_tree(
            "plan-usage-hidden",
            &home,
            WIDE,
            "dark",
            theme::Theme::dark(),
        );
        assert!(tree.contains("1 in Connect; this device not counted"));
        assert!(!tree.contains("1 of 7"));
        home.remote_cubes.pop();

        // A Cube going elsewhere frees the slot; the line stays as a quota.
        home.remote_cubes.pop();
        let projection = project(&home);
        assert!(!projection.at_limit());
        let tree = home_tree("plan-usage-free", &home, WIDE, "dark", theme::Theme::dark());
        assert!(!tree.contains("Full"));
        assert!(!tree.contains("Upgrade"));
        assert!(tree.contains("6 of 7"));
        assert!(tree.contains("1 only in Connect"));
        assert!(tree.has(&create_id()));
        assert!(tree.has(&import_id()));
    }

    /// Billing v2 D5: an account already over its plan's limit (Free's one
    /// Cube with two on the network) is told what the plan includes and that
    /// its Cubes keep working, never an impossible "2 of 1".
    #[test]
    fn over_the_limit_copy_says_existing_cubes_keep_working() {
        let mut home = home_with(&[("a", "A"), ("b", "B")]);
        home.server_cube_limit = Some(1);
        let projection = project(&home);
        assert!(projection.at_limit());
        let full = super::super::launcher_chrome::plan_full(&projection).unwrap();
        assert!(full.contains("2 Cubes; the"), "{}", full);
        assert!(
            full.contains("includes 1. Existing Cubes keep working."),
            "{}",
            full
        );
        assert!(!full.contains("2 of 1"), "{}", full);
        // The longer dock line still renders at the minimum window size.
        let compact = home_tree(
            "plan-over-limit-compact",
            &home,
            COMPACT,
            "dark",
            theme::Theme::dark(),
        );
        assert!(compact.contains(&full), "dock line at 1024x768");
        home.catalog.plan_menu = true;
        let tree = home_tree("plan-over-limit", &home, WIDE, "dark", theme::Theme::dark());
        assert!(tree.contains("2 · plan includes 1"));
        assert!(!tree.contains("2 of 1"));
    }

    /// Switching network makes it current before its list is read (and keeps
    /// it so if the read fails), while the checked list is still the old
    /// network's. Those Cubes are not the new network's: the gate, the dock
    /// and the summary must neither count them there nor tell the user to
    /// delete a Cube on a network they show as empty.
    #[test]
    fn a_pending_network_switch_does_not_count_the_old_networks_cubes() {
        let mut home = home_with(&[
            ("a", "A"),
            ("b", "B"),
            ("c", "C"),
            ("d", "D"),
            ("e", "E"),
            ("f", "F"),
            ("g", "G"),
        ]);
        home.server_cube_limit = Some(7);
        home.displayed_networks = vec![ChainId::Bitcoin, ChainId::Testnet4];
        home.network = ChainId::Testnet4;
        assert!(matches!(
            home.state,
            State::Cubes {
                source: ChainId::Bitcoin,
                ..
            }
        ));

        assert_eq!(home.cube_usage(ChainId::Testnet4), (None, 0));
        assert_eq!(home.cube_usage(ChainId::Bitcoin), (Some(7), 0));
        let projection = project(&home);
        assert!(!projection.at_limit());
        let current = projection
            .usage
            .iter()
            .find(|u| u.chain == ChainId::Testnet4)
            .expect("the current network is always listed");
        assert_eq!(current.local, None);
        home.catalog.plan_menu = true;
        let tree = home_tree("pending-switch", &home, WIDE, "dark", theme::Theme::dark());
        assert!(tree.contains("Not counted"));
        assert!(tree.contains("This device's Cubes could not be counted"));
        assert!(tree.contains("7 of 7"));
        assert!(tree.contains("Full"));
        assert!(!tree.contains("delete a Bitcoin Testnet4 Cube"));
        assert!(!tree.contains("Upgrade, or delete"));
        let summary = home.cube_limit_summary(ChainId::Testnet4);
        assert!(!summary.contains("7/7"), "{}", summary);
        assert!(summary.contains("could not be counted"), "{}", summary);

        // Once the new network's list is in, its own count applies.
        home.network_catalog = vec![(ChainId::Testnet4, Vec::new())];
        assert_eq!(home.cube_usage(ChainId::Testnet4), (Some(0), 0));
    }

    #[test]
    fn plan_limit_disables_creation_with_the_existing_explanation() {
        let mut home = home_with(&[("a", "Alpha"), ("b", "Beta")]);
        home.server_cube_limit = Some(2);
        let tree = home_tree(
            "r05b-limit",
            &home,
            (1024, 768),
            "light",
            theme::Theme::light(),
        );
        assert!(!tree.has(&create_id()));
        assert!(!tree.has(&import_id()));
        assert!(tree.has(&super::super::launcher_chrome::plan_id()));
        // The plan's menu says why.
        home.catalog.plan_menu = true;
        let tree = home_tree(
            "r05b-limit-menu",
            &home,
            (1024, 768),
            "light",
            theme::Theme::light(),
        );
        assert!(tree.contains("2 of 2"));
        assert!(tree.contains("Full"));
        assert!(tree.contains("Upgrade, or delete a Bitcoin mainnet Cube, to add one."));

        // Import or restore opens on every network.
        let mut testnet = home_with(&[("a", "Alpha")]);
        testnet.server_cube_limit = Some(10);
        testnet.network = ChainId::Testnet4;
        if let State::Cubes { source, cubes, .. } = &mut testnet.state {
            *source = ChainId::Testnet4;
            cubes[0].network = ChainId::Testnet4;
        }
        let tree = home_tree(
            "r05b-testnet",
            &testnet,
            (1024, 768),
            "dark",
            theme::Theme::dark(),
        );
        assert!(tree.has(&create_id()));
        assert!(tree.has(&import_id()));
    }

    #[test]
    fn blake2b_cubes_offer_delete() {
        let mut home = home_with(&[("a", "Fork Vault")]);
        // The List is only visible on an available network.
        home.connect_account.bitcoin_blake2b_opt_in = true;
        home.global_blake2b_enabled = true;
        home.network = ChainId::BitcoinBlake2b;
        if let State::Cubes { source, cubes, .. } = &mut home.state {
            *source = ChainId::BitcoinBlake2b;
            cubes[0].network = ChainId::BitcoinBlake2b;
        }
        let fork = CubeCatalogKey {
            chain: ChainId::BitcoinBlake2b,
            cube_id: "a".to_string(),
        };
        catalog(&mut home, CatalogMessage::Select(fork.clone()));
        assert_eq!(home.catalog.selected, Some(fork.clone()));
        let inspection = capture::inspect(
            "catalog-blake2b-details",
            catalog_view(&home),
            WIDE,
            "dark",
            theme::Theme::dark(),
        );
        assert!(inspection.has_focus(rename_id(&fork)));
        assert!(inspection.has_focus(delete_id(&fork)));

        // The Blake2b Launcher's action allowlist lets the delete through.
        drop(home.update(Message::View(ViewMessage::DeleteCatalog(fork))));
        assert!(home.error.is_none());
        assert!(home.delete_cube_modal.is_some());
    }

    #[test]
    fn blake2b_connect_only_cubes_are_listed_counted_restored_and_deleted() {
        let mut home = home_with(&[("a", "Fork Vault")]);
        home.connect_account.bitcoin_blake2b_opt_in = true;
        home.global_blake2b_enabled = true;
        home.network = ChainId::BitcoinBlake2b;
        if let State::Cubes { source, cubes, .. } = &mut home.state {
            *source = ChainId::BitcoinBlake2b;
            cubes[0].network = ChainId::BitcoinBlake2b;
        }
        // Kept in Connect when it was deleted here, with every backup Connect
        // can hold.
        let mut kept = remote("kept", "Kept", true, true, true, true);
        kept.network = settings::network_to_api_string(ChainId::BitcoinBlake2b);
        assert_eq!(kept.network, ChainId::BitcoinBlake2b.api_str());
        home.remote_cubes = vec![kept.clone()];
        home.server_cube_limit = Some(10);

        // Listed and on the floor, and it takes a slot.
        let projection = project(&home);
        assert_eq!(projection.remote_total, 1);
        assert_eq!(projection.ghosts.len(), 1);
        assert_eq!(projection.total_count, 2);
        assert_eq!(home.cube_usage(ChainId::BitcoinBlake2b), (Some(1), 1));
        assert_eq!(home.cube_usage(ChainId::Bitcoin).1, 0);

        // It restores as a Bitcoin Cube does.
        let restore = remote_restore(&kept);
        assert!(restore.action.is_some());
        assert!(matches!(
            ghost_kind(&kept),
            super::super::launcher_scene::Ghost::Restorable
        ));
        let inspection = capture::inspect(
            "catalog-blake2b-connect-only",
            catalog_view(&home),
            WIDE,
            "dark",
            theme::Theme::dark(),
        );
        assert!(inspection.has_focus(remote_delete_id("kept")));
        assert!(inspection.has_focus(restore_id("kept")));

        // Delete from Connect gets past the Blake2b allowlist.
        for pointer in [false, true] {
            assert!(matches!(
                activate_once(catalog_view(&home), remote_delete_id("kept"), pointer),
                ViewMessage::DeleteCube(DeleteCubeMessage::ShowRemoteModal(uuid)) if uuid == "kept"
            ));
        }
        drop(home.update(Message::View(ViewMessage::DeleteCube(
            DeleteCubeMessage::ShowRemoteModal("kept".to_string()),
        ))));
        assert!(home.error.is_none());
        assert!(home.delete_remote_cube_modal.is_some());

        // So do its restore and the Recovery Kit launch.
        home.delete_remote_cube_modal = None;
        let (open, _) = remote_restore(&kept).action.expect("restorable");
        drop(home.update(Message::View(open)));
        assert!(home.error.is_none());
        drop(
            home.update(Message::View(ViewMessage::RestoreFromRecoveryKit(
                "kept".to_string(),
            ))),
        );
        assert!(home.error.is_none());
    }

    #[test]
    fn the_launcher_includes_empty_catalogs_but_preserves_explicit_forms() {
        let mut home = home_with(&[("a", "Alpha")]);
        assert!(shows_catalog(&home));
        if let State::Cubes { create_cube, .. } = &mut home.state {
            *create_cube = true;
        }
        assert!(!shows_catalog(&home));

        let mut fresh = home_with(&[]);
        fresh.state = State::NoCube { create_cube: false };
        // An empty device lands on the Launcher too.
        assert!(shows_catalog(&fresh));
        fresh.state = State::NoCube { create_cube: true };
        assert!(!shows_catalog(&fresh));
        fresh.state = State::NoCube { create_cube: false };
        fresh.remote_cubes = vec![remote("r1", "Remote", true, true, false, false)];
        assert!(shows_catalog(&fresh));
        let remote_only = project(&fresh);
        assert_eq!(remote_only.remotes.len(), 1);

        fresh.state = State::RecoveryInput;
        assert!(!shows_catalog(&fresh));
    }

    /// Focus targets and their bounds for an element laid out at `width`.
    fn focus_bounds(mut element: Element<'_, ViewMessage>, width: f32) -> Vec<(Id, Rectangle)> {
        #[derive(Default)]
        struct Probe(Vec<(Id, Rectangle)>);
        impl Operation for Probe {
            fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
                operate(self);
            }
            fn focusable(&mut self, id: Option<&Id>, bounds: Rectangle, _: &mut dyn Focusable) {
                if let Some(id) = id {
                    self.0.push((id.clone(), bounds));
                }
            }
        }
        for bytes in font::load() {
            iced::advanced::graphics::text::font_system()
                .write()
                .unwrap()
                .load_font(bytes);
        }
        let renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
            font::REGULAR,
            iced::Pixels(16.0),
            Some("tiny-skia"),
        ))
        .unwrap();
        let mut tree = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &widget_layout::Limits::new(Size::ZERO, Size::new(width, 20_000.0)),
        );
        let mut probe = Probe::default();
        element
            .as_widget_mut()
            .operate(&mut tree, Layout::new(&node), &renderer, &mut probe);
        probe.0
    }

    fn bounds_of(targets: &[(Id, Rectangle)], id: &Id) -> Rectangle {
        targets
            .iter()
            .find_map(|(target, bounds)| (target == id).then_some(*bounds))
            .expect("focus target present")
    }

    #[test]
    fn details_sit_beside_the_list_when_wide_and_above_it_when_compact() {
        let mut home = home_with(&[("a", "Alpha"), ("b", "Beta")]);
        catalog(&mut home, CatalogMessage::Select(key("b")));
        let projection = project(&home);
        // The List body's width: the window less the two 24px gutters.
        let wide_width = 1440.0 - 2.0 * GUTTER;
        let compact_width = 1024.0 - 2.0 * GUTTER;

        let wide = focus_bounds(layout(&projection, wide_width), wide_width);
        let close = bounds_of(&wide, &close_details_id());
        let row = bounds_of(&wide, &select_id(&key("a")));
        assert!(close.x > row.x + LIST_MIN_WIDTH, "{:?} {:?}", close, row);
        assert!(close.x + close.width <= wide_width + 1.0);

        let compact = focus_bounds(layout(&projection, compact_width), compact_width);
        let close = bounds_of(&compact, &close_details_id());
        let row = bounds_of(&compact, &select_id(&key("a")));
        assert!(close.y < row.y, "{:?} {:?}", close, row);
        for (id, bounds) in &compact {
            assert!(
                bounds.x >= -1.0 && bounds.x + bounds.width <= compact_width + 1.0,
                "{:?} {:?} overflows {}",
                id,
                bounds,
                compact_width
            );
        }
    }

    struct Mounted<'a> {
        element: Element<'a, ViewMessage>,
        tree: Tree,
        node: widget_layout::Node,
        renderer: iced::Renderer,
        width: f32,
        height: f32,
    }

    fn mount(element: Element<'_, ViewMessage>, width: f32) -> Mounted<'_> {
        mount_window(element, width, 20_000.0)
    }

    /// Lays the element out in exactly a window-sized box, as the shell does.
    fn mount_window(element: Element<'_, ViewMessage>, width: f32, height: f32) -> Mounted<'_> {
        for bytes in font::load() {
            iced::advanced::graphics::text::font_system()
                .write()
                .unwrap()
                .load_font(bytes);
        }
        let renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
            font::REGULAR,
            iced::Pixels(16.0),
            Some("tiny-skia"),
        ))
        .unwrap();
        let mut element = element;
        let mut tree = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &widget_layout::Limits::new(Size::ZERO, Size::new(width, height)),
        );
        Mounted {
            element,
            tree,
            node,
            renderer,
            width,
            height,
        }
    }

    impl<'a> Mounted<'a> {
        fn operate<T: 'static>(&mut self, operation: impl Operation<T> + 'static) -> Option<T> {
            use iced::advanced::widget::operation::{self, Outcome};
            let mut operation: Box<dyn Operation<T>> = Box::new(operation);
            loop {
                self.element.as_widget_mut().operate(
                    &mut self.tree,
                    Layout::new(&self.node),
                    &self.renderer,
                    &mut operation::black_box(operation.as_mut()),
                );
                match operation.finish() {
                    Outcome::None => return None,
                    Outcome::Some(value) => return Some(value),
                    Outcome::Chain(next) => operation = next,
                }
            }
        }

        fn focused(&mut self) -> Option<Id> {
            self.operate(iced::advanced::widget::operation::focusable::find_focused())
        }

        fn send(&mut self, event: iced::Event, cursor: iced::mouse::Cursor) -> Vec<ViewMessage> {
            let mut messages = Vec::new();
            self.element.as_widget_mut().update(
                &mut self.tree,
                &event,
                Layout::new(&self.node),
                cursor,
                &self.renderer,
                &mut iced::advanced::clipboard::Null,
                &mut iced::advanced::Shell::new(&mut messages),
                &Rectangle::with_size(Size::new(self.width, self.height)),
            );
            messages
        }

        /// Swap in a re-rendered element and reconcile the same tree, as the
        /// runtime does after a state change.
        fn rerender(&mut self, element: Element<'a, ViewMessage>) {
            self.element = element;
            self.tree.diff(self.element.as_widget());
            self.node = self.element.as_widget_mut().layout(
                &mut self.tree,
                &self.renderer,
                &widget_layout::Limits::new(Size::ZERO, Size::new(self.width, self.height)),
            );
        }
    }

    fn space(pressed: bool) -> iced::Event {
        use iced::keyboard;
        let key = keyboard::Key::Named(keyboard::key::Named::Space);
        let physical_key =
            keyboard::key::Physical::Unidentified(keyboard::key::NativeCode::Unidentified);
        iced::Event::Keyboard(if pressed {
            keyboard::Event::KeyPressed {
                key: key.clone(),
                modified_key: key,
                physical_key,
                location: keyboard::Location::Standard,
                modifiers: keyboard::Modifiers::empty(),
                text: None,
                repeat: false,
            }
        } else {
            keyboard::Event::KeyReleased {
                key: key.clone(),
                modified_key: key,
                physical_key,
                location: keyboard::Location::Standard,
                modifiers: keyboard::Modifiers::empty(),
            }
        })
    }

    /// The List body's width in a 1024px window.
    fn compact_width() -> f32 {
        1024.0 - 2.0 * GUTTER
    }

    #[test]
    fn tab_and_shift_tab_follow_the_visual_order_and_skip_disabled_controls() {
        use iced::advanced::widget::operation::focusable;
        let mut home = home_with(&[("a", "Alpha"), ("b", "Beta"), ("", "Nameless")]);
        home.server_cube_limit = Some(10);
        catalog(&mut home, CatalogMessage::Select(key("b")));
        let mut mounted = mount_window(launcher_view(&home), 1024.0, 768.0);
        // Top bar (the pending session offers no Connect control), then the
        // body: details stacked first, search, each actionable row (the
        // nameless row has no press target), then the pinned dock.
        let expected = vec![
            network_id(),
            settings_id(),
            close_details_id(),
            details_open_id(&key("b")),
            rename_id(&key("b")),
            delete_id(&key("b")),
            search_id(),
            select_id(&key("a")),
            open_id(&key("a")),
            select_id(&key("b")),
            open_id(&key("b")),
            create_id(),
            import_id(),
            super::super::launcher_chrome::plan_id(),
            key_id(),
        ];
        let mut forward = Vec::new();
        for _ in 0..expected.len() {
            mounted.operate(focusable::focus_next::<()>());
            forward.push(mounted.focused().expect("a focused control"));
        }
        assert_eq!(forward, expected);
        mounted.operate(focusable::focus_next::<()>());
        assert_eq!(mounted.focused(), None);

        let mut backward = Vec::new();
        for _ in 0..expected.len() {
            mounted.operate(focusable::focus_previous::<()>());
            backward.push(mounted.focused().expect("a focused control"));
        }
        backward.reverse();
        assert_eq!(backward, expected);
    }

    #[test]
    fn space_activates_once_while_held_and_a_held_pointer_never_acts_on_a_replacement() {
        use iced::advanced::widget::operation::focusable;
        let mut home = home_with(&[("a", "Alpha"), ("b", "Beta")]);
        let projection = project(&home);
        let mut mounted = mount(layout(&projection, compact_width()), compact_width());

        mounted.operate(focusable::focus::<()>(select_id(&key("a"))));
        let pressed = mounted.send(space(true), iced::mouse::Cursor::Unavailable);
        assert!(
            matches!(
                pressed.as_slice(),
                [ViewMessage::Catalog(CatalogMessage::Select(k))] if k == &key("a")
            ),
            "{:?}",
            pressed
        );
        assert!(mounted
            .send(space(true), iced::mouse::Cursor::Unavailable)
            .is_empty());
        assert!(mounted
            .send(space(false), iced::mouse::Cursor::Unavailable)
            .is_empty());

        // Pointer held on Alpha's Open; the list reorders under it before the
        // release. The control now at that position belongs to Beta and must
        // not open, nor may Alpha open from a stale press.
        let target = bounds_of(
            &focus_bounds(layout(&projection, compact_width()), compact_width()),
            &open_id(&key("a")),
        );
        let cursor = iced::mouse::Cursor::Available(target.center());
        let down = mounted.send(
            iced::Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left)),
            cursor,
        );
        // The press really landed on Alpha's Open: exactly its focus intent.
        assert!(
            matches!(
                down.as_slice(),
                [ViewMessage::Catalog(CatalogMessage::Focus(id))] if id == &open_id(&key("a"))
            ),
            "{:?}",
            down
        );
        cubes_mut(&mut home).reverse();
        let reordered = project(&home);
        mounted.rerender(layout(&reordered, compact_width()));
        let up = mounted.send(
            iced::Event::Mouse(iced::mouse::Event::ButtonReleased(
                iced::mouse::Button::Left,
            )),
            cursor,
        );
        assert!(
            !up.iter()
                .any(|message| matches!(message, ViewMessage::RunCatalog(_))),
            "{:?}",
            up
        );
    }

    fn record(id: &str, name: &str, chain: ChainId) -> CubeSettings {
        CubeSettings::new_with_raw_id(id.to_string(), name.to_string(), chain)
    }

    #[test]
    fn mixed_chain_records_sharing_an_id_never_borrow_each_others_identity() {
        let foreign_key = CubeCatalogKey {
            chain: ChainId::Testnet4,
            cube_id: "a".to_string(),
        };
        for foreign_first in [true, false] {
            let foreign = record("a", "Foreign", ChainId::Testnet4);
            let local = record("a", "Local", ChainId::Bitcoin);
            let mut home = home_with(&[]);
            home.state = State::Cubes {
                cubes: if foreign_first {
                    vec![foreign, local]
                } else {
                    vec![local, foreign]
                },
                create_cube: false,
                source: ChainId::Bitcoin,
            };
            {
                let projection = project(&home);
                let row = |name: &str| {
                    projection
                        .locals
                        .iter()
                        .find(|row| row.name == name)
                        .expect("row present")
                        .clone()
                };
                assert_eq!(row("Local").key, key("a"));
                assert_eq!(row("Foreign").key, foreign_key);
                assert!(row("Local").actionable, "{}", foreign_first);
                assert!(!row("Foreign").actionable, "{}", foreign_first);
            }

            catalog(&mut home, CatalogMessage::Select(foreign_key.clone()));
            assert_eq!(home.catalog.selected, None);
            catalog(&mut home, CatalogMessage::Select(key("a")));
            assert_eq!(home.catalog.selected, Some(key("a")));
            {
                let projection = project(&home);
                let selected = &projection.locals[projection.selected.expect("selected")];
                assert_eq!(selected.name, "Local");
                assert_eq!(selected.cube.network, ChainId::Bitcoin);
            }

            let inspection = capture::inspect(
                "catalog-mixed-chain-same-id",
                catalog_view(&home),
                WIDE,
                "dark",
                theme::Theme::dark(),
            );
            assert!(inspection.has_focus(select_id(&key("a"))));
            assert!(inspection.has_focus(details_open_id(&key("a"))));
            assert!(!inspection.has_focus(select_id(&foreign_key)));
            assert!(!inspection.has_focus(open_id(&foreign_key)));
            // The foreign record shows its own chain, not the current one.
            assert!(inspection.contains("Testnet4"));
            let ids: Vec<_> = inspection.focus.iter().map(|(id, _)| id.clone()).collect();
            let unique: HashSet<_> = ids.iter().cloned().collect();
            assert_eq!(unique.len(), ids.len(), "aliased action IDs");
            assert!(matches!(
                activate_once(catalog_view(&home), details_open_id(&key("a")), false),
                ViewMessage::RunCatalog(k) if k == key("a")
            ));
        }
    }

    #[test]
    fn leaving_the_list_clears_selection_through_real_home_dispatch() {
        let mut home = home_with(&[("a", "Alpha"), ("b", "Beta")]);
        home.server_cube_limit = Some(10);
        catalog(&mut home, CatalogMessage::Query("a".to_string()));

        catalog(&mut home, CatalogMessage::Select(key("a")));
        let _ = home.update(Message::View(ViewMessage::ShowCreateCube(true)));
        assert!(!catalog_visible(&home));
        assert_eq!(home.catalog.selected, None);
        let _ = home.update(Message::View(ViewMessage::ShowCreateCube(false)));
        assert!(catalog_visible(&home));
        assert_eq!(home.catalog.selected, None);
        assert_eq!(project(&home).selected, None);

        catalog(&mut home, CatalogMessage::Select(key("a")));
        let _ = home.update(Message::View(ViewMessage::GoToSection(
            HomeSection::Settings,
        )));
        assert_eq!(home.catalog.selected, None);
        // Nothing can be selected while the List is not on screen.
        catalog(&mut home, CatalogMessage::Select(key("a")));
        assert_eq!(home.catalog.selected, None);
        let _ = home.update(Message::View(ViewMessage::GoToSection(HomeSection::Cubes)));
        assert_eq!(home.catalog.selected, None);
        assert_eq!(project(&home).selected, None);
        // Search is a List preference for this Home and survives the visit.
        assert_eq!(home.catalog.query, "a");
        catalog(&mut home, CatalogMessage::Select(key("a")));
        assert_eq!(selected_id(&home).as_deref(), Some("a"));
    }

    fn checked(cubes: Vec<CubeSettings>) -> Message {
        Message::Checked {
            for_chain: ChainId::Bitcoin,
            res: Ok(State::Cubes {
                cubes,
                create_cube: false,
                source: ChainId::Bitcoin,
            }),
        }
    }

    #[test]
    fn list_refresh_removal_or_replacement_never_migrates_the_selection() {
        let mut home = home_with(&[("a", "Alpha"), ("b", "Beta")]);
        catalog(&mut home, CatalogMessage::Select(key("b")));

        // Same Cube, new position: still selected.
        let _ = home.update(checked(vec![
            record("b", "Beta", ChainId::Bitcoin),
            record("a", "Alpha", ChainId::Bitcoin),
            record("c", "Gamma", ChainId::Bitcoin),
        ]));
        assert_eq!(selected_id(&home).as_deref(), Some("b"));

        // Beta replaced by another Cube in its place: cleared, not moved.
        let _ = home.update(checked(vec![
            record("c", "Gamma", ChainId::Bitcoin),
            record("a", "Alpha", ChainId::Bitcoin),
        ]));
        assert_eq!(home.catalog.selected, None);
        assert_eq!(selected_id(&home), None);

        // A record keeping the ID but claiming another chain is a different Cube.
        catalog(&mut home, CatalogMessage::Select(key("a")));
        let _ = home.update(checked(vec![record("a", "Alpha", ChainId::Testnet4)]));
        assert_eq!(home.catalog.selected, None);
        assert_eq!(project(&home).selected, None);
        assert!(!project(&home).locals[0].actionable);
    }

    #[test]
    fn space_held_on_open_fires_once_and_focus_never_moves_to_another_cube() {
        use iced::advanced::widget::operation::focusable;
        let none = iced::mouse::Cursor::Unavailable;
        let mut home = home_with(&[("a", "Alpha"), ("b", "Beta")]);
        let projection = project(&home);
        let mut mounted = mount(layout(&projection, compact_width()), compact_width());

        // Activation fires on press, once, for the focused Cube.
        mounted.operate(focusable::focus::<()>(open_id(&key("a"))));
        let pressed = mounted.send(space(true), none);
        assert!(
            matches!(pressed.as_slice(), [ViewMessage::RunCatalog(k)] if k == &key("a")),
            "{:?}",
            pressed
        );
        // Reordered while Space is held: focus is dropped, not handed to Beta.
        cubes_mut(&mut home).reverse();
        let reordered = project(&home);
        mounted.rerender(layout(&reordered, compact_width()));
        assert_eq!(mounted.focused(), None);
        assert!(mounted.send(space(false), none).is_empty());
        assert!(mounted.send(space(true), none).is_empty());
        let _ = mounted.send(space(false), none);

        // Removed while Space is held: nothing else acts on release or repeat.
        mounted.operate(focusable::focus::<()>(open_id(&key("a"))));
        let pressed = mounted.send(space(true), none);
        assert!(
            matches!(pressed.as_slice(), [ViewMessage::RunCatalog(k)] if k == &key("a")),
            "{:?}",
            pressed
        );
        cubes_mut(&mut home).retain(|cube| cube.id != "a");
        let removed = project(&home);
        mounted.rerender(layout(&removed, compact_width()));
        assert_eq!(mounted.focused(), None);
        assert!(mounted.send(space(false), none).is_empty());
        assert!(mounted.send(space(true), none).is_empty());
    }

    /// Focus targets, text and scrollables of the real `Home::view` tree.
    struct HomeTree {
        focus: Vec<(Id, Rectangle)>,
        text: Vec<(String, Rectangle)>,
        /// (id, viewport bounds, content bounds)
        scrollables: Vec<(Option<Id>, Rectangle, Rectangle)>,
        /// (id, current scroll translation)
        offsets: Vec<(Option<Id>, iced::Vector)>,
        /// Messages the tree published for the initial redraw and any wheel.
        emitted: usize,
    }

    impl HomeTree {
        fn has(&self, id: &Id) -> bool {
            self.focus.iter().any(|(target, _)| target == id)
        }
        fn bounds(&self, id: &Id) -> Rectangle {
            bounds_of(&self.focus, id)
        }
        fn contains(&self, needle: &str) -> bool {
            self.text.iter().any(|(value, _)| value.contains(needle))
        }
        fn scroll(&self, id: &Id) -> (Rectangle, Rectangle, iced::Vector) {
            let (_, bounds, content) = self
                .scrollables
                .iter()
                .find(|(target, _, _)| target.as_ref() == Some(id))
                .cloned()
                .expect("named scrollable present");
            let offset = self
                .offsets
                .iter()
                .find(|(target, _)| target.as_ref() == Some(id))
                .map(|(_, offset)| *offset)
                .unwrap_or_default();
            (bounds, content, offset)
        }
        fn has_any_catalog_control(&self) -> bool {
            self.has(&search_id())
                || self.has(&create_id())
                || self.has(&import_id())
                || self.has(&settings_id())
                || self.has(&theme_id())
                || self
                    .scrollables
                    .iter()
                    .any(|(id, _, _)| id.as_ref() == Some(&Id::from(CATALOG_SCROLL_ID)))
        }
    }

    fn home_tree(
        scenario: &str,
        home: &Home,
        window: (u32, u32),
        appearance: &str,
        theme: theme::Theme,
    ) -> HomeTree {
        home_tree_scrolled(scenario, home, window, appearance, theme, None)
    }

    /// Lays out the actual `Home::view` (the Launcher chrome when the List is
    /// visible, otherwise the legacy sidebar frame, and the modal chain) at a
    /// window size, sends the normal first `RedrawRequested`, optionally a
    /// mouse wheel of `wheel_lines` over the List body (negative scrolls down),
    /// and probes it. With `R05B_CAPTURE_DIR` set it also draws that tree with
    /// a fresh headless tiny-skia renderer to a PNG labelled
    /// `home-tree-synthetic`: synthetic fixtures, no OS window, pane grid,
    /// tab bar, services or native app.
    fn home_tree_scrolled(
        scenario: &str,
        home: &Home,
        window: (u32, u32),
        appearance: &str,
        theme: theme::Theme,
        wheel_lines: Option<f32>,
    ) -> HomeTree {
        #[derive(Default)]
        struct Probe {
            focus: Vec<(Id, Rectangle)>,
            text: Vec<(String, Rectangle)>,
            scrollables: Vec<(Option<Id>, Rectangle, Rectangle)>,
            offsets: Vec<(Option<Id>, iced::Vector)>,
        }
        impl Operation for Probe {
            fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
                operate(self);
            }
            fn focusable(&mut self, id: Option<&Id>, bounds: Rectangle, _: &mut dyn Focusable) {
                if let Some(id) = id {
                    self.focus.push((id.clone(), bounds));
                }
            }
            fn text(&mut self, _id: Option<&Id>, bounds: Rectangle, value: &str) {
                self.text.push((value.to_owned(), bounds));
            }
            fn scrollable(
                &mut self,
                id: Option<&Id>,
                bounds: Rectangle,
                content_bounds: Rectangle,
                translation: iced::Vector,
                _state: &mut dyn iced::advanced::widget::operation::Scrollable,
            ) {
                self.scrollables.push((id.cloned(), bounds, content_bounds));
                self.offsets.push((id.cloned(), translation));
            }
        }
        for bytes in font::load() {
            iced::advanced::graphics::text::font_system()
                .write()
                .unwrap()
                .load_font(bytes);
        }
        let mut renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
            font::REGULAR,
            iced::Pixels(16.0),
            Some("tiny-skia"),
        ))
        .unwrap();
        let size = Size::new(window.0 as f32, window.1 as f32);
        let mut element = home.view();
        let mut tree = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &widget_layout::Limits::new(Size::ZERO, size),
        );
        let viewport = Rectangle::with_size(size);
        let mut emitted: Vec<Message> = Vec::new();
        element.as_widget_mut().update(
            &mut tree,
            &iced::Event::Window(iced::window::Event::RedrawRequested(
                std::time::Instant::now(),
            )),
            Layout::new(&node),
            iced::mouse::Cursor::Unavailable,
            &renderer,
            &mut iced::advanced::clipboard::Null,
            &mut iced::advanced::Shell::new(&mut emitted),
            &viewport,
        );
        if let Some(lines) = wheel_lines {
            let mut first = Probe::default();
            element
                .as_widget_mut()
                .operate(&mut tree, Layout::new(&node), &renderer, &mut first);
            let body = first
                .scrollables
                .iter()
                .find(|(id, _, _)| id.as_ref() == Some(&Id::from(CATALOG_SCROLL_ID)))
                .map(|(_, bounds, _)| *bounds)
                .expect("the List body to scroll");
            element.as_widget_mut().update(
                &mut tree,
                &iced::Event::Mouse(iced::mouse::Event::WheelScrolled {
                    delta: iced::mouse::ScrollDelta::Lines { x: 0.0, y: lines },
                }),
                Layout::new(&node),
                iced::mouse::Cursor::Available(body.center()),
                &renderer,
                &mut iced::advanced::clipboard::Null,
                &mut iced::advanced::Shell::new(&mut emitted),
                &viewport,
            );
        }
        let mut probe = Probe::default();
        element
            .as_widget_mut()
            .operate(&mut tree, Layout::new(&node), &renderer, &mut probe);
        if let Ok(directory) = std::env::var("R05B_CAPTURE_DIR") {
            std::fs::create_dir_all(&directory).expect("create capture directory");
            element.as_widget().draw(
                &tree,
                &mut renderer,
                &theme,
                &iced::advanced::renderer::Style {
                    text_color: theme.colors.text.primary,
                },
                Layout::new(&node),
                iced::mouse::Cursor::Unavailable,
                &viewport,
            );
            let pixels = renderer.screenshot(
                Size::new(window.0, window.1),
                1.0,
                theme.colors.general.background,
            );
            let file = format!(
                "{}-{}-{}x{}-home-tree-synthetic.png",
                scenario, appearance, window.0, window.1
            );
            ::image::save_buffer(
                std::path::Path::new(&directory).join(file),
                &pixels,
                window.0,
                window.1,
                ::image::ColorType::Rgba8,
            )
            .expect("save synthetic Home capture");
        }
        HomeTree {
            focus: probe.focus,
            text: probe.text,
            scrollables: probe.scrollables,
            offsets: probe.offsets,
            emitted: emitted.len(),
        }
    }

    fn themed(mut home: Home, appearance: &str) -> (Home, theme::Theme) {
        use coincube_ui::theme::palette::ThemeMode;
        if appearance == "light" {
            home.theme_mode = ThemeMode::Light;
            (home, theme::Theme::light())
        } else {
            home.theme_mode = ThemeMode::Dark;
            (home, theme::Theme::dark())
        }
    }

    fn rows(count: usize) -> Home {
        let names: Vec<(String, String)> = (0..count)
            .map(|index| (format!("cube-{}", index), format!("Cube {}", index)))
            .collect();
        let pairs: Vec<(&str, &str)> = names
            .iter()
            .map(|(id, name)| (id.as_str(), name.as_str()))
            .collect();
        let mut home = home_with(&pairs);
        home.server_cube_limit = Some(30);
        home
    }

    const WINDOWS: [(u32, u32); 2] = [(1024, 768), (1440, 900)];

    #[test]
    fn real_home_tree_puts_details_beside_the_list_at_1440_and_above_it_at_1024() {
        let mut home = rows(9);
        catalog(&mut home, CatalogMessage::Select(key("cube-4")));
        for window in WINDOWS {
            let tree = home_tree("r05b-layout", &home, window, "dark", theme::Theme::dark());
            let (width, height) = (window.0 as f32, window.1 as f32);
            let close = tree.bounds(&close_details_id());
            let row = tree.bounds(&select_id(&key("cube-0")));
            if window.0 >= 1440 {
                assert!(close.x > row.x + LIST_MIN_WIDTH, "{:?} {:?}", close, row);
            } else {
                assert!(close.y < row.y, "{:?} {:?}", close, row);
            }
            // The top bar sits in its 64px band and the dock is visible at the
            // bottom of the window, not clipped below it.
            let settings = tree.bounds(&settings_id());
            assert!(
                settings.y + settings.height <= TOP_BAR_HEIGHT,
                "{:?}",
                settings
            );
            let create = tree.bounds(&create_id());
            assert!(create.y + create.height <= height, "{:?}", create);
            assert!(create.y > height - 120.0, "{:?}", create);
            // No control runs off the side of the window.
            for (id, bounds) in &tree.focus {
                assert!(
                    bounds.x >= 0.0 && bounds.x + bounds.width <= width,
                    "{:?} {:?} in {}",
                    id,
                    bounds,
                    width
                );
            }
            assert_eq!(tree.emitted, 0);
        }
    }

    #[test]
    fn the_named_body_alone_owns_overflow_and_the_chrome_stays_outside_it() {
        let home = rows(20);
        let tree = home_tree(
            "r05b-scroll",
            &home,
            (1024, 768),
            "dark",
            theme::Theme::dark(),
        );
        let catalog_id = Id::from(CATALOG_SCROLL_ID);
        let overflowing: Vec<_> = tree
            .scrollables
            .iter()
            .filter(|(_, bounds, content)| content.height > bounds.height + 1.0)
            .collect();
        assert_eq!(overflowing.len(), 1, "{:?}", tree.scrollables);
        assert_eq!(overflowing[0].0.as_ref(), Some(&catalog_id));
        let body = overflowing[0].1;
        let settings = tree.bounds(&settings_id());
        assert!(
            settings.y + settings.height <= body.y,
            "{:?} {:?}",
            settings,
            body
        );
        let create = tree.bounds(&create_id());
        assert!(create.y >= body.y + body.height, "{:?} {:?}", create, body);
        // One dock, one copy of each control.
        for id in [create_id(), import_id(), settings_id(), key_id()] {
            assert_eq!(
                tree.focus
                    .iter()
                    .filter(|(target, _)| target == &id)
                    .count(),
                1
            );
        }
    }

    #[test]
    fn a_mouse_wheel_scrolls_only_the_body_and_reaches_the_last_row() {
        let home = rows(20);
        let catalog_id = Id::from(CATALOG_SCROLL_ID);
        let before = home_tree_scrolled(
            "r05b-wheel",
            &home,
            (1024, 768),
            "dark",
            theme::Theme::dark(),
            None,
        );
        let after = home_tree_scrolled(
            "r05b-wheel",
            &home,
            (1024, 768),
            "dark",
            theme::Theme::dark(),
            Some(-200.0),
        );
        // The chrome does not move.
        assert_eq!(before.bounds(&create_id()), after.bounds(&create_id()));
        assert_eq!(before.bounds(&settings_id()), after.bounds(&settings_id()));
        let (body, _, offset) = after.scroll(&catalog_id);
        assert!(offset.y > 0.0, "{:?}", offset);
        // The last row is now inside the body viewport.
        let last = after.bounds(&select_id(&key("cube-19")));
        let top = last.y - offset.y;
        assert!(top >= body.y - 1.0, "{:?} {:?} {:?}", last, offset, body);
        assert!(
            top + last.height <= body.y + body.height + 1.0,
            "{:?} {:?} {:?}",
            last,
            offset,
            body
        );
        assert_eq!(after.emitted, 0);
    }

    /// How a chrome control is activated in a test.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Press {
        Pointer,
        Enter,
        /// Space pressed, pressed again while held (no repeat), released.
        SpaceHeld,
    }

    /// The single non-focus message from activating `target` in the real
    /// chrome at 1440x900; a pointer press also publishes exactly its focus.
    fn activate_chrome(home: &Home, target: Id, press: Press) -> ViewMessage {
        let pointer = press == Press::Pointer;
        use iced::advanced::widget::operation::focusable;
        let bounds = bounds_of(
            &focus_bounds_window(launcher_view(home), 1440.0, 900.0),
            &target,
        );
        let mut mounted = mount_window(launcher_view(home), 1440.0, 900.0);
        let messages = if pointer {
            let cursor = iced::mouse::Cursor::Available(bounds.center());
            let mut messages = mounted.send(
                iced::Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left)),
                cursor,
            );
            messages.extend(mounted.send(
                iced::Event::Mouse(iced::mouse::Event::ButtonReleased(
                    iced::mouse::Button::Left,
                )),
                cursor,
            ));
            messages
        } else {
            mounted.operate(focusable::focus::<()>(target.clone()));
            let none = iced::mouse::Cursor::Unavailable;
            if press == Press::Enter {
                let mut messages = mounted.send(enter(true), none);
                messages.extend(mounted.send(enter(false), none));
                messages
            } else {
                let mut messages = mounted.send(space(true), none);
                messages.extend(mounted.send(space(true), none));
                messages.extend(mounted.send(space(false), none));
                messages
            }
        };
        let (focus, rest): (Vec<_>, Vec<_>) = messages
            .into_iter()
            .partition(|message| matches!(message, ViewMessage::Catalog(CatalogMessage::Focus(_))));
        if pointer {
            assert!(
                matches!(
                    focus.as_slice(),
                    [ViewMessage::Catalog(CatalogMessage::Focus(id))] if id == &target
                ),
                "{:?}",
                focus
            );
        } else {
            assert!(focus.is_empty(), "{:?}", focus);
        }
        assert_eq!(rest.len(), 1, "{:?}", rest);
        rest.into_iter().next().unwrap()
    }

    fn enter(pressed: bool) -> iced::Event {
        use iced::keyboard;
        let key = keyboard::Key::Named(keyboard::key::Named::Enter);
        let physical_key =
            keyboard::key::Physical::Unidentified(keyboard::key::NativeCode::Unidentified);
        iced::Event::Keyboard(if pressed {
            keyboard::Event::KeyPressed {
                key: key.clone(),
                modified_key: key,
                physical_key,
                location: keyboard::Location::Standard,
                modifiers: keyboard::Modifiers::empty(),
                text: None,
                repeat: false,
            }
        } else {
            keyboard::Event::KeyReleased {
                key: key.clone(),
                modified_key: key,
                physical_key,
                location: keyboard::Location::Standard,
                modifiers: keyboard::Modifiers::empty(),
            }
        })
    }

    /// Focus targets of an element laid out in exactly a window-sized box.
    fn focus_bounds_window(
        element: Element<'_, ViewMessage>,
        width: f32,
        height: f32,
    ) -> Vec<(Id, Rectangle)> {
        #[derive(Default)]
        struct Probe(Vec<(Id, Rectangle)>);
        impl Operation for Probe {
            fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
                operate(self);
            }
            fn focusable(&mut self, id: Option<&Id>, bounds: Rectangle, _: &mut dyn Focusable) {
                if let Some(id) = id {
                    self.0.push((id.clone(), bounds));
                }
            }
        }
        let mut mounted = mount_window(element, width, height);
        let mut probe = Probe::default();
        mounted.element.as_widget_mut().operate(
            &mut mounted.tree,
            Layout::new(&mounted.node),
            &mounted.renderer,
            &mut probe,
        );
        probe.0
    }

    /// Bounds of the container carrying `id` in a window-sized layout.
    #[test]
    fn the_network_chip_opens_the_prototype_menu_and_filters_by_network() {
        use super::super::launcher_chrome::network_option_id;
        let mut home = rows(2);
        home.displayed_networks = vec![ChainId::Bitcoin, ChainId::Testnet4];
        let field = bounds_of(
            &focus_bounds_window(launcher_view(&home), 1440.0, 900.0),
            &network_id(),
        );
        assert!(field.y + field.height <= TOP_BAR_HEIGHT, "{:?}", field);
        assert!(field.width < 220.0, "the chip hugs its label: {:?}", field);
        assert!(matches!(
            activate_chrome(&home, network_id(), Press::Pointer),
            ViewMessage::Catalog(CatalogMessage::NetworkMenu(true))
        ));

        catalog(&mut home, CatalogMessage::NetworkMenu(true));
        let tree = home_tree("network-menu", &home, WIDE, "dark", theme::Theme::dark());
        assert!(tree.contains("Filters change what is shown, never a Cube's network."));
        assert!(tree.contains("Bitcoin mainnet"));
        assert!(tree.contains("Testnet4"));
        let option = bounds_of(
            &focus_bounds_window(launcher_view(&home), 1440.0, 900.0),
            &network_option_id(ChainId::Testnet4),
        );
        assert!(
            option.y >= field.y + 40.0 && (option.x - field.x).abs() < 12.0,
            "the menu opens under the chip: chip {:?}, row {:?}",
            field,
            option
        );
        home.catalog.filter = NetworkFilter::Network(ChainId::Bitcoin);
        for press in [Press::Pointer, Press::Enter] {
            assert!(matches!(
                activate_chrome(&home, network_option_id(ChainId::Testnet4), press),
                ViewMessage::Catalog(CatalogMessage::Filter(NetworkFilter::Network(
                    ChainId::Testnet4
                )))
            ));
            // The network already filtered to closes the menu.
            assert!(matches!(
                activate_chrome(&home, network_option_id(ChainId::Bitcoin), press),
                ViewMessage::Catalog(CatalogMessage::NetworkMenu(false))
            ));
        }
        let _ = home.update(Message::View(ViewMessage::SelectNetwork(ChainId::Testnet4)));
        assert!(
            !home.catalog.network_menu,
            "choosing a network closes the menu"
        );
    }

    /// Bitcoin "a", "b" checked; Testnet4 "t1", "t2" and Signet "s1" read
    /// from their directories. Developer mode lists them all.
    fn all_networks() -> Home {
        let mut home = home_with(&[("a", "Alpha"), ("b", "Beta")]);
        home.catalog.mode = LauncherMode::Cubes;
        home.developer_mode = true;
        // What `refresh_displayed_networks` lists in Developer mode.
        home.displayed_networks = ChainId::LAUNCHER.to_vec();
        let cube = |id: &str, name: &str, chain| {
            CubeSettings::new_with_raw_id(id.to_string(), name.to_string(), chain)
        };
        home.network_catalog_requested = ChainId::LAUNCHER
            .iter()
            .copied()
            .filter(|chain| *chain != ChainId::Bitcoin)
            .collect();
        home.network_catalog = vec![
            (
                ChainId::Testnet4,
                vec![
                    cube("t1", "Lab", ChainId::Testnet4),
                    cube("t2", "Bench", ChainId::Testnet4),
                ],
            ),
            (ChainId::Signet, vec![cube("s1", "Sig", ChainId::Signet)]),
        ];
        home
    }

    fn names(home: &Home) -> Vec<String> {
        project(home)
            .locals
            .iter()
            .map(|row| row.name.clone())
            .collect()
    }

    #[test]
    fn the_floor_lists_every_network_and_each_filter_narrows_it() {
        let mut home = all_networks();
        assert_eq!(names(&home), ["Alpha", "Beta", "Lab", "Bench", "Sig"]);
        let projection = project(&home);
        assert_eq!((projection.local_total, projection.hidden), (5, 0));
        assert_eq!(
            projection.network_counts,
            [
                (ChainId::Bitcoin, 2),
                (ChainId::Testnet4, 2),
                (ChainId::Testnet, 0),
                (ChainId::Signet, 1),
                (ChainId::Regtest, 0)
            ]
        );

        catalog(&mut home, CatalogMessage::Filter(NetworkFilter::Live));
        assert_eq!(names(&home), ["Alpha", "Beta"]);
        assert_eq!(project(&home).hidden, 3);

        catalog(&mut home, CatalogMessage::Filter(NetworkFilter::Test));
        assert_eq!(names(&home), ["Lab", "Bench", "Sig"]);

        // One network also makes it current, for Create and Import.
        let task = home.update_catalog(CatalogMessage::Filter(NetworkFilter::Network(
            ChainId::Signet,
        )));
        assert!(task.units() > 0, "filtering to Signet selects that network");
        assert_eq!(names(&home), ["Sig"]);

        catalog(&mut home, CatalogMessage::Filter(NetworkFilter::All));
        assert_eq!(names(&home).len(), 5);
    }

    #[test]
    fn test_filters_fall_back_to_all_without_developer_mode() {
        let mut home = all_networks();
        home.catalog.filter = NetworkFilter::Test;
        home.developer_mode = false;
        home.refresh_displayed_networks();
        assert_eq!(home.effective_filter(), NetworkFilter::All);
        home.catalog.filter = NetworkFilter::Network(ChainId::Regtest);
        assert_eq!(
            home.effective_filter(),
            NetworkFilter::All,
            "a network that is not listed cannot be the filter"
        );
    }

    #[test]
    fn a_filter_that_hides_every_cube_shows_the_empty_state() {
        let mut home = all_networks();
        catalog(
            &mut home,
            CatalogMessage::Filter(NetworkFilter::Network(ChainId::Regtest)),
        );
        let projection = project(&home);
        assert!(projection.locals.is_empty());
        assert_eq!(projection.hidden, 5);
        assert!(projection.no_matches());
        let tree = home_tree("filtered-out", &home, WIDE, "dark", theme::Theme::dark());
        assert!(tree.contains("No Cubes match these filters"));
        assert!(tree.contains("5 Cubes hidden."));
        assert!(tree.has(&clear_filters_empty_id()));
        assert!(matches!(
            activate_chrome(&home, clear_filters_id(), Press::Pointer),
            ViewMessage::Catalog(CatalogMessage::Filter(NetworkFilter::All))
        ));
    }

    #[test]
    fn the_top_bar_shows_the_canvas_chips_count_and_clear_link() {
        let mut home = all_networks();
        let tree = home_tree("filters", &home, WIDE, "dark", theme::Theme::dark());
        for label in [
            "All networks",
            "Live networks",
            "Test networks",
            "Network",
            "5 Cubes",
        ] {
            assert!(tree.contains(label), "missing {:?}", label);
        }
        assert!(!tree.contains("Clear filters"));
        catalog(&mut home, CatalogMessage::Filter(NetworkFilter::Live));
        let tree = home_tree("filters-live", &home, WIDE, "dark", theme::Theme::dark());
        assert!(tree.contains("2 of 5 Cubes · 3 hidden"));
        assert!(tree.has(&clear_filters_id()));
        assert!(matches!(
            activate_chrome(&home, filter_id("test"), Press::Pointer),
            ViewMessage::Catalog(CatalogMessage::Filter(NetworkFilter::Test))
        ));
        // Without Developer mode: one network, so no group chips at all.
        let mut plain = home_with(&[("a", "Alpha")]);
        plain.catalog.mode = LauncherMode::Cubes;
        let tree = home_tree("filters-plain", &plain, WIDE, "dark", theme::Theme::dark());
        assert!(!tree.contains("All networks"));
        assert!(!tree.contains("Live networks"));
        assert!(tree.contains("Network"));
    }

    #[test]
    fn picking_a_cube_on_another_network_switches_then_selects_it() {
        let mut home = all_networks();
        let lab = CubeCatalogKey {
            chain: ChainId::Testnet4,
            cube_id: "t1".into(),
        };
        let bench = CubeCatalogKey {
            chain: ChainId::Testnet4,
            cube_id: "t2".into(),
        };
        let task = home.update_catalog(CatalogMessage::SelectFloor(lab.clone()));
        assert!(
            run_task(task).iter().any(|m| matches!(
                m,
                Message::View(ViewMessage::SelectNetwork(ChainId::Testnet4))
            )),
            "the switch to Testnet4 is requested"
        );
        assert_eq!(
            home.catalog.pending_select,
            Some((lab.clone(), PendingIntent::Select))
        );
        assert!(
            home.catalog.selected.is_none(),
            "nothing selected until checked"
        );

        // The real switch. Its probe is not run: Testnet4's list arrives below.
        drop(home.update(Message::View(ViewMessage::SelectNetwork(ChainId::Testnet4))));
        assert_eq!(home.network, ChainId::Testnet4);
        // A later pick while the switch is checked wins.
        drop(home.update_catalog(CatalogMessage::SelectFloor(bench.clone())));
        assert_eq!(
            home.catalog.pending_select,
            Some((bench.clone(), PendingIntent::Select))
        );

        let checked = State::Cubes {
            cubes: home.network_catalog(ChainId::Testnet4).to_vec(),
            create_cube: false,
            source: ChainId::Testnet4,
        };
        let replay = run_task(home.update(Message::Checked {
            for_chain: ChainId::Testnet4,
            res: Ok(checked),
        }));
        assert!(home.catalog.pending_select.is_none());
        assert!(
            replay.iter().any(|m| matches!(
                m,
                Message::View(ViewMessage::Catalog(CatalogMessage::SelectFloor(key))) if *key == bench
            )),
            "the pending selection is replayed: {:?}",
            replay
        );
        // As the runtime would: feed the replay back.
        for message in replay {
            if matches!(message, Message::View(ViewMessage::Catalog(_))) {
                drop(home.update(message));
            }
        }
        assert_eq!(home.catalog.selected, Some(bench));
        // The floor keeps every network, in the same order.
        assert_eq!(names(&home), ["Alpha", "Beta", "Lab", "Bench", "Sig"]);
    }

    /// Runs a task's futures to completion and returns its messages.
    fn run_task(task: Task<Message>) -> Vec<Message> {
        use iced_runtime::futures::futures::StreamExt;
        let Some(stream) = iced_runtime::task::into_stream(task) else {
            return Vec::new();
        };
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async move {
                stream
                    .filter_map(|action| async move {
                        match action {
                            iced_runtime::Action::Output(message) => Some(message),
                            _ => None,
                        }
                    })
                    .collect()
                    .await
            })
    }

    #[test]
    fn a_refused_switch_drops_the_pending_selection() {
        let mut home = all_networks();
        home.catalog.pending_select = Some((
            CubeCatalogKey {
                chain: ChainId::Testnet4,
                cube_id: "t1".into(),
            },
            PendingIntent::Select,
        ));
        home.developer_mode = false;
        drop(home.update(Message::View(ViewMessage::SelectNetwork(ChainId::Testnet4))));
        assert_eq!(home.network, ChainId::Bitcoin);
        assert!(home.catalog.pending_select.is_none());
    }

    #[test]
    fn restore_rows_stay_on_screen_when_the_filter_hides_every_local_cube() {
        let mut home = all_networks();
        home.remote_cubes = vec![remote("r-1", "Cloud copy", true, true, false, false)];
        home.remote_cubes[0].network = settings::network_to_api_string(ChainId::Bitcoin);
        // Regtest is not current, so Bitcoin's restore rows are filtered too:
        // every Cube is hidden and the canvas empty state shows.
        home.catalog.filter = NetworkFilter::Network(ChainId::Regtest);
        let tree = home_tree(
            "filtered-remotes",
            &home,
            WIDE,
            "dark",
            theme::Theme::dark(),
        );
        assert!(tree.contains("No Cubes match these filters"));
        // Live hides only the test networks' Cubes, and Bitcoin has none
        // local here: its restore row must stay reachable.
        if let State::Cubes { cubes, .. } = &mut home.state {
            cubes.clear();
        }
        home.catalog.filter = NetworkFilter::Live;
        let projection = project(&home);
        assert!(projection.locals.is_empty() && projection.hidden > 0);
        assert!(!projection.no_matches());
        let tree = home_tree("live-remotes", &home, WIDE, "dark", theme::Theme::dark());
        assert!(tree.contains("Cloud copy"), "the restore row is gone");
        assert!(!tree.contains("No Cubes match these filters"));
    }

    #[test]
    fn open_on_another_network_opens_once_its_list_is_checked() {
        let mut home = all_networks();
        let lab = CubeCatalogKey {
            chain: ChainId::Testnet4,
            cube_id: "t1".into(),
        };
        let bench = CubeCatalogKey {
            chain: ChainId::Testnet4,
            cube_id: "t2".into(),
        };
        // List Open on a Testnet4 Cube while Bitcoin is current.
        let switch = run_task(home.update(Message::View(ViewMessage::RunCatalog(lab.clone()))));
        assert!(switch.iter().any(|m| matches!(
            m,
            Message::View(ViewMessage::SelectNetwork(ChainId::Testnet4))
        )));
        assert_eq!(
            home.catalog.pending_select,
            Some((lab.clone(), PendingIntent::Open))
        );
        drop(home.update(Message::View(ViewMessage::SelectNetwork(ChainId::Testnet4))));
        // A second Open during the probe is kept, and wins.
        drop(home.update(Message::View(ViewMessage::RunCatalog(bench.clone()))));
        assert_eq!(
            home.catalog.pending_select,
            Some((bench.clone(), PendingIntent::Open))
        );
        let checked = State::Cubes {
            cubes: home.network_catalog(ChainId::Testnet4).to_vec(),
            create_cube: false,
            source: ChainId::Testnet4,
        };
        let replay = run_task(home.update(Message::Checked {
            for_chain: ChainId::Testnet4,
            res: Ok(checked),
        }));
        assert!(
            replay.iter().any(|m| matches!(
                m,
                Message::View(ViewMessage::RunCatalog(key)) if *key == bench
            )),
            "Open replays as Open: {:?}",
            replay
        );
        // ...and that Open now resolves on the checked list.
        assert_eq!(home.catalog_index(&bench), Some(1));
    }

    #[test]
    fn live_and_test_keep_create_and_import_on_a_network_they_show() {
        let mut home = all_networks();
        // Bitcoin current: Test moves to the first test network.
        let switch = run_task(home.update_catalog(CatalogMessage::Filter(NetworkFilter::Test)));
        assert!(switch.iter().any(|m| matches!(
            m,
            Message::View(ViewMessage::SelectNetwork(ChainId::Testnet4))
        )));
        // Testnet4 current: Live moves back to Bitcoin; All keeps it.
        home.network = ChainId::Testnet4;
        assert_eq!(
            home.filter_target(NetworkFilter::Live),
            Some(ChainId::Bitcoin)
        );
        assert_eq!(home.filter_target(NetworkFilter::Test), None);
        assert_eq!(home.filter_target(NetworkFilter::All), None);
        home.network = ChainId::Bitcoin;
        // The dock no longer captions the network: the filter keeps it shown.
        let tree = home_tree("dock-target", &home, WIDE, "dark", theme::Theme::dark());
        assert!(!tree.contains("New Cubes"));
    }

    /// Developer mode on Testnet4 with its two Cubes checked, Signet's read
    /// alongside, and no Cube on Bitcoin or any other live network.
    fn cubes_on_test_networks_only() -> Home {
        let mut home = all_networks();
        let testnet4 = home.network_catalog(ChainId::Testnet4).to_vec();
        home.network = ChainId::Testnet4;
        home.state = State::Cubes {
            cubes: testnet4,
            create_cube: false,
            source: ChainId::Testnet4,
        };
        home.network_catalog
            .retain(|(chain, _)| *chain != ChainId::Testnet4);
        home.network_catalog_requested = ChainId::LAUNCHER
            .iter()
            .copied()
            .filter(|chain| *chain != ChainId::Testnet4)
            .collect();
        home
    }

    #[test]
    fn live_onto_a_network_without_cubes_keeps_launcher_and_can_create() {
        let mut home = cubes_on_test_networks_only();
        let switch = run_task(home.update_catalog(CatalogMessage::Filter(NetworkFilter::Live)));
        assert!(switch.iter().any(|m| matches!(
            m,
            Message::View(ViewMessage::SelectNetwork(ChainId::Bitcoin))
        )));
        drop(home.update(Message::View(ViewMessage::SelectNetwork(ChainId::Bitcoin))));
        drop(home.update(Message::Checked {
            for_chain: ChainId::Bitcoin,
            res: Ok(State::NoCube { create_cube: false }),
        }));
        // No live Cube to show: keep the empty Launcher until Create is chosen.
        assert_eq!(home.network, ChainId::Bitcoin);
        assert!(catalog_visible(&home));
        assert!(!home.showing_create_journey());
        drop(home.update(Message::View(ViewMessage::ShowCreateCube(true))));
        assert!(home.showing_create_journey());
        assert!(!home.create_network_menu);
        // The test networks' Cubes are still a list to go back to.
        assert!(home.can_leave_create_journey());

        drop(home.update(Message::View(ViewMessage::ShowCreateCube(false))));
        assert!(!home.showing_create_journey());
        assert!(catalog_visible(&home));
        assert_eq!(home.effective_filter(), NetworkFilter::All);
        assert_eq!(names(&home), ["Lab", "Bench", "Sig"]);
        // Create and Import still act on the network Live picked.
        assert_eq!(home.network, ChainId::Bitcoin);
    }

    #[test]
    fn back_from_create_keeps_a_filter_that_still_lists_cubes() {
        let mut home = all_networks();
        catalog(&mut home, CatalogMessage::Filter(NetworkFilter::Live));
        drop(home.update(Message::View(ViewMessage::ShowCreateCube(true))));
        assert!(home.can_leave_create_journey());
        drop(home.update(Message::View(ViewMessage::ShowCreateCube(false))));
        assert!(!home.showing_create_journey());
        assert_eq!(home.effective_filter(), NetworkFilter::Live);
        assert_eq!(names(&home), ["Alpha", "Beta"]);
    }

    #[test]
    fn another_networks_read_preserves_launcher_or_explicit_creation() {
        // Bitcoin checked before any other network was read: start on the Launcher.
        let empty_bitcoin = || {
            let mut home = all_networks();
            home.state = State::NoCube { create_cube: false };
            home.network_catalog.clear();
            home.network_catalog_generation = 1;
            assert!(catalog_visible(&home));
            assert!(!home.showing_create_journey());
            home
        };
        let testnet4_read = || Message::NetworkCatalogLoaded {
            generation: 1,
            chain: ChainId::Testnet4,
            cubes: Ok(vec![CubeSettings::new_with_raw_id(
                "t1".into(),
                "Lab".into(),
                ChainId::Testnet4,
            )]),
        };

        let mut untouched = empty_bitcoin();
        drop(untouched.update(testnet4_read()));
        assert!(!untouched.showing_create_journey());
        assert!(catalog_visible(&untouched));

        let mut typing = empty_bitcoin();
        drop(typing.update(Message::View(ViewMessage::ShowCreateCube(true))));
        drop(typing.update(Message::View(ViewMessage::CubeNameEdited(
            "Half typed".into(),
        ))));
        drop(typing.update(testnet4_read()));
        assert!(matches!(typing.state, State::NoCube { create_cube: true }));
        assert!(typing.showing_create_journey());
        assert_eq!(typing.create_cube_name.value, "Half typed");
        assert!(
            typing.can_leave_create_journey(),
            "the read did land a list"
        );
    }

    #[test]
    fn choosing_from_the_network_menu_returns_focus_to_its_chip() {
        let mut home = all_networks();
        home.catalog.network_menu = true;
        // Bitcoin is current, so no switch: the task is the focus alone.
        let task = home.update_catalog(CatalogMessage::Filter(NetworkFilter::Network(
            ChainId::Bitcoin,
        )));
        assert!(task.units() > 0, "focus returns to the Network chip");
        assert!(!home.catalog.network_menu);
        home.catalog.network_menu = false;
        let task = home.update_catalog(CatalogMessage::Filter(NetworkFilter::All));
        assert_eq!(task.units(), 0, "a chip click keeps its own focus");
    }

    #[test]
    fn a_failed_read_keeps_the_last_list_and_says_unavailable() {
        let mut home = all_networks();
        home.network_catalog_generation = 1;
        drop(home.update(Message::NetworkCatalogLoaded {
            generation: 1,
            chain: ChainId::Testnet4,
            cubes: Err("busy".into()),
        }));
        assert_eq!(home.network_catalog(ChainId::Testnet4).len(), 2);
        assert_eq!(project(&home).unread, [ChainId::Testnet4]);
        home.catalog.network_menu = true;
        let tree = home_tree("unread", &home, WIDE, "dark", theme::Theme::dark());
        assert!(tree.contains("Unavailable"));
        drop(home.update(Message::NetworkCatalogLoaded {
            generation: 1,
            chain: ChainId::Testnet4,
            cubes: Ok(Vec::new()),
        }));
        assert!(project(&home).unread.is_empty());
    }

    #[test]
    fn many_cubes_across_networks_still_stand_on_the_floor() {
        let mut home = all_networks();
        let many = |prefix: &str, n: usize, chain| -> Vec<CubeSettings> {
            (0..n)
                .map(|i| {
                    CubeSettings::new_with_raw_id(
                        format!("{prefix}{i}"),
                        format!("{prefix} {i}"),
                        chain,
                    )
                })
                .collect()
        };
        home.state = State::Cubes {
            cubes: many("b", 15, ChainId::Bitcoin),
            create_cube: false,
            source: ChainId::Bitcoin,
        };
        home.network_catalog = vec![(ChainId::Testnet4, many("t", 10, ChainId::Testnet4))];
        assert!(
            project(&home).floor_eligible(home.catalog.mode),
            "25 shown: still the floor, paged"
        );
        home.catalog.filter = NetworkFilter::Live;
        let projection = project(&home);
        assert_eq!(
            (projection.filtered_total, projection.local_total),
            (15, 25)
        );
        assert!(
            projection.floor_eligible(home.catalog.mode),
            "15 shown: floor"
        );
        assert_eq!(projection.floor_len(), 15);
    }

    #[test]
    fn a_connect_change_to_the_listed_networks_reads_the_new_one() {
        let mut home = all_networks();
        home.network_catalog_generation = 1;
        // Requested before Regtest was listed; a Connect result lists it.
        home.network_catalog_requested
            .retain(|chain| *chain != ChainId::Regtest);
        let messages = run_task(home.update(Message::View(ViewMessage::ConnectAccount(
            crate::app::view::ConnectAccountMessage::Init,
        ))));
        assert!(
            messages.iter().any(|m| matches!(
                m,
                Message::NetworkCatalogLoaded {
                    chain: ChainId::Regtest,
                    ..
                }
            )),
            "{:?}",
            messages
        );
    }

    #[test]
    fn a_connect_change_reads_the_new_network_before_any_catalog_read() {
        // A Bitcoin-only start reads no other network, so no generation is
        // ever claimed; a Connect grant must still read what it lists.
        let mut home = all_networks();
        home.network_catalog_generation = 0;
        home.network_catalog_requested
            .retain(|chain| *chain != ChainId::Regtest);
        let messages = run_task(home.update(Message::View(ViewMessage::ConnectAccount(
            crate::app::view::ConnectAccountMessage::Init,
        ))));
        assert!(
            messages.iter().any(|m| matches!(
                m,
                Message::NetworkCatalogLoaded {
                    chain: ChainId::Regtest,
                    ..
                }
            )),
            "{:?}",
            messages
        );
    }

    #[test]
    fn a_connect_change_off_the_list_is_read_before_the_list_returns() {
        // A creation backup returns to the list without another check.
        let mut home = all_networks();
        home.state = State::RecoveryInput;
        home.network_catalog_requested
            .retain(|chain| *chain != ChainId::Regtest);
        let messages = run_task(home.update(Message::View(ViewMessage::ConnectAccount(
            crate::app::view::ConnectAccountMessage::Init,
        ))));
        assert!(
            messages.iter().any(|m| matches!(
                m,
                Message::NetworkCatalogLoaded {
                    chain: ChainId::Regtest,
                    ..
                }
            )),
            "{:?}",
            messages
        );
    }

    #[test]
    fn a_filter_that_hides_every_local_cube_closes_the_key_card() {
        let mut home = all_networks();
        home.catalog.key.mode = KeyMode::Pinned;
        catalog(
            &mut home,
            CatalogMessage::Filter(NetworkFilter::Network(ChainId::Regtest)),
        );
        assert_eq!(project(&home).filtered_total, 0);
        assert!(!home.catalog.key_open(), "the card outlived its button");
    }

    #[test]
    fn leaving_the_launcher_drops_a_pending_open() {
        let mut home = all_networks();
        let lab = CubeCatalogKey {
            chain: ChainId::Testnet4,
            cube_id: "t1".into(),
        };
        drop(home.update(Message::View(ViewMessage::RunCatalog(lab))));
        assert!(home.catalog.pending_select.is_some());
        drop(home.update(Message::View(ViewMessage::GoToSection(
            HomeSection::RecoverVault,
        ))));
        assert_eq!(home.catalog.pending_select, None);
        // A direct section change, such as a Connect banner CTA, too.
        let lab = CubeCatalogKey {
            chain: ChainId::Testnet4,
            cube_id: "t1".into(),
        };
        home.active_section = HomeSection::Cubes;
        drop(home.update(Message::View(ViewMessage::RunCatalog(lab))));
        assert!(home.catalog.pending_select.is_some());
        home.active_section = HomeSection::Connect(crate::app::menu::ConnectSubMenu::Overview);
        drop(home.update(Message::View(ViewMessage::Catalog(
            CatalogMessage::NetworkMenu(false),
        ))));
        assert_eq!(home.catalog.pending_select, None);
    }

    #[test]
    fn an_imported_cube_opening_still_consumes_the_pending_pick() {
        let mut home = all_networks();
        let lab = CubeCatalogKey {
            chain: ChainId::Testnet4,
            cube_id: "t1".into(),
        };
        drop(home.update(Message::View(ViewMessage::RunCatalog(lab.clone()))));
        drop(home.update(Message::View(ViewMessage::SelectNetwork(ChainId::Testnet4))));
        assert!(home.catalog.pending_select.is_some());
        // An import finished during the probe opens its own Cube first.
        home.open_created_cube = Some("t2".into());
        drop(home.update(Message::Checked {
            for_chain: ChainId::Testnet4,
            res: Ok(State::Cubes {
                cubes: home.network_catalog(ChainId::Testnet4).to_vec(),
                create_cube: false,
                source: ChainId::Testnet4,
            }),
        }));
        assert_eq!(
            home.catalog.pending_select, None,
            "a stale Open would replay on the next checked list"
        );
    }

    #[test]
    fn the_pin_screen_snapshot_keeps_the_plan_button() {
        let home = all_networks();
        let chrome = super::super::launcher_chrome::snapshot(&home, &project(&home));
        assert_eq!(
            chrome.plan,
            (
                format!("{} plan", home.account_tier.display_name()),
                project(&home).at_limit()
            )
        );
    }

    #[test]
    fn a_superseded_catalog_read_is_dropped() {
        let mut home = all_networks();
        home.network_catalog_generation = 2;
        drop(home.update(Message::NetworkCatalogLoaded {
            generation: 1,
            chain: ChainId::Signet,
            cubes: Ok(Vec::new()),
        }));
        assert_eq!(home.network_catalog(ChainId::Signet).len(), 1);
        drop(home.update(Message::NetworkCatalogLoaded {
            generation: 2,
            chain: ChainId::Signet,
            cubes: Ok(Vec::new()),
        }));
        assert!(home.network_catalog(ChainId::Signet).is_empty());
    }

    #[test]
    fn chrome_controls_send_their_existing_destinations() {
        use crate::app::menu::ConnectSubMenu;
        use crate::app::state::connect::ConnectFlowStep;
        let overview = |message: &ViewMessage| {
            matches!(
                message,
                ViewMessage::GoToSection(HomeSection::Connect(ConnectSubMenu::Overview))
            )
        };
        for pointer in [Press::Enter, Press::Pointer, Press::SpaceHeld] {
            // A pending session: Checking only, no new sign-in path.
            let mut checking = rows(2);
            assert!(checking.connect_account.is_checking_session());
            let tree = home_tree(
                "r05b-chrome",
                &checking,
                (1440, 900),
                "dark",
                theme::Theme::dark(),
            );
            assert!(tree.contains("Checking…"));
            // The theme lives in App preferences, as in the prototype.
            assert!(!tree.contains("Light Mode"));
            assert!(!tree.has(&theme_id()));
            assert!(!tree.has(&sign_in_id()));
            assert!(!tree.has(&connect_id()));
            assert!(!tree.has(&recover_vault_id()));
            assert!(matches!(
                activate_chrome(&checking, settings_id(), pointer),
                ViewMessage::OpenAppPreferences
            ));
            assert!(matches!(
                activate_chrome(&checking, key_id(), pointer),
                ViewMessage::Catalog(CatalogMessage::KeyToggle)
            ));
            assert!(matches!(
                activate_chrome(&checking, create_id(), pointer),
                ViewMessage::ShowCreateCube(true)
            ));
            assert!(matches!(
                activate_chrome(&checking, import_id(), pointer),
                ViewMessage::ShowRestoreHub
            ));
            checking.theme_mode = coincube_ui::theme::palette::ThemeMode::Light;
            let tree = home_tree(
                "r05b-chrome",
                &checking,
                (1440, 900),
                "light",
                theme::Theme::light(),
            );
            assert!(!tree.contains("Dark Mode"));

            // Signed out: Sign In to the existing Overview destination.
            let mut signed_out = rows(2);
            signed_out.connect_account.step = ConnectFlowStep::Login {
                email: String::new(),
                loading: false,
            };
            // One id in every state (L6).
            assert!(overview(&activate_chrome(
                &signed_out,
                connect_id(),
                pointer
            )));

            // Signed in: Connect reaches the same Overview; the gated
            // utilities follow the sidebar's exact predicates.
            let mut signed_in = rows(2);
            crate::home::tests::enable_btcb2(&mut signed_in);
            assert!(signed_in.connect_account.is_authenticated());
            assert!(overview(&activate_chrome(
                &signed_in,
                connect_id(),
                pointer
            )));
            let tree = home_tree(
                "r05b-chrome",
                &signed_in,
                (1440, 900),
                "dark",
                theme::Theme::dark(),
            );
            let recover = feature_flags::RECOVER_VAULT_ENABLED;
            assert_eq!(tree.has(&recover_vault_id()), recover);
            if recover {
                assert!(matches!(
                    activate_chrome(&signed_in, recover_vault_id(), pointer),
                    ViewMessage::GoToSection(HomeSection::RecoverVault)
                ));
            }
            // "Split a Bitcoin wallet" is not in the Launcher design.
            assert!(signed_in
                .connect_chain_availability(ChainId::BitcoinBlake2b)
                .is_available());
            assert!(!tree.has(&split_wallet_id()));
            assert!(!tree.contains("Split a Bitcoin wallet"));
        }
        // Nor anywhere else.
        let mut testnet = rows(2);
        crate::home::tests::enable_btcb2(&mut testnet);
        testnet.network = ChainId::Testnet4;
        if let State::Cubes { source, cubes, .. } = &mut testnet.state {
            *source = ChainId::Testnet4;
            for cube in cubes.iter_mut() {
                cube.network = ChainId::Testnet4;
            }
        }
        let tree = home_tree(
            "r05b-chrome",
            &testnet,
            (1440, 900),
            "dark",
            theme::Theme::dark(),
        );
        assert!(!tree.has(&split_wallet_id()));
    }

    #[test]
    fn explicit_forms_and_unavailable_branches_render_no_catalog_controls() {
        let listed = rows(2);
        assert!(home_tree(
            "r05b-branch-list",
            &listed,
            (1024, 768),
            "dark",
            theme::Theme::dark()
        )
        .has_any_catalog_control());

        let mut first_run = home_with(&[]);
        first_run.state = State::NoCube { create_cube: true };
        let mut create = rows(2);
        if let State::Cubes { create_cube, .. } = &mut create.state {
            *create_cube = true;
        }
        let mut recovery = rows(2);
        recovery.state = State::RecoveryInput;
        let mut settings_section = rows(2);
        settings_section.active_section = HomeSection::Settings;
        // Bitcoin Blake2b without the beta opt-in renders the unavailable body.
        let mut unavailable = home_with(&[]);
        unavailable.network = ChainId::BitcoinBlake2b;
        unavailable.state = State::Cubes {
            cubes: vec![record("f", "Fork", ChainId::BitcoinBlake2b)],
            create_cube: false,
            source: ChainId::BitcoinBlake2b,
        };
        for (name, home) in [
            ("first-run", &first_run),
            ("create-form", &create),
            ("recovery", &recovery),
            ("settings", &settings_section),
            ("unavailable", &unavailable),
        ] {
            let tree = home_tree(
                "r05b-branch",
                home,
                (1024, 768),
                "dark",
                theme::Theme::dark(),
            );
            assert!(!tree.has_any_catalog_control(), "{}", name);
            assert!(!catalog_visible(home), "{}", name);
        }
    }

    /// The 20 synthetic Home-tree captures: 9 rows, 9 rows with details and
    /// 20 rows in both themes at both sizes, and compact no-match, remote-only
    /// restore, plan limit and long name with a sync refusal in both themes;
    /// plus one wheel-scrolled 20-row sample per theme showing the pinned
    /// dock. Images are written only with `R05B_CAPTURE_DIR`; the assertions
    /// run either way.
    #[test]
    fn launcher_list_capture_matrix() {
        for appearance in ["dark", "light"] {
            for window in WINDOWS {
                let (nine, theme) = themed(rows(9), appearance);
                let tree = home_tree("r05b-nine", &nine, window, appearance, theme);
                assert!(tree.contains("9 Cubes"));
                assert!(tree.contains("App preferences"));
                for index in 0..9 {
                    assert!(tree.has(&select_id(&key(&format!("cube-{}", index)))));
                }
                assert!(!tree.has(&close_details_id()));

                let (mut selected, theme) = themed(rows(9), appearance);
                catalog(&mut selected, CatalogMessage::Select(key("cube-4")));
                let tree = home_tree("r05b-nine-selected", &selected, window, appearance, theme);
                assert!(tree.has(&close_details_id()));
                assert!(tree.has(&details_open_id(&key("cube-4"))));
                assert!(tree.contains("On this device"));

                let (twenty, theme) = themed(rows(20), appearance);
                let tree = home_tree("r05b-twenty", &twenty, window, appearance, theme);
                for index in 0..20 {
                    assert!(tree.has(&select_id(&key(&format!("cube-{}", index)))));
                }
            }

            let compact = (1024, 768);
            let (mut no_match, theme) = themed(rows(9), appearance);
            catalog(&mut no_match, CatalogMessage::Query("zzz".to_string()));
            let tree = home_tree("r05b-no-match", &no_match, compact, appearance, theme);
            assert!(tree.contains("No Cubes match"));
            assert!(tree.contains("0 of 9 Cubes"));
            assert!(!tree.has(&select_id(&key("cube-0"))));

            let (mut remote_only, theme) = themed(home_with(&[]), appearance);
            remote_only.state = State::NoCube { create_cube: false };
            remote_only.server_cube_limit = Some(30);
            remote_only.remote_cubes = vec![remote("r1", "Remote", true, true, false, false)];
            let tree = home_tree("r05b-remote-only", &remote_only, compact, appearance, theme);
            assert!(tree.contains("Available in Connect"));
            assert!(tree.has(&restore_id("r1")));

            let (mut limited, theme) = themed(rows(2), appearance);
            limited.server_cube_limit = Some(2);
            limited.catalog.plan_menu = true;
            let tree = home_tree("r05b-plan-limit", &limited, compact, appearance, theme);
            assert!(tree.contains("2 of 2"));
            assert!(tree.contains("Full"));
            assert!(!tree.has(&create_id()));

            let (mut refused, theme) = themed(rows(3), appearance);
            cubes_mut(&mut refused)[1].name =
                "A very long Cube name that keeps going to test wrapping behaviour on a compact window"
                    .to_string();
            refused.connect_account.user = Some(crate::services::coincube::User {
                id: 7,
                email: "owner@example.test".to_string(),
                email_verified: Some(true),
            });
            refused.cube_sync_errors.insert(
                "cube-1".to_string(),
                "Cube limit reached for this network".to_string(),
            );
            let tree = home_tree(
                "r05b-long-name-refusal",
                &refused,
                compact,
                appearance,
                theme,
            );
            assert!(tree.contains("Out of Cube slots"));
            let refused_row = tree.bounds(&select_id(&key("cube-1")));
            let warning = tree
                .text
                .iter()
                .filter(|(value, bounds)| {
                    value.starts_with("Out of Cube slots")
                        && bounds.y >= refused_row.y
                        && bounds.y < refused_row.y + refused_row.height
                })
                .map(|(_, bounds)| *bounds)
                .next()
                .expect("refusal visible in its Cube row");
            let (viewport, _, _) = tree.scroll(&Id::from(CATALOG_SCROLL_ID));
            assert!(warning.x + warning.width <= refused_row.x + refused_row.width + 1.0);
            assert!(warning.y + warning.height <= viewport.y + viewport.height + 1.0);
            let long = tree
                .text
                .iter()
                .find(|(value, _)| value.starts_with("A very long Cube name"))
                .map(|(_, bounds)| *bounds)
                .expect("long name rendered");
            assert!(long.x + long.width <= 1024.0 + 1.0, "{:?}", long);

            let (twenty, theme) = themed(rows(20), appearance);
            let tree = home_tree_scrolled(
                "r05b-twenty-scrolled",
                &twenty,
                compact,
                appearance,
                theme,
                Some(-200.0),
            );
            let (body, _, offset) = tree.scroll(&Id::from(CATALOG_SCROLL_ID));
            assert!(offset.y > 0.0, "{:?}", offset);
            let create = tree.bounds(&create_id());
            assert!(create.y >= body.y + body.height, "{:?} {:?}", create, body);
            assert_eq!(tree.emitted, 0);
        }
    }

    #[test]
    fn mounted_cube_avatar_stays_in_its_32px_slot_at_compact_and_wide_sizes() {
        for window in [COMPACT, WIDE] {
            let home = home_with(&[("a", "Alpha")]);
            let tree = home_tree(
                "r05c-avatar-geometry",
                &home,
                window,
                "light",
                theme::Theme::light(),
            );
            let row = tree.bounds(&select_id(&key("a")));
            let glyph = tree
                .text
                .iter()
                .filter(|(value, bounds)| {
                    value == "\u{f1c8}" && bounds.y >= row.y && bounds.y < row.y + row.height
                })
                .map(|(_, bounds)| *bounds)
                .next()
                .expect("Cube avatar glyph inside mounted List row");
            assert!(glyph.x >= row.x + 12.0 - 1.0, "{:?}: {:?}", window, glyph);
            assert!(
                glyph.x + glyph.width <= row.x + 12.0 + 32.0 + 1.0,
                "{:?}: {:?} {:?}",
                window,
                glyph,
                row
            );
            assert!(glyph.width <= 32.0 + 1.0 && glyph.height <= 32.0 + 1.0);
        }
    }

    fn apply_widget_task(task: Task<Message>, mounted: &mut Mounted<'_>) {
        use iced::advanced::widget::operation::{self, Outcome};
        use iced_runtime::futures::futures::StreamExt;

        let stream = iced_runtime::task::into_stream(task).expect("focus task has an action");
        let actions =
            iced_runtime::futures::futures::executor::block_on(stream.collect::<Vec<_>>());
        let mut applied = false;
        for action in actions {
            if let iced_runtime::Action::Widget(mut operation) = action {
                applied = true;
                loop {
                    mounted.element.as_widget_mut().operate(
                        &mut mounted.tree,
                        Layout::new(&mounted.node),
                        &mounted.renderer,
                        &mut operation::black_box(operation.as_mut()),
                    );
                    match operation.finish() {
                        Outcome::Chain(next) => operation = next,
                        Outcome::None | Outcome::Some(()) => break,
                    }
                }
            }
        }
        assert!(applied, "selection must return a mounted focus operation");
    }

    #[test]
    fn responsive_list_fallback_never_claims_a_floor_unlock_turn() {
        let selected = key("cube-0");
        let mut narrow = rows(4);
        narrow.catalog.mode = LauncherMode::Cubes;
        let narrow_select = capture::activate(
            super::super::launcher_presenter::floor_body(&narrow),
            select_id(&selected),
            (800, 900),
            false,
        );
        assert!(matches!(
            narrow_select.as_slice(),
            [ViewMessage::Catalog(CatalogMessage::Select(key))] if key == &selected
        ));
        catalog(&mut narrow, CatalogMessage::Select(selected.clone()));
        assert!(narrow.catalog.motion.is_none());
        let narrow_open = capture::activate(
            super::super::launcher_presenter::floor_body(&narrow),
            details_open_id(&selected),
            (800, 900),
            false,
        );
        assert!(matches!(
            narrow_open.as_slice(),
            [ViewMessage::RunCatalog(key)] if key == &selected
        ));
        drop(narrow.update(Message::View(narrow_open.into_iter().next().unwrap())));
        let cube = match &narrow.state {
            State::Cubes { cubes, .. } => cubes[0].clone(),
            _ => unreachable!(),
        };
        assert!(!narrow.take_launcher_details_face(&cube));

        let mut wide = rows(4);
        wide.catalog.mode = LauncherMode::Cubes;
        let wide_select = capture::activate(
            super::super::launcher_presenter::floor_body(&wide),
            select_id(&selected),
            (1440, 900),
            false,
        );
        assert!(matches!(
            wide_select.as_slice(),
            [ViewMessage::Catalog(CatalogMessage::SelectFloor(key))] if key == &selected
        ));
        catalog(&mut wide, CatalogMessage::SelectFloor(selected.clone()));
        assert!(wide.catalog.motion.is_some());
        catalog(
            &mut wide,
            CatalogMessage::MotionTick(Instant::now() + Duration::from_secs(2)),
        );
        let wide_open = capture::activate(
            super::super::launcher_presenter::floor_body(&wide),
            details_open_id(&selected),
            (1440, 900),
            false,
        );
        assert!(matches!(
            wide_open.as_slice(),
            [ViewMessage::RunFloorCatalog(key)] if key == &selected
        ));
        drop(wide.update(Message::View(wide_open.into_iter().next().unwrap())));
        assert!(wide.catalog.open_from_floor);

        // Resizing this selected floor into the List changes the emitted Open
        // origin, even though the selection itself survives the resize.
        let resized_open = capture::activate(
            super::super::launcher_presenter::floor_body(&wide),
            details_open_id(&selected),
            (800, 900),
            false,
        );
        assert!(matches!(
            resized_open.as_slice(),
            [ViewMessage::RunCatalog(key)] if key == &selected
        ));
        drop(wide.update(Message::View(resized_open.into_iter().next().unwrap())));
        let cube = match &wide.state {
            State::Cubes { cubes, .. } => cubes[0].clone(),
            _ => unreachable!(),
        };
        assert!(!wide.take_launcher_details_face(&cube));

        // The real floor action remains eligible and the handoff consumes it
        // once, so a later unrelated Run cannot inherit it.
        drop(wide.update(Message::View(ViewMessage::RunFloorCatalog(
            selected.clone(),
        ))));
        assert!(wide.take_launcher_details_face(&cube));
        assert!(!wide.take_launcher_details_face(&cube));

        let mut reduced = rows(4);
        reduced.catalog.mode = LauncherMode::List;
        catalog(&mut reduced, CatalogMessage::Select(selected));
        assert!(reduced.catalog.motion.is_none());
    }

    #[test]
    fn unlock_floor_snapshot_requires_the_current_selected_filtered_page() {
        let mut home = rows(8);
        home.catalog.mode = LauncherMode::Cubes;
        let selected = key("cube-5");
        catalog(&mut home, CatalogMessage::SelectFloor(selected.clone()));
        let cube = match &home.state {
            State::Cubes { cubes, .. } => cubes[5].clone(),
            _ => unreachable!(),
        };
        let scene = home
            .launcher_scene_context(&cube)
            .expect("visible floor selection");
        assert_eq!(scene.selected, selected);
        assert_eq!(scene.page, 1);
        assert_eq!(scene.selected_slot, 1);
        assert_eq!(scene.peers.len(), 4);
        assert_eq!(scene.peers[scene.selected_slot].name, "Cube 5");
        // Peers carry only persisted face marks, never a rendered name.
        assert!(scene
            .peers
            .iter()
            .all(|peer| peer.marks.network == ChainId::Bitcoin));
        assert!(home.floor_open_matches_current_cube(&cube));

        // A queued Run still names the same ID after a reload, but its
        // selected face and unlock metadata no longer describe that record.
        if let State::Cubes { cubes, .. } = &mut home.state {
            cubes[5].name = "Renamed while opening".into();
        }
        assert!(!home.floor_open_matches_current_cube(&cube));
        assert!(home.launcher_scene_context(&cube).is_none());
        if let State::Cubes { cubes, .. } = &mut home.state {
            cubes[5].name = cube.name.clone();
            cubes[5].backed_up = true;
        }
        assert!(!home.floor_open_matches_current_cube(&cube));
        assert!(home.launcher_scene_context(&cube).is_none());
        if let State::Cubes { cubes, .. } = &mut home.state {
            cubes[5].backed_up = cube.backed_up;
        }

        home.catalog.page = 0;
        assert!(home.launcher_scene_context(&cube).is_none());
        home.catalog.page = 1;
        home.catalog.query = "other".into();
        assert!(home.launcher_scene_context(&cube).is_none());
        home.catalog.query.clear();
        home.catalog.selected = Some(key("cube-6"));
        assert!(home.launcher_scene_context(&cube).is_none());
        home.catalog.selected = Some(selected);
        home.catalog.mode = LauncherMode::List;
        assert!(home.launcher_scene_context(&cube).is_none());
    }

    #[test]
    fn delayed_floor_run_refuses_same_id_with_changed_metadata() {
        let mut home = rows(2);
        home.catalog.mode = LauncherMode::Cubes;
        catalog(&mut home, CatalogMessage::SelectFloor(key("cube-0")));
        home.catalog.open_from_floor = true;
        home.catalog.floor_open_generation = 1;
        let queued = match &home.state {
            State::Cubes { cubes, .. } => cubes[0].clone(),
            _ => unreachable!(),
        };
        if let State::Cubes { cubes, .. } = &mut home.state {
            cubes[0].name = "Changed after click".into();
        }
        let datadir = home.datadir_path.clone();
        let mut tab = crate::gui::tab::Tab::new(0, crate::gui::tab::State::Home(home));
        let queued_run = Message::RunFromFloor(
            datadir,
            crate::app::config::Config::new(false),
            ChainId::Bitcoin,
            queued,
            1,
        );
        assert!(tab.validate_home_exit(&queued_run).is_err());
        drop(tab.update(crate::gui::tab::Message::Launch(queued_run)));
        let crate::gui::tab::State::Home(home) = &tab.state else {
            panic!("stale Cube must not enter unlock");
        };
        assert!(home
            .error()
            .is_some_and(|error| error.contains("changed while opening")));
    }

    #[test]
    fn delayed_floor_run_refuses_intervening_selection_query_and_mode() {
        for intervention in ["selection", "query", "mode", "second open"] {
            let mut home = rows(2);
            home.catalog.mode = LauncherMode::Cubes;
            catalog(&mut home, CatalogMessage::SelectFloor(key("cube-0")));
            home.catalog.open_from_floor = true;
            home.catalog.floor_open_generation = 1;
            let queued = match &home.state {
                State::Cubes { cubes, .. } => cubes[0].clone(),
                _ => unreachable!(),
            };
            match intervention {
                "selection" => {
                    catalog(&mut home, CatalogMessage::SelectFloor(key("cube-1")));
                    catalog(&mut home, CatalogMessage::SelectFloor(key("cube-0")));
                }
                "query" => {
                    catalog(&mut home, CatalogMessage::Query("Cube".into()));
                    catalog(&mut home, CatalogMessage::ClearQuery);
                    catalog(&mut home, CatalogMessage::SelectFloor(key("cube-0")));
                }
                "mode" => {
                    home.catalog.mode = LauncherMode::List;
                    home.reconcile_catalog();
                }
                "second open" => {
                    home.catalog.floor_open_generation = 2;
                }
                _ => unreachable!(),
            }
            let queued_run = Message::RunFromFloor(
                home.datadir_path.clone(),
                crate::app::config::Config::new(false),
                ChainId::Bitcoin,
                queued,
                1,
            );
            let mut tab = crate::gui::tab::Tab::new(0, crate::gui::tab::State::Home(home));
            assert!(
                tab.validate_home_exit(&queued_run).is_err(),
                "{} must be refused before GUI retires the public owner",
                intervention
            );
            drop(tab.update(crate::gui::tab::Message::Launch(queued_run)));
            assert!(
                matches!(tab.state, crate::gui::tab::State::Home(_)),
                "{}",
                intervention
            );
        }
    }

    #[test]
    fn floor_selection_focus_moves_to_switcher_then_mounted_face_action() {
        let mut home = rows(4);
        home.catalog.mode = LauncherMode::Cubes;
        let selected = key("cube-0");
        let task = home.update(Message::View(ViewMessage::Catalog(
            CatalogMessage::SelectFloor(selected.clone()),
        )));
        let mut turning = mount_window(
            super::super::launcher_presenter::floor_body(&home),
            1024.0,
            900.0,
        );
        apply_widget_task(task, &mut turning);
        assert_eq!(turning.focused(), Some(select_id(&selected)));
        drop(turning);

        let task = home.update(Message::View(ViewMessage::Catalog(
            CatalogMessage::MotionTick(Instant::now() + Duration::from_secs(2)),
        )));
        let mut resting = mount_window(
            super::super::launcher_presenter::floor_body(&home),
            1024.0,
            900.0,
        );
        apply_widget_task(task, &mut resting);
        assert_eq!(resting.focused(), Some(details_open_id(&selected)));
    }

    #[test]
    fn back_spins_the_cube_home_before_peers_settle() {
        let mut home = home_with(&[("a", "Alpha"), ("b", "Beta")]);
        home.catalog.mode = LauncherMode::Cubes;
        catalog(&mut home, CatalogMessage::SelectFloor(key("a")));
        let settled = std::time::Instant::now() + std::time::Duration::from_secs(5);
        catalog(&mut home, CatalogMessage::MotionTick(settled));
        assert!(home.catalog.motion.is_none());

        // A real pointer click on the face's Back link.
        let back = bounds_of(
            &focus_bounds_window(launcher_view(&home), 1440.0, 900.0),
            &close_details_id(),
        )
        .center();
        let cursor = iced::mouse::Cursor::Available(back);
        let messages = {
            let mut mounted = mount_window(launcher_view(&home), 1440.0, 900.0);
            [
                iced::mouse::Event::CursorMoved { position: back },
                iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left),
                iced::mouse::Event::ButtonReleased(iced::mouse::Button::Left),
            ]
            .iter()
            .flat_map(|event| mounted.send(iced::Event::Mouse(*event), cursor))
            .collect::<Vec<_>>()
        };
        assert!(messages.iter().any(|message| matches!(
            message,
            ViewMessage::Catalog(CatalogMessage::ClearSelection)
        )));
        for message in messages {
            let _ = home.update(Message::View(message));
        }

        assert_eq!(home.catalog.selected, None);
        let motion = home
            .catalog
            .motion
            .as_ref()
            .expect("Back keeps the return spin running after the selection clears");
        assert_eq!(motion.returning_key(), Some(&key("a")));
        let midway = motion.pose(
            &key("a"),
            None,
            std::time::Instant::now() + super::super::launcher_scene::TURN / 2,
        );
        assert!(midway.spin > 0.0 && midway.spin < 1.0, "{:?}", midway);

        catalog(
            &mut home,
            CatalogMessage::MotionTick(settled + std::time::Duration::from_secs(5)),
        );
        assert!(home.catalog.motion.is_none());
    }

    #[test]
    fn a_press_beside_an_open_face_closes_it_and_a_press_on_it_does_not() {
        let mut home = home_with(&[("a", "Alpha"), ("b", "Beta")]);
        home.catalog.mode = LauncherMode::Cubes;
        catalog(&mut home, CatalogMessage::SelectFloor(key("a")));
        let settled = std::time::Instant::now() + std::time::Duration::from_secs(5);
        catalog(&mut home, CatalogMessage::MotionTick(settled));
        assert!(home.catalog.motion.is_none());

        let open = bounds_of(
            &focus_bounds_window(launcher_view(&home), 1440.0, 900.0),
            &details_open_id(&key("a")),
        );
        let press = |point: iced::Point| {
            let cursor = iced::mouse::Cursor::Available(point);
            let mut mounted = mount_window(launcher_view(&home), 1440.0, 900.0);
            [
                iced::mouse::Event::CursorMoved { position: point },
                iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left),
                iced::mouse::Event::ButtonReleased(iced::mouse::Button::Left),
            ]
            .iter()
            .flat_map(|event| mounted.send(iced::Event::Mouse(*event), cursor))
            .any(|message| {
                matches!(
                    message,
                    ViewMessage::Catalog(CatalogMessage::ClearSelection)
                )
            })
        };
        // The face's facts, above its Open button, are not a way out.
        assert!(!press(iced::Point::new(open.x + 4.0, open.y - 80.0)));
        // The stage beside the face, above the peers' labels, is.
        assert!(press(iced::Point::new(open.x - 80.0, open.y - 240.0)));
    }

    /// The slot the open Cube left is part of the stage. A press there closes
    /// the face like a press beside it; a peer's slot still selects the peer.
    /// The slot stays live while its Cube is turned (`Pose::DETAILS` has
    /// spin), and it used to send `SelectFloor` for the Cube already open,
    /// which changes nothing.
    #[test]
    fn a_press_on_the_slot_the_open_cube_left_closes_its_face() {
        let mut home = home_with(&[("a", "Alpha"), ("b", "Beta"), ("c", "Gamma")]);
        home.catalog.mode = LauncherMode::Cubes;
        catalog(&mut home, CatalogMessage::SelectFloor(key("a")));
        let settled = std::time::Instant::now() + std::time::Duration::from_secs(5);
        catalog(&mut home, CatalogMessage::MotionTick(settled));
        assert!(home.catalog.motion.is_none());

        let bounds = focus_bounds_window(launcher_view(&home), 1440.0, 900.0);
        let press = |point: iced::Point| {
            let cursor = iced::mouse::Cursor::Available(point);
            let mut mounted = mount_window(launcher_view(&home), 1440.0, 900.0);
            [
                iced::mouse::Event::CursorMoved { position: point },
                iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left),
                iced::mouse::Event::ButtonReleased(iced::mouse::Button::Left),
            ]
            .iter()
            .flat_map(|event| mounted.send(iced::Event::Mouse(*event), cursor))
            .collect::<Vec<_>>()
        };
        // Near each slot's outer edge, clear of the centred face, which
        // covers the inner part of its neighbours.
        let left = bounds_of(&bounds, &select_id(&key("a")));
        let messages = press(iced::Point::new(left.x + 12.0, left.center().y));
        assert!(
            messages.iter().any(|message| matches!(
                message,
                ViewMessage::Catalog(CatalogMessage::ClearSelection)
            )),
            "{:?}",
            messages
        );
        let right = bounds_of(&bounds, &select_id(&key("c")));
        let peer = press(iced::Point::new(
            right.x + right.width - 12.0,
            right.center().y,
        ));
        assert!(peer.iter().any(|message| matches!(
            message,
            ViewMessage::Catalog(CatalogMessage::SelectFloor(k)) if *k == key("c")
        )));
        assert!(!peer.iter().any(|message| matches!(
            message,
            ViewMessage::Catalog(CatalogMessage::ClearSelection)
        )));
    }

    /// Presses `target` in the Create journey with the keyboard, as a user
    /// does: focus, then Enter. A disabled control is not focusable, so it
    /// yields nothing.
    fn press_in_journey(home: &Home, target: &'static str) -> Vec<ViewMessage> {
        use iced::advanced::widget::operation::focusable;
        let target = Id::new(target);
        let mut mounted = mount_window(home.create_journey_frame(), 1440.0, 900.0);
        mounted.operate(focusable::focus::<()>(target.clone()));
        if mounted.focused() != Some(target) {
            return Vec::new();
        }
        let none = iced::mouse::Cursor::Unavailable;
        let mut messages = mounted.send(enter(true), none);
        messages.extend(mounted.send(enter(false), none));
        messages
            .into_iter()
            .filter(|message| !matches!(message, ViewMessage::Catalog(CatalogMessage::Focus(_))))
            .collect()
    }

    #[test]
    fn empty_startup_respects_launcher_mode_in_both_themes() {
        use super::super::launcher_presenter::empty_create_id;
        for (appearance, theme) in [
            ("dark", theme::Theme::dark()),
            ("light", theme::Theme::light()),
        ] {
            for mode in [LauncherMode::Cubes, LauncherMode::List] {
                let mut home = home_with(&[]);
                home.state = State::NoCube { create_cube: false };
                home.catalog.mode = mode;
                home.theme_mode = if appearance == "light" {
                    coincube_ui::theme::palette::ThemeMode::Light
                } else {
                    coincube_ui::theme::palette::ThemeMode::Dark
                };
                let projection = project(&home);
                assert_eq!((projection.local_total, projection.remote_total), (0, 0));
                assert_eq!(projection.floor_eligible(mode), mode == LauncherMode::Cubes);
                for window in [COMPACT, WIDE] {
                    let tree = home_tree("empty-startup", &home, window, appearance, theme);
                    assert!(tree.has(&create_id()));
                    assert!(tree.has(&import_id()));
                    assert_eq!(tree.has(&empty_create_id()), mode == LauncherMode::Cubes);
                    assert_eq!(tree.has(&search_id()), mode == LauncherMode::List);
                    assert_eq!(
                        tree.contains("No Cubes on this network."),
                        mode == LauncherMode::List
                    );
                    assert!(!tree.contains(super::super::launcher_presenter::GHOST_INTRO));
                    assert!(!tree.contains("Back to Cube list"));
                }
            }
        }
    }

    #[test]
    fn empty_placeholder_opens_creation_by_pointer_enter_and_space() {
        use super::super::launcher_presenter::empty_create_id;
        for press in [Press::Pointer, Press::Enter, Press::SpaceHeld] {
            let mut home = home_with(&[]);
            home.state = State::NoCube { create_cube: false };
            home.catalog.mode = LauncherMode::Cubes;
            let message = activate_chrome(&home, empty_create_id(), press);
            assert!(matches!(message, ViewMessage::ShowCreateCube(true)));
            drop(home.update(Message::View(message)));
            assert!(home.showing_create_journey());
            drop(home.update(Message::View(ViewMessage::ShowCreateCube(false))));
            assert!(project(&home).floor_eligible(home.catalog.mode));
            assert!(project(&home).locals.is_empty());
        }
        let mut home = home_with(&[]);
        home.catalog.mode = LauncherMode::Cubes;
        home.server_cube_limit = Some(0);
        let tree = home_tree("empty-limit", &home, WIDE, "dark", theme::Theme::dark());
        assert!(!tree.has(&empty_create_id()));
        assert!(!tree.has(&create_id()));
        assert!(tree.contains("Cube limit reached"));
    }

    /// No local Cubes, and the filter hides every Connect-only Cube: the
    /// floor shows the filter empty state, not the create placeholder.
    #[test]
    fn filtered_out_ghosts_keep_the_filter_empty_state_on_the_floor() {
        use super::super::launcher_presenter::empty_create_id;
        let mut home = all_networks();
        home.network_catalog.clear();
        if let State::Cubes { cubes, .. } = &mut home.state {
            cubes.clear();
        }
        home.remote_cubes = vec![
            remote("r-1", "Cloud copy", true, true, false, false),
            remote("r-2", "Phone copy", false, false, true, true),
        ];
        home.remote_cubes[1].network = settings::network_to_api_string(ChainId::Testnet4);
        home.catalog.filter = NetworkFilter::Network(ChainId::Regtest);
        let projection = project(&home);
        assert_eq!(projection.floor_len(), 0);
        assert_eq!((projection.hidden, projection.hidden_ghosts), (0, 2));
        let tree = home_tree("filtered-ghosts", &home, WIDE, "dark", theme::Theme::dark());
        assert!(tree.contains("No Cubes match these filters"));
        assert!(tree.contains("2 Cubes hidden."));
        assert!(tree.has(&clear_filters_empty_id()));
        assert!(!tree.has(&empty_create_id()));
        // Clearing the filter brings the ghosts back to the floor.
        home.catalog.filter = NetworkFilter::All;
        let projection = project(&home);
        assert_eq!((projection.floor_len(), projection.hidden_ghosts), (2, 0));
    }

    /// The Create Cube placeholder floats under the pointer as a resting Cube
    /// does, behind its static plus, and settles once the pointer leaves.
    #[test]
    fn create_placeholder_floats_under_the_pointer_and_settles() {
        use super::super::launcher_presenter::empty_create_id;
        use super::super::launcher_scene::{BOB_HEIGHT, BOB_PERIOD, BOB_SETTLE};
        let mut home = home_with(&[]);
        home.state = State::NoCube { create_cube: false };
        home.catalog.mode = LauncherMode::Cubes;
        let placeholder = create_placeholder_key();
        let slot = bounds_of(
            &focus_bounds_window(launcher_view(&home), 1440.0, 900.0),
            &empty_create_id(),
        );
        let away = iced::Point::new(slot.center_x(), slot.y - 40.0);
        pointer_path(&mut home, &[away, slot.center()]);
        assert!(
            home.catalog.animating(),
            "the placeholder floats under the pointer"
        );
        let entered = home.catalog.hover[0].clone();
        assert_eq!(entered.key, placeholder);

        // Frames keep it floating; at the top of the bob it is fully lifted.
        let top = Instant::now() + BOB_PERIOD / 2;
        catalog(&mut home, CatalogMessage::MotionTick(top));
        assert!(home.catalog.animating());
        let lift = home.catalog.lift(&placeholder, top);
        assert!(lift > 0.5 * BOB_HEIGHT, "lift {}", lift);
        // Leaving settles it, and a settled float stops the frames.
        pointer_path(&mut home, &[away]);
        catalog(
            &mut home,
            CatalogMessage::MotionTick(Instant::now() + BOB_SETTLE),
        );
        assert!(
            !home.catalog.animating(),
            "a settled placeholder stops the frames"
        );

        // At the Cube limit nothing opens, so nothing floats.
        home.server_cube_limit = Some(0);
        pointer_path(&mut home, &[away, slot.center()]);
        assert!(home.catalog.hover.is_empty());
    }

    /// Close returns to the Launcher even when the device has no Cubes.
    #[test]
    fn close_leaves_import_or_restore_on_a_device_with_no_cubes() {
        let mut home = home_with(&[]);
        home.state = State::NoCube { create_cube: true };
        let _ = home.update(Message::View(ViewMessage::ShowImportBitcoin));
        assert!(home.showing_import_or_restore());
        let pressed = press_in_journey(&home, super::super::RESTORE_CLOSE_ID);
        assert!(
            matches!(pressed.as_slice(), [ViewMessage::ShowCreateCube(false)]),
            "{:?}",
            pressed
        );
        for message in pressed {
            let _ = home.update(Message::View(message));
        }
        assert!(!home.showing_import_or_restore());
        assert!(home.pending_bitcoin_import.is_none());
        assert!(catalog_visible(&home), "the empty Launcher");

        // From the Overview too.
        let _ = home.update(Message::View(ViewMessage::ShowRestoreHub));
        let pressed = press_in_journey(&home, super::super::RESTORE_CLOSE_ID);
        assert!(matches!(
            pressed.as_slice(),
            [ViewMessage::ShowCreateCube(false)]
        ));
    }

    /// With no free Bitcoin slot, no Import entry can be pressed and the
    /// handler refuses a stale press before any name or PIN is typed.
    #[test]
    fn every_import_entry_closes_when_bitcoin_is_full() {
        let mut home = home_with(&[("a", "Alpha"), ("b", "Beta")]);
        home.server_cube_limit = Some(2);
        assert!(project(&home).at_limit());
        // The dock.
        let tree = home_tree(
            "import-full-dock",
            &home,
            WIDE,
            "dark",
            theme::Theme::dark(),
        );
        assert!(!tree.has(&import_id()));
        // The handler, and so the Create form's own Import button, which
        // takes the same predicate.
        let _ = home.update(Message::View(ViewMessage::ShowImportBitcoin));
        assert!(home.pending_bitcoin_import.is_none());
        assert!(!home.can_start_bitcoin_import());
        // The Overview's Start and its path list.
        let _ = home.update(Message::View(ViewMessage::ShowRestoreHub));
        assert!(home.restore_hub);
        assert!(press_in_journey(&home, super::super::RESTORE_START_IMPORT_ID).is_empty());
        assert!(press_in_journey(&home, super::super::RESTORE_IMPORT_ID).is_empty());

        // A free slot opens them again.
        home.server_cube_limit = Some(3);
        assert!(matches!(
            press_in_journey(&home, super::super::RESTORE_START_IMPORT_ID).as_slice(),
            [ViewMessage::ShowImportBitcoin]
        ));
    }

    /// The design's Recovery Kit and Recover with phone paths: the Kit
    /// path opens the installer's sign-in and Cube picker; the phone path
    /// lists only Cubes a phone can recover.
    #[test]
    fn import_or_restore_offers_the_kit_and_phone_paths() {
        let mut home = home_with(&[("a", "Alpha")]);
        crate::home::tests::enable_btcb2(&mut home);
        home.remote_cubes = vec![
            remote("kit", "Kit only", true, true, false, false),
            remote("phone", "Phone only", false, false, true, true),
            remote("vault", "Vault only", false, false, true, false),
        ];
        let _ = home.update(Message::View(ViewMessage::ShowRestoreHub));
        for target in [
            super::super::RESTORE_START_KIT_ID,
            super::super::RESTORE_KIT_ID,
        ] {
            let pressed = press_in_journey(&home, target);
            assert!(
                matches!(pressed.as_slice(), [ViewMessage::RestoreFromAnyRecoveryKit]),
                "{} {:?}",
                target,
                pressed
            );
        }
        if !feature_flags::OWNER_KEYCHAIN_RECOVERY_ENABLED {
            return;
        }
        for target in [
            super::super::RESTORE_START_PHONE_ID,
            super::super::RESTORE_PHONE_ID,
        ] {
            let pressed = press_in_journey(&home, target);
            assert!(
                matches!(pressed.as_slice(), [ViewMessage::ShowRestorePhone]),
                "{} {:?}",
                target,
                pressed
            );
        }
        let _ = home.update(Message::View(ViewMessage::ShowRestorePhone));
        assert!(home.restore_hub && home.restore_phone);
        let pressed = press_in_journey(&home, "tenshu.restore:phone:phone");
        assert!(
            matches!(
                pressed.as_slice(),
                [ViewMessage::RestoreWithPhoneOn {
                    chain: ChainId::Bitcoin,
                    cube_id: 7,
                    full_cube: true
                }]
            ),
            "{:?}",
            pressed
        );
        // A Kit-only Cube, and a phone backup that restores only a Vault,
        // are not on the phone path.
        assert!(press_in_journey(&home, "tenshu.restore:phone:kit").is_empty());
        assert!(press_in_journey(&home, "tenshu.restore:phone:vault").is_empty());
        // The Overview and Close leave it.
        let _ = home.update(Message::View(ViewMessage::ShowRestoreHub));
        assert!(home.restore_hub && !home.restore_phone);
        let _ = home.update(Message::View(ViewMessage::ShowRestorePhone));
        let _ = home.update(Message::View(ViewMessage::ShowCreateCube(false)));
        assert!(!home.restore_hub && !home.restore_phone);
    }

    /// Changing the network keeps Import or restore open and re-filters
    /// it; only a pending Bitcoin import is dropped.
    #[test]
    fn leaving_bitcoin_keeps_import_or_restore_open() {
        let mut home = all_networks();
        let _ = home.update(Message::View(ViewMessage::ShowRestoreHub));
        assert!(home.restore_hub);
        let _ = home.update(Message::View(ViewMessage::SelectNetwork(ChainId::Testnet4)));
        assert!(home.restore_hub);
        assert!(home.showing_import_or_restore());
        assert!(home.pending_bitcoin_import.is_none());
    }

    /// The plan menu, the activity menu and the Key card: one at a time.
    #[test]
    fn dock_popovers_open_one_at_a_time() {
        use super::super::launcher_activity::ActivityMessage;
        let mut home = home_with(&[("a", "Alpha")]);
        catalog(&mut home, CatalogMessage::PlanMenu(true));
        catalog(&mut home, CatalogMessage::KeyToggle);
        assert!(home.catalog.key_open());
        assert!(!home.catalog.plan_menu);
        catalog(&mut home, CatalogMessage::PlanMenu(true));
        assert!(!home.catalog.key_open());
        catalog(&mut home, CatalogMessage::KeyToggle);
        catalog(
            &mut home,
            CatalogMessage::Activity(ActivityMessage::Menu(true)),
        );
        assert!(!home.catalog.key_open());
        assert!(!home.catalog.plan_menu);
        // A hover peek waits while a menu is up.
        catalog(&mut home, CatalogMessage::KeyHover(KeyPart::Button, true));
        assert!(super::super::launcher_chrome::key_layer(&home, &project(&home)).is_none());
    }

    /// The plan menu marks the network Create and Import act on, now that
    /// the dock no longer captions it.
    #[test]
    fn the_plan_menu_marks_the_network_new_cubes_go_to() {
        let mut home = all_networks();
        home.network = ChainId::Testnet4;
        let projection = project(&home);
        let rows = super::super::launcher_chrome::plan_rows(&projection);
        let current: Vec<_> = rows.iter().filter(|row| row.current).collect();
        assert_eq!(current.len(), 1);
        assert_eq!(current[0].network, "Bitcoin Testnet4");
        home.catalog.plan_menu = true;
        let tree = home_tree("plan-current", &home, WIDE, "dark", theme::Theme::dark());
        assert!(tree.contains("New Cubes"));
    }
}
