use chrono::NaiveDateTime;

use iced::{alignment, Alignment, Length};

use liana::miniscript::bitcoin;
use liana_ui::{
    color,
    component::{badge, card, text::*},
    icon, theme,
    util::Collection,
    widget::*,
};

use crate::{
    app::{
        cache::Cache,
        view::{message::Message, util::*},
    },
    daemon::model::HistoryTransaction,
};

pub const HISTORY_EVENT_PAGE_SIZE: u64 = 20;

pub fn home_view<'a>(
    balance: &'a bitcoin::Amount,
    recovery_warning: Option<&(bitcoin::Amount, usize)>,
    recovery_alert: Option<&(bitcoin::Amount, usize)>,
    pending_events: &[HistoryTransaction],
    events: &Vec<HistoryTransaction>,
) -> Element<'a, Message> {
    Column::new()
        .push(amount_with_size(balance, 50))
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
        .align_items(Alignment::Center)
        .spacing(20)
        .into()
}

fn event_list_view<'a>(i: usize, event: &HistoryTransaction) -> Element<'a, Message> {
    Container::new(
        Button::new(
            Row::new()
                .push(
                    Row::new()
                        .push(if event.is_external() {
                            badge::receive()
                        } else {
                            badge::spend()
                        })
                        .push(if let Some(t) = event.time {
                            Container::new(
                                text(format!(
                                    "{}",
                                    NaiveDateTime::from_timestamp_opt(t as i64, 0).unwrap(),
                                ))
                                .small(),
                            )
                        } else {
                            badge::unconfirmed()
                        })
                        .spacing(10)
                        .align_items(Alignment::Center)
                        .width(Length::Fill),
                )
                .push(if event.is_external() {
                    Row::new()
                        .spacing(5)
                        .push(text("+"))
                        .push(amount(&event.incoming_amount))
                        .align_items(Alignment::Center)
                } else {
                    Row::new()
                        .spacing(5)
                        .push(text("-"))
                        .push(amount(&event.outgoing_amount))
                        .align_items(Alignment::Center)
                })
                .align_items(Alignment::Center)
                .spacing(20),
        )
        .padding(10)
        .on_press(Message::Select(i))
        .style(theme::Button::TransparentBorder),
    )
    .style(theme::Container::Card(theme::Card::Simple))
    .into()
}

pub fn event_view<'a>(cache: &Cache, event: &'a HistoryTransaction) -> Element<'a, Message> {
    Column::new()
        .push(
            Row::new()
                .push(if event.is_external() {
                    badge::receive()
                } else {
                    badge::spend()
                })
                .spacing(10)
                .align_items(Alignment::Center),
        )
        .push(if event.is_external() {
            amount_with_size(&event.incoming_amount, 50)
        } else {
            amount_with_size(&event.outgoing_amount, 50)
        })
        .push_maybe(
            event
                .fee_amount
                .map(|fee| Row::new().push(text("Miner Fee: ")).push(amount(&fee))),
        )
        .push(card::simple(
            Column::new()
                .push_maybe(event.time.map(|t| {
                    let date = NaiveDateTime::from_timestamp_opt(t as i64, 0).unwrap();
                    Row::new()
                        .width(Length::Fill)
                        .push(Container::new(text("Date:").bold()).width(Length::Fill))
                        .push(Container::new(text(format!("{}", date))).width(Length::Shrink))
                }))
                .push(
                    Row::new()
                        .width(Length::Fill)
                        .align_items(Alignment::Center)
                        .push(Container::new(text("Txid:").bold()).width(Length::Fill))
                        .push(
                            Row::new()
                                .align_items(Alignment::Center)
                                .push(Container::new(text(format!("{}", event.tx.txid())).small()))
                                .push(
                                    Button::new(icon::clipboard_icon())
                                        .on_press(Message::Clipboard(event.tx.txid().to_string()))
                                        .style(theme::Button::TransparentBorder),
                                )
                                .width(Length::Shrink),
                        ),
                )
                .spacing(5),
        ))
        .push(super::spend::detail::inputs_and_outputs_view(
            &event.coins,
            &event.tx,
            cache.network,
            if event.is_external() {
                None
            } else {
                Some(event.change_indexes.clone())
            },
            if event.is_external() {
                Some(event.change_indexes.clone())
            } else {
                None
            },
        ))
        .align_items(Alignment::Center)
        .spacing(20)
        .max_width(800)
        .into()
}
