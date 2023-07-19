use std::collections::HashMap;

use iced::{
    alignment,
    widget::{checkbox, scrollable, Space},
    Alignment, Length,
};

use liana::{
    descriptors::LianaPolicy,
    miniscript::bitcoin::{bip32::Fingerprint, Amount, Network},
};

use liana_ui::{
    color,
    component::{amount::*, badge, button, form, text::*},
    icon, theme,
    util::Collection,
    widget::*,
};

use crate::{
    app::{
        cache::Cache,
        error::Error,
        menu::Menu,
        view::{coins, dashboard, message::*, psbt},
    },
    daemon::model::{remaining_sequence, Coin, SpendTx},
};

pub fn spend_view<'a>(
    cache: &'a Cache,
    tx: &'a SpendTx,
    saved: bool,
    desc_info: &'a LianaPolicy,
    key_aliases: &'a HashMap<Fingerprint, String>,
    network: Network,
) -> Element<'a, Message> {
    dashboard(
        &Menu::CreateSpendTx,
        cache,
        None,
        Column::new()
            .spacing(20)
            .push(Container::new(h3("Send")).width(Length::Fill))
            .push(psbt::spend_header(tx))
            .push(psbt::spend_overview_view(tx, desc_info, key_aliases))
            .push(psbt::inputs_and_outputs_view(
                &tx.coins,
                &tx.psbt.unsigned_tx,
                network,
                Some(tx.change_indexes.clone()),
                None,
            ))
            .push(if saved {
                Row::new()
                    .push(
                        button::secondary(None, "Delete")
                            .width(Length::Fixed(200.0))
                            .on_press(Message::Spend(SpendTxMessage::Delete)),
                    )
                    .width(Length::Fill)
            } else {
                Row::new()
                    .push(
                        button::secondary(None, "< Previous")
                            .width(Length::Fixed(150.0))
                            .on_press(Message::Previous),
                    )
                    .push(Space::with_width(Length::Fill))
                    .push(
                        button::secondary(None, "Save")
                            .width(Length::Fixed(150.0))
                            .on_press(Message::Spend(SpendTxMessage::Save)),
                    )
                    .width(Length::Fill)
            }),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn create_spend_tx<'a>(
    cache: &'a Cache,
    balance_available: &'a Amount,
    recipients: Vec<Element<'a, Message>>,
    total_amount: Amount,
    is_valid: bool,
    duplicate: bool,
    timelock: u16,
    coins: &[(Coin, bool)],
    amount_left: Option<&Amount>,
    feerate: &form::Value<String>,
    error: Option<&Error>,
) -> Element<'a, Message> {
    let is_self_send = recipients.is_empty();
    dashboard(
        &Menu::CreateSpendTx,
        cache,
        error,
        Column::new()
            .push(h3(if is_self_send {
                "Self-transfer"
            } else {
                "Send"
            }))
            .push(
                Column::new()
                    .push(Column::with_children(recipients).spacing(10))
                    .push(
                        Row::new()
                            .push_maybe(if duplicate {
                                Some(
                                    Container::new(
                                        text("Two recipient addresses are the same")
                                            .style(color::RED),
                                    )
                                    .padding(10),
                                )
                            } else {
                                None
                            })
                            .push(Space::with_width(Length::Fill))
                            .push_maybe(if is_self_send {
                                None
                            } else {
                                Some(
                                    button::secondary(Some(icon::plus_icon()), "Add recipient")
                                        .on_press(Message::CreateSpend(
                                            CreateSpendMessage::AddRecipient,
                                        )),
                                )
                            }),
                    )
                    .spacing(20),
            )
            .push(
                Row::new()
                    .push(
                        Row::new()
                            .push(Container::new(p1_bold("Feerate")).padding(10))
                            .spacing(10)
                            .push(
                                form::Form::new("42 (in sats/vbyte)", feerate, move |msg| {
                                    Message::CreateSpend(CreateSpendMessage::FeerateEdited(msg))
                                })
                                .warning("Invalid feerate")
                                .size(20)
                                .padding(10),
                            )
                            .width(Length::FillPortion(1)),
                    )
                    .push(Space::with_width(Length::FillPortion(1))),
            )
            .push(
                Container::new(
                    Column::new()
                        .spacing(10)
                        .push(
                            Row::new()
                                .align_items(Alignment::Center)
                                .push(p1_bold("Coins selection").width(Length::Fill))
                                .push(if is_self_send {
                                    Row::new()
                                        .spacing(5)
                                        .push(amount_with_size(
                                            &Amount::from_sat(
                                                coins
                                                    .iter()
                                                    .filter_map(|(coin, selected)| {
                                                        if *selected {
                                                            Some(coin.amount.to_sat())
                                                        } else {
                                                            None
                                                        }
                                                    })
                                                    .sum(),
                                            ),
                                            P2_SIZE,
                                        ))
                                        .push(p2_regular("selected").style(color::GREY_3))
                                } else if let Some(amount_left) = amount_left {
                                    Row::new()
                                        .spacing(5)
                                        .push(amount_with_size(amount_left, P2_SIZE))
                                        .push(p2_regular("left to select").style(color::GREY_3))
                                } else {
                                    Row::new()
                                        .push(text("Feerate needs to be set.").style(color::GREY_3))
                                })
                                .width(Length::Fill),
                        )
                        .push(
                            Container::new(scrollable(coins.iter().enumerate().fold(
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
                            )))
                            .max_height(300),
                        ),
                )
                .padding(20)
                .style(theme::Card::Simple),
            )
            .push(
                Row::new()
                    .spacing(20)
                    .align_items(Alignment::Center)
                    .push(Space::with_width(Length::Fill))
                    .push(
                        button::primary(None, "Clear")
                            .on_press(Message::Menu(Menu::CreateSpendTx))
                            .width(Length::Fixed(100.0)),
                    )
                    .push(
                        if is_valid
                            && (is_self_send
                                || (total_amount < *balance_available
                                    && Some(&Amount::from_sat(0)) == amount_left))
                        {
                            button::primary(None, "Next")
                                .on_press(Message::CreateSpend(CreateSpendMessage::Generate))
                                .width(Length::Fixed(100.0))
                        } else {
                            button::primary(None, "Next").width(Length::Fixed(100.0))
                        },
                    ),
            )
            .push(Space::with_height(Length::Fixed(20.0)))
            .spacing(20),
    )
}

