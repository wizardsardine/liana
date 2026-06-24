use iced::widget::scrollable::{Direction, Scrollbar};

use crate::widget::{Element, Scrollable};

const SPACING: f32 = 12.0;
const THIN_SPACING: f32 = 7.0;
const THIN_SCROLLBAR_WIDTH: u32 = 5;

fn thin_scrollbar() -> Scrollbar {
    Scrollbar::default()
        .width(THIN_SCROLLBAR_WIDTH)
        .scroller_width(THIN_SCROLLBAR_WIDTH)
}

pub fn horizontal<'a, M: 'a>(content: impl Into<Element<'a, M>>) -> Scrollable<'a, M> {
    Scrollable::new(content)
        .direction(Direction::Horizontal(Scrollbar::default()))
        .spacing(SPACING)
}

pub fn vertical<'a, M: 'a>(content: impl Into<Element<'a, M>>) -> Scrollable<'a, M> {
    Scrollable::new(content)
        .direction(Direction::Vertical(Scrollbar::default()))
        .spacing(SPACING)
}

pub fn horizontal_thin<'a, M: 'a>(content: impl Into<Element<'a, M>>) -> Scrollable<'a, M> {
    Scrollable::new(content)
        .direction(Direction::Horizontal(thin_scrollbar()))
        .spacing(THIN_SPACING)
}

pub fn vertical_thin<'a, M: 'a>(content: impl Into<Element<'a, M>>) -> Scrollable<'a, M> {
    Scrollable::new(content)
        .direction(Direction::Vertical(thin_scrollbar()))
        .spacing(THIN_SPACING)
}
