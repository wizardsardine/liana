use iced::{
    pure::{column, container, row, Element},
    Alignment, Length,
};

use crate::ui::{
    color,
    component::{badge, card, collapse::collapse, separation, text::*},
    icon,
    util::Collection,
};

use crate::{
    app::{cache::Cache, view::message::Message},
    daemon::model::Coin,
};

pub fn coins_view<'a>(cache: &Cache, coins: &'a [Coin], timelock: u32) -> Element<'a, Message> {
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
            column()
                .spacing(10)
                .push(coins.iter().fold(column().spacing(10), |col, coin| {
                    col.push(coin_list_view(coin, timelock, cache.blockheight as u32))
                })),
        )
        .align_items(Alignment::Center)
        .spacing(20)
        .into()
}

#[allow(clippy::collapsible_else_if)]
fn coin_list_view(coin: &Coin, timelock: u32, blockheight: u32) -> Element<Message> {
    container(collapse::<_, _, _, _, _>(
        move || {
            row::<Message, _>()
                .push(
                    row()
                        .push(badge::coin())
                        .push_maybe(if coin.spend_info.is_some() {
                            Some(
                                container(text("  Spent  ").small())
                                    .padding(3)
                                    .style(badge::PillStyle::Success),
                            )
                        } else {
                            if let Some(b) = coin.block_height {
                                if blockheight > b as u32 + timelock {
                                    Some(container(
                                        row()
                                            .spacing(5)
                                            .push(text(" 0").small().color(color::ALERT))
                                            .push(
                                                icon::hourglass_done_icon()
                                                    .small()
                                                    .color(color::ALERT),
                                            )
                                            .align_items(Alignment::Center),
                                    ))
                                } else {
                                    Some(container(
                                        row()
                                            .spacing(5)
                                            .push(
                                                text(&format!(
                                                    " {}",
                                                    b as u32 + timelock - blockheight
                                                ))
                                                .small(),
                                            )
                                            .push(icon::hourglass_icon().small())
                                            .align_items(Alignment::Center),
                                    ))
                                }
                            } else {
                                None
                            }
                        })
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
                .spacing(20)
                .into()
        },
        move || {
            column()
                .spacing(10)
                .push(separation().width(Length::Fill))
                .push(
                    column()
                        .padding(10)
                        .spacing(5)
                        .push_maybe(if coin.spend_info.is_none() {
                            if let Some(b) = coin.block_height {
                                if blockheight > b as u32 + timelock {
                                    Some(container(
                                        text("The recovery path is available")
                                            .bold()
                                            .small()
                                            .color(color::ALERT),
                                    ))
                                } else {
                                    Some(container(
                                        text(&format!(
                                            "The recovery path will be available in {} blocks",
                                            b as u32 + timelock - blockheight
                                        ))
                                        .bold()
                                        .small(),
                                    ))
                                }
                            } else {
                                None
                            }
                        } else {
                            None
                        })
                        .push(
                            column()
                                .push(
                                    row()
                                        .push(text("Outpoint:").small().bold())
                                        .push(text(&format!("{}", coin.outpoint)).small())
                                        .spacing(5),
                                )
                                .push_maybe(coin.block_height.map(|b| {
                                    row()
                                        .push(text("Block height:").small().bold())
                                        .push(text(&format!("{}", b)).small())
                                        .spacing(5)
                                })),
                        )
                        .push_maybe(coin.spend_info.map(|info| {
                            column()
                                .push(
                                    row()
                                        .push(text("Spend txid:").small().bold())
                                        .push(text(&format!("{}", info.txid)).small())
                                        .spacing(5),
                                )
                                .push(if let Some(height) = info.height {
                                    row()
                                        .push(text("Spend block height:").small().bold())
                                        .push(text(&format!("{}", height)).small())
                                        .spacing(5)
                                } else {
                                    row().push(text("Not in a block").bold().small())
                                })
                                .spacing(5)
                        })),
                )
                .into()
        },
    ))
    .style(card::SimpleCardStyle)
    .into()
}
