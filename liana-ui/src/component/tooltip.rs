use crate::{icon, theme, widget::*};

use iced::widget::tooltip::Position;

pub fn tooltip<'a, T: 'a>(help: &'a str) -> Container<'a, T> {
    tooltip_custom(help, icon::tooltip_icon(), Position::Right)
}

pub fn tooltip_custom<'a, T: 'a>(
    help: &'a str,
    content: impl Into<Element<'a, T>>,
    position: Position,
) -> Container<'a, T> {
    Container::new(
        iced::widget::tooltip::Tooltip::new(content, help, position).style(theme::card::simple),
    )
}
