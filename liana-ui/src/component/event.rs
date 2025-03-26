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

use chrono::{DateTime, Local, Utc};

pub fn unconfirmed_outgoing_event<T: Clone + 'static>(
    label: Option<Text<'static>>,
    amount: &Amount,
    msg: T,
) -> Container<'static, T> {
    Container::new(
        button(
            row!(
                row!(badge::spend(), Column::new().push_maybe(label),)
                    .spacing(10)
                    .align_y(Alignment::Center)
                    .width(Length::Fill),
                badge::unconfirmed(),
                row!(text::p1_regular("-"), amount::amount(amount))
                    .spacing(5)
                    .align_y(Alignment::Center),
            )
            .align_y(Alignment::Center)
            .padding(5)
            .spacing(20),
        )
        .on_press(msg)
        .style(theme::button::transparent_border),
    )
    .style(theme::card::simple)
}

pub fn confirmed_outgoing_event<T: Clone + 'static>(
    label: Option<Text<'static>>,
    date: DateTime<Utc>,
    amount: &Amount,
    msg: T,
) -> Container<'static, T> {
    Container::new(
        button(
            row!(
                row!(
                    badge::spend(),
                    Column::new().push_maybe(label).push(
                        text::p2_regular(
                            date.with_timezone(&Local)
                                .format("%b. %d, %Y - %T")
                                .to_string()
                        )
                        .style(theme::text::secondary)
                    )
                )
                .spacing(10)
                .align_y(Alignment::Center)
                .width(Length::Fill),
                row!(text::p1_regular("-"), amount::amount(amount))
                    .spacing(5)
                    .align_y(Alignment::Center),
            )
            .align_y(Alignment::Center)
            .padding(5)
            .spacing(20),
        )
        .on_press(msg)
        .style(theme::button::transparent_border),
    )
    .style(theme::card::simple)
}

pub fn unconfirmed_incoming_event<T: Clone + 'static>(
    label: Option<Text<'static>>,
    amount: &Amount,
    msg: T,
) -> Container<'static, T> {
    Container::new(
        button(
            row!(
                row!(badge::receive(), Column::new().push_maybe(label))
                    .spacing(10)
                    .align_y(Alignment::Center)
                    .width(Length::Fill),
                badge::unconfirmed(),
                row!(text::p1_regular("+"), amount::amount(amount))
                    .spacing(5)
                    .align_y(Alignment::Center),
            )
            .align_y(Alignment::Center)
            .padding(5)
            .spacing(20),
        )
        .on_press(msg)
        .style(theme::button::transparent_border),
    )
    .style(theme::card::simple)
}

pub fn confirmed_incoming_event<T: Clone + 'static>(
    label: Option<Text<'static>>,
    date: DateTime<Utc>,
    amount: &Amount,
    msg: T,
) -> Container<'static, T> {
    Container::new(
        button(
            row!(
                row!(
                    badge::receive(),
                    Column::new().push_maybe(label).push(
                        text::p2_regular(
                            date.with_timezone(&Local)
                                .format("%b. %d, %Y - %T")
                                .to_string()
                        )
                        .style(theme::text::secondary)
                    )
                )
                .spacing(10)
                .align_y(Alignment::Center)
                .width(Length::Fill),
                row!(text::p1_regular("+"), amount::amount(amount))
                    .spacing(5)
                    .align_y(Alignment::Center),
            )
            .align_y(Alignment::Center)
            .padding(5)
            .spacing(20),
        )
        .on_press(msg)
        .style(theme::button::transparent_border),
    )
    .style(theme::card::simple)
}
