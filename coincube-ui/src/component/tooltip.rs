use crate::{icon, theme, widget::Container};

/// Maximum width (logical pixels) of the tooltip bubble. Forces long
/// copy to wrap instead of extending off the edge of the window.
const TOOLTIP_MAX_WIDTH: f32 = 320.0;

pub fn tooltip<'a, T: 'a>(help: &'a str) -> Container<'a, T> {
    // Wrap the help string inside a sized container so long copy soft-wraps
    // at `TOOLTIP_MAX_WIDTH` rather than shooting off the edge of the modal.
    let tip = Container::new(iced::widget::text(help).size(14))
        .max_width(TOOLTIP_MAX_WIDTH)
        .padding([6, 10])
        .style(theme::card::simple);

    Container::new(iced::widget::tooltip::Tooltip::new(
        icon::tooltip_icon(),
        tip,
        iced::widget::tooltip::Position::Right,
    ))
}
