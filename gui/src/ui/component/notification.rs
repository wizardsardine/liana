use crate::ui::{color, icon};
use iced::{
    widget::{container, tooltip, Container, Row, Text, Tooltip},
    Length,
};

pub fn warning<'a, T: 'a>(message: String, error: String) -> Container<'a, T> {
    Container::new(Container::new(
        Tooltip::new(
            Row::new()
                .push(icon::warning_icon())
                .push(Text::new(message))
                .spacing(20),
            error,
            tooltip::Position::Bottom,
        )
        .style(TooltipWarningStyle),
    ))
    .padding(15)
    .center_x()
    .style(WarningStyle)
    .width(Length::Fill)
}

pub struct WarningStyle;
impl container::StyleSheet for WarningStyle {
    type Style = iced::Theme;
    fn appearance(&self, _style: &Self::Style) -> container::Appearance {
        container::Appearance {
            border_radius: 0.0,
            text_color: iced::Color::BLACK.into(),
            background: color::WARNING.into(),
            border_color: color::WARNING,
            ..container::Appearance::default()
        }
    }
}

impl From<WarningStyle> for Box<dyn container::StyleSheet<Style = iced::Theme>> {
    fn from(s: WarningStyle) -> Box<dyn container::StyleSheet<Style = iced::Theme>> {
        Box::new(s)
    }
}

impl From<WarningStyle> for iced::theme::Container {
    fn from(i: WarningStyle) -> iced::theme::Container {
        iced::theme::Container::Custom(i.into())
    }
}

pub struct TooltipWarningStyle;
impl container::StyleSheet for TooltipWarningStyle {
    type Style = iced::Theme;
    fn appearance(&self, _style: &Self::Style) -> container::Appearance {
        container::Appearance {
            border_radius: 0.0,
            border_width: 1.0,
            text_color: color::WARNING.into(),
            background: color::FOREGROUND.into(),
            border_color: color::WARNING,
        }
    }
}

impl From<TooltipWarningStyle> for Box<dyn container::StyleSheet<Style = iced::Theme>> {
    fn from(s: TooltipWarningStyle) -> Box<dyn container::StyleSheet<Style = iced::Theme>> {
        Box::new(s)
    }
}

impl From<TooltipWarningStyle> for iced::theme::Container {
    fn from(i: TooltipWarningStyle) -> iced::theme::Container {
        iced::theme::Container::Custom(i.into())
    }
}
