use crate::{
    color,
    component::{amount, badge, text},
    theme,
    util::Collection,
    widget::*,
};
use bitcoin::Amount;
use iced::{
    widget::{button, row},
    Alignment, Length,
};

pub fn unconfirmed_outgoing_event<'a, T: Clone + 'a>(
    label: Option<iced::widget::Text<'a, iced::Renderer<theme::Theme>>>,
    amount: &Amount,
    msg: T,
) -> Container<'a, T> {
    Container::new(
        button(
            row!(
                row!(
                    badge::spend(),
                    Column::new().push_maybe(label).push(badge::unconfirmed())
                )
                .spacing(10)
                .align_items(Alignment::Center)
                .width(Length::Fill),
                row!(text::p1_regular("-"), amount::amount(amount))
                    .spacing(5)
                    .align_items(Alignment::Center),
            )
            .align_items(Alignment::Center)
            .padding(5)
            .spacing(20),
        )
        .on_press(msg)
        .style(theme::Button::TransparentBorder),
    )
    .style(theme::Container::Card(theme::Card::Simple))
}

pub fn confirmed_outgoing_event<'a, T: Clone + 'a>(
    label: Option<iced::widget::Text<'a, iced::Renderer<theme::Theme>>>,
    date: chrono::NaiveDateTime,
    amount: &Amount,
    msg: T,
) -> Container<'a, T> {
    Container::new(
        button(
            row!(
                row!(
                    badge::spend(),
                    Column::new().push_maybe(label).push(
                        text::p2_regular(date.format("%b. %d, %Y - %T").to_string())
                            .style(color::GREY_3)
                    )
                )
                .spacing(10)
                .align_items(Alignment::Center)
                .width(Length::Fill),
                row!(text::p1_regular("-"), amount::amount(amount))
                    .spacing(5)
                    .align_items(Alignment::Center),
            )
            .align_items(Alignment::Center)
            .padding(5)
            .spacing(20),
        )
        .on_press(msg)
        .style(theme::Button::TransparentBorder),
    )
    .style(theme::Container::Card(theme::Card::Simple))
}

pub fn unconfirmed_incoming_event<'a, T: Clone + 'a>(
    label: Option<iced::widget::Text<'a, iced::Renderer<theme::Theme>>>,
    amount: &Amount,
    msg: T,
) -> Container<'a, T> {
    Container::new(
        button(
            row!(
                row!(
                    badge::receive(),
                    Column::new().push_maybe(label).push(badge::unconfirmed())
                )
                .spacing(10)
                .align_items(Alignment::Center)
                .width(Length::Fill),
                row!(text::p1_regular("+"), amount::amount(amount))
                    .spacing(5)
                    .align_items(Alignment::Center),
            )
            .align_items(Alignment::Center)
            .padding(5)
            .spacing(20),
        )
        .on_press(msg)
        .style(theme::Button::TransparentBorder),
    )
    .style(theme::Container::Card(theme::Card::Simple))
}

pub fn confirmed_incoming_event<'a, T: Clone + 'a>(
    label: Option<iced::widget::Text<'a, iced::Renderer<theme::Theme>>>,
    date: chrono::NaiveDateTime,
    amount: &Amount,
    msg: T,
) -> Container<'a, T> {
    Container::new(
        button(
            row!(
                row!(
                    badge::receive(),
                    Column::new().push_maybe(label).push(
                        text::p2_regular(date.format("%b. %d, %Y - %T").to_string())
                            .style(color::GREY_3)
                    )
                )
                .spacing(10)
                .align_items(Alignment::Center)
                .width(Length::Fill),
                row!(text::p1_regular("+"), amount::amount(amount))
                    .spacing(5)
                    .align_items(Alignment::Center),
            )
            .align_items(Alignment::Center)
            .padding(5)
            .spacing(20),
        )
        .on_press(msg)
        .style(theme::Button::TransparentBorder),
    )
    .style(theme::Container::Card(theme::Card::Simple))
}
