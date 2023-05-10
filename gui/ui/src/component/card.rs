use crate::{color, component::text::text, icon, theme, widget::*};

pub fn simple<'a, T: 'a, C: Into<Element<'a, T>>>(content: C) -> Container<'a, T> {
    Container::new(content)
        .padding(15)
        .style(theme::Container::Card(theme::Card::Simple))
}

pub fn invalid<'a, T: 'a, C: Into<Element<'a, T>>>(content: C) -> Container<'a, T> {
    Container::new(content)
        .padding(15)
        .style(theme::Container::Card(theme::Card::Invalid))
}

/// display an error card with the message and the error in a tooltip.
pub fn warning<'a, T: 'a>(message: String) -> Container<'a, T> {
    Container::new(
        Row::new()
            .spacing(20)
            .align_items(iced::Alignment::Center)
            .push(icon::warning_icon())
            .push(text(message)),
    )
    .padding(15)
    .style(theme::Container::Card(theme::Card::Warning))
}

/// display an error card with the message and the error in a tooltip.
pub fn error<'a, T: 'a>(message: &'static str, error: String) -> Container<'a, T> {
    Container::new(
        iced::widget::tooltip::Tooltip::new(
            Row::new()
                .spacing(20)
                .align_items(iced::Alignment::Center)
                .push(icon::warning_icon().style(color::RED))
                .push(text(message).style(color::RED)),
            error,
            iced::widget::tooltip::Position::Bottom,
        )
        .style(theme::Container::Card(theme::Card::Error)),
    )
    .padding(15)
    .style(theme::Container::Card(theme::Card::Error))
}
