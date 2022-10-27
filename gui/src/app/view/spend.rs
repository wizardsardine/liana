use iced::{
    pure::{button, column, container, row, Element},
    Alignment, Length,
};

use crate::{
    daemon::model::SpendTx,
    ui::component::{badge, button::Style, card, text::*},
};

use super::message::Message;

pub fn spend_view<'a>(spend_txs: &[SpendTx]) -> Element<'a, Message> {
    column()
        .push(
            container(
                row()
                    .push(text(&format!(" {}", spend_txs.len())).bold())
                    .push(text(" draft transactions")),
            )
            .width(Length::Fill),
        )
        .push(
            column().spacing(10).push(
                spend_txs
                    .iter()
                    .enumerate()
                    .fold(column().spacing(10), |col, (i, tx)| {
                        col.push(spend_tx_list_view(i, tx))
                    }),
            ),
        )
        .align_items(Alignment::Center)
        .spacing(20)
        .into()
}

fn spend_tx_list_view<'a>(i: usize, _tx: &SpendTx) -> Element<'a, Message> {
    container(
        button(
            row()
                .push(
                    row()
                        .push(badge::spend())
                        .spacing(10)
                        .align_items(Alignment::Center)
                        .width(Length::Fill),
                )
                .push(text(&format!("{} BTC", 0)).bold().width(Length::Shrink))
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
