//! Generic pan/zoom graph canvas. It takes no gui types: children are placed
//! at graph coordinates and shown through a camera.
use std::{
    any::Any,
    cell::Cell,
    collections::{HashMap, HashSet},
    hash::{DefaultHasher, Hash, Hasher},
};

use iced::{
    advanced::{
        graphics::geometry::Renderer as _,
        layout::{self, Layout},
        overlay as iced_overlay, renderer,
        widget::{tree, Id, Operation, Tree, Widget},
        Clipboard, Renderer as _, Shell,
    },
    animation::Easing,
    border, keyboard, mouse,
    time::Instant,
    widget::canvas,
    window, Animation, Event, Length, Point, Rectangle, Size, Transformation, Vector,
};
use iced_runtime::{task, Action, Task};

use crate::{
    theme::{
        card::{CARD_RADIUS, CARD_SHADOW_HOVER},
        Theme,
    },
    widget::{Element, Renderer},
};

mod draw;
pub mod geometry;
mod overlay;

use geometry::{
    accumulate_wheel, anchor_point, area_pick, distance_to_curve, drag_set, edge_curve, frame_rect,
    hit_item, interpolate_camera, rect_from_points, reorder_target, reordered_row, snap_to_grid,
    wheel_delta, wheel_zoom_factor, Camera, ClickTracker, ItemHit, CLICK_THRESHOLD,
    EDGE_HIT_HALF_WIDTH, FOCUS_DURATION, FOCUS_ZOOM, SLOT_HEIGHT,
};

type Curve = (EdgeKind, bool, [Point; 4]);

/// Opaque id chosen by the app, unique per item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ItemId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Block { inputs: usize, outputs: usize },
    Leaf,
}

