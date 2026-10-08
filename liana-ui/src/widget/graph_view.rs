//! Generic pan/zoom graph canvas. It takes no gui types: children are placed
//! at graph coordinates and shown through a camera.
use std::any::Any;

use iced::{
    advanced::{
        layout::{self, Layout},
        renderer,
        widget::{tree, Id, Operation, Tree, Widget},
        Clipboard, Renderer as _, Shell,
    },
    mouse, Event, Length, Point, Rectangle, Size, Transformation,
};
use iced_runtime::{task, Action, Task};

use crate::{
    theme::Theme,
    widget::{Element, Renderer},
};

pub mod geometry;

use geometry::{wheel_delta, wheel_zoom_factor, Camera, CLICK_THRESHOLD, FOCUS_ZOOM};

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

#[derive(Debug, Clone, PartialEq)]
pub enum GraphEvent {
    Zoom(f32),
}

/// Items must be passed in a stable order: child state is diffed by index.
pub struct GraphView<'a, M> {
    id: Id,
    items: Vec<GraphItem<'a, M>>,
    on_event: Option<Box<dyn Fn(GraphEvent) -> M + 'a>>,
}

impl<'a, M> GraphView<'a, M> {
    pub fn new(id: impl Into<Id>, items: Vec<GraphItem<'a, M>>) -> Self {
        Self {
            id: id.into(),
            items,
            on_event: None,
        }
    }

    pub fn on_event(mut self, f: impl Fn(GraphEvent) -> M + 'a) -> Self {
        self.on_event = Some(Box::new(f));
        self
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
