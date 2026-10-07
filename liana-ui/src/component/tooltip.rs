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

/// Like [`tooltip_custom`] but the help content stays reachable, so it can hold
/// a link or a button. See [`crate::widget::hover_tooltip`].
pub fn tooltip_interactive<'a, T: 'a>(
    help: impl Into<Element<'a, T>>,
    content: impl Into<Element<'a, T>>,
) -> Element<'a, T> {
    let help = Container::new(help).padding(10).style(theme::card::simple);
    crate::widget::hover_tooltip::HoverTooltip::new(content, help).into()
}

// pub fn time(theme: &Theme) -> Style {
//     Style {
//         color: Some(theme.colors.text.time),
//     }
// }
