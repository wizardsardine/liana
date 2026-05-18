use iced::{
    widget::scrollable::{Direction, Scrollbar},
    Padding,
};

use crate::widget::{Container, Element, Scrollable};

const PADDING: f32 = 12.0;
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

pub fn horizontal<'a, M: 'a>(content: impl Into<Element<'a, M>>) -> Scrollable<'a, M> {
    let content = Container::new(content).padding(BOTTOM_PADDING);
    Scrollable::new(content).direction(Direction::Horizontal(Scrollbar::default()))
}

pub fn vertical<'a, M: 'a>(content: impl Into<Element<'a, M>>) -> Scrollable<'a, M> {
    let content = Container::new(content).padding(RIGHT_PADDING);
    Scrollable::new(content).direction(Direction::Vertical(Scrollbar::default()))
}
