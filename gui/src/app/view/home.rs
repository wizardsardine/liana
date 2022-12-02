use chrono::NaiveDateTime;

use iced::{
    alignment,
    widget::{Button, Column, Container, Row},
    Alignment, Element, Length,
};

use crate::ui::{
    color,
    component::{badge, button::Style, card, text::*},
    icon,
    util::Collection,
};
use liana::miniscript::bitcoin;

use crate::{
    app::{cache::Cache, view::message::Message},
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
        .push(Column::new().padding(40))
        .push(text(format!("{} BTC", balance.to_btc())).bold().size(50))
        .push_maybe(recovery_warning.map(|(a, c)| {
            Row::new()
                .spacing(15)
                .align_items(Alignment::Center)
                .push(icon::hourglass_icon().size(30).style(color::WARNING))
                .push(Container::new(text(format!(
                    "Recovery path will be soon available for {} coins ( {} )",
                    c, a
                ))))
                .padding(10)
        }))
        .push_maybe(recovery_alert.map(|(a, c)| {
            Row::new()
                .spacing(15)
                .align_items(Alignment::Center)
                .push(icon::hourglass_done_icon().style(color::ALERT))
                .push(Container::new(text(format!(
                    "Recovery path is available for {} coins ( {} )",
                    c, a
                ))))
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
                            col.push(event_list_view(i, event))
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
                                .style(Style::TransparentBorder.into())
                                .on_press(Message::Next),
                            )
                            .width(Length::Fill)
                            .style(card::SimpleCardStyle),
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
                                text(format!("{}", NaiveDateTime::from_timestamp(t as i64, 0)))
                                    .small(),
                            )
                        } else {
                            Container::new(text("  Pending  ").small())
                                .padding(3)
                                .style(badge::PillStyle::Success)
                        })
                        .spacing(10)
                        .align_items(Alignment::Center)
                        .width(Length::Fill),
                )
                .push(
                    Row::new()
                        .push(
                            text({
                                if event.is_external() {
                                    format!("+ {:.8}", event.incoming_amount.to_btc())
                                } else {
                                    format!("- {:.8}", event.outgoing_amount.to_btc())
                                }
                            })
                            .bold()
                            .width(Length::Shrink),
                        )
                        .push(text("BTC"))
                        .spacing(5)
                        .align_items(Alignment::Center),
                )
                .align_items(Alignment::Center)
                .spacing(20),
        )
        .padding(10)
        .on_press(Message::Select(i))
        .style(Style::TransparentBorder.into()),
    )
    .style(card::SimpleCardStyle)
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
        .push(
            text({
                if event.is_external() {
                    format!("+ {} BTC", event.incoming_amount.to_btc())
                } else {
                    format!("- {} BTC", event.outgoing_amount.to_btc())
                }
            })
            .bold()
            .size(50)
            .width(Length::Shrink),
        )
        .push_maybe(
            event
                .fee_amount
                .map(|fee| Container::new(text(format!("Miner Fee: {} BTC", fee.to_btc())))),
        )
        .push(card::simple(
            Column::new()
                .push_maybe(event.time.map(|t| {
                    let date = NaiveDateTime::from_timestamp(t as i64, 0);
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
                                        .style(Style::TransparentBorder.into()),
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
        .max_width(750)
        .into()
}
