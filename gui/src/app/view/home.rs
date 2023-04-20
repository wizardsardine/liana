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

use crate::{app::view::message::Message, daemon::model::HistoryTransaction};

pub const HISTORY_EVENT_PAGE_SIZE: u64 = 20;

pub fn home_view<'a>(
    balance: &'a bitcoin::Amount,
    recovery_warning: Option<&(bitcoin::Amount, usize)>,
    recovery_alert: Option<&(bitcoin::Amount, usize)>,
    pending_events: &[HistoryTransaction],
    events: &Vec<HistoryTransaction>,
) -> Element<'a, Message> {
    Column::new()
        .push(h3("Balance"))
        .push(amount_with_size(balance, H1_SIZE))
        .push_maybe(recovery_warning.map(|(a, c)| {
            Row::new()
                .spacing(15)
                .align_items(Alignment::Center)
                .push(icon::hourglass_icon().size(30).style(color::ORANGE))
                .push(
                    Row::new()
                        .spacing(5)
                        .push(text(format!(
                            "Recovery path will be soon available for {} coins",
                            c
                        )))
                        .push(text("("))
                        .push(amount(a))
                        .push(text(")")),
                )
                .padding(10)
        }))
        .push_maybe(recovery_alert.map(|(a, c)| {
            Row::new()
                .spacing(15)
                .align_items(Alignment::Center)
                .push(icon::hourglass_done_icon().style(color::RED))
                .push(
                    Row::new()
                        .spacing(5)
                        .push(text(format!("Recovery path is available for {} coins", c)))
                        .push(text("("))
                        .push(amount(a))
                        .push(text(")")),
                )
                .padding(10)
        }))
        .push(
            Column::new()
                .spacing(10)
                .push(h4_bold("Last payments"))
                .push(
                    pending_events
                        .iter()
                        .enumerate()
                        .fold(Column::new().spacing(10), |col, (i, event)| {
                            col.push(event_list_view(i, event))
                        }),
                )
                .push(
                    events
                        .iter()
                        .filter(|event| !event.is_self_send())
                        .enumerate()
                        .fold(Column::new().spacing(10), |col, (i, event)| {
                            col.push(event_list_view(i + pending_events.len(), event))
                        }),
                )
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
