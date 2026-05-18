use iced::{
    widget::scrollable::{Direction, Scrollbar},
    Padding,
};

use crate::widget::{Container, Element, Scrollable};

const PADDING: f32 = 12.0;
const THIN_PADDING: f32 = 7.0;
const THIN_SCROLLBAR_WIDTH: u32 = 5;

const BOTTOM_PADDING: Padding = Padding {
    top: 0.0,
    right: 0.0,
    bottom: 10.0,
    left: PADDING,
};

const RIGHT_PADDING: Padding = Padding {
    top: 0.0,
    right: 0.0,
    bottom: PADDING,
    left: 0.0,
};

const THIN_BOTTOM_PADDING: Padding = Padding {
    top: 0.0,
    right: 0.0,
    bottom: THIN_PADDING,
    left: 0.0,
};

const THIN_RIGHT_PADDING: Padding = Padding {
    top: 0.0,
    right: THIN_PADDING,
    bottom: 0.0,
    left: 0.0,
};

fn thin_scrollbar() -> Scrollbar {
    Scrollbar::default()
        .width(THIN_SCROLLBAR_WIDTH)
        .scroller_width(THIN_SCROLLBAR_WIDTH)
}

pub fn horizontal<'a, M: 'a>(content: impl Into<Element<'a, M>>) -> Scrollable<'a, M> {
    let content = Container::new(content).padding(BOTTOM_PADDING);
    Scrollable::new(content).direction(Direction::Horizontal(Scrollbar::default()))
}

pub fn vertical<'a, M: 'a>(content: impl Into<Element<'a, M>>) -> Scrollable<'a, M> {
    let content = Container::new(content).padding(RIGHT_PADDING);
    Scrollable::new(content).direction(Direction::Vertical(Scrollbar::default()))
}

pub fn horizontal_thin<'a, M: 'a>(content: impl Into<Element<'a, M>>) -> Scrollable<'a, M> {
    let content = Container::new(content).padding(THIN_BOTTOM_PADDING);
    Scrollable::new(content).direction(Direction::Horizontal(thin_scrollbar()))
}

pub fn vertical_thin<'a, M: 'a>(content: impl Into<Element<'a, M>>) -> Scrollable<'a, M> {
    let content = Container::new(content).padding(THIN_RIGHT_PADDING);
    Scrollable::new(content).direction(Direction::Vertical(thin_scrollbar()))
}
