use crate::{
    component::{amount, badge, card, text},
    theme,
    widget::*,
};
use bitcoin::Amount;
use iced::{widget::row, Alignment, Length};

use chrono::{DateTime, Local, Utc};

pub fn event_card<'a, M>(content: Row<'a, M>, msg: M) -> Element<'a, M>
where
    M: Clone + 'a,
{
    let content = content.align_y(Alignment::Center).padding(5).spacing(20);
    card::clickable_card(content, Some(msg))
}

pub fn unconfirmed_outgoing_event<T: Clone + 'static>(
    label: Option<Text<'static>>,
    amount: &Amount,
    msg: T,
) -> Element<'static, T> {
    let content = row!(
        row!(badge::spend(), Column::new().push_maybe(label),)
            .spacing(10)
            .align_y(Alignment::Center)
            .width(Length::Fill),
        badge::unconfirmed(),
        row!(text::p1_regular("-"), amount::amount(amount))
            .spacing(5)
            .align_y(Alignment::Center),
    );
    event_card(content, msg)
}

pub fn confirmed_outgoing_event<T: Clone + 'static>(
    label: Option<Text<'static>>,
    date: DateTime<Utc>,
    amount: &Amount,
    msg: T,
) -> Element<'static, T> {
    let content = row!(
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
    );
    event_card(content, msg)
}

pub fn unconfirmed_incoming_event<T: Clone + 'static>(
    label: Option<Text<'static>>,
    amount: &Amount,
    msg: T,
) -> Element<'static, T> {
    let content = row!(
        row!(badge::receive(), Column::new().push_maybe(label))
            .spacing(10)
            .align_y(Alignment::Center)
            .width(Length::Fill),
        badge::unconfirmed(),
        row!(text::p1_regular("+"), amount::amount(amount))
            .spacing(5)
            .align_y(Alignment::Center),
    );
    event_card(content, msg)
}

pub fn confirmed_incoming_event<T: Clone + 'static>(
    label: Option<Text<'static>>,
    date: DateTime<Utc>,
    amount: &Amount,
    msg: T,
) -> Element<'static, T> {
    let content = row!(
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
    );
    event_card(content, msg)
}
