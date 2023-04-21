use iced::{Alignment, Length};

use liana::miniscript::bitcoin::Amount;

use liana_ui::{
    color,
    component::{
        amount::*,
        badge, button, form,
        text::{text, Text},
    },
    icon, theme,
    util::Collection,
    widget::*,
};

use crate::{
    app::{
        cache::Cache,
        error::Error,
        view::{coins, message::*, modal},
    },
    daemon::model::{remaining_sequence, Coin},
};

pub fn choose_recipients_view<'a>(
    balance_available: &'a Amount,
    recipients: Vec<Element<'a, Message>>,
    total_amount: Amount,
    is_valid: bool,
    duplicate: bool,
) -> Element<'a, Message> {
    modal(
        false,
        None,
        Column::new()
            .push(text("Choose recipients").bold().size(50))
            .push(
                Column::new()
                    .push(Column::with_children(recipients).spacing(10))
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
                        Container::new(
                            Row::new()
                                .align_items(Alignment::Center)
                                .spacing(5)
                                .push(text(format!("{}", total_amount)).bold())
                                .push(text(format!("/ {}", balance_available))),
                        )
                        .width(Length::Fill),
                    )
                    .push_maybe(if duplicate {
                        Some(text("Two recipient addresses are the same").style(color::ORANGE))
                    } else {
                        None
                    })
                    .push(if is_valid && total_amount < *balance_available {
                        button::primary(None, "Next")
                            .on_press(Message::Next)
                            .width(Length::Units(100))
                    } else {
                        button::primary(None, "Next").width(Length::Units(100))
                    }),
            )
            .style(theme::Container::Foreground)
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
            .warning("Invalid address (maybe it is for another network?)")
            .size(20)
            .padding(10),
        )
        .push(
            Container::new(
                form::Form::new("Amount", amount, move |msg| {
                    CreateSpendMessage::RecipientEdited(index, "amount", msg)
                })
                .warning("Invalid amount. Must be > 0.00005000 BTC.")
                .size(20)
                .padding(10),
            )
            .width(Length::Units(300)),
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
    timelock: u16,
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
                    .warning("Invalid feerate")
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
                            Row::new().push(text("Feerate needs to be set."))
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
            .style(theme::Container::Foreground)
            .padding(20),
        ),
    )
}

fn coin_list_view<'a>(
    i: usize,
    coin: &Coin,
    timelock: u16,
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
                        .push(if coin.spend_info.is_some() {
                            badge::spent()
                        } else if coin.block_height.is_none() {
                            badge::unconfirmed()
                        } else {
                            let seq = remaining_sequence(coin, blockheight, timelock);
                            coins::coin_sequence_label(seq, timelock as u32)
                        })
                        .spacing(10)
                        .align_items(Alignment::Center)
                        .width(Length::Fill),
                )
                .push(amount(&coin.amount))
                .align_items(Alignment::Center)
                .spacing(20),
        )
        .padding(10)
        .on_press(Message::CreateSpend(CreateSpendMessage::SelectCoin(i)))
        .style(theme::Button::TransparentBorder),
    )
    .style(theme::Container::Card(theme::Card::Simple))
    .into()
}
