use iced::widget::tooltip::Position;

use crate::{
    icon, theme,
    widget::{Container, Element},
};

/// Maximum width (logical pixels) of the tooltip bubble. Forces long
/// copy to wrap instead of extending off the edge of the window.
const TOOLTIP_MAX_WIDTH: f32 = 320.0;

pub fn tooltip<'a, T: 'a>(help: &'a str) -> Container<'a, T> {
    tooltip_custom(help, icon::tooltip_icon(), Position::Right)
}

/// A tooltip whose hover trigger and position can be customised (e.g. a warning
/// icon anchored below), while keeping the soft-wrapping bubble used by [`tooltip`].
pub fn tooltip_custom<'a, T: 'a>(
    help: &'a str,
    content: impl Into<Element<'a, T>>,
    position: Position,
) -> Container<'a, T> {
    // Wrap the help string inside a sized container so long copy soft-wraps
    // at `TOOLTIP_MAX_WIDTH` rather than shooting off the edge of the modal.
    let tip = Container::new(iced::widget::text(help).size(14))
        .max_width(TOOLTIP_MAX_WIDTH)
        .padding([6, 10])
        .style(theme::card::simple);

    Container::new(iced::widget::tooltip::Tooltip::new(content, tip, position))
}
