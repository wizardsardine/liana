use std::f32::consts::{FRAC_PI_2, TAU};
use std::fmt::Display;
use std::time::Duration;

use iced::{
    advanced::{
        layout, renderer,
        widget::tree::{self, Tree},
        Clipboard, Layout, Shell, Widget,
    },
    alignment, mouse,
    time::Instant,
    widget::{
        canvas::{self, path::Arc, Canvas, LineCap, Path, Stroke},
        container,
    },
    window, Background, Border, Element as IcedElement, Event, Length, Padding, Point, Radians,
    Rectangle, Renderer, Size,
};

use crate::{
    color,
    component::text::{h3, p1_regular},
    theme::{self, Theme},
    widget::{Column, Container, Element},
};

const RING_SIZE: f32 = 56.0;
const RING_RADIUS: f32 = 26.0;
const RING_STROKE: f32 = 4.0;
const RING_PERIOD: f32 = 0.9;

#[derive(Debug, Clone, Copy)]
pub struct Ring;

#[derive(Debug)]
pub struct RingState {
    start: Instant,
    elapsed: Duration,
}

impl Default for RingState {
    fn default() -> Self {
        Self {
            start: Instant::now(),
            elapsed: Duration::ZERO,
        }
    }
}

pub fn ring<'a, Message: 'a>() -> crate::widget::Element<'a, Message> {
    spinner()
}

pub fn spinner<'a, Message: 'a>() -> Element<'a, Message> {
    Canvas::<Ring, Message, Theme, Renderer>::new(Ring)
        .width(Length::Fixed(RING_SIZE))
        .height(Length::Fixed(RING_SIZE))
        .into()
}

pub fn spinner_modal<'a, Message: 'a>(
    title: impl Display,
    description: impl Display,
) -> Element<'a, Message> {
    let text = Column::new()
        .align_x(alignment::Horizontal::Center)
        .spacing(8)
        .push(h3(format!("{title}")).style(theme::text::primary))
        .push(
            p1_regular(format!("{description}"))
                .style(theme::text::secondary)
                .align_x(alignment::Horizontal::Center),
        );

    Container::new(
        Column::new()
            .align_x(alignment::Horizontal::Center)
            .spacing(22)
            .push(spinner())
            .push(text),
    )
    .width(380)
    .padding(Padding {
        top: 40.0,
        right: 36.0,
        bottom: 40.0,
        left: 36.0,
    })
    .style(|_theme| container::Style {
        background: Some(Background::Color(color::LIGHT_BLACK)),
        border: Border {
            color: color::GREY_7,
            width: 1.0,
            radius: 16.0.into(),
        },
        ..Default::default()
    })
    .into()
}

impl<Message> canvas::Program<Message, Theme, Renderer> for Ring {
    type State = RingState;

    fn update(
        &self,
        state: &mut Self::State,
        event: &canvas::Event,
        _bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        if let canvas::Event::Window(window::Event::RedrawRequested(now)) = event {
            state.elapsed = now.duration_since(state.start);
            return Some(canvas::Action::request_redraw());
        }
        None
    }

    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        _theme: &crate::theme::Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let center = Point::new(RING_SIZE / 2.0, RING_SIZE / 2.0);
        let track = Path::circle(center, RING_RADIUS);
        frame.stroke(
            &track,
            Stroke::default()
                .with_color(color::GREY_5)
                .with_width(RING_STROKE),
        );

        let start_angle = (state.elapsed.as_secs_f32() / RING_PERIOD * TAU) % TAU;
        let arc = Path::new(|arc| {
            arc.arc(Arc {
                center,
                radius: RING_RADIUS,
                start_angle: Radians(start_angle),
                end_angle: Radians(start_angle + FRAC_PI_2),
            });
        });
        frame.stroke(
            &arc,
            Stroke::default()
                .with_color(color::GREEN)
                .with_width(RING_STROKE)
                .with_line_cap(LineCap::Butt),
        );
        vec![frame.into_geometry()]
    }
}

/// A loading spinner widget that cycles through a collection of
/// `children` at a fixed rate.
///
/// `interval` is how long to wait before displaying the next child.
pub struct Carousel<'a, Message, Theme> {
    interval: Duration,
    children: Vec<IcedElement<'a, Message, Theme>>,
}

impl<'a, Message, Theme> Carousel<'a, Message, Theme> {
    pub fn new(
        interval: Duration,
        children: Vec<impl Into<IcedElement<'a, Message, Theme>>>,
    ) -> Self {
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
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let state = tree.state.downcast_mut::<CarouselState>();
        let child_nodes: Vec<_> = self
            .children
            .iter_mut()
            .enumerate()
            .map(|(i, child)| {
                child
                    .as_widget_mut()
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

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        _layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _renderer: &Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_mut::<CarouselState>();
        if let Event::Window(window::Event::RedrawRequested(now)) = event {
            if now.duration_since(state.last_transition) > self.interval {
                state.last_transition = *now;
                state.current = (state.current + 1) % self.children.len();
            }
            shell.request_redraw();
        }
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

impl<'a, Message, Theme> From<Carousel<'a, Message, Theme>> for IcedElement<'a, Message, Theme>
where
    Message: 'a + Clone,
    Theme: 'a,
{
    fn from(carousel: Carousel<'a, Message, Theme>) -> Self {
        IcedElement::new(carousel)
    }
}

/// Create a `Carousel` that types out the given `content` one character
/// at a time.
///
/// If `show_empty` is `true`, the text will begin with an empty string.
///
/// `interval` is how long to wait before the next character appears.
///
/// `text_builder` is used to build each `Text` element with the required
/// style etc.
pub fn typing_text_carousel<'a, Message, Theme>(
    content: &'a str,
    show_empty: bool,
    interval: Duration,
    text_builder: impl Fn(&'a str) -> iced::widget::Text<'a, Theme, Renderer>,
) -> Carousel<'a, Message, Theme>
where
    Theme: 'a + iced::widget::text::Catalog,
{
    let mut children = Vec::new();
    if show_empty {
        children.push(text_builder(""));
    }
    for end_char in 0..content.chars().count() {
        children.push(text_builder(&content[0..=end_char]));
    }
    Carousel::new(interval, children)
}
