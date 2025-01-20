use crate::{color, component::text::text, icon, theme, widget::*};

pub fn modal<'a, T: 'a, C: Into<Element<'a, T>>>(content: C) -> Container<'a, T> {
    Container::new(content)
        .padding(15)
        .style(theme::card::modal)
}

pub fn simple<'a, T: 'a, C: Into<Element<'a, T>>>(content: C) -> Container<'a, T> {
    Container::new(content)
        .padding(15)
        .style(theme::card::simple)
}

pub fn invalid<'a, T: 'a, C: Into<Element<'a, T>>>(content: C) -> Container<'a, T> {
    Container::new(content)
        .padding(15)
        .style(theme::card::invalid)
}

/// display an error card with the message and the error in a tooltip.
pub fn warning<'a, T: 'a>(message: String) -> Container<'a, T> {
    Container::new(
        Row::new()
            .spacing(20)
            .align_y(iced::Alignment::Center)
            .push(icon::warning_icon())
            .push(text(message)),
    )
    .padding(15)
    .style(theme::card::warning)
}

/// display an error card with the message and the error in a tooltip.
pub fn error<'a, T: 'a>(message: &'static str, error: String) -> Container<'a, T> {
    Container::new(
        iced::widget::tooltip::Tooltip::new(
            Row::new()
                .spacing(20)
                .align_y(iced::Alignment::Center)
                .push(icon::warning_icon().color(color::RED))
                .push(text(message).color(color::RED)),
            Text::new(error),
            iced::widget::tooltip::Position::Bottom,
        )
        .style(theme::card::error),
    )
    .padding(15)
    .style(theme::card::error)
}
