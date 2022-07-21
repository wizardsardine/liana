use crate::{color, icon};
use iced::{container, tooltip, Container, Length, Row, Text, Tooltip};

pub fn warning<'a, T: 'a>(message: &str, error: &str) -> Container<'a, T> {
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

struct WarningStyle;
impl container::StyleSheet for WarningStyle {
    fn style(&self) -> container::Style {
        container::Style {
            border_radius: 0.0,
            text_color: iced::Color::BLACK.into(),
            background: color::WARNING.into(),
            border_color: color::WARNING,
            ..container::Style::default()
        }
    }
}

struct TooltipWarningStyle;
impl container::StyleSheet for TooltipWarningStyle {
    fn style(&self) -> container::Style {
        container::Style {
            border_radius: 0.0,
            border_width: 1.0,
            text_color: color::WARNING.into(),
            background: color::FOREGROUND.into(),
            border_color: color::WARNING,
        }
    }
}
