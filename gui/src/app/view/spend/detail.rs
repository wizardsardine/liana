use std::collections::{HashMap, HashSet};

use iced::{
    widget::{scrollable, tooltip, Space},
    Alignment, Length,
};

use liana::{
    descriptors::{LianaPolicy, PathInfo, PathSpendInfo},
    miniscript::bitcoin::{
        util::bip32::{DerivationPath, Fingerprint},
        Address, Amount, Network, Transaction,
    },
};

use liana_ui::{
    color,
    component::{
        badge, button, card,
        collapse::Collapse,
        form, hw, separation,
        text::{text, Text},
    },
    icon, theme,
    util::Collection,
    widget::*,
};

use crate::{
    app::{
        error::Error,
        view::{hw::hw_list_view, message::*, util::*, warning::warn},
    },
    daemon::model::{Coin, SpendStatus, SpendTx},
    hw::HardwareWallet,
};

pub fn spend_view<'a>(
    tx: &'a SpendTx,
    saved: bool,
    desc_info: &'a LianaPolicy,
    key_aliases: &'a HashMap<Fingerprint, String>,
    network: Network,
) -> Element<'a, Message> {
    spend_modal(
        saved,
        None,
        Column::new()
            .align_items(Alignment::Center)
            .spacing(20)
            .push(spend_header(tx))
            .push(spend_overview_view(tx, desc_info, key_aliases))
            .push(inputs_and_outputs_view(
                &tx.coins,
                &tx.psbt.unsigned_tx,
                network,
                Some(tx.change_indexes.clone()),
                None,
            )),
    )
}

pub fn save_action<'a>(warning: Option<&Error>, saved: bool) -> Element<'a, Message> {
    if saved {
        card::simple(text("Transaction is saved"))
            .width(Length::Units(400))
            .align_x(iced::alignment::Horizontal::Center)
            .into()
    } else {
        card::simple(
            Column::new()
                .spacing(10)
                .push_maybe(warning.map(|w| warn(Some(w))))
                .push(text("Save the transaction as draft"))
                .push(
                    Row::new()
                        .push(Column::new().width(Length::Fill))
                        .push(button::alert(None, "Ignore").on_press(Message::Close))
                        .push(
                            button::primary(None, "Save")
                                .on_press(Message::Spend(SpendTxMessage::Confirm)),
                        ),
                ),
        )
        .width(Length::Units(400))
        .into()
    }
}

pub fn broadcast_action<'a>(warning: Option<&Error>, saved: bool) -> Element<'a, Message> {
    if saved {
        card::simple(text("Transaction is broadcast"))
            .width(Length::Units(400))
            .align_x(iced::alignment::Horizontal::Center)
            .into()
    } else {
        card::simple(
            Column::new()
                .spacing(10)
                .push_maybe(warning.map(|w| warn(Some(w))))
                .push(text("Broadcast the transaction"))
                .push(
                    Row::new().push(Column::new().width(Length::Fill)).push(
                        button::primary(None, "Broadcast")
                            .on_press(Message::Spend(SpendTxMessage::Confirm)),
                    ),
                ),
        )
        .width(Length::Units(400))
        .into()
    }
}

