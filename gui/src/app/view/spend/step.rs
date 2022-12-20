use iced::{
    widget::{self, Button, Column, Container, Row},
    Alignment, Element, Length,
};

use liana::miniscript::bitcoin::Amount;

use crate::{
    app::{
        cache::Cache,
        error::Error,
        view::{message::*, modal},
    },
    daemon::model::{remaining_sequence, Coin},
    ui::{
        color,
        component::{
            badge, button, card, form,
            text::{text, Text},
        },
        icon,
        util::Collection,
    },
};

pub fn choose_recipients_view(
    recipients: Vec<Element<Message>>,
    total_amount: Amount,
    is_valid: bool,
    duplicate: bool,
) -> Element<Message> {
    modal(
        false,
        None,
        Column::new()
            .push(text("Choose recipients").bold().size(50))
            .push(
                Column::new()
                    .push(widget::Column::with_children(recipients).spacing(10))
                    .push(
                        button::transparent(Some(icon::plus_icon()), "Add recipient")
                            .on_press(Message::CreateSpend(CreateSpendMessage::AddRecipient)),
                    )
                    .padding(10)
                    .max_width(1000)
                    .spacing(10),
            )
            .spacing(20)
            .align_items(Alignment::Center),
        Some(
            Container::new(
                Row::new()
                    .spacing(20)
                    .align_items(Alignment::Center)
                    .push(
                        Container::new(text(format!("{}", total_amount)).bold())
                            .width(Length::Fill),
                    )
                    .push_maybe(if duplicate {
                        Some(text("Two recipient addresses are the same").style(color::WARNING))
                    } else {
                        None
                    })
                    .push(if is_valid {
                        button::primary(None, "Next")
                            .on_press(Message::Next)
                            .width(Length::Units(100))
                    } else {
                        button::primary(None, "Next").width(Length::Units(100))
                    }),
            )
            .padding(20),
        ),
    )
}

pub fn recipient_view<'a>(
    index: usize,
    address: &form::Value<String>,
    amount: &form::Value<String>,
) -> Element<'a, CreateSpendMessage> {
    Row::new()
        .push(
            form::Form::new("Address", address, move |msg| {
                CreateSpendMessage::RecipientEdited(index, "address", msg)
            })
            .warning("Please enter correct bitcoin address for the current network")
            .size(20)
            .padding(10),
        )
        .push(
            Container::new(
                form::Form::new("Amount", amount, move |msg| {
                    CreateSpendMessage::RecipientEdited(index, "amount", msg)
                })
                .warning("Please enter correct amount")
                .size(20)
                .padding(10),
            )
            .width(Length::Units(250)),
        )
        .spacing(5)
        .push(
            button::transparent(Some(icon::trash_icon()), "")
                .on_press(CreateSpendMessage::DeleteRecipient(index))
                .width(Length::Shrink),
        )
        .width(Length::Fill)
        .into()
}

pub fn choose_coins_view<'a>(
    cache: &Cache,
    timelock: u32,
    coins: &[(Coin, bool)],
    amount_left: Option<&Amount>,
    feerate: &form::Value<String>,
    error: Option<&Error>,
) -> Element<'a, Message> {
    modal(
        true,
        error,
        Column::new()
            .push(text("Choose coins and feerate").bold().size(50))
            .push(
                Container::new(
                    form::Form::new("Feerate (sat/vbyte)", feerate, move |msg| {
                        Message::CreateSpend(CreateSpendMessage::FeerateEdited(msg))
                    })
                    .warning("Please enter correct feerate (sat/vbyte)")
                    .size(20)
                    .padding(10),
                )
                .width(Length::Units(250)),
            )
            .push(
                Column::new()
                    .padding(10)
                    .spacing(10)
                    .push(coins.iter().enumerate().fold(
                        Column::new().spacing(10),
                        |col, (i, (coin, selected))| {
                            col.push(coin_list_view(
                                i,
                                coin,
                                timelock,
                                cache.blockheight as u32,
                                *selected,
                            ))
                        },
                    )),
            )
            .spacing(20)
            .align_items(Alignment::Center),
        Some(
            Container::new(
                Row::new()
                    .align_items(Alignment::Center)
                    .push(
                        Container::new(if let Some(amount_left) = amount_left {
                            Row::new()
                                .spacing(5)
                                .push(text("Amount left to select:"))
                                .push(text(amount_left.to_string()).bold())
                        } else {
                            Row::new().push(text("Please, define feerate"))
                        })
                        .width(Length::Fill),
                    )
                    .push(if Some(&Amount::from_sat(0)) == amount_left {
                        button::primary(None, "Next")
                            .on_press(Message::CreateSpend(CreateSpendMessage::Generate))
                            .width(Length::Units(100))
                    } else {
                        button::primary(None, "Next").width(Length::Units(100))
                    }),
            )
            .padding(20),
        ),
    )
}

fn coin_list_view<'a>(
    i: usize,
    coin: &Coin,
    timelock: u32,
    blockheight: u32,
    selected: bool,
) -> Element<'a, Message> {
    Container::new(
        Button::new(
            Row::new()
                .push(
                    Row::new()
                        .push(if selected {
                            icon::square_check_icon()
                        } else {
                            icon::square_icon()
                        })
                        .push(badge::coin())
                        .push_maybe(if coin.spend_info.is_some() {
                            Some(
                                Container::new(text("  Spent  ").small())
                                    .padding(3)
                                    .style(badge::PillStyle::Success),
                            )
                        } else {
                            let seq = remaining_sequence(coin, blockheight, timelock);
                            if seq == 0 {
                                Some(Container::new(
                                    Row::new()
                                        .spacing(5)
                                        .push(text(" 0").small().style(color::ALERT))
                                        .push(
                                            icon::hourglass_done_icon().small().style(color::ALERT),
                                        )
                                        .align_items(Alignment::Center),
                                ))
                            } else if seq < timelock * 10 / 100 {
                                Some(Container::new(
                                    Row::new()
                                        .spacing(5)
                                        .push(
                                            text(format!(" {}", seq)).small().style(color::WARNING),
                                        )
                                        .push(icon::hourglass_icon().small().style(color::WARNING))
                                        .align_items(Alignment::Center),
                                ))
                            } else {
                                Some(Container::new(
                                    Row::new()
                                        .spacing(5)
                                        .push(text(format!(" {}", seq)).small())
                                        .push(icon::hourglass_icon().small())
                                        .align_items(Alignment::Center),
                                ))
                            }
                        })
                        .push_maybe(if coin.block_height.is_none() {
                            Some(
                                Container::new(text("  Unconfirmed  ").small())
                                    .padding(3)
                                    .style(badge::PillStyle::Simple),
                            )
                        } else {
                            None
                        })
                        .spacing(10)
                        .align_items(Alignment::Center)
                        .width(Length::Fill),
                )
                .push(
                    text(format!("{} BTC", coin.amount.to_btc()))
                        .bold()
                        .width(Length::Shrink),
                )
                .align_items(Alignment::Center)
                .spacing(20),
        )
        .padding(10)
        .on_press(Message::CreateSpend(CreateSpendMessage::SelectCoin(i)))
        .style(button::Style::TransparentBorder.into()),
    )
    .style(card::SimpleCardStyle)
    .into()
}
