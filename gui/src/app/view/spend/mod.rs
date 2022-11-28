pub mod detail;
pub mod step;

use iced::{
    widget::{Button, Column, Container, Row},
    Alignment, Element, Length,
};

use crate::{
    app::menu::Menu,
    daemon::model::{SpendStatus, SpendTx},
    ui::{
        component::{badge, button, card, text::*},
        icon,
        util::Collection,
    },
};

use super::message::Message;

pub fn spend_view<'a>(spend_txs: &[SpendTx]) -> Element<'a, Message> {
    Column::new()
        .push(
            Row::new().push(Column::new().width(Length::Fill)).push(
                button::primary(Some(icon::plus_icon()), "Create a new transaction")
                    .on_press(Message::Menu(Menu::CreateSpendTx)),
            ),
        )
        .push(
            Container::new(
                Row::new()
                    .push(text(format!(" {}", spend_txs.len())).bold())
                    .push(text(" draft transactions")),
            )
            .width(Length::Fill),
        )
        .push(
            Column::new().spacing(10).push(
                spend_txs
                    .iter()
                    .enumerate()
                    .fold(Column::new().spacing(10), |col, (i, tx)| {
                        col.push(spend_tx_list_view(i, tx))
                    }),
            ),
        )
        .align_items(Alignment::Center)
        .spacing(20)
        .into()
}

fn spend_tx_list_view<'a>(i: usize, tx: &SpendTx) -> Element<'a, Message> {
    Container::new(
        Button::new(
            Row::new()
                .push(
                    Row::new()
                        .push(badge::spend())
                        .push_maybe(match tx.status {
                            SpendStatus::Deprecated => Some(
                                Container::new(text("  Deprecated  ").small())
                                    .padding(3)
                                    .style(badge::PillStyle::Simple),
                            ),
                            SpendStatus::Broadcasted => Some(
                                Container::new(text("  Broadcasted  ").small())
                                    .padding(3)
                                    .style(badge::PillStyle::Success),
                            ),
                            _ => None,
                        })
                        .spacing(10)
                        .align_items(Alignment::Center)
                        .width(Length::Fill),
                )
                .push(
                    Column::new()
                        .push(text(format!("{} BTC", tx.spend_amount.to_btc())).bold())
                        .push(text(format!("fee: {}", tx.fee_amount.to_btc())).small())
                        .width(Length::Shrink),
                )
                .align_items(Alignment::Center)
                .spacing(20),
        )
        .padding(10)
        .on_press(Message::Select(i))
        .style(button::Style::TransparentBorder.into()),
    )
    .style(card::SimpleCardStyle)
    .into()
}