pub struct GraphItem<'a, M> {
    pub id: ItemId,
    /// Top-left corner in graph px. The child is laid out with max size
    /// `shape.size()`, so use fixed or `Fill` sizes inside. Tooltips inside
    /// must use `.snap_within_viewport(false)`: their overlay is drawn at the
    /// map scale and iced would clamp it in graph coordinates.
    pub position: Point,
    pub shape: Shape,
    pub content: Element<'a, M>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnchorSide {
    Input,
    Output,
    LeafLeft,
    LeafRight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Anchor {
    pub item: ItemId,
    pub side: AnchorSide,
    /// Display row (after any slot reorder), ignored for leaves.
    pub row: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeKind {
    Coin,
    Counterparty,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Edge {
    pub from: Anchor,
    pub to: Anchor,
    pub kind: EdgeKind,
    pub active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    Input,
    Output,
}

/// What the pointer is on. Slot rows are display rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Item(ItemId),
    Slot(ItemId, Side, usize),
    /// Index into the edges passed to `GraphView::new`.
    Edge(usize),
    Frame,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GraphEvent {
    Zoom(f32),
    Click {
        target: Target,
        modifiers: keyboard::Modifiers,
    },
    DoubleClick {
        target: Target,
    },
    /// Published only when the hovered target changes.
    Hover(Option<Target>),
    /// Raw graph px delta. With snap on, each item lands on
    /// `geometry::snap_to_grid(position + delta)`.
    Moved {
        items: Vec<ItemId>,
        delta: Vector,
    },
    /// `rect` in graph px; `items` already picked (inside, or touched when
    /// `crossing`).
    AreaSelected {
        rect: Rectangle,
        crossing: bool,
        additive: bool,
        items: Vec<ItemId>,
    },
    EmptyClick,
    /// Live while a slot is dragged. `offset_y`: top of the dragged slot from
    /// its column top, graph px, clamped to the column. During the drag the
    /// app keeps passing edges and markers in the committed display order: the
    /// widget remaps that column live.
    SlotDrag {
        item: ItemId,
        side: Side,
        from: usize,
        to_display_index: usize,
        offset_y: f32,
    },
    /// End of a slot drag, also when `from == to` (it ends the live state).
    SlotDropped {
        item: ItemId,
        side: Side,
        from: usize,
        to: usize,
    },
    /// Wheel steps over the `wheel_slot`, positive when scrolling down.
    SlotWheel {
        item: ItemId,
        side: Side,
        row: usize,
        steps: i32,
    },
}

/// Items must be passed in a stable order: child state is diffed by index.
/// Anchor rows are display rows.
pub struct GraphView<'a, M> {
    id: Id,
    items: Vec<GraphItem<'a, M>>,
    edges: Vec<Edge>,
    frame: Option<Rectangle>,
    markers: Vec<Anchor>,
    grid: bool,
    dim_edges: bool,
    selected: HashSet<ItemId>,
    snap: bool,
    area_mode: bool,
    wheel_slot: Option<(ItemId, Side, usize)>,
    on_event: Option<Box<dyn Fn(GraphEvent) -> M + 'a>>,
}

impl<'a, M> GraphView<'a, M> {
    pub fn new(id: impl Into<Id>, items: Vec<GraphItem<'a, M>>, edges: Vec<Edge>) -> Self {
        Self {
            id: id.into(),
            items,
            edges,
            frame: None,
            markers: Vec::new(),
            grid: true,
            dim_edges: false,
            selected: HashSet::new(),
            snap: false,
            area_mode: false,
            wheel_slot: None,
            on_event: None,
        }
    }

    /// Selection members bounding box in graph px; the padding is added here.
    pub fn frame(mut self, frame: Option<Rectangle>) -> Self {
        self.frame = frame;
        self
    }

    /// Output anchors holding an unspent coin of ours.
    pub fn markers(mut self, markers: Vec<Anchor>) -> Self {
        self.markers = markers;
        self
    }

    pub fn grid(mut self, grid: bool) -> Self {
        self.grid = grid;
        self
    }

    /// Draws every edge at a reduced alpha. Markers are not dimmed.
    pub fn dim_edges(mut self, dim: bool) -> Self {
        self.dim_edges = dim;
        self
    }

    /// Dragging a member of two or more selected items moves all of them.
    pub fn selected(mut self, selected: &HashSet<ItemId>) -> Self {
        self.selected = selected.clone();
        self
    }

    /// Dragged items land on the grid, each on its own.
    pub fn snap(mut self, snap: bool) -> Self {
        self.snap = snap;
        self
    }

    /// Dragging on empty canvas draws an area instead of panning.
    pub fn area_mode(mut self, area_mode: bool) -> Self {
        self.area_mode = area_mode;
        self
    }

    /// The slot whose tag highlight cycles with the wheel instead of zooming.
    pub fn wheel_slot(mut self, slot: Option<(ItemId, Side, usize)>) -> Self {
        self.wheel_slot = slot;
        self
    }

    pub fn on_event(mut self, f: impl Fn(GraphEvent) -> M + 'a) -> Self {
        self.on_event = Some(Box::new(f));
        self
    }

    /// Where each item is now, in graph px, including the live drag.
    fn placements(&self, state: &State) -> HashMap<ItemId, (Point, Shape)> {
        let drag = match &state.interaction {
            Interaction::DraggingItems { items, delta, .. } => Some((items, *delta)),
            _ => None,
        };
        self.items
            .iter()
            .map(|item| {
                let mut position = item.position;
                if let Some((items, delta)) = drag {
                    if items.contains(&item.id) {
                        position += delta;
                        if self.snap {
                            position = snap_to_grid(position);
                        }
                    }
                }
                (item.id, (position, item.shape))
            })
            .collect()
    }

    /// Selection bounding box, moved with the raw delta when the whole
    /// selection is being dragged.
    fn frame_members(&self, state: &State) -> Option<Rectangle> {
        let members = self.frame?;
        match &state.interaction {
            Interaction::DraggingItems { items, delta, .. }
                if !self.selected.is_empty()
                    && self.selected.iter().all(|id| items.contains(id)) =>
            {
                Some(members + *delta)
            }
            _ => Some(members),
        }
    }

    fn screen_curve(
        &self,
        state: &State,
        placements: &HashMap<ItemId, (Point, Shape)>,
        edge: &Edge,
    ) -> Option<Curve> {
        let from = Self::anchor_point(state, placements, edge.from)?;
        let to = Self::anchor_point(state, placements, edge.to)?;
        let curve = edge_curve(from, to).map(|p| state.camera.to_screen(p));
        Some((edge.kind, edge.active, curve))
    }

    /// `cursor` is absolute.
    fn hit_test(&self, state: &State, bounds: Rectangle, cursor: Point) -> Option<Target> {
        let local = Point::new(cursor.x - bounds.x, cursor.y - bounds.y);
        let graph = state.camera.to_graph(local);
        let placements = self.placements(state);
        let dragged: &[ItemId] = match &state.interaction {
            Interaction::DraggingItems { items, .. } => items,
            _ => &[],
        };
        let order = dragged
            .iter()
            .copied()
            .chain(self.items.iter().rev().map(|item| item.id));
        for id in order {
            let Some((position, shape)) = placements.get(&id) else {
                continue;
            };
            match hit_item(graph, *position, *shape) {
                Some(ItemHit::Body) => return Some(Target::Item(id)),
                Some(ItemHit::Slot(side, row)) => return Some(Target::Slot(id, side, row)),
                None => {}
            }
        }
        let edge = self.edges.iter().enumerate().find_map(|(index, edge)| {
            let (_, _, curve) = self.screen_curve(state, &placements, edge)?;
            (distance_to_curve(local, &curve) < EDGE_HIT_HALF_WIDTH).then_some(Target::Edge(index))
        });
        edge.or_else(|| {
            let members = self
                .frame_members(state)
                .filter(|_| !self.selected.is_empty())?;
            frame_rect(members).contains(graph).then_some(Target::Frame)
        })
    }

    /// Publishes `Hover` when the target under the cursor changed.
    fn refresh_hover(
        &self,
        state: &mut State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
        shell: &mut Shell<'_, M>,
    ) {
        let hover = cursor
            .position()
            .filter(|_| cursor.is_over(bounds))
            .and_then(|position| self.hit_test(state, bounds, position));
        if hover != state.hover {
            state.hover = hover;
            self.publish(shell, GraphEvent::Hover(hover));
        }
    }

    /// Row of `anchor`, remapped live when its column is being reordered.
    fn anchor_row(state: &State, anchor: Anchor) -> f32 {
        match state.interaction {
            Interaction::ReorderingSlot {
                item,
                side,
                from,
                rows,
                offset_y,
                ..
            } if item == anchor.item
                && matches!(
                    (anchor.side, side),
                    (AnchorSide::Input, Side::Input) | (AnchorSide::Output, Side::Output)
                ) =>
            {
                reordered_row(anchor.row, from, reorder_target(offset_y, rows), offset_y)
            }
            _ => anchor.row as f32,
        }
    }

    fn anchor_point(
        state: &State,
        placements: &HashMap<ItemId, (Point, Shape)>,
        anchor: Anchor,
    ) -> Option<Point> {
        let (position, shape) = placements.get(&anchor.item)?;
        Some(anchor_point(
            anchor.side,
            Self::anchor_row(state, anchor),
            *position,
            *shape,
        ))
    }

    fn publish(&self, shell: &mut Shell<'_, M>, event: GraphEvent) {
        if let Some(on_event) = &self.on_event {
            shell.publish(on_event(event));
        }
    }

    fn publish_zoom(&self, state: &mut State, shell: &mut Shell<'_, M>) {
        let zoom = state.camera.zoom;
        if state.published_zoom != Some(zoom) {
            state.published_zoom = Some(zoom);
            self.publish(shell, GraphEvent::Zoom(zoom));
        }
    }
}

#[derive(Debug, Default)]
struct State {
    camera: Camera,
    interaction: Interaction,
    modifiers: keyboard::Modifiers,
    hover: Option<Target>,
    clicks: ClickTracker,
    size: Size,
    /// Union of the item rects, in graph px.
    content: Option<Rectangle>,
    published_zoom: Option<f32>,
    wheel_accumulated: f32,
    animation: Option<CameraAnimation>,
    edges_cache: canvas::Cache,
    edges_key: Cell<u64>,
}

#[derive(Debug)]
struct CameraAnimation {
    from: Camera,
    to: Camera,
    progress: Animation<bool>,
}

#[derive(Debug, Clone, Default)]
enum Interaction {
    #[default]
    Idle,
    /// `origin` is the absolute cursor position at press.
    Pressed {
        origin: Point,
        camera: Camera,
        target: Option<Target>,
        modifiers: keyboard::Modifiers,
    },
    Panning {
        origin: Point,
        camera: Camera,
    },
    /// `delta` is in graph px.
    DraggingItems {
        origin: Point,
        items: Vec<ItemId>,
        delta: Vector,
    },
    /// `origin` and `current` are absolute screen px.
    Area {
        origin: Point,
        current: Point,
        additive: bool,
    },
    /// `offset_y` is the top of the dragged slot from its column top, graph px.
    ReorderingSlot {
        origin: Point,
        item: ItemId,
        side: Side,
        from: usize,
        rows: usize,
        offset_y: f32,
    },
}

#[derive(Debug, Clone, Copy)]
enum CameraRequest {
    Fit,
    ZoomBy(f32),
    Focus(Rectangle),
}

impl State {
    fn area_active(&self, view_area_mode: bool) -> bool {
        view_area_mode || self.modifiers.command() || self.modifiers.shift()
    }

    fn apply(&mut self, request: CameraRequest) {
        self.animation = None;
        match request {
            CameraRequest::Fit => {
                self.camera = self
                    .content
                    .map(|content| Camera::fit(content, self.size))
                    .unwrap_or_default();
            }
            CameraRequest::ZoomBy(factor) => {
                self.camera = self.camera.zoom_around(
                    Point::new(self.size.width / 2.0, self.size.height / 2.0),
                    factor,
                );
            }
            CameraRequest::Focus(target) => {
                let mut progress = Animation::new(false)
                    .easing(Easing::EaseInOutCubic)
                    .duration(FOCUS_DURATION);
                progress.go_mut(true, Instant::now());
                self.animation = Some(CameraAnimation {
                    from: self.camera,
                    to: Camera::centered_on(target.center(), FOCUS_ZOOM, self.size),
                    progress,
                });
            }
        }
        self.interaction = Interaction::Idle;
    }

    fn child_cursor(
        &self,
        cursor: mouse::Cursor,
        bounds: Rectangle,
        inverse: Transformation,
    ) -> mouse::Cursor {
        match self.interaction {
            Interaction::Idle | Interaction::Pressed { .. } if cursor.is_over(bounds) => {
                cursor * inverse
            }
            _ => mouse::Cursor::Unavailable,
        }
    }
}

impl<'a, M: 'a> Widget<M, Theme, Renderer> for GraphView<'a, M> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn children(&self) -> Vec<Tree> {
        self.items
            .iter()
            .map(|item| Tree::new(&item.content))
            .collect()
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(&self.items.iter().map(|i| &i.content).collect::<Vec<_>>());
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let size = limits.resolve(Length::Fill, Length::Fill, Size::ZERO);
        let mut content: Option<Rectangle> = None;
        let mut nodes = Vec::with_capacity(self.items.len());
        for (item, child) in self.items.iter_mut().zip(&mut tree.children) {
            let item_size = item.shape.size();
            let node = item.content.as_widget_mut().layout(
                child,
                renderer,
                &layout::Limits::new(Size::ZERO, item_size),
            );
            nodes.push(node.move_to(item.position));
            let rect = Rectangle::new(item.position, item_size);
            content = Some(content.map_or(rect, |c| c.union(&rect)));
        }
        let state = tree.state.downcast_mut::<State>();
        state.size = size;
        state.content = content;
        layout::Node::with_children(size, nodes)
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let Some(clip) = bounds.intersection(viewport) else {
            return;
        };
        let state = tree.state.downcast_ref::<State>();
        let transformation = state.camera.transformation(bounds);
        let inverse = transformation.inverse();
        let child_cursor = state.child_cursor(cursor, bounds, inverse);
        let child_viewport = clip * inverse;
        let placements = self.placements(state);
        let palette = &theme.colors.graph;
        let dragged: &[ItemId] = match &state.interaction {
            Interaction::DraggingItems { items, .. } => items,
            _ => &[],
        };
        let curves: Vec<Curve> = self
            .edges
            .iter()
            .filter_map(|edge| self.screen_curve(state, &placements, edge))
            .collect();
        let mut hasher = DefaultHasher::new();
        for (kind, active, curve) in &curves {
            (*kind as u8, *active).hash(&mut hasher);
            for p in curve {
                (p.x.to_bits(), p.y.to_bits()).hash(&mut hasher);
            }
        }
        (
            self.dim_edges,
            bounds.width.to_bits(),
            bounds.height.to_bits(),
        )
            .hash(&mut hasher);
        let key = hasher.finish();
        if state.edges_key.replace(key) != key {
            state.edges_cache.clear();
        }
        let edges = state.edges_cache.draw(renderer, bounds.size(), |frame| {
            draw::edges(frame, palette, &curves, self.dim_edges)
        });
        renderer.with_layer(clip, |renderer| {
            if self.grid {
                draw::grid(renderer, palette, bounds, clip, state.camera);
            }
            if let Some(members) = self.frame_members(state) {
                draw::frame(renderer, palette, bounds, state.camera, members);
            }
            renderer.with_translation(Vector::new(bounds.x, bounds.y), |renderer| {
                renderer.draw_geometry(edges);
            });
        });
        renderer.with_layer(clip, |renderer| {
            renderer.with_transformation(transformation, |renderer| {
                for ((item, child), child_layout) in
                    self.items.iter().zip(&tree.children).zip(layout.children())
                {
                    if dragged.contains(&item.id)
                        || child_layout
                            .bounds()
                            .intersection(&child_viewport)
                            .is_none()
                    {
                        continue;
                    }
                    item.content.as_widget().draw(
                        child,
                        renderer,
                        theme,
                        style,
                        child_layout,
                        child_cursor,
                        &child_viewport,
                    );
                }
            });
        });
        renderer.with_layer(clip, |renderer| {
            for ((item, child), child_layout) in
                self.items.iter().zip(&tree.children).zip(layout.children())
            {
                if !dragged.contains(&item.id) {
                    continue;
                }
                let Some((position, _)) = placements.get(&item.id) else {
                    continue;
                };
                let offset = *position - item.position;
                let child_bounds = child_layout.bounds();
                let radius = CARD_RADIUS.min(child_bounds.height / 2.0);
                renderer.with_transformation(
                    transformation * Transformation::translate(offset.x, offset.y),
                    |renderer| {
                        renderer.fill_quad(
                            renderer::Quad {
                                bounds: child_bounds.shrink(1.0),
                                shadow: CARD_SHADOW_HOVER,
                                border: border::rounded(radius),
                                ..renderer::Quad::default()
                            },
                            CARD_SHADOW_HOVER.color,
                        );
                        item.content.as_widget().draw(
                            child,
                            renderer,
                            theme,
                            style,
                            child_layout,
                            mouse::Cursor::Unavailable,
                            &child_viewport,
                        );
                    },
                );
            }
        });
        let marker_points: Vec<Point> = self
            .markers
            .iter()
            .filter_map(|anchor| Self::anchor_point(state, &placements, *anchor))
            .collect();
        renderer.with_layer(clip, |renderer| {
            // Marker points are graph coordinates, not absolute ones like child layouts.
            let markers = transformation * Transformation::translate(bounds.x, bounds.y);
            renderer.with_transformation(markers, |renderer| {
                draw::markers(renderer, palette, &marker_points);
            });
            if let Interaction::Area {
                origin, current, ..
            } = state.interaction
            {
                draw::area(
                    renderer,
                    palette,
                    rect_from_points(origin, current),
                    current.x < origin.x,
                );
            }
        });
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let state = tree.state.downcast_mut::<State>();
        // Camera operations run outside `update`.
        self.publish_zoom(state, shell);
        let inverse = state.camera.transformation(bounds).inverse();
        let child_cursor = state.child_cursor(cursor, bounds, inverse);
        let child_viewport = bounds.intersection(viewport).unwrap_or(bounds) * inverse;
        for ((item, child), child_layout) in self
            .items
            .iter_mut()
            .zip(&mut tree.children)
            .zip(layout.children())
        {
            item.content.as_widget_mut().update(
                child,
                event,
                child_layout,
                child_cursor,
                renderer,
                clipboard,
                shell,
                &child_viewport,
            );
        }
        if shell.is_event_captured() {
            return;
        }

        match event {
            Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) => {
                state.modifiers = *modifiers;
                shell.request_redraw();
            }
            Event::Mouse(mouse::Event::CursorLeft) => {
                if let Interaction::Idle = state.interaction {
                    self.refresh_hover(state, bounds, mouse::Cursor::Unavailable, shell);
                }
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if let (Interaction::Idle, Some(origin)) = (&state.interaction, cursor.position()) {
                    if cursor.is_over(bounds) {
                        state.interaction = Interaction::Pressed {
                            origin,
                            camera: state.camera,
                            target: self.hit_test(state, bounds, origin),
                            modifiers: state.modifiers,
                        };
                        shell.capture_event();
                    }
                }
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let Some(position) = cursor.position() else {
                    return;
                };
                if let Interaction::Idle = state.interaction {
                    self.refresh_hover(state, bounds, cursor, shell);
                }
                if let Interaction::Pressed {
                    origin,
                    camera,
                    target,
                    modifiers,
                } = state.interaction.clone()
                {
                    if origin.distance(position) > CLICK_THRESHOLD {
                        state.animation = None;
                        let area = state.area_active(self.area_mode);
                        match target {
                            Some(Target::Item(id)) => {
                                state.interaction = Interaction::DraggingItems {
                                    origin,
                                    items: drag_set(id, &self.selected),
                                    delta: Vector::ZERO,
                                };
                            }
                            Some(Target::Frame) => {
                                let mut items: Vec<ItemId> =
                                    self.selected.iter().copied().collect();
                                items.sort();
                                state.interaction = Interaction::DraggingItems {
                                    origin,
                                    items,
                                    delta: Vector::ZERO,
                                };
                            }
                            Some(Target::Slot(id, side, from)) => {
                                let rows = self.items.iter().find(|item| item.id == id).map_or(
                                    0,
                                    |item| match (item.shape, side) {
                                        (Shape::Block { inputs, .. }, Side::Input) => inputs,
                                        (Shape::Block { outputs, .. }, Side::Output) => outputs,
                                        (Shape::Leaf, _) => 0,
                                    },
                                );
                                state.interaction = Interaction::ReorderingSlot {
                                    origin,
                                    item: id,
                                    side,
                                    from,
                                    rows,
                                    offset_y: from as f32 * SLOT_HEIGHT,
                                };
                            }
                            None | Some(Target::Edge(_)) if area => {
                                state.interaction = Interaction::Area {
                                    origin,
                                    current: position,
                                    additive: modifiers.shift(),
                                };
                            }
                            None | Some(Target::Edge(_)) => {
                                state.interaction = Interaction::Panning { origin, camera };
                            }
                        }
                    }
                }
                match &mut state.interaction {
                    Interaction::Panning { origin, camera } => {
                        state.camera.offset = camera.offset + (position - *origin);
                    }
                    Interaction::DraggingItems { origin, delta, .. } => {
                        *delta = (position - *origin) * (1.0 / state.camera.zoom);
                    }
                    Interaction::Area { current, .. } => *current = position,
                    Interaction::ReorderingSlot {
                        origin,
                        item,
                        side,
                        from,
                        rows,
                        offset_y,
                    } => {
                        let max = rows.saturating_sub(1) as f32 * SLOT_HEIGHT;
                        let next = (*from as f32 * SLOT_HEIGHT
                            + (position.y - origin.y) / state.camera.zoom)
                            .clamp(0.0, max);
                        if next != *offset_y {
                            *offset_y = next;
                            let event = GraphEvent::SlotDrag {
                                item: *item,
                                side: *side,
                                from: *from,
                                to_display_index: reorder_target(next, *rows),
                                offset_y: next,
                            };
                            self.publish(shell, event);
                        }
                    }
                    _ => {}
                }
                if !matches!(
                    state.interaction,
                    Interaction::Idle | Interaction::Pressed { .. }
                ) {
                    shell.request_redraw();
                    shell.capture_event();
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                let interaction = std::mem::take(&mut state.interaction);
                if matches!(interaction, Interaction::Idle) {
                    return;
                }
                match interaction {
                    Interaction::Pressed {
                        origin,
                        target,
                        modifiers,
                        ..
                    } if cursor
                        .position()
                        .is_some_and(|p| origin.distance(p) <= CLICK_THRESHOLD) =>
                    {
                        match target {
                            None => self.publish(shell, GraphEvent::EmptyClick),
                            Some(Target::Edge(_)) => {}
                            Some(target) => {
                                let event = if state.clicks.register(target, Instant::now()) {
                                    GraphEvent::DoubleClick { target }
                                } else {
                                    GraphEvent::Click { target, modifiers }
                                };
                                self.publish(shell, event);
                            }
                        }
                    }
                    Interaction::DraggingItems { items, delta, .. } if delta != Vector::ZERO => {
                        self.publish(shell, GraphEvent::Moved { items, delta });
                    }
                    Interaction::ReorderingSlot {
                        item,
                        side,
                        from,
                        rows,
                        offset_y,
                        ..
                    } => {
                        let to = reorder_target(offset_y, rows);
                        self.publish(
                            shell,
                            GraphEvent::SlotDropped {
                                item,
                                side,
                                from,
                                to,
                            },
                        );
                    }
                    Interaction::Area {
                        origin,
                        current,
                        additive,
                    } => {
                        let to_graph = |p: Point| {
                            state
                                .camera
                                .to_graph(Point::new(p.x - bounds.x, p.y - bounds.y))
                        };
                        let rect = rect_from_points(to_graph(origin), to_graph(current));
                        let crossing = current.x < origin.x;
                        let items = area_pick(
                            rect,
                            crossing,
                            self.items.iter().map(|item| {
                                (item.id, Rectangle::new(item.position, item.shape.size()))
                            }),
                        );
                        self.publish(
                            shell,
                            GraphEvent::AreaSelected {
                                rect,
                                crossing,
                                additive,
                                items,
                            },
                        );
                    }
                    _ => {}
                }
                shell.capture_event();
                shell.request_redraw();
                self.refresh_hover(state, bounds, cursor, shell);
            }
            Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                if let Some(anchor) = cursor.position_in(bounds) {
                    let amount = wheel_delta(*delta);
                    let slot =
                        match self.hit_test(state, bounds, cursor.position().unwrap_or_default()) {
                            Some(Target::Slot(item, side, row))
                                if self.wheel_slot == Some((item, side, row)) =>
                            {
                                Some((item, side, row))
                            }
                            _ => None,
                        };
                    if let Some((item, side, row)) = slot {
                        let (rest, steps) = accumulate_wheel(state.wheel_accumulated, amount);
                        state.wheel_accumulated = rest;
                        if steps != 0 {
                            self.publish(
                                shell,
                                GraphEvent::SlotWheel {
                                    item,
                                    side,
                                    row,
                                    steps,
                                },
                            );
                        }
                    } else {
                        state.wheel_accumulated = 0.0;
                        state.animation = None;
                        state.camera = state.camera.zoom_around(anchor, wheel_zoom_factor(amount));
                        shell.request_redraw();
                    }
                    shell.capture_event();
                }
            }
            Event::Window(window::Event::RedrawRequested(now)) => {
                if let Some(animation) = state.animation.take() {
                    if animation.progress.is_animating(*now) {
                        let t = animation.progress.interpolate(0.0, 1.0, *now);
                        state.camera =
                            interpolate_camera(animation.from, animation.to, t, state.size);
                        state.animation = Some(animation);
                        shell.request_redraw();
                    } else {
                        state.camera = animation.to;
                    }
                }
            }
            _ => {}
        }

        self.publish_zoom(state, shell);
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        let bounds = layout.bounds();
        let state = tree.state.downcast_ref::<State>();
        match state.interaction {
            Interaction::Panning { .. }
            | Interaction::DraggingItems { .. }
            | Interaction::ReorderingSlot { .. } => {
                return mouse::Interaction::Grabbing;
            }
            Interaction::Area { .. } => return mouse::Interaction::Crosshair,
            Interaction::Idle | Interaction::Pressed { .. } => {}
        }
        let inverse = state.camera.transformation(bounds).inverse();
        let child_cursor = state.child_cursor(cursor, bounds, inverse);
        let child_viewport = bounds.intersection(viewport).unwrap_or(bounds) * inverse;
        let children = self
            .items
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
            .map(|((item, child), child_layout)| {
                item.content.as_widget().mouse_interaction(
                    child,
                    child_layout,
                    child_cursor,
                    &child_viewport,
                    renderer,
                )
            })
            .max()
            .unwrap_or_default();
        if children != mouse::Interaction::None || !cursor.is_over(bounds) {
            return children;
        }
        match state.hover {
            Some(Target::Slot(..)) => mouse::Interaction::Pointer,
            Some(Target::Item(_) | Target::Frame) => mouse::Interaction::Grab,
            None | Some(Target::Edge(_)) if state.area_active(self.area_mode) => {
                mouse::Interaction::Crosshair
            }
            None | Some(Target::Edge(_)) => mouse::Interaction::None,
        }
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<iced_overlay::Element<'b, M, Theme, Renderer>> {
        let state = tree.state.downcast_ref::<State>();
        if !matches!(state.interaction, Interaction::Idle) {
            return None;
        }
        let transformation = Transformation::translate(translation.x, translation.y)
            * state.camera.transformation(layout.bounds());
        let viewport = *viewport * transformation.inverse();
        let children: Vec<_> = self
            .items
            .iter_mut()
            .zip(&mut tree.children)
            .zip(layout.children())
            .filter_map(|((item, child), child_layout)| {
                item.content
                    .as_widget_mut()
                    .overlay(child, child_layout, renderer, &viewport, Vector::ZERO)
                    .map(|content| {
                        iced_overlay::Element::new(Box::new(overlay::Transformed::new(
                            content,
                            transformation,
                        )))
                    })
            })
            .collect();
        (!children.is_empty()).then(|| iced_overlay::Group::with_children(children).overlay())
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        let state = tree.state.downcast_mut::<State>();
        operation.custom(Some(&self.id), layout.bounds(), state);
        operation.traverse(&mut |operation| {
            for ((item, child), child_layout) in self
                .items
                .iter_mut()
                .zip(&mut tree.children)
                .zip(layout.children())
            {
                item.content
                    .as_widget_mut()
                    .operate(child, child_layout, renderer, operation);
            }
        });
    }
}

