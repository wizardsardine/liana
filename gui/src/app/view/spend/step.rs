use iced::{
    pure::{column, container, row, widget, Element},
    Alignment, Length,
};

use minisafe::miniscript::bitcoin::Amount;

use crate::{
    app::view::{message::*, modal},
    daemon::model::Coin,
    ui::{
        component::{
            badge, button, card, form,
            text::{text, Text},
        },
        icon,
        util::Collection,
    },
};

pub fn choose_recipients_view<'a>(
    recipients: Vec<Element<'a, Message>>,
    is_valid: bool,
) -> Element<'a, Message> {
    modal(
        false,
        None,
        column()
            .push(text("Choose recipients").bold().size(50))
            .push(
                column()
                    .push(widget::Column::with_children(recipients).spacing(10))
                    .push(
                        button::transparent(Some(icon::plus_icon()), "Add recipient")
                            .on_press(Message::CreateSpend(CreateSpendMessage::AddRecipient)),
                    )
                    .max_width(1000)
                    .spacing(10),
            )
            .push_maybe(if is_valid {
                Some(
                    button::primary(None, "Next")
                        .on_press(Message::Next)
                        .width(Length::Units(100)),
                )
            } else {
                None
            })
            .spacing(20)
            .align_items(Alignment::Center),
    )
}

pub fn recipient_view<'a>(
    index: usize,
    address: &form::Value<String>,
    amount: &form::Value<String>,
) -> Element<'a, CreateSpendMessage> {
    row()
        .push(
            form::Form::new("Address", address, move |msg| {
                CreateSpendMessage::RecipientEdited(index, "address", msg)
            })
            .warning("Please enter correct bitcoin address")
            .size(20)
            .padding(10),
        )
        .push(
            container(
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
        .align_items(Alignment::Center)
        .width(Length::Fill)
        .into()
}

pub fn choose_feerate_view<'a>(
    feerate: &form::Value<String>,
    is_valid: bool,
) -> Element<'a, Message> {
    modal(
        true,
        None,
        column()
            .push(text("Choose feerate").bold().size(50))
            .push(
                container(
                    form::Form::new("Feerate", feerate, move |msg| {
                        Message::CreateSpend(CreateSpendMessage::FeerateEdited(msg))
                    })
                    .warning("Please enter correct feerate")
                    .size(20)
                    .padding(10),
                )
                .width(Length::Units(250)),
            )
            .push_maybe(if is_valid {
                Some(
                    button::primary(None, "Next")
                        .on_press(Message::Next)
                        .width(Length::Units(100)),
                )
            } else {
                None
            })
            .spacing(20)
            .align_items(Alignment::Center),
    )
}

pub fn choose_coins_view<'a>(
    coins: &[(Coin, bool)],
    total_needed: Option<&Amount>,
    is_valid: bool,
) -> Element<'a, Message> {
    modal(
        true,
        None,
        column()
            .push(text("Choose coins").bold().size(50))
            .push(
                column().spacing(10).push(
                    coins
                        .iter()
                        .enumerate()
                        .fold(column().spacing(10), |col, (i, (coin, selected))| {
                            col.push(coin_list_view(i, coin, *selected))
                        }),
                ),
            )
            .push_maybe(if is_valid {
                Some(container(
                    button::primary(None, "Next")
                        .on_press(Message::Next)
                        .width(Length::Units(100)),
                ))
            } else if total_needed.is_some() {
                Some(container(card::warning(&format!(
                    "Total amount must be superior to {}",
                    total_needed.unwrap().to_btc(),
                ))))
            } else {
                None
            })
            .spacing(20)
            .align_items(Alignment::Center),
    )
}

fn coin_list_view<'a>(i: usize, coin: &Coin, selected: bool) -> Element<'a, Message> {
    container(
        iced::pure::button(
            row()
                .push(
                    row()
                        .push(if selected {
                            icon::square_check_icon()
                        } else {
                            icon::square_icon()
                        })
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
        .on_press(Message::CreateSpend(CreateSpendMessage::SelectCoin(i)))
        .style(button::Style::TransparentBorder),
    )
    .style(card::SimpleCardStyle)
    .into()
}
