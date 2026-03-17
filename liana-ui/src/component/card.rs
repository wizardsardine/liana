use crate::{color, component::text::text, icon, theme, widget::*};
use iced::{widget::button, Alignment};
const CARD_PADDING: [u16; 2] = [15, 30];

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

pub fn clickable_card<'a, M>(content: Row<'a, M>, msg: Option<M>) -> Container<'a, M>
where
    M: Clone + 'a,
{
    Container::new(
        button(content.align_y(Alignment::Center).padding(CARD_PADDING))
            .on_press_maybe(msg)
            .style(theme::button::transparent_border),
    )
    .style(theme::card::simple)
}