pub fn delete_action<'a>(warning: Option<&Error>, deleted: bool) -> Element<'a, Message> {
    if deleted {
        card::simple(
            Column::new()
                .spacing(20)
                .align_items(Alignment::Center)
                .push(text("Transaction is deleted"))
                .push(button::primary(None, "Go back to drafts").on_press(Message::Close)),
        )
        .align_x(iced::alignment::Horizontal::Center)
        .width(Length::Units(400))
        .into()
    } else {
        card::simple(
            Column::new()
                .spacing(10)
                .push_maybe(warning.map(|w| warn(Some(w))))
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
        .width(Length::Units(400))
        .into()
    }
}

pub fn spend_modal<'a, T: Into<Element<'a, Message>>>(
    saved: bool,
    warning: Option<&Error>,
    content: T,
) -> Element<'a, Message> {
    Column::new()
        .push(warn(warning))
        .push(
            Container::new(
                Row::new()
                    .push(if saved {
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
                    .push(if saved {
                        button::primary(Some(icon::cross_icon()), "Close").on_press(Message::Close)
                    } else {
                        button::primary(Some(icon::cross_icon()), "Close")
                            .on_press(Message::Spend(SpendTxMessage::Save))
                    }),
            )
            .padding(10)
            .style(theme::Container::Background),
        )
        .push(
            Container::new(scrollable(
                Container::new(Container::new(content).max_width(800))
                    .width(Length::Fill)
                    .center_x(),
            ))
            .height(Length::Fill)
            .style(theme::Container::Background),
        )
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
                .push(badge::Badge::new(icon::send_icon()).style(theme::Badge::Standard))
                .push(if !tx.sigs.recovery_paths().is_empty() {
                    text("Recovery").bold()
                } else {
                    text("Spend").bold()
                })
                .spacing(5)
                .align_items(Alignment::Center),
        )
        .push_maybe(match tx.status {
            SpendStatus::Deprecated => Some(badge::deprecated()),
            SpendStatus::Broadcast => Some(badge::unconfirmed()),
            SpendStatus::Spent => Some(badge::spent()),
            _ => None,
        })
        .push(
            Column::new()
                .align_items(Alignment::Center)
                .push(amount_with_size(&tx.spend_amount, 50))
                .push(
                    Row::new()
                        .push(text("Miner fee: "))
                        .push(amount(&tx.fee_amount)),
                ),
        )
        .into()
}

fn spend_overview_view<'a>(
    tx: &'a SpendTx,
    desc_info: &'a LianaPolicy,
    key_aliases: &'a HashMap<Fingerprint, String>,
) -> Element<'a, Message> {
    Container::new(
        Column::new()
            .push(
                Column::new()
                    .padding(15)
                    .spacing(10)
                    .push(
                        Row::new()
                            .align_items(Alignment::Center)
                            .push(text("PSBT:").bold().width(Length::Fill))
                            .push(
                                Row::new()
                                    .spacing(5)
                                    .push(
                                        button::secondary(Some(icon::clipboard_icon()), "Copy")
                                            .on_press(Message::Clipboard(tx.psbt.to_string())),
                                    )
                                    .push(
                                        button::secondary(Some(icon::import_icon()), "Update")
                                            .on_press(Message::Spend(SpendTxMessage::EditPsbt)),
                                    ),
                            )
                            .align_items(Alignment::Center),
                    )
                    .push(
                        Row::new()
                            .push(text("Tx ID:").bold().width(Length::Fill))
                            .push(text(tx.psbt.unsigned_tx.txid().to_string()).small())
                            .push(
                                Button::new(icon::clipboard_icon())
                                    .on_press(Message::Clipboard(
                                        tx.psbt.unsigned_tx.txid().to_string(),
                                    ))
                                    .style(theme::Button::TransparentBorder),
                            )
                            .align_items(Alignment::Center),
                    ),
            )
            .push(signatures(tx, desc_info, key_aliases)),
    )
    .style(theme::Container::Card(theme::Card::Simple))
    .into()
}

pub fn signatures<'a>(
    tx: &'a SpendTx,
    desc_info: &'a LianaPolicy,
    keys_aliases: &'a HashMap<Fingerprint, String>,
) -> Element<'a, Message> {
    Column::new()
        .push(
            if let Some(sigs) = tx.path_ready() {
            Container::new(
                scrollable(
                    Row::new()
                        .spacing(5)
                        .align_items(Alignment::Center)
                        .push(icon::circle_check_icon().style(color::GREEN))
                        .push(text("Ready").bold().style(color::GREEN))
                        .push(text(", signed by"))
                        .push(
                            sigs.signed_pubkeys
                            .keys()
                            .fold(Row::new().spacing(5), |row, value| {
                                row.push(if let Some(alias) = keys_aliases.get(&value.0) {
                                Container::new(
                                    tooltip::Tooltip::new(
                                        Container::new(text(alias))
                                            .padding(3)
                                            .style(theme::Container::Pill(theme::Pill::Simple)),
                                            value.0.to_string(),
                                            tooltip::Position::Bottom,
                                    )
                                    .style(theme::Container::Card(theme::Card::Simple)),
                                )
                            } else {
                                Container::new(text(value.0.to_string()))
                                    .padding(3)
                                    .style(theme::Container::Pill(theme::Pill::Simple))
                            })
                            }),
                    )
                ).horizontal_scroll(scrollable::Properties::new().width(2).scroller_width(2))
            ).padding(15)
        } else{
            Container::new(
            Collapse::new(
            move || {
                Button::new(
                    Row::new()
                        .align_items(Alignment::Center)
                        .push(Row::new()
                                .spacing(5)
                                .align_items(Alignment::Center)
                                .push(icon::circle_cross_icon())
                                .push(text("Not ready").bold())
                                .width(Length::Fill)
                        )
                        .push(icon::collapse_icon()),
                )
                .padding(15)
                .width(Length::Fill)
                .style(theme::Button::TransparentBorder)
            },
            move || {
                Button::new(
                    Row::new()
                        .align_items(Alignment::Center)
                        .push(
                            Row::new()
                                .spacing(5)
                                .align_items(Alignment::Center)
                                .push(icon::circle_cross_icon())
                                .push(text("Not ready").bold())
                                .width(Length::Fill)
                        )
                        .push(icon::collapsed_icon()),
                )
                .padding(15)
                .width(Length::Fill)
                .style(theme::Button::TransparentBorder)
            },
            move || {
                Into::<Element<'a, Message>>::into(
                    Column::new().push(separation().width(Length::Fill)).push(
                        Column::new()
                            .padding(15)
                            .spacing(10)
                            .push(text(if !tx.sigs.recovery_paths().is_empty() {
                                "Multiple spending paths available. Finalizing this transaction requires either:"
                            } else {
                                "1 spending path available. Finalizing this transaction requires:"
                            }))
                            .push(path_view(
                                desc_info.primary_path(),
                                tx.sigs.primary_path(),
                                keys_aliases,
                            ))
                            .push(tx.sigs.recovery_paths().iter().fold(Column::new().spacing(10), |col, (seq, path)| {
                                let keys = &desc_info.recovery_paths()[seq];
                                col.push(path_view(keys, path, keys_aliases))
                            })),
                    ),
                )
            },
        ))})
        .push_maybe(if tx.status == SpendStatus::Pending {
            Some(
                Column::new().push(separation().width(Length::Fill)).push(
                    Container::new(
                        Row::new()
                            .push(Space::with_width(Length::Fill))
                            .push_maybe(if tx.path_ready().is_none() {
                                Some(
                                    button::primary(None, "Sign")
                                        .on_press(Message::Spend(SpendTxMessage::Sign))
                                        .width(Length::Units(150)),
                                )
                            } else {
                                Some(
                                    button::primary(None, "Broadcast")
                                        .on_press(Message::Spend(SpendTxMessage::Broadcast))
                                        .width(Length::Units(150)),
                                )
                            })
                            .align_items(Alignment::Center)
                            .spacing(20),
                    )
                    .padding(15),
                ),
            )
        } else {
            None
        })
        .into()
}

