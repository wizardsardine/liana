use std::time::Duration;

use iced::{
    advanced::{
        layout, renderer,
        widget::tree::{self, Tree},
        Clipboard, Layout, Shell, Widget,
    },
    event, mouse,
    time::Instant,
    window, Element, Event, Length, Rectangle, Renderer, Size,
};

/// A loading spinner widget that cycles through a collection of
/// `children` at a fixed rate.
///
/// `interval` is how long to wait before displaying the next child.
pub struct Carousel<'a, Message, Theme> {
    interval: Duration,
    children: Vec<Element<'a, Message, Theme>>,
}

impl<'a, Message, Theme> Carousel<'a, Message, Theme> {
    pub fn new(interval: Duration, children: Vec<impl Into<Element<'a, Message, Theme>>>) -> Self {
        Carousel {
            interval,
            children: children.into_iter().map(|child| child.into()).collect(),
        }
    }
}

/// The state of a `Carousel`.
///
/// `last_transition` is when the `current`th child
/// of `Carousel::children` was selected.
struct CarouselState {
    last_transition: Instant,
    current: usize,
}

impl CarouselState {
    fn new() -> Self {
        Self {
            last_transition: Instant::now(),
            current: 0,
        }
    }
}

impl<'a, Message, Theme> Widget<Message, Theme, Renderer> for Carousel<'a, Message, Theme>
where
    Message: 'a + Clone,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<CarouselState>()
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(self.children.as_slice());
    }

    fn layout(
        &self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let state = tree.state.downcast_mut::<CarouselState>();
        let child_nodes: Vec<_> = self
            .children
            .iter()
            .enumerate()
            .map(|(i, child)| {
                child
                    .as_widget()
                    .layout(&mut tree.children[i], renderer, limits)
            })
            .collect();
        layout::Node::with_children(child_nodes[state.current].size(), child_nodes)
    }

    fn children(&self) -> Vec<Tree> {
        self.children.iter().map(|child| Tree::new(child)).collect()
    }

    fn state(&self) -> tree::State {
        tree::State::new(CarouselState::new())
    }

    fn size(&self) -> Size<Length> {
        // Use an arbitrary size here as the layout node size
        // is determined from the current child.
        Size {
            width: Length::Shrink,
            height: Length::Shrink,
        }
    }

    fn on_event(
        &mut self,
        tree: &mut Tree,
        event: Event,
        _layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _renderer: &Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) -> event::Status {
        let state = tree.state.downcast_mut::<CarouselState>();
        if let Event::Window(_, window::Event::RedrawRequested(now)) = event {
            if now.duration_since(state.last_transition) > self.interval {
                state.last_transition = now;
                state.current = (state.current + 1) % self.children.len();
            }
            shell.request_redraw(window::RedrawRequest::NextFrame);
        }
        event::Status::Ignored
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
        let state = tree.state.downcast_ref::<CarouselState>();
        let current = self.children.get(state.current).expect("current");
        let current_layout = layout
            .children()
            .nth(state.current)
            .expect("current layout");
        current.as_widget().draw(
            &tree.children[state.current],
            renderer,
            theme,
            style,
            current_layout,
            cursor,
            viewport,
        );
    }
}

impl<'a, Message, Theme> From<Carousel<'a, Message, Theme>> for Element<'a, Message, Theme>
where
    Message: 'a + Clone,
    Theme: 'a,
{
    fn from(carousel: Carousel<'a, Message, Theme>) -> Self {
        Element::new(carousel)
    }
}
