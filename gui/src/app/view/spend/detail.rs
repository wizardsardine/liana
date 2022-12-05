use iced::{
    widget::{Button, Column, Container, Row, Scrollable},
    Alignment, Element, Length,
};

use liana::miniscript::bitcoin::{util::bip32::Fingerprint, Address, Amount, Network, Transaction};

use crate::{
    app::{
        error::Error,
        view::{message::*, modal_section, warning::warn},
    },
    daemon::model::{Coin, SpendStatus, SpendTx},
    hw::HardwareWallet,
    ui::{
        color,
        component::{
            badge, button, card,
            collapse::Collapse,
            container, separation,
            text::{text, Text},
        },
        icon,
        util::Collection,
    },
};

pub fn spend_view<'a, T: Into<Element<'a, Message>>>(
    warning: Option<&Error>,
    tx: &'a SpendTx,
    action: T,
    show_delete: bool,
    network: Network,
) -> Element<'a, Message> {
    spend_modal(
        show_delete,
        warning,
        Column::new()
            .align_items(Alignment::Center)
            .spacing(20)
            .push(spend_header(tx))
            .push(action)
            .push(spend_overview_view(tx))
            .push(inputs_and_outputs_view(
                &tx.coins,
                &tx.psbt.unsigned_tx,
                network,
                tx.change_index.map(|i| vec![i]),
                None,
            )),
    )
}

pub fn save_action<'a>(saved: bool) -> Element<'a, Message> {
    if saved {
        card::simple(text("Transaction is saved"))
            .width(Length::Fill)
            .align_x(iced::alignment::Horizontal::Center)
            .into()
    } else {
        card::simple(
            Column::new()
                .spacing(10)
                .push(text("Save the transaction"))
                .push(Row::new().push(Column::new().width(Length::Fill)).push(
                    button::primary(None, "Save").on_press(Message::Spend(SpendTxMessage::Confirm)),
                )),
        )
        .width(Length::Fill)
        .into()
    }
}

pub fn broadcast_action<'a>(saved: bool) -> Element<'a, Message> {
    if saved {
        card::simple(text("Transaction is broadcasted"))
            .width(Length::Fill)
            .align_x(iced::alignment::Horizontal::Center)
            .into()
    } else {
        card::simple(
            Column::new()
                .spacing(10)
                .push(text("Broadcast the transaction"))
                .push(
                    Row::new().push(Column::new().width(Length::Fill)).push(
                        button::primary(None, "Broadcast")
                            .on_press(Message::Spend(SpendTxMessage::Confirm)),
                    ),
                ),
        )
        .width(Length::Fill)
        .into()
    }
}

pub fn delete_action<'a>(deleted: bool) -> Element<'a, Message> {
    if deleted {
        card::simple(text("Transaction is deleted"))
            .align_x(iced::alignment::Horizontal::Center)
            .width(Length::Fill)
            .into()
    } else {
        card::simple(
            Column::new()
                .spacing(10)
                .push(text("Delete the transaction draft"))
                .push(
                    Row::new()
                        .push(Column::new().width(Length::Fill))
                        .push(
                            button::transparent(None, "Cancel")
                                .on_press(Message::Spend(SpendTxMessage::Cancel)),
                        )
                        .push(
                            button::alert(None, "Delete")
                                .on_press(Message::Spend(SpendTxMessage::Confirm)),
                        ),
                ),
        )
        .width(Length::Fill)
        .into()
    }
}

pub fn spend_modal<'a, T: Into<Element<'a, Message>>>(
    show_delete: bool,
    warning: Option<&Error>,
    content: T,
) -> Element<'a, Message> {
    Column::new()
        .push(warn(warning))
        .push(
            Container::new(
                Row::new()
                    .push(if show_delete {
                        Column::new()
                            .push(
                                button::alert(Some(icon::trash_icon()), "Delete")
                                    .on_press(Message::Spend(SpendTxMessage::Delete)),
                            )
                            .width(Length::Fill)
                    } else {
                        Column::new()
                            .push(
                                button::transparent(None, "< Previous").on_press(Message::Previous),
                            )
                            .width(Length::Fill)
                    })
                    .align_items(iced::Alignment::Center)
                    .push(
                        button::primary(Some(icon::cross_icon()), "Close").on_press(Message::Close),
                    ),
            )
            .padding(10)
            .style(container::Style::Background),
        )
        .push(modal_section(Container::new(
            Container::new(Scrollable::new(content)).max_width(750),
        )))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn spend_header<'a>(tx: &SpendTx) -> Element<'a, Message> {
    Column::new()
        .spacing(20)
        .align_items(Alignment::Center)
        .push(
            Row::new()
                .push(badge::Badge::new(icon::send_icon()).style(badge::Style::Standard))
                .push(text("Spend").bold())
                .spacing(5)
                .align_items(Alignment::Center),
        )
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
        .push(
            Column::new()
                .align_items(Alignment::Center)
                .push(
                    text(format!("- {} BTC", tx.spend_amount.to_btc()))
                        .bold()
                        .size(50),
                )
                .push(Container::new(text(format!(
                    "Miner Fee: {} BTC",
                    tx.fee_amount.to_btc()
                )))),
        )
        .into()
}

