//! Generic pan/zoom graph canvas. It takes no gui types: children are placed
//! at graph coordinates and shown through a camera.
use std::{
    any::Any,
    cell::Cell,
    collections::HashMap,
    hash::{DefaultHasher, Hash, Hasher},
};

use iced::{
    advanced::{
        graphics::geometry::Renderer as _,
        layout::{self, Layout},
        renderer,
        widget::{tree, Id, Operation, Tree, Widget},
        Clipboard, Renderer as _, Shell,
    },
    mouse,
    widget::canvas,
    Event, Length, Point, Rectangle, Size, Transformation, Vector,
};
use iced_runtime::{task, Action, Task};

use crate::{
    theme::Theme,
    widget::{Element, Renderer},
};

mod draw;
pub mod geometry;

use geometry::{
    anchor_point, edge_curve, wheel_delta, wheel_zoom_factor, Camera, CLICK_THRESHOLD, FOCUS_ZOOM,
};

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
    /// `shape.size()`, so use fixed or `Fill` sizes inside.
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

#[derive(Debug, Clone, PartialEq)]
pub enum GraphEvent {
    Zoom(f32),
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

    pub fn on_event(mut self, f: impl Fn(GraphEvent) -> M + 'a) -> Self {
        self.on_event = Some(Box::new(f));
        self
    }

    /// Where each item is now, in graph px.
    fn placements(&self) -> HashMap<ItemId, (Point, Shape)> {
        self.items
            .iter()
            .map(|item| (item.id, (item.position, item.shape)))
            .collect()
    }

    fn anchor_point(placements: &HashMap<ItemId, (Point, Shape)>, anchor: Anchor) -> Option<Point> {
        let (position, shape) = placements.get(&anchor.item)?;
        Some(anchor_point(
            anchor.side,
            anchor.row as f32,
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
    size: Size,
    /// Union of the item rects, in graph px.
    content: Option<Rectangle>,
    published_zoom: Option<f32>,
    edges_cache: canvas::Cache,
    edges_key: Cell<u64>,
}

#[derive(Debug, Clone, Default)]
enum Interaction {
    #[default]
    Idle,
    /// `origin` is the absolute cursor position at press.
    Pressed {
        origin: Point,
        camera: Camera,
    },
    Panning {
        origin: Point,
        camera: Camera,
    },
}

#[derive(Debug, Clone, Copy)]
enum CameraRequest {
    Fit,
    ZoomBy(f32),
    Focus(Rectangle),
}

impl State {
    fn apply(&mut self, request: CameraRequest) {
        self.camera = match request {
            CameraRequest::Fit => self
                .content
                .map(|content| Camera::fit(content, self.size))
                .unwrap_or_default(),
            CameraRequest::ZoomBy(factor) => self.camera.zoom_around(
                Point::new(self.size.width / 2.0, self.size.height / 2.0),
                factor,
            ),
            CameraRequest::Focus(target) => {
                Camera::centered_on(target.center(), FOCUS_ZOOM, self.size)
            }
        };
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
        let placements = self.placements();
        let palette = &theme.colors.graph;
        let curves: Vec<_> = self
            .edges
            .iter()
            .filter_map(|edge| {
                let from = Self::anchor_point(&placements, edge.from)?;
                let to = Self::anchor_point(&placements, edge.to)?;
                let curve = edge_curve(from, to).map(|p| state.camera.to_screen(p));
                Some((edge.kind, edge.active, curve))
            })
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
            if let Some(members) = self.frame {
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
                    if child_layout
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
        let marker_points: Vec<Point> = self
            .markers
            .iter()
            .filter_map(|anchor| Self::anchor_point(&placements, *anchor))
            .collect();
        renderer.with_layer(clip, |renderer| {
            // Marker points are graph coordinates, not absolute ones like child layouts.
            let markers = transformation * Transformation::translate(bounds.x, bounds.y);
            renderer.with_transformation(markers, |renderer| {
                draw::markers(renderer, palette, &marker_points);
            });
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
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if let (Interaction::Idle, Some(origin)) = (&state.interaction, cursor.position()) {
                    if cursor.is_over(bounds) {
                        state.interaction = Interaction::Pressed {
                            origin,
                            camera: state.camera,
                        };
                        shell.capture_event();
                    }
                }
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                if let (
                    Interaction::Pressed { origin, camera }
                    | Interaction::Panning { origin, camera },
                    Some(position),
                ) = (&state.interaction, cursor.position())
                {
                    let (origin, camera) = (*origin, *camera);
                    let panning = matches!(state.interaction, Interaction::Panning { .. });
                    if panning || origin.distance(position) > CLICK_THRESHOLD {
                        state.interaction = Interaction::Panning { origin, camera };
                        state.camera.offset = camera.offset + (position - origin);
                        shell.request_redraw();
                        shell.capture_event();
                    }
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                if !matches!(state.interaction, Interaction::Idle) {
                    state.interaction = Interaction::Idle;
                    shell.capture_event();
                }
            }
            Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                if let Some(anchor) = cursor.position_in(bounds) {
                    let factor = wheel_zoom_factor(wheel_delta(*delta));
                    state.camera = state.camera.zoom_around(anchor, factor);
                    shell.request_redraw();
                    shell.capture_event();
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
        if matches!(state.interaction, Interaction::Panning { .. }) {
            return mouse::Interaction::Grabbing;
        }
        let inverse = state.camera.transformation(bounds).inverse();
        let child_cursor = state.child_cursor(cursor, bounds, inverse);
        let child_viewport = bounds.intersection(viewport).unwrap_or(bounds) * inverse;
        self.items
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
            .unwrap_or_default()
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