pub fn recipient_view<'a>(
    index: usize,
    address: &form::Value<String>,
    amount: &form::Value<String>,
) -> Element<'a, CreateSpendMessage> {
    Container::new(
        Column::new()
            .spacing(10)
            .push(
                Row::new().push(Space::with_width(Length::Fill)).push(
                    Button::new(icon::cross_icon())
                        .style(theme::Button::Transparent)
                        .on_press(CreateSpendMessage::DeleteRecipient(index))
                        .width(Length::Shrink),
                ),
            )
            .push(
                Row::new()
                    .align_items(Alignment::Start)
                    .spacing(10)
                    .push(
                        Container::new(p1_bold("Pay to"))
                            .align_x(alignment::Horizontal::Right)
                            .padding(10)
                            .width(Length::Fixed(80.0)),
                    )
                    .push(
                        form::Form::new("Address", address, move |msg| {
                            CreateSpendMessage::RecipientEdited(index, "address", msg)
                        })
                        .warning("Invalid address (maybe it is for another network?)")
                        .size(20)
                        .padding(10),
                    ),
            )
            .push(
                Row::new()
                    .align_items(Alignment::Start)
                    .spacing(10)
                    .push(
                        Container::new(p1_bold("Amount"))
                            .padding(10)
                            .align_x(alignment::Horizontal::Right)
                            .width(Length::Fixed(80.0)),
                    )
                    .push(
                        form::Form::new("0.001 (in BTC)", amount, move |msg| {
                            CreateSpendMessage::RecipientEdited(index, "amount", msg)
                        })
                        .warning(
                            "Invalid amount. (Note amounts lower than 0.00005 BTC are invalid.)",
                        )
                        .size(20)
                        .padding(10),
                    )
                    .width(Length::Fill),
            ),
    )
    .padding(20)
    .style(theme::Card::Simple)
    .into()
}

fn coin_list_view<'a>(
    i: usize,
    coin: &Coin,
    timelock: u16,
    blockheight: u32,
    selected: bool,
) -> Element<'a, Message> {
    Row::new()
        .push(
            Row::new()
                .push(checkbox("", selected, move |_| {
                    Message::CreateSpend(CreateSpendMessage::SelectCoin(i))
                }))
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
        // give some space for the scroll bar without using padding
        .push(Space::with_width(Length::Fixed(0.0)))
        .align_items(Alignment::Center)
        .spacing(20)
        .into()
}