pub fn path_view<'a>(
    path: &'a PathInfo,
    sigs: &'a PathSpendInfo,
    key_aliases: &'a HashMap<Fingerprint, String>,
) -> Element<'a, Message> {
    let mut keys: Vec<(Fingerprint, DerivationPath)> =
        path.thresh_origins().1.into_iter().collect();
    let missing_signatures = if sigs.sigs_count >= sigs.threshold {
        0
    } else {
        sigs.threshold - sigs.sigs_count
    };
    keys.sort();
    scrollable(
        Row::new()
            .align_items(Alignment::Center)
            .push(if sigs.sigs_count >= sigs.threshold {
                icon::circle_check_icon().style(color::GREEN)
            } else {
                icon::circle_cross_icon()
            })
            .push(text(format!(" {}", missing_signatures)).bold())
            .push(text(format!(
                " more signature{}",
                if missing_signatures > 1 {
                    "s from "
                } else if missing_signatures == 0 {
                    ""
                } else {
                    " from "
                }
            )))
            .push_maybe(if keys.is_empty() {
                None
            } else {
                Some(keys.iter().fold(Row::new().spacing(5), |row, value| {
                    row.push_maybe(if !sigs.signed_pubkeys.contains_key(value) {
                        Some(if let Some(alias) = key_aliases.get(&value.0) {
                            Container::new(
                                tooltip::Tooltip::new(
                                    Container::new(text(alias))
                                        .padding(3)
                                        .style(theme::Container::Pill(theme::Pill::Simple)),
                                    value.0.to_string(),
                                    tooltip::Position::Bottom,
                                )
                                .style(theme::Container::Card(theme::Card::Simple)),
                            )
                        } else {
                            Container::new(text(value.0.to_string()))
                                .padding(3)
                                .style(theme::Container::Pill(theme::Pill::Simple))
                        })
                    } else {
                        None
                    })
                }))
            })
            .push_maybe(if sigs.signed_pubkeys.is_empty() {
                None
            } else {
                Some(text(", already signed by "))
            })
            .push(
                sigs.signed_pubkeys
                    .keys()
                    .fold(Row::new().spacing(5), |row, value| {
                        row.push(if let Some(alias) = key_aliases.get(&value.0) {
                            Container::new(
                                tooltip::Tooltip::new(
                                    Container::new(text(alias))
                                        .padding(3)
                                        .style(theme::Container::Pill(theme::Pill::Simple)),
                                    value.0.to_string(),
                                    tooltip::Position::Bottom,
                                )
                                .style(theme::Container::Card(theme::Card::Simple)),
                            )
                        } else {
                            Container::new(text(value.0.to_string()))
                                .padding(3)
                                .style(theme::Container::Pill(theme::Pill::Simple))
                        })
                    }),
            ),
    )
    .horizontal_scroll(scrollable::Properties::new().width(2).scroller_width(2))
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
                                .style(theme::Button::TransparentBorder)
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
                                .style(theme::Button::TransparentBorder)
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
                                                                    theme::Button::TransparentBorder,
                                                                ),
                                                        ),
                                                )
                                                .push(amount(&coin.amount)),
                                        )
                                    })
                                    .into()
                            },
                        ))
                        .style(theme::Container::Card(theme::Card::Simple)),
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
                            .style(theme::Button::TransparentBorder)
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
                            .style(theme::Button::TransparentBorder)
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
                                                                    theme::Button::TransparentBorder,
                                                                ),
                                                        ),
                                                    )
                                                    .push(
                                                        amount(&Amount::from_sat(output.value))
                                                    ),
                                            )
                                            .push_maybe(
                                                if let Some(indexes) = change_indexes.as_ref() {
                                                    if indexes.contains(&i) {
                                                        Some(
                                                            Container::new(text("Change"))
                                                                .padding(5)
                                                                .style(theme::Container::Pill(theme::Pill::Success)),
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
                                                                .style(theme::Container::Pill(theme::Pill::Success)),
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
                    .style(theme::Container::Card(theme::Card::Simple)),
                ),
        )
        .into()
}

pub fn sign_action<'a>(
    warning: Option<&Error>,
    hws: &'a [HardwareWallet],
    signer: Option<Fingerprint>,
    signer_alias: Option<&'a String>,
    processing: bool,
    chosen_hw: Option<usize>,
    signed: &HashSet<Fingerprint>,
) -> Element<'a, Message> {
    Column::new()
        .push_maybe(warning.map(|w| warn(Some(w))))
        .push(card::simple(
            Column::new()
                .push(
                    Column::new()
                        .push(
                            Row::new()
                                .push(
                                    text("Select signing device to sign with:")
                                        .bold()
                                        .width(Length::Fill),
                                )
                                .push(button::secondary(None, "Refresh").on_press(Message::Reload))
                                .align_items(Alignment::Center),
                        )
                        .spacing(10)
                        .push(hws.iter().enumerate().fold(
                            Column::new().spacing(10),
                            |col, (i, hw)| {
                                col.push(hw_list_view(
                                    i,
                                    hw,
                                    Some(i) == chosen_hw,
                                    processing,
                                    hw.fingerprint()
                                        .map(|f| signed.contains(&f))
                                        .unwrap_or(false),
                                ))
                            },
                        ))
                        .push_maybe(signer.map(|fingerprint| {
                            Button::new(if signed.contains(&fingerprint) {
                                hw::sign_success_hot_signer(fingerprint, signer_alias)
                            } else {
                                hw::hot_signer(fingerprint, signer_alias)
                            })
                            .on_press(Message::Spend(SpendTxMessage::SelectHotSigner))
                            .padding(10)
                            .style(theme::Button::Border)
                            .width(Length::Fill)
                        }))
                        .width(Length::Fill),
                )
                .spacing(20)
                .width(Length::Fill)
                .align_items(Alignment::Center),
        ))
        .width(Length::Units(500))
        .into()
}

