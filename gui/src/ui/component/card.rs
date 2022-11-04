use iced::pure::{container, row, tooltip, widget, Element};

use crate::ui::{color, component::text::text, icon};

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

/// display an error card with the message and the error in a tooltip.
pub fn warning<'a, T: 'a>(message: &str) -> widget::Container<'a, T> {
    container(
        row()
            .spacing(20)
            .align_items(iced::Alignment::Center)
            .push(icon::warning_octagon_icon().color(color::WARNING))
            .push(text(message).color(color::WARNING)),
    )
    .padding(15)
    .style(WarningCardStyle)
}

pub struct WarningCardStyle;
impl widget::container::StyleSheet for WarningCardStyle {
    fn style(&self) -> widget::container::Style {
        widget::container::Style {
            border_radius: 10.0,
            border_color: color::WARNING,
            border_width: 1.5,
            background: color::FOREGROUND.into(),
            ..widget::container::Style::default()
        }
    }
}

/// display an error card with the message and the error in a tooltip.
pub fn error<'a, T: 'a>(message: &str, error: &str) -> widget::Container<'a, T> {
    container(
        tooltip(
            row()
                .spacing(20)
                .align_items(iced::Alignment::Center)
                .push(icon::block_icon().color(color::ALERT))
                .push(text(message).color(color::ALERT)),
            error,
            widget::tooltip::Position::Bottom,
        )
        .style(ErrorCardStyle),
    )
    .padding(15)
    .style(ErrorCardStyle)
}

pub struct ErrorCardStyle;
impl widget::container::StyleSheet for ErrorCardStyle {
    fn style(&self) -> widget::container::Style {
        widget::container::Style {
            border_radius: 10.0,
            border_color: color::ALERT,
            border_width: 1.5,
            background: color::FOREGROUND.into(),
            ..widget::container::Style::default()
        }
    }
}
