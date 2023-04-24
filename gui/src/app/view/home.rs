use chrono::NaiveDateTime;

use iced::{alignment, Alignment, Length};

use liana::miniscript::bitcoin;
use liana_ui::{
    color,
    component::{amount::*, event, text::*},
    icon, theme,
    util::Collection,
    widget::*,
};

use crate::{
    app::view::{coins, message::Message},
    daemon::model::HistoryTransaction,
};

pub const HISTORY_EVENT_PAGE_SIZE: u64 = 20;

pub fn home_view<'a>(
    balance: &'a bitcoin::Amount,
    unconfirmed_balance: &'a bitcoin::Amount,
    remaining_sequence: &Option<u32>,
    number_of_expiring_coins: usize,
    pending_events: &[HistoryTransaction],
    events: &Vec<HistoryTransaction>,
) -> Element<'a, Message> {
    Column::new()
        .push(h3("Balance"))
        .push(
            Column::new()
                .push(amount_with_size(balance, H1_SIZE))
                .push_maybe(if unconfirmed_balance.to_sat() != 0 {
                    Some(
                        Row::new()
                            .spacing(10)
                            .push(text("+").size(H3_SIZE).style(color::GREY_3))
                            .push(unconfirmed_amount_with_size(unconfirmed_balance, H3_SIZE))
                            .push(text("unconfirmed").size(H3_SIZE).style(color::GREY_3)),
                    )
                } else {
                    None
                }),
        )
        .push_maybe(if number_of_expiring_coins == 0 {
            remaining_sequence.map(|sequence| {
                Container::new(
                    Row::new()
                        .spacing(15)
                        .align_items(Alignment::Center)
                        .push(
                            h4_regular(format!(
                                "Your next coin to expire will in ≈ {}",
                                coins::expire_message_units(sequence).join(",")
                            ))
                            .width(Length::Fill),
                        )
                        .push(
                            icon::tooltip_icon()
                                .size(20)
                                .style(color::GREY_3)
                                .width(Length::Units(20)),
                        )
                        .width(Length::Fill),
                )
                .padding(25)
                .style(theme::Card::Border)
            })
        } else {
            Some(
                Container::new(
                    Row::new().spacing(15).align_items(Alignment::Center).push(
                        h4_regular(format!(
                            "You have {} coins that are already or about to be expired",
                            number_of_expiring_coins
                        ))
                        .width(Length::Fill),
                    ),
                )
                .padding(25)
                .style(theme::Card::Invalid),
            )
        })
        .push(
            Column::new()
                .spacing(10)
                .push(h4_bold("Last payments"))
                .push(pending_events.iter().enumerate().fold(
                    Column::new().spacing(10),
                    |col, (i, event)| {
                        if !event.is_self_send() {
                            col.push(event_list_view(i, event))
                        } else {
                            col
                        }
                    },
                ))
                .push(events.iter().enumerate().fold(
                    Column::new().spacing(10),
                    |col, (i, event)| {
                        if !event.is_self_send() {
                            col.push(event_list_view(i + pending_events.len(), event))
                        } else {
                            col
                        }
                    },
                ))
                .push_maybe(
                    if events.len() % HISTORY_EVENT_PAGE_SIZE as usize == 0 && !events.is_empty() {
                        Some(
                            Container::new(
                                Button::new(
                                    text("See more")
                                        .width(Length::Fill)
                                        .horizontal_alignment(alignment::Horizontal::Center),
                                )
                                .width(Length::Fill)
                                .padding(15)
                                .style(theme::Button::TransparentBorder)
                                .on_press(Message::Next),
                            )
                            .width(Length::Fill)
                            .style(theme::Container::Card(theme::Card::Simple)),
                        )
                    } else {
                        None
                    },
                ),
        )
        .spacing(20)
        .into()
}

fn event_list_view<'a>(i: usize, event: &HistoryTransaction) -> Column<'a, Message> {
    event.tx.output.iter().enumerate().fold(
        Column::new().spacing(10),
        |col, (output_index, output)| {
            if event.is_external() {
                if !event.change_indexes.contains(&output_index) {
                    col
                } else if let Some(t) = event.time {
                    col.push(event::confirmed_incoming_event(
                        NaiveDateTime::from_timestamp_opt(t as i64, 0).unwrap(),
                        &Amount::from_sat(output.value),
                        Message::Select(i),
                    ))
                } else {
                    col.push(event::unconfirmed_incoming_event(
                        &Amount::from_sat(output.value),
                        Message::Select(i),
                    ))
                }
            } else if event.change_indexes.contains(&output_index) {
                col
            } else if let Some(t) = event.time {
                col.push(event::confirmed_outgoing_event(
                    NaiveDateTime::from_timestamp_opt(t as i64, 0).unwrap(),
                    &Amount::from_sat(output.value),
                    Message::Select(i),
                ))
            } else {
                col.push(event::unconfirmed_outgoing_event(
                    &Amount::from_sat(output.value),
                    Message::Select(i),
                ))
            }
        },
    )
}
