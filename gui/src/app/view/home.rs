use chrono::NaiveDateTime;

use iced::{
    alignment,
    pure::{button, column, container, row, Element},
    Alignment, Length,
};

use crate::ui::{
    component::{badge, button::Style, card::SimpleCardStyle, text::*},
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
                .push_maybe(if events.len() % HISTORY_EVENT_PAGE_SIZE as usize == 0 {
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
                        .style(SimpleCardStyle),
                    )
                } else {
                    None
                }),
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
    .style(SimpleCardStyle)
    .into()
}
