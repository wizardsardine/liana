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
    daemon::model::Coin,
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
                    .align_items(Alignment::Center)
                    .push(
                        Container::new(text(format!("{}", total_amount)).bold())
                            .width(Length::Fill),
                    )
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
            .warning("Please enter correct bitcoin address")
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

pub fn choose_feerate_view<'a>(
    feerate: &form::Value<String>,
    is_valid: bool,
    error: Option<&Error>,
) -> Element<'a, Message> {
    modal(
        true,
        None,
        Column::new()
            .push(text("Choose feerate").bold().size(50))
            .push(
                Container::new(
                    form::Form::new("Feerate", feerate, move |msg| {
                        Message::CreateSpend(CreateSpendMessage::FeerateEdited(msg))
                    })
                    .warning("Please enter correct feerate")
                    .size(20)
                    .padding(10),
                )
                .width(Length::Units(250)),
            )
            .push_maybe(error.map(|e| card::error("Failed to create spend", e.to_string())))
            .push_maybe(if is_valid {
                Some(
                    button::primary(None, "Next")
                        .on_press(Message::CreateSpend(CreateSpendMessage::Generate))
                        .width(Length::Units(100)),
                )
            } else {
                None
            })
            .spacing(20)
            .align_items(Alignment::Center),
        None::<Element<Message>>,
    )
}

pub fn choose_coins_view<'a>(
    cache: &Cache,
    timelock: u32,
    coins: &[(Coin, bool)],
    total_needed: Option<&Amount>,
    is_valid: bool,
) -> Element<'a, Message> {
    modal(
        true,
        None,
        Column::new()
            .push(text("Choose coins").bold().size(50))
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
            .push_maybe(if is_valid {
                Some(Container::new(
                    button::primary(None, "Next")
                        .on_press(Message::Next)
                        .width(Length::Units(100)),
                ))
            } else if total_needed.is_some() {
                Some(Container::new(card::warning(format!(
                    "Total amount must be superior to {}",
                    total_needed.unwrap().to_btc(),
                ))))
            } else {
                None
            })
            .spacing(20)
            .align_items(Alignment::Center),
        None::<Element<Message>>,
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
                        .push_maybe(if let Some(b) = coin.block_height {
                            if blockheight > b as u32 + timelock {
                                Some(Container::new(
                                    Row::new()
                                        .spacing(5)
                                        .push(text(" 0").small().style(color::ALERT))
                                        .push(
                                            icon::hourglass_done_icon().small().style(color::ALERT),
                                        )
                                        .align_items(Alignment::Center),
                                ))
                            } else {
                                Some(Container::new(
                                    Row::new()
                                        .spacing(5)
                                        .push(
                                            text(format!(" {}", b as u32 + timelock - blockheight))
                                                .small(),
                                        )
                                        .push(icon::hourglass_icon().small())
                                        .align_items(Alignment::Center),
                                ))
                            }
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
