use chrono::NaiveDateTime;

use iced::{
    alignment,
    pure::{button, column, container, row, Element},
    Alignment, Length,
};

use crate::ui::{
    component::{badge, button::Style, card, text::*},
    util::Collection,
};
use minisafe::miniscript::bitcoin;

use crate::{
    app::view::message::Message,
    daemon::model::{HistoryEvent, HistoryEventKind},
};

pub const HISTORY_EVENT_PAGE_SIZE: u64 = 20;

pub fn home_view<'a>(
    balance: &'a bitcoin::Amount,
    events: &Vec<HistoryEvent>,
) -> Element<'a, Message> {
    column()
        .push(column().padding(40))
        .push(text(&format!("{} BTC", balance.to_btc())).bold().size(50))
        .push(
            column()
                .spacing(10)
                .push(
                    events
                        .iter()
                        .enumerate()
                        .fold(column().spacing(10), |col, (i, event)| {
                            col.push(event_list_view(i, event))
                        }),
                )
                .push_maybe(
                    if events.len() % HISTORY_EVENT_PAGE_SIZE as usize == 0 && !events.is_empty() {
                        Some(
                            container(
                                button(
                                    text("See more")
                                        .width(Length::Fill)
                                        .horizontal_alignment(alignment::Horizontal::Center),
                                )
                                .width(Length::Fill)
                                .padding(15)
                                .style(Style::TransparentBorder)
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

fn event_list_view<'a>(i: usize, event: &HistoryEvent) -> Element<'a, Message> {
    let date = NaiveDateTime::from_timestamp(event.date.into(), 0);
    container(
        button(
            row()
                .push(
                    row()
                        .push(match event.kind {
                            HistoryEventKind::Receive => badge::receive(),
                            HistoryEventKind::Spend => badge::spend(),
                        })
                        .push(text(&format!("{}", date)).small())
                        .spacing(10)
                        .align_items(Alignment::Center)
                        .width(Length::Fill),
                )
                .push(
                    text(&format!("{} BTC", event.amount.to_btc()))
                        .bold()
                        .width(Length::Shrink),
                )
                .align_items(Alignment::Center)
                .spacing(20),
        )
        .padding(10)
        .on_press(Message::Select(i))
        .style(Style::TransparentBorder),
    )
    .style(card::SimpleCardStyle)
    .into()
}

pub fn event_view<'a>(event: &HistoryEvent) -> Element<'a, Message> {
    let date = NaiveDateTime::from_timestamp(event.date.into(), 0);
    column()
        .push(
            row()
                .push(match event.kind {
                    HistoryEventKind::Receive => badge::receive(),
                    HistoryEventKind::Spend => badge::spend(),
                })
                .push(
                    text(match event.kind {
                        HistoryEventKind::Receive => "Receive",
                        HistoryEventKind::Spend => "Spend",
                    })
                    .small(),
                )
                .spacing(10)
                .align_items(Alignment::Center),
        )
        .push(
            text(&format!("{} BTC", event.amount.to_btc()))
                .bold()
                .size(50)
                .width(Length::Shrink),
        )
        .push(card::simple(
            column()
                .push(
                    row()
                        .width(Length::Fill)
                        .push(container(text("Date:").bold()).width(Length::Fill))
                        .push(container(text(&format!("{}", date))).width(Length::Shrink)),
                )
                .push(
                    row()
                        .width(Length::Fill)
                        .push(container(text("Txid:").bold()).width(Length::Fill))
                        .push(if let Some(outpoint) = event.outpoint {
                            container(text(&format!("{}", outpoint.txid))).width(Length::Shrink)
                        } else {
                            container(text(&format!("{}", event.tx.as_ref().unwrap().txid())))
                                .width(Length::Shrink)
                        }),
                )
                .spacing(5),
        ))
        .align_items(Alignment::Center)
        .spacing(20)
        .max_width(750)
        .into()
}
