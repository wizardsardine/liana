use crate::{icon, theme, widget::*};

use iced::widget::tooltip::Position;

pub fn tooltip<T: 'static>(help: &'static str) -> Container<'static, T> {
    tooltip_custom(help, icon::tooltip_icon(), Position::Right)
}

pub fn tooltip_custom<T: 'static>(
    help: &'static str,
    content: impl Into<Element<'static, T>>,
    position: Position,
) -> Container<'static, T> {
    Container::new(
        iced::widget::tooltip::Tooltip::new(content, help, position).style(theme::card::simple),
    )
}
