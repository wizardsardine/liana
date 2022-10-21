use iced::{
    pure::{button, column, container, row, Element},
    Alignment, Length,
};

use crate::ui::component::{badge, button::Style, card, text::*};

use crate::{app::view::message::Message, daemon::model::Coin};

pub fn coins_view<'a>(coins: &[Coin]) -> Element<'a, Message> {
    column()
        .push(
            container(
                row()
                    .push(text(&format!(" {}", coins.len())).bold())
                    .push(text(" coins")),
            )
            .width(Length::Fill),
        )
        .push(
            column().spacing(10).push(
                coins
                    .iter()
                    .enumerate()
                    .fold(column().spacing(10), |col, (i, coin)| {
                        col.push(coin_list_view(i, coin))
                    }),
            ),
        )
        .align_items(Alignment::Center)
        .spacing(20)
        .into()
}

fn coin_list_view<'a>(i: usize, coin: &Coin) -> Element<'a, Message> {
    container(
        button(
            row()
                .push(
                    row()
                        .push(badge::coin())
                        .push(text(&format!("block: {}", coin.block_height.unwrap_or(0))).small())
                        .spacing(10)
                        .align_items(Alignment::Center)
                        .width(Length::Fill),
                )
                .push(
                    text(&format!("{} BTC", coin.amount.to_btc()))
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
