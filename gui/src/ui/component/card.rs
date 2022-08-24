use iced::pure::{container, widget, Element};

use crate::ui::color;

pub fn simple<'a, T: 'a, C: Into<Element<'a, T>>>(content: C) -> widget::Container<'a, T> {
    container(content).padding(15).style(SimpleCardStyle)
}

pub struct SimpleCardStyle;
impl widget::container::StyleSheet for SimpleCardStyle {
    fn style(&self) -> widget::container::Style {
        widget::container::Style {
            border_radius: 10.0,
            background: color::FOREGROUND.into(),
            ..widget::container::Style::default()
        }
    }
}
