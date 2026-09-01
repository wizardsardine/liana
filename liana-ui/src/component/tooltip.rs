use crate::{
    icon,
    theme::{self, Theme},
    widget::*,
};

use iced::widget::{text::Style, tooltip::Position};

pub fn tooltip_with_style<'a, T: 'a>(
    help: impl Into<String>,
    icon_style: fn(&Theme) -> Style,
) -> Container<'a, T> {
    tooltip_custom(
        iced::widget::text(help.into()),
        icon::tooltip_icon().style(icon_style),
        Position::Top,
    )
}

pub fn tooltip<'a, T: 'a>(help: impl Into<String>) -> Container<'a, T> {
    tooltip_custom(
        iced::widget::text(help.into()),
        icon::tooltip_icon(),
        Position::Right,
    )
}

pub fn tooltip_custom<'a, T: 'a>(
    help: impl Into<Element<'a, T>>,
    content: impl Into<Element<'a, T>>,
    position: Position,
) -> Container<'a, T> {
    Container::new(
        iced::widget::tooltip::Tooltip::new(content, help, position).style(theme::card::simple),
    )
}

// pub fn time(theme: &Theme) -> Style {
//     Style {
//         color: Some(theme.colors.text.time),
//     }
// }
