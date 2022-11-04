use iced::{
    pure::{column, container, row, scrollable, Element},
    Alignment, Length,
};

use minisafe::miniscript::bitcoin::{
    util::{bip32::Fingerprint, psbt::Psbt},
    Address, Amount, Network,
};

use crate::{
    app::{
        error::Error,
        view::{message::*, modal_section, warning::warn, ModalSectionStyle},
    },
    daemon::model::{Coin, SpendStatus, SpendTx},
    hw::HardwareWallet,
    ui::{
        color,
        component::{
            badge, button, card, separation,
            text::{text, Text},
        },
        icon,
        util::Collection,
    },
};

pub fn spend_view<'a, T: Into<Element<'a, Message>>>(
    warning: Option<&Error>,
    tx: &SpendTx,
    action: T,
    show_delete: bool,
    network: Network,
) -> Element<'a, Message> {
    spend_modal(
        show_delete,
        warning,
        column()
            .spacing(20)
            .push(spend_overview_view(tx))
            .push(action)
            .push(inputs_and_outputs_view(
                &tx.coins,
                &tx.psbt,
                network,
                tx.change_index,
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
            column()
                .spacing(10)
                .push(text("Save the transaction"))
                .push(row().push(column().width(Length::Fill)).push(
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
            column()
                .spacing(10)
                .push(text("Broadcast the transaction"))
                .push(
                    row().push(column().width(Length::Fill)).push(
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
            column()
                .spacing(10)
                .push(text("Delete the transaction draft"))
                .push(
                    row()
                        .push(column().width(Length::Fill))
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
    column()
        .push(warn(warning))
        .push(
            container(
                row()
                    .push(if show_delete {
                        column()
                            .push(
                                button::alert(Some(icon::trash_icon()), "Delete")
                                    .on_press(Message::Spend(SpendTxMessage::Delete)),
                            )
                            .width(Length::Fill)
                    } else {
                        column().width(Length::Fill)
                    })
                    .align_items(iced::Alignment::Center)
                    .push(
                        button::primary(Some(icon::cross_icon()), "Close").on_press(Message::Close),
                    ),
            )
            .padding(10)
            .style(ModalSectionStyle),
        )
        .push(modal_section(container(scrollable(content))))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn spend_overview_view<'a>(tx: &SpendTx) -> Element<'a, Message> {
    column()
        .spacing(20)
        .align_items(Alignment::Center)
        .push(
            row()
                .push(badge::Badge::new(icon::send_icon()).style(badge::Style::Standard))
                .push(text("Spend").bold())
                .spacing(5)
                .align_items(Alignment::Center),
        )
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
        .push(
            column()
                .align_items(Alignment::Center)
                .push(
                    text(&format!("- {} BTC", tx.spend_amount.to_btc()))
                        .bold()
                        .size(50),
                )
                .push(container(text(&format!(
                    "Miner Fee: {} BTC",
                    tx.fee_amount.to_btc()
                )))),
        )
        .push(card::simple(
            column()
                .push(container(
                    row()
                        .push(
                            container(
                                row()
                                    .push(container(icon::key_icon().size(30).width(Length::Fill)))
                                    .push(column().push(text("Number of signatures:").bold()).push(
                                        text(&format!("{}", tx.psbt.inputs[0].partial_sigs.len(),)),
                                    ))
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
                    column()
                        .push(
                            row()
                                .push(text("Tx ID:").bold().width(Length::Fill))
                                .push(text(&format!("{}", tx.psbt.unsigned_tx.txid())).small())
                                .align_items(Alignment::Center),
                        )
                        .push(
                            row()
                                .push(text("Psbt:").bold().width(Length::Fill))
                                .push(
                                    button::transparent(Some(icon::clipboard_icon()), "Copy")
                                        .on_press(Message::Clipboard(tx.psbt.to_string())),
                                )
                                .align_items(Alignment::Center),
                        ),
                )
                .spacing(20),
        ))
        .into()
}

fn inputs_and_outputs_view<'a>(
    coins: &[Coin],
    psbt: &Psbt,
    network: Network,
    change_index: Option<usize>,
) -> Element<'a, Message> {
    column()
        .push(
            row()
                .spacing(10)
                .push(
                    column()
                        .spacing(10)
                        .push(text("Spent coins:").bold())
                        .push(coins.iter().fold(column().spacing(10), |col, coin| {
                            col.push(
                                card::simple(
                                    column()
                                        .width(Length::Fill)
                                        .push(text(&format!("{} BTC", coin.amount.to_btc())).bold())
                                        .push(text(&format!("{}", coin.outpoint)).small()),
                                )
                                .width(Length::Fill),
                            )
                        }))
                        .width(Length::FillPortion(1)),
                )
                .push(
                    column()
                        .spacing(10)
                        .push(text("Recipients:").bold())
                        .push(psbt.unsigned_tx.output.iter().enumerate().fold(
                            column().spacing(10),
                            |col, (i, output)| {
                                col.push(
                                    card::simple(
                                        column()
                                            .width(Length::Fill)
                                            .push(
                                                text(&format!(
                                                    "{} BTC",
                                                    Amount::from_sat(output.value).to_btc()
                                                ))
                                                .bold(),
                                            )
                                            .push(
                                                text(&format!(
                                                    "{}",
                                                    Address::from_script(
                                                        &output.script_pubkey,
                                                        network
                                                    )
                                                    .unwrap()
                                                ))
                                                .small(),
                                            )
                                            .push_maybe(if Some(i) == change_index {
                                                Some(
                                                    container(text("Change"))
                                                        .padding(5)
                                                        .style(badge::PillStyle::Success),
                                                )
                                            } else {
                                                None
                                            }),
                                    )
                                    .width(Length::Fill),
                                )
                            },
                        ))
                        .width(Length::FillPortion(1)),
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
        column()
            .push(if !hws.is_empty() {
                column()
                    .push(text("Select hardware wallet to sign with:").bold())
                    .spacing(10)
                    .push(
                        hws.iter()
                            .enumerate()
                            .fold(column().spacing(10), |col, (i, hw)| {
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
                column()
                    .push(
                        card::simple(
                            column()
                                .spacing(20)
                                .width(Length::Fill)
                                .push("Please connect a hardware wallet")
                                .push(button::primary(None, "Refresh").on_press(Message::Reload))
                                .align_items(Alignment::Center),
                        )
                        .width(Length::Fill),
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
    let mut bttn = iced::pure::button(
        row()
            .push(
                column()
                    .push(text(&format!("{}", hw.kind)).bold())
                    .push(text(&format!("fingerprint: {}", hw.fingerprint)).small())
                    .spacing(5)
                    .width(Length::Fill),
            )
            .push_maybe(if chosen && processing {
                Some(
                    column()
                        .push(text("Processing..."))
                        .push(text("Please check your device").small()),
                )
            } else {
                None
            })
            .push_maybe(if signed {
                Some(
                    column().push(
                        row()
                            .spacing(5)
                            .push(icon::circle_check_icon().color(color::SUCCESS))
                            .push(text("Signed").color(color::SUCCESS)),
                    ),
                )
            } else {
                None
            })
            .align_items(Alignment::Center)
            .width(Length::Fill),
    )
    .padding(10)
    .style(button::Style::Border)
    .width(Length::Fill);
    if !processing {
        bttn = bttn.on_press(Message::Spend(SpendTxMessage::SelectHardwareWallet(i)));
    }
    container(bttn)
        .width(Length::Fill)
        .style(card::SimpleCardStyle)
        .into()
}