impl<'a, M: 'a> From<GraphView<'a, M>> for Element<'a, M> {
    fn from(view: GraphView<'a, M>) -> Self {
        Element::new(view)
    }
}

struct CameraOperation {
    target: Id,
    request: CameraRequest,
}

impl Operation for CameraOperation {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        operate(self);
    }

    fn custom(&mut self, id: Option<&Id>, _bounds: Rectangle, state: &mut dyn Any) {
        if id == Some(&self.target) {
            if let Some(state) = state.downcast_mut::<State>() {
                state.apply(self.request);
            }
        }
    }
}

/// Widget operations sent from a task do not request a redraw by themselves.
fn camera_task<T: 'static>(id: impl Into<Id>, request: CameraRequest) -> Task<T> {
    task::effect(Action::widget(CameraOperation {
        target: id.into(),
        request,
    }))
    .chain(task::effect(Action::Window(
        iced_runtime::window::Action::RedrawAll,
    )))
}

/// Fits the whole content in view.
pub fn fit<T: 'static>(id: impl Into<Id>) -> Task<T> {
    camera_task(id, CameraRequest::Fit)
}

/// Zooms by `factor` around the widget center.
pub fn zoom_by<T: 'static>(id: impl Into<Id>, factor: f32) -> Task<T> {
    camera_task(id, CameraRequest::ZoomBy(factor))
}

/// Centers `target` (graph px) at the focus zoom.
pub fn focus<T: 'static>(id: impl Into<Id>, target: Rectangle) -> Task<T> {
    camera_task(id, CameraRequest::Focus(target))
}
