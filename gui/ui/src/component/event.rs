use crate::{
    component::{amount, badge, text},
    theme,
    widget::*,
};
use bitcoin::Amount;
use iced::{
    widget::{button, row},
    Alignment, Length,
};

pub fn unconfirmed_outgoing_event<'a, T: Clone + 'a>(amount: &Amount, msg: T) -> Container<'a, T> {
    Container::new(
        button(
            row!(
                row!(badge::spend(), badge::unconfirmed())
                    .spacing(10)
                    .align_items(Alignment::Center)
                    .width(Length::Fill),
                row!(text::p1_regular("-"), amount::amount(amount))
                    .spacing(5)
                    .align_items(Alignment::Center),
            )
            .align_items(Alignment::Center)
            .padding(10)
            .spacing(20),
        )
        .on_press(msg)
        .style(theme::Button::TransparentBorder),
    )
    .style(theme::Container::Card(theme::Card::Simple))
}

pub fn confirmed_outgoing_event<'a, T: Clone + 'a>(
    date: chrono::NaiveDate,
    amount: &Amount,
    msg: T,
) -> Container<'a, T> {
    Container::new(
        button(
            row!(
                row!(badge::spend(), text::p2_regular(date.to_string()))
                    .spacing(10)
                    .align_items(Alignment::Center)
                    .width(Length::Fill),
                row!(text::p1_regular("-"), amount::amount(amount))
                    .spacing(5)
                    .align_items(Alignment::Center),
            )
            .align_items(Alignment::Center)
            .padding(10)
            .spacing(20),
        )
        .on_press(msg)
        .style(theme::Button::TransparentBorder),
    )
    .style(theme::Container::Card(theme::Card::Simple))
}

pub fn unconfirmed_incoming_event<'a, T: Clone + 'a>(amount: &Amount, msg: T) -> Container<'a, T> {
    Container::new(
        button(
            row!(
                row!(badge::receive(), badge::unconfirmed())
                    .spacing(10)
                    .align_items(Alignment::Center)
                    .width(Length::Fill),
                row!(text::p1_regular("+"), amount::amount(amount))
                    .spacing(5)
                    .align_items(Alignment::Center),
            )
            .align_items(Alignment::Center)
            .padding(10)
            .spacing(20),
        )
        .on_press(msg)
        .style(theme::Button::TransparentBorder),
    )
    .style(theme::Container::Card(theme::Card::Simple))
}

pub fn confirmed_incoming_event<'a, T: Clone + 'a>(
    date: chrono::NaiveDate,
    amount: &Amount,
    msg: T,
) -> Container<'a, T> {
    Container::new(
        button(
            row!(
                row!(badge::receive(), text::p2_regular(date.to_string()))
                    .spacing(10)
                    .align_items(Alignment::Center)
                    .width(Length::Fill),
                row!(text::p1_regular("+"), amount::amount(amount))
                    .spacing(5)
                    .align_items(Alignment::Center),
            )
            .align_items(Alignment::Center)
            .padding(10)
            .spacing(20),
        )
        .on_press(msg)
        .style(theme::Button::TransparentBorder),
    )
    .style(theme::Container::Card(theme::Card::Simple))
}
