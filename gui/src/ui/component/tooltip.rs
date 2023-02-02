use crate::ui::{color, icon};
use iced::widget::{self, Tooltip};

pub fn tooltip<'a, T: 'a>(help: &'static str) -> Tooltip<'a, T> {
    Tooltip::new(
        icon::tooltip_icon().style(color::DARK_GREY),
        help,
        widget::tooltip::Position::Right,
    )
    .style(TooltipStyle)
}
pub struct TooltipStyle;
impl widget::container::StyleSheet for TooltipStyle {
    type Style = iced::Theme;
    fn appearance(&self, _style: &Self::Style) -> widget::container::Appearance {
        widget::container::Appearance {
            border_radius: 10.0,
            border_color: color::DARK_GREY,
            border_width: 1.5,
            background: color::FOREGROUND.into(),
            ..widget::container::Appearance::default()
        }
    }
}

impl From<TooltipStyle> for Box<dyn widget::container::StyleSheet<Style = iced::Theme>> {
    fn from(s: TooltipStyle) -> Box<dyn widget::container::StyleSheet<Style = iced::Theme>> {
        Box::new(s)
    }
}

impl From<TooltipStyle> for iced::theme::Container {
    fn from(i: TooltipStyle) -> iced::theme::Container {
        iced::theme::Container::Custom(i.into())
    }
}
