use std::collections::{HashMap, HashSet};

use iced::{
    widget::{scrollable, tooltip, Space},
    Alignment, Length,
};

use liana::{
    descriptors::{LianaPolicy, PathInfo, PathSpendInfo},
    miniscript::bitcoin::{
        bip32::{DerivationPath, Fingerprint},
        blockdata::transaction::TxOut,
        Address, Amount, Network, OutPoint, Transaction, Txid,
    },
};

use liana_ui::{
    color,
    component::{
        amount::*,
        badge, button, card,
        collapse::Collapse,
        form, hw, separation,
        text::{self, *},
    },
    icon, theme,
    util::Collection,
    widget::*,
};

use crate::{
    app::{
        cache::Cache,
        error::Error,
        menu::Menu,
        view::{dashboard, hw::hw_list_view, label, message::*, warning::warn},
    },
    daemon::model::{Coin, SpendStatus, SpendTx},
    hw::HardwareWallet,
};

#[allow(clippy::too_many_arguments)]
pub fn psbt_view<'a>(
    cache: &'a Cache,
    tx: &'a SpendTx,
    saved: bool,
    desc_info: &'a LianaPolicy,
    key_aliases: &'a HashMap<Fingerprint, String>,
    labels_editing: &'a HashMap<String, form::Value<String>>,
    network: Network,
    warning: Option<&Error>,
) -> Element<'a, Message> {
    dashboard(
        &Menu::PSBTs,
        cache,
        warning,
        Column::new()
            .spacing(20)
            .push(
                Row::new()
                    .align_items(Alignment::Center)
                    .spacing(10)
                    .push(Container::new(h3("PSBT")).width(Length::Fill))
                    .push_maybe(if !tx.sigs.recovery_paths().is_empty() {
                        Some(badge::recovery())
                    } else {
                        None
                    })
                    .push_maybe(match tx.status {
                        SpendStatus::Deprecated => Some(badge::deprecated()),
                        SpendStatus::Broadcast => Some(badge::unconfirmed()),
                        SpendStatus::Spent => Some(badge::spent()),
                        _ => None,
                    }),
            )
            .push(spend_header(tx, labels_editing))
            .push(spend_overview_view(tx, desc_info, key_aliases))
            .push(inputs_and_outputs_view(
                &tx.coins,
                &tx.psbt.unsigned_tx,
                network,
                Some(tx.change_indexes.clone()),
                &tx.labels,
                labels_editing,
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

pub fn save_action<'a>(warning: Option<&Error>, saved: bool) -> Element<'a, Message> {
    if saved {
        card::simple(text("Transaction is saved"))
            .width(Length::Fixed(400.0))
            .align_x(iced::alignment::Horizontal::Center)
            .into()
    } else {
        card::simple(
            Column::new()
                .spacing(10)
                .push_maybe(warning.map(|w| warn(Some(w))))
                .push(text("Save this transaction"))
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
        .width(Length::Fixed(400.0))
        .into()
    }
}

pub fn broadcast_action<'a>(warning: Option<&Error>, saved: bool) -> Element<'a, Message> {
    if saved {
        card::simple(text("Transaction is broadcast"))
            .width(Length::Fixed(400.0))
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
        .width(Length::Fixed(400.0))
        .into()
    }
}

pub fn delete_action<'a>(warning: Option<&Error>, deleted: bool) -> Element<'a, Message> {
    if deleted {
        card::simple(
            Column::new()
                .spacing(20)
                .align_items(Alignment::Center)
                .push(text("Successfully deleted this transaction."))
                .push(button::primary(None, "Go back to PSBTs").on_press(Message::Close)),
        )
        .align_x(iced::alignment::Horizontal::Center)
        .width(Length::Fixed(400.0))
        .into()
    } else {
        card::simple(
            Column::new()
                .spacing(10)
                .push_maybe(warning.map(|w| warn(Some(w))))
                .push(text("Delete this PSBT"))
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
        .width(Length::Fixed(400.0))
        .into()
    }
}

pub fn spend_header<'a>(
    tx: &'a SpendTx,
    labels_editing: &'a HashMap<String, form::Value<String>>,
) -> Element<'a, Message> {
    let txid = tx.psbt.unsigned_tx.txid().to_string();
    Column::new()
        .spacing(20)
        .push(if let Some(outpoint) = tx.is_single_payment() {
            let outpoint = outpoint.to_string();
            if let Some(label) = labels_editing.get(&outpoint) {
                label::label_editing(vec![outpoint.clone(), txid.clone()], label, H3_SIZE)
            } else {
                label::label_editable(
                    vec![outpoint.clone(), txid.clone()],
                    tx.labels.get(&outpoint),
                    H3_SIZE,
                )
            }
        } else if let Some(label) = labels_editing.get(&txid) {
            label::label_editing(vec![txid.clone()], label, H3_SIZE)
        } else {
            label::label_editable(vec![txid.clone()], tx.labels.get(&txid), H3_SIZE)
        })
        .push(
            Column::new()
                .push(if tx.is_send_to_self() {
                    Container::new(h1("Self-transfer"))
                } else {
                    Container::new(amount_with_size(&tx.spend_amount, H1_SIZE))
                })
                .push(
                    Row::new()
                        .align_items(Alignment::Center)
                        .push(h3("Miner fee: ").style(color::GREY_3))
                        .push(amount_with_size(&tx.fee_amount, H3_SIZE))
                        .push(text(" ").size(H3_SIZE))
                        .push(
                            text(format!("(~{} sats/vbyte)", &tx.min_feerate_vb()))
                                .size(H4_SIZE)
                                .style(color::GREY_3),
                        ),
                ),
        )
        .into()
}

pub fn spend_overview_view<'a>(
    tx: &'a SpendTx,
    desc_info: &'a LianaPolicy,
    key_aliases: &'a HashMap<Fingerprint, String>,
) -> Element<'a, Message> {
    Column::new()
        .spacing(20)
        .push(
            Container::new(
                Column::new()
                    .push(
                        Column::new()
                            .padding(15)
                            .spacing(10)
                            .push(
                                Row::new()
                                    .align_items(Alignment::Center)
                                    .push(text("PSBT").bold().width(Length::Fill))
                                    .push(
                                        Row::new()
                                            .spacing(5)
                                            .push(
                                                button::secondary(
                                                    Some(icon::clipboard_icon()),
                                                    "Copy",
                                                )
                                                .on_press(Message::Clipboard(tx.psbt.to_string())),
                                            )
                                            .push(
                                                button::secondary(
                                                    Some(icon::import_icon()),
                                                    "Update",
                                                )
                                                .on_press(Message::Spend(SpendTxMessage::EditPsbt)),
                                            ),
                                    )
                                    .align_items(Alignment::Center),
                            )
                            .push(
                                Row::new()
                                    .push(p1_bold("Tx ID").width(Length::Fill))
                                    .push(
                                        p2_regular(tx.psbt.unsigned_tx.txid().to_string())
                                            .style(color::GREY_3),
                                    )
                                    .push(
                                        Button::new(icon::clipboard_icon().style(color::GREY_3))
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
            .style(theme::Container::Card(theme::Card::Simple)),
        )
        .push_maybe(if tx.status == SpendStatus::Pending {
            Some(
                Row::new()
                    .push(Space::with_width(Length::Fill))
                    .push_maybe(if tx.path_ready().is_none() {
                        Some(
                            button::primary(None, "Sign")
                                .on_press(Message::Spend(SpendTxMessage::Sign))
                                .width(Length::Fixed(150.0)),
                        )
                    } else {
                        Some(
                            button::primary(None, "Broadcast")
                                .on_press(Message::Spend(SpendTxMessage::Broadcast))
                                .width(Length::Fixed(150.0)),
                        )
                    })
                    .align_items(Alignment::Center)
                    .spacing(20),
            )
        } else {
            None
        })
        .into()
}

pub fn signatures<'a>(
    tx: &'a SpendTx,
    desc_info: &'a LianaPolicy,
    keys_aliases: &'a HashMap<Fingerprint, String>,
) -> Element<'a, Message> {
    Column::new()
        .push(if let Some(sigs) = tx.path_ready() {
            Container::new(
                scrollable(
                    Row::new()
                        .spacing(5)
                        .align_items(Alignment::Center)
                        .spacing(10)
                        .push(p1_bold("Status"))
                        .push(icon::circle_check_icon().style(color::GREEN))
                        .push(text("Ready").bold().style(color::GREEN))
                        .push(text("  signed by"))
                        .push(sigs.signed_pubkeys.keys().fold(
                            Row::new().spacing(5),
                            |row, value| {
                                row.push(if let Some(alias) = keys_aliases.get(value) {
                                    Container::new(
                                        tooltip::Tooltip::new(
                                            Container::new(text(alias))
                                                .padding(10)
                                                .style(theme::Container::Pill(theme::Pill::Simple)),
                                            value.to_string(),
                                            tooltip::Position::Bottom,
                                        )
                                        .style(theme::Container::Card(theme::Card::Simple)),
                                    )
                                } else {
                                    Container::new(text(value.to_string()))
                                        .padding(10)
                                        .style(theme::Container::Pill(theme::Pill::Simple))
                                })
                            },
                        )),
                )
                .horizontal_scroll(scrollable::Properties::new().width(2).scroller_width(2)),
            )
            .padding(15)
        } else {
            Container::new(Collapse::new(
                move || {
                    Button::new(
                        Row::new()
                            .align_items(Alignment::Center)
                            .spacing(20)
                            .push(p1_bold("Status"))
                            .push(
                                Row::new()
                                    .spacing(5)
                                    .align_items(Alignment::Center)
                                    .push(icon::circle_cross_icon().style(color::RED))
                                    .push(text("Not ready").style(color::RED))
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
                            .spacing(20)
                            .push(p1_bold("Status"))
                            .push(
                                Row::new()
                                    .spacing(5)
                                    .align_items(Alignment::Center)
                                    .push(icon::circle_cross_icon().style(color::RED))
                                    .push(text("Not ready").style(color::RED))
                                    .width(Length::Fill),
                            )
                            .push(icon::collapsed_icon()),
                    )
                    .padding(15)
                    .width(Length::Fill)
                    .style(theme::Button::TransparentBorder)
                },
                move || {
                    Into::<Element<'a, Message>>::into(
                        Column::new()
                            .padding(15)
                            .spacing(10)
                            .push(text("Finalizing this transaction requires:"))
                            .push_maybe(if tx.sigs.recovery_paths().is_empty() {
                                Some(path_view(
                                    desc_info.primary_path(),
                                    tx.sigs.primary_path(),
                                    keys_aliases,
                                ))
                            } else {
                                tx.sigs.recovery_paths().iter().last().map(|(seq, path)| {
                                    let keys = &desc_info.recovery_paths()[seq];
                                    path_view(keys, path, keys_aliases)
                                })
                            }),
                    )
                },
            ))
        })
        .into()
}

pub fn path_view<'a>(
    path: &'a PathInfo,
    sigs: &'a PathSpendInfo,
    key_aliases: &'a HashMap<Fingerprint, String>,
) -> Element<'a, Message> {
    let mut keys: Vec<(Fingerprint, HashSet<DerivationPath>)> =
        path.thresh_origins().1.into_iter().collect();
    let missing_signatures = if sigs.sigs_count >= sigs.threshold {
        0
    } else {
        sigs.threshold - sigs.sigs_count
    };
    keys.sort_by_key(|a| a.0);
    scrollable(
        Row::new()
            .align_items(Alignment::Center)
            .push(
                Row::new()
                    .push(if sigs.sigs_count >= sigs.threshold {
                        icon::circle_check_icon().style(color::GREEN)
                    } else {
                        icon::circle_cross_icon().style(color::GREY_3)
                    })
                    .push(Space::with_width(Length::Fixed(20.0))),
            )
            .push(
                p1_regular(format!(
                    "{} more signature{}",
                    missing_signatures,
                    if missing_signatures > 1 {
                        "s from "
                    } else if missing_signatures == 0 {
                        ""
                    } else {
                        " from "
                    }
                ))
                .style(color::GREY_3),
            )
            .push_maybe(if keys.is_empty() {
                None
            } else {
                Some(
                    keys.iter()
                        .fold(Row::new().spacing(5), |row, (key_fg, paths)| {
                            row.push_maybe(
                                if !sigs.signed_pubkeys.iter().any(|(fg, &total_sigs)| {
                                    fg == key_fg && paths.len() == total_sigs
                                }) {
                                    Some(if let Some(alias) = key_aliases.get(key_fg) {
                                        Container::new(
                                            tooltip::Tooltip::new(
                                                Container::new(text(alias)).padding(10).style(
                                                    theme::Container::Pill(theme::Pill::Simple),
                                                ),
                                                key_fg.to_string(),
                                                tooltip::Position::Bottom,
                                            )
                                            .style(theme::Container::Card(theme::Card::Simple)),
                                        )
                                    } else {
                                        Container::new(text(key_fg.to_string()))
                                            .padding(10)
                                            .style(theme::Container::Pill(theme::Pill::Simple))
                                    })
                                } else {
                                    None
                                },
                            )
                        }),
                )
            })
            .push_maybe(if sigs.signed_pubkeys.is_empty() {
                None
            } else {
                Some(p1_regular(", already signed by ").style(color::GREY_3))
            })
            .push(
                sigs.signed_pubkeys
                    .keys()
                    .fold(Row::new().spacing(5), |row, value| {
                        row.push(if let Some(alias) = key_aliases.get(value) {
                            Container::new(
                                tooltip::Tooltip::new(
                                    Container::new(text(alias))
                                        .padding(10)
                                        .style(theme::Container::Pill(theme::Pill::Simple)),
                                    value.to_string(),
                                    tooltip::Position::Bottom,
                                )
                                .style(theme::Container::Card(theme::Card::Simple)),
                            )
                        } else {
                            Container::new(text(value.to_string()))
                                .padding(10)
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
    labels: &'a HashMap<String, String>,
    labels_editing: &'a HashMap<String, form::Value<String>>,
) -> Element<'a, Message> {
    let change_indexes_copy = change_indexes.clone();
    Column::new()
        .spacing(20)
        .push_maybe(if !coins.is_empty() {
            Some(
                Container::new(Collapse::new(
                    move || {
                        Button::new(
                            Row::new()
                                .align_items(Alignment::Center)
                                .push(
                                    h4_bold(format!(
                                        "{} coin{} spent",
                                        coins.len(),
                                        if coins.len() == 1 { "" } else { "s" }
                                    ))
                                    .width(Length::Fill),
                                )
                                .push(icon::collapse_icon()),
                        )
                        .padding(20)
                        .width(Length::Fill)
                        .style(theme::Button::TransparentBorder)
                    },
                    move || {
                        Button::new(
                            Row::new()
                                .align_items(Alignment::Center)
                                .push(
                                    h4_bold(format!(
                                        "{} coin{} spent",
                                        coins.len(),
                                        if coins.len() == 1 { "" } else { "s" }
                                    ))
                                    .width(Length::Fill),
                                )
                                .push(icon::collapsed_icon()),
                        )
                        .padding(20)
                        .width(Length::Fill)
                        .style(theme::Button::TransparentBorder)
                    },
                    move || {
                        coins
                            .iter()
                            .fold(
                                Column::new().spacing(10).padding(20),
                                |col: Column<'a, Message>, coin| {
                                    col.push(input_view(coin, labels, labels_editing))
                                },
                            )
                            .into()
                    },
                ))
                .style(theme::Container::Card(theme::Card::Simple)),
            )
        } else {
            None
        })
        .push({
            let count = tx
                .output
                .iter()
                .enumerate()
                .filter(|(i, _)| {
                    if let Some(indexes) = change_indexes_copy.as_ref() {
                        !indexes.contains(i)
                    } else {
                        true
                    }
                })
                .count();
            if count > 0 {
                Container::new(Collapse::new(
                    move || {
                        Button::new(
                            Row::new()
                                .align_items(Alignment::Center)
                                .push(
                                    h4_bold(format!(
                                        "{} payment{}",
                                        count,
                                        if count == 1 { "" } else { "s" }
                                    ))
                                    .width(Length::Fill),
                                )
                                .push(icon::collapse_icon()),
                        )
                        .padding(20)
                        .width(Length::Fill)
                        .style(theme::Button::TransparentBorder)
                    },
                    move || {
                        Button::new(
                            Row::new()
                                .align_items(Alignment::Center)
                                .push(
                                    h4_bold(format!(
                                        "{} payment{}",
                                        count,
                                        if count == 1 { "" } else { "s" }
                                    ))
                                    .width(Length::Fill),
                                )
                                .push(icon::collapsed_icon()),
                        )
                        .padding(20)
                        .width(Length::Fill)
                        .style(theme::Button::TransparentBorder)
                    },
                    move || {
                        tx.output
                            .iter()
                            .enumerate()
                            .filter(|(i, _)| {
                                if let Some(indexes) = change_indexes_copy.as_ref() {
                                    !indexes.contains(i)
                                } else {
                                    true
                                }
                            })
                            .fold(
                                Column::new().padding(20),
                                |col: Column<'a, Message>, (i, output)| {
                                    col.spacing(10).push(payment_view(
                                        i,
                                        tx.txid(),
                                        output,
                                        network,
                                        labels,
                                        labels_editing,
                                    ))
                                },
                            )
                            .into()
                    },
                ))
                .style(theme::Container::Card(theme::Card::Simple))
            } else {
                Container::new(h4_bold("0 payment"))
                    .padding(20)
                    .width(Length::Fill)
                    .style(theme::Container::Card(theme::Card::Simple))
            }
        })
        .push_maybe(
            if change_indexes
                .as_ref()
                .map(|indexes| !indexes.is_empty())
                .unwrap_or(false)
            {
                Some(
                    Container::new(Collapse::new(
                        move || {
                            Button::new(
                                Row::new()
                                    .align_items(Alignment::Center)
                                    .push(h4_bold("Change").width(Length::Fill))
                                    .push(icon::collapse_icon()),
                            )
                            .padding(20)
                            .width(Length::Fill)
                            .style(theme::Button::TransparentBorder)
                        },
                        move || {
                            Button::new(
                                Row::new()
                                    .align_items(Alignment::Center)
                                    .push(h4_bold("Change").width(Length::Fill))
                                    .push(icon::collapsed_icon()),
                            )
                            .padding(20)
                            .width(Length::Fill)
                            .style(theme::Button::TransparentBorder)
                        },
                        move || {
                            tx.output
                                .iter()
                                .enumerate()
                                .filter(|(i, _)| change_indexes.as_ref().unwrap().contains(i))
                                .fold(
                                    Column::new().padding(20),
                                    |col: Column<'a, Message>, (_, output)| {
                                        col.spacing(10).push(change_view(output, network))
                                    },
                                )
                                .into()
                        },
                    ))
                    .style(theme::Container::Card(theme::Card::Simple)),
                )
            } else {
                None
            },
        )
        .into()
}

fn input_view<'a>(
    coin: &'a Coin,
    labels: &'a HashMap<String, String>,
    labels_editing: &'a HashMap<String, form::Value<String>>,
) -> Element<'a, Message> {
    let outpoint = coin.outpoint.to_string();
    let addr = coin.address.to_string();
    Column::new()
        .width(Length::Fill)
        .push(
            Row::new()
                .spacing(5)
                .align_items(Alignment::Center)
                .push(
                    Container::new(if let Some(label) = labels_editing.get(&outpoint) {
                        label::label_editing(vec![outpoint.clone()], label, text::P1_SIZE)
                    } else {
                        label::label_editable(
                            vec![outpoint.clone()],
                            labels.get(&outpoint),
                            text::P1_SIZE,
                        )
                    })
                    .width(Length::Fill),
                )
                .push(amount(&coin.amount)),
        )
        .push(
            Column::new()
                .push(
                    Row::new()
                        .align_items(Alignment::Center)
                        .spacing(5)
                        .push(p1_bold("Outpoint:").style(color::GREY_3))
                        .push(p2_regular(outpoint.clone()).style(color::GREY_3))
                        .push(
                            Button::new(icon::clipboard_icon().style(color::GREY_3))
                                .on_press(Message::Clipboard(coin.outpoint.to_string()))
                                .style(theme::Button::TransparentBorder),
                        ),
                )
                .push(
                    Row::new()
                        .align_items(Alignment::Center)
                        .width(Length::Fill)
                        .push(
                            Row::new()
                                .align_items(Alignment::Center)
                                .width(Length::Fill)
                                .spacing(5)
                                .push(p1_bold("Address:").style(color::GREY_3))
                                .push(p2_regular(addr.clone()).style(color::GREY_3))
                                .push(
                                    Button::new(icon::clipboard_icon().style(color::GREY_3))
                                        .on_press(Message::Clipboard(addr.clone()))
                                        .style(theme::Button::TransparentBorder),
                                ),
                        ),
                )
                .push_maybe(labels.get(&addr).map(|label| {
                    Row::new()
                        .align_items(Alignment::Center)
                        .width(Length::Fill)
                        .push(
                            Row::new()
                                .align_items(Alignment::Center)
                                .width(Length::Fill)
                                .spacing(5)
                                .push(p1_bold("Address label:").style(color::GREY_3))
                                .push(p2_regular(label).style(color::GREY_3)),
                        )
                })),
        )
        .spacing(5)
        .into()
}

fn payment_view<'a>(
    i: usize,
    txid: Txid,
    output: &'a TxOut,
    network: Network,
    labels: &'a HashMap<String, String>,
    labels_editing: &'a HashMap<String, form::Value<String>>,
) -> Element<'a, Message> {
    let addr = Address::from_script(&output.script_pubkey, network)
        .unwrap()
        .to_string();
    let outpoint = OutPoint {
        txid,
        vout: i as u32,
    }
    .to_string();
    Column::new()
        .width(Length::Fill)
        .spacing(5)
        .push(
            Row::new()
                .spacing(5)
                .align_items(Alignment::Center)
                .push(
                    Container::new(if let Some(label) = labels_editing.get(&outpoint) {
                        label::label_editing(vec![outpoint.clone()], label, text::P1_SIZE)
                    } else {
                        label::label_editable(
                            vec![outpoint.clone()],
                            labels.get(&outpoint),
                            text::P1_SIZE,
                        )
                    })
                    .width(Length::Fill),
                )
                .push(amount(&Amount::from_sat(output.value))),
        )
        .push(
            Column::new()
                .push(
                    Row::new()
                        .align_items(Alignment::Center)
                        .width(Length::Fill)
                        .push(
                            Row::new()
                                .align_items(Alignment::Center)
                                .width(Length::Fill)
                                .spacing(5)
                                .push(p1_bold("Address:").style(color::GREY_3))
                                .push(p2_regular(addr.clone()).style(color::GREY_3))
                                .push(
                                    Button::new(icon::clipboard_icon().style(color::GREY_3))
                                        .on_press(Message::Clipboard(addr.clone()))
                                        .style(theme::Button::TransparentBorder),
                                ),
                        ),
                )
                .push_maybe(labels.get(&addr).map(|label| {
                    Row::new()
                        .align_items(Alignment::Center)
                        .width(Length::Fill)
                        .push(
                            Row::new()
                                .align_items(Alignment::Center)
                                .width(Length::Fill)
                                .spacing(5)
                                .push(p1_bold("Address label:").style(color::GREY_3))
                                .push(p2_regular(label).style(color::GREY_3)),
                        )
                })),
        )
        .into()
}

fn change_view(output: &TxOut, network: Network) -> Element<Message> {
    let addr = Address::from_script(&output.script_pubkey, network)
        .unwrap()
        .to_string();
    Row::new()
        .width(Length::Fill)
        .spacing(5)
        .push(
            Row::new()
                .align_items(Alignment::Center)
                .width(Length::Fill)
                .push(
                    Row::new()
                        .align_items(Alignment::Center)
                        .width(Length::Fill)
                        .spacing(5)
                        .push(p1_bold("Address:").style(color::GREY_3))
                        .push(p2_regular(addr.clone()).style(color::GREY_3))
                        .push(
                            Button::new(icon::clipboard_icon().style(color::GREY_3))
                                .on_press(Message::Clipboard(addr))
                                .style(theme::Button::TransparentBorder),
                        ),
                ),
        )
        .push(amount(&Amount::from_sat(output.value)))
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
                            text("Select signing device to sign with:")
                                .bold()
                                .width(Length::Fill),
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
        .width(Length::Fixed(500.0))
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
                            form::Form::new_trimmed("PSBT", updated, move |msg| {
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
        .width(Length::Fixed(400.0))
        .align_items(Alignment::Center)
        .into()
}
