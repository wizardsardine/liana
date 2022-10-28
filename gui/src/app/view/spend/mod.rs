pub mod detail;
pub mod step;

use iced::{
    pure::{button, column, container, row, Element},
    Alignment, Length,
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
    column()
        .push(
            row().push(column().width(Length::Fill)).push(
                button::primary(Some(icon::plus_icon()), "Create a new transaction")
                    .on_press(Message::Menu(Menu::CreateSpendTx)),
            ),
        )
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

fn spend_tx_list_view<'a>(i: usize, tx: &SpendTx) -> Element<'a, Message> {
    container(
        button(
            row()
                .push(
                    row()
                        .push(badge::spend())
                        .push_maybe(match tx.status {
                            SpendStatus::Deprecated => Some(
                                container(text("  Deprecated  ").small())
                                    .padding(3)
                                    .style(badge::PillStyle::Simple),
                            ),
                            SpendStatus::Broadcasted => Some(
                                container(text("  Broadcasted  ").small())
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
                    column()
                        .push(text(&format!("{} BTC", tx.spend_amount.to_btc())).bold())
                        .push(text(&format!("fee: {}", tx.fee_amount.to_btc())).small())
                        .width(Length::Shrink),
                )
                .align_items(Alignment::Center)
                .spacing(20),
        )
        .padding(10)
        .on_press(Message::Select(i))
        .style(button::Style::TransparentBorder),
    )
    .style(card::SimpleCardStyle)
    .into()
}