pub fn update_spend_view<'a>(
    psbt: String,
    updated: &form::Value<String>,
    error: Option<&Error>,
    processing: bool,
) -> Element<'a, Message> {
    Column::new()
        .push(warn(error))
        .push(card::simple(
            Column::new()
                .spacing(20)
                .push(
                    Row::new()
                        .push(text("PSBT:").bold().width(Length::Fill))
                        .push(
                            button::border(Some(icon::clipboard_icon()), "Copy")
                                .on_press(Message::Clipboard(psbt)),
                        )
                        .align_items(Alignment::Center),
                )
                .push(separation().width(Length::Fill))
                .push(
                    Column::new()
                        .spacing(10)
                        .push(text("Insert updated PSBT:").bold())
                        .push(
                            form::Form::new("PSBT", updated, move |msg| {
                                Message::ImportSpend(ImportSpendMessage::PsbtEdited(msg))
                            })
                            .warning("Please enter the correct base64 encoded PSBT")
                            .size(20)
                            .padding(10),
                        )
                        .push(Row::new().push(Space::with_width(Length::Fill)).push(
                            if updated.valid && !updated.value.is_empty() && !processing {
                                button::primary(None, "Update")
                                    .on_press(Message::ImportSpend(ImportSpendMessage::Confirm))
                            } else if processing {
                                button::primary(None, "Processing...")
                            } else {
                                button::primary(None, "Update")
                            },
                        )),
                ),
        ))
        .max_width(400)
        .into()
}

pub fn update_spend_success_view<'a>() -> Element<'a, Message> {
    Column::new()
        .push(
            card::simple(Container::new(
                text("Spend transaction is updated").style(color::GREEN),
            ))
            .padding(50),
        )
        .width(Length::Units(400))
        .align_items(Alignment::Center)
        .into()
}