fn spend_overview_view<'a>(tx: &SpendTx) -> Element<'a, Message> {
    card::simple(
        Column::new()
            .push(Container::new(
                Row::new()
                    .push(
                        Container::new(
                            Row::new()
                                .push(Container::new(
                                    icon::key_icon().size(30).width(Length::Fill),
                                ))
                                .push(
                                    Column::new()
                                        .push(text("Number of signatures:").bold())
                                        .push(text(format!(
                                            "{}",
                                            tx.psbt.inputs[0].partial_sigs.len(),
                                        ))),
                                )
                                .align_items(Alignment::Center)
                                .spacing(20),
                        )
                        .width(Length::FillPortion(1)),
                    )
                    .align_items(Alignment::Center)
                    .spacing(20),
            ))
            .push(separation().width(Length::Fill))
            .push(
                Column::new()
                    .push(
                        Row::new()
                            .push(text("Tx ID:").bold().width(Length::Fill))
                            .push(text(format!("{}", tx.psbt.unsigned_tx.txid())).small())
                            .align_items(Alignment::Center),
                    )
                    .push(
                        Row::new()
                            .push(text("Psbt:").bold().width(Length::Fill))
                            .push(
                                button::transparent(Some(icon::clipboard_icon()), "Copy")
                                    .on_press(Message::Clipboard(tx.psbt.to_string())),
                            )
                            .align_items(Alignment::Center),
                    ),
            )
            .spacing(20),
    )
    .into()
}

pub fn inputs_and_outputs_view<'a>(
    coins: &'a [Coin],
    tx: &'a Transaction,
    network: Network,
    change_indexes: Option<Vec<usize>>,
    receive_indexes: Option<Vec<usize>>,
) -> Element<'a, Message> {
    Column::new()
        .push(
            Column::new()
                .spacing(10)
                .push_maybe(if !coins.is_empty() {
                    Some(
                        Container::new(Collapse::new(
                            move || {
                                Button::new(
                                    Row::new()
                                        .align_items(Alignment::Center)
                                        .push(
                                            text(format!(
                                                "{} spent coin{}",
                                                coins.len(),
                                                if coins.len() == 1 { "" } else { "s" }
                                            ))
                                            .bold()
                                            .width(Length::Fill),
                                        )
                                        .push(icon::collapse_icon()),
                                )
                                .padding(15)
                                .width(Length::Fill)
                                .style(button::Style::TransparentBorder.into())
                            },
                            move || {
                                Button::new(
                                    Row::new()
                                        .align_items(Alignment::Center)
                                        .push(
                                            text(format!(
                                                "{} spent coin{}",
                                                coins.len(),
                                                if coins.len() == 1 { "" } else { "s" }
                                            ))
                                            .bold()
                                            .width(Length::Fill),
                                        )
                                        .push(icon::collapsed_icon()),
                                )
                                .padding(15)
                                .width(Length::Fill)
                                .style(button::Style::TransparentBorder.into())
                            },
                            move || {
                                coins
                                    .iter()
                                    .fold(Column::new(), |col: Column<'a, Message>, coin| {
                                        col.push(separation().width(Length::Fill)).push(
                                            Row::new()
                                                .padding(15)
                                                .align_items(Alignment::Center)
                                                .width(Length::Fill)
                                                .push(
                                                    Row::new()
                                                        .width(Length::Fill)
                                                        .align_items(Alignment::Center)
                                                        .push(
                                                            text(coin.outpoint.to_string())
                                                                .small()
                                                        )
                                                        .push(
                                                            Button::new(icon::clipboard_icon())
                                                                .on_press(Message::Clipboard(
                                                                    coin.outpoint.to_string(),
                                                                ))
                                                                .style(
                                                                    button::Style::TransparentBorder.into(),
                                                                ),
                                                        ),
                                                )
                                                .push(
                                                    text(format!("{} BTC", coin.amount.to_btc()))
                                                        .bold(),
                                                ),
                                        )
                                    })
                                    .into()
                            },
                        ))
                        .style(card::SimpleCardStyle),
                    )
                } else {
                    None
                })
                .push(
                    Container::new(Collapse::new(
                        move || {
                            Button::new(
                                Row::new()
                                    .align_items(Alignment::Center)
                                    .push(
                                        text(format!(
                                            "{} recipient{}",
                                            tx.output.len(),
                                            if tx.output.len() == 1 { "" } else { "s" }
                                        ))
                                        .bold()
                                        .width(Length::Fill),
                                    )
                                    .push(icon::collapse_icon()),
                            )
                            .padding(15)
                            .width(Length::Fill)
                            .style(button::Style::TransparentBorder.into())
                        },
                        move || {
                            Button::new(
                                Row::new()
                                    .align_items(Alignment::Center)
                                    .push(
                                        text(format!(
                                            "{} recipient{}",
                                            tx.output.len(),
                                            if tx.output.len() == 1 { "" } else { "s" }
                                        ))
                                        .bold()
                                        .width(Length::Fill),
                                    )
                                    .push(icon::collapsed_icon()),
                            )
                            .padding(15)
                            .width(Length::Fill)
                            .style(button::Style::TransparentBorder.into())
                        },
                        move || {
                            tx.output
                                .iter()
                                .enumerate()
                                .fold(Column::new(), |col: Column<'a, Message>, (i, output)| {
                                    let addr = Address::from_script(&output.script_pubkey, network).unwrap();
                                    col.push(separation().width(Length::Fill)).push(
                                        Column::new()
                                            .padding(15)
                                            .width(Length::Fill)
                                            .spacing(10)
                                            .push(
                                                Row::new()
                                                    .width(Length::Fill)
                                                    .push(
                                                        Row::new()
                                                        .align_items(Alignment::Center)
                                                        .width(Length::Fill)
                                                        .push(text(addr.to_string()).small())
                                                        .push(
                                                            Button::new(icon::clipboard_icon())
                                                                .on_press(Message::Clipboard(
                                                                    addr.to_string(),
                                                                ))
                                                                .style(
                                                                    button::Style::TransparentBorder.into(),
                                                                ),
                                                        ),
                                                    )
                                                    .push(
                                                        text(format!(
                                                            "{} BTC",
                                                            Amount::from_sat(output.value).to_btc()
                                                        ))
                                                        .bold(),
                                                    ),
                                            )
                                            .push_maybe(
                                                if let Some(indexes) = change_indexes.as_ref() {
                                                    if indexes.contains(&i) {
                                                        Some(
                                                            Container::new(text("Change"))
                                                                .padding(5)
                                                                .style(badge::PillStyle::Success),
                                                        )
                                                    } else {
                                                        None
                                                    }
                                                } else {
                                                    None
                                                },
                                            )
                                            .push_maybe(
                                                if let Some(indexes) = receive_indexes.as_ref() {
                                                    if indexes.contains(&i) {
                                                        Some(
                                                            Container::new(text("Deposit"))
                                                                .padding(5)
                                                                .style(badge::PillStyle::Success),
                                                        )
                                                    } else {
                                                        None
                                                    }
                                                } else {
                                                    None
                                                },
                                            ),
                                    )
                                })
                                .into()
                        },
                    ))
                    .style(card::SimpleCardStyle),
                ),
        )
        .into()
}

pub fn sign_action<'a>(
    hws: &[HardwareWallet],
    processing: bool,
    chosen_hw: Option<usize>,
    signed: &[Fingerprint],
) -> Element<'a, Message> {
    card::simple(
        Column::new()
            .push(if !hws.is_empty() {
                Column::new()
                    .push(text("Select hardware wallet to sign with:").bold())
                    .spacing(10)
                    .push(
                        hws.iter()
                            .enumerate()
                            .fold(Column::new().spacing(10), |col, (i, hw)| {
                                col.push(hw_list_view(
                                    i,
                                    hw,
                                    Some(i) == chosen_hw,
                                    processing,
                                    signed.contains(&hw.fingerprint),
                                ))
                            }),
                    )
                    .width(Length::Fill)
            } else {
                Column::new()
                    .push(
                        Column::new()
                            .spacing(20)
                            .width(Length::Fill)
                            .push("Please connect a hardware wallet")
                            .push(button::primary(None, "Refresh").on_press(Message::Reload))
                            .align_items(Alignment::Center),
                    )
                    .width(Length::Fill)
            })
            .width(Length::Fill)
            .align_items(Alignment::Center),
    )
    .width(Length::Fill)
    .into()
}

fn hw_list_view<'a>(
    i: usize,
    hw: &HardwareWallet,
    chosen: bool,
    processing: bool,
    signed: bool,
) -> Element<'a, Message> {
    let mut bttn = Button::new(
        Row::new()
            .push(
                Column::new()
                    .push(text(format!("{}", hw.kind)).bold())
                    .push(text(format!("fingerprint: {}", hw.fingerprint)).small())
                    .spacing(5)
                    .width(Length::Fill),
            )
            .push_maybe(if chosen && processing {
                Some(
                    Column::new()
                        .push(text("Processing..."))
                        .push(text("Please check your device").small()),
                )
            } else {
                None
            })
            .push_maybe(if signed {
                Some(
                    Column::new().push(
                        Row::new()
                            .spacing(5)
                            .push(icon::circle_check_icon().style(color::SUCCESS))
                            .push(text("Signed").style(color::SUCCESS)),
                    ),
                )
            } else {
                None
            })
            .align_items(Alignment::Center)
            .width(Length::Fill),
    )
    .padding(10)
    .style(button::Style::Border.into())
    .width(Length::Fill);
    if !processing {
        bttn = bttn.on_press(Message::Spend(SpendTxMessage::SelectHardwareWallet(i)));
    }
    Container::new(bttn)
        .width(Length::Fill)
        .style(card::SimpleCardStyle)
        .into()
}
