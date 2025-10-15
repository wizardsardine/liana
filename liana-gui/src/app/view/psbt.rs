use std::collections::{HashMap, HashSet};

use iced::{
    widget::{scrollable, tooltip, Space},
    Alignment, Length,
};

use liana::descriptors::LianaDescriptor;
use liana::{
    descriptors::{LianaPolicy, PathInfo, PathSpendInfo},
    miniscript::bitcoin::{
        bip32::Fingerprint, blockdata::transaction::TxOut, Address, Network, OutPoint, Transaction,
        Txid,
    },
};

use liana_ui::{
    component::{
        amount::*,
        badge, button, card,
        collapse::Collapse,
        form, hw, separation,
        text::{self, *},
    },
    icon, theme,
    widget::*,
};
use lianad::payjoin::types::PayjoinStatus;

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
    currently_signing: bool,
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
                    .align_y(Alignment::Center)
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
            .push(spend_overview_view(
                tx,
                desc_info,
                key_aliases,
                currently_signing,
            ))
            .push(
                Column::new()
                    .spacing(20)
                    .push(inputs_view(
                        &tx.coins,
                        &tx.psbt.unsigned_tx,
                        &tx.labels,
                        labels_editing,
                    ))
                    .push(outputs_view(
                        &tx.psbt.unsigned_tx,
                        network,
                        Some(tx.change_indexes.clone()),
                        &tx.labels,
                        labels_editing,
                        tx.is_single_payment().is_some(),
                    )),
            )
            .push(if saved {
                Row::new()
                    .push(
                        button::secondary(None, "Delete")
                            .width(Length::Fixed(200.0))
                            .on_press_maybe(if currently_signing {
                                None
                            } else {
                                Some(Message::Spend(SpendTxMessage::Delete))
                            }),
                    )
                    .width(Length::Fill)
            } else {
                Row::new()
                    .push(Space::with_width(Length::Fill))
                    .push(
                        button::secondary(None, "Save")
                            .width(Length::Fixed(150.0))
                            .on_press_maybe(if currently_signing {
                                None
                            } else {
                                Some(Message::Spend(SpendTxMessage::Save))
                            }),
                    )
                    .width(Length::Fill)
            })
            .push(Space::with_height(10)),
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
                        .spacing(10)
                        .push(Space::with_width(Length::Fill))
                        .push(button::secondary(None, "Ignore").on_press(Message::Close))
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

/// Return the modal view to broadcast a transaction.
///
/// `conflicting_txids` contains the IDs of any directly conflicting transactions
/// of the transaction to be broadcast.
pub fn broadcast_action<'a>(
    conflicting_txids: &HashSet<Txid>,
    warning: Option<&Error>,
    saved: bool,
) -> Element<'a, Message> {
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
                .push(Container::new(h4_bold("Broadcast the transaction")).width(Length::Fill))
                .push_maybe(if conflicting_txids.is_empty() {
                    None
                } else {
                    Some(
                        conflicting_txids.iter().fold(
                            Column::new()
                                .spacing(5)
                                .push(Row::new().spacing(10).push(icon::warning_icon()).push(text(
                                    if conflicting_txids.len() > 1 {
                                        "WARNING: Broadcasting this transaction \
                                        will invalidate some pending payments."
                                    } else {
                                        "WARNING: Broadcasting this transaction \
                                        will invalidate a pending payment."
                                    },
                                )))
                                .push(Row::new().padding([0, 30]).push(text(
                                    if conflicting_txids.len() > 1 {
                                        "The following transactions are \
                                        spending one or more inputs \
                                        from the transaction to be \
                                        broadcast and will be \
                                        dropped, along with any other \
                                        transactions that depend on them:"
                                    } else {
                                        "The following transaction is \
                                        spending one or more inputs \
                                        from the transaction to be \
                                        broadcast and will be \
                                        dropped, along with any other \
                                        transactions that depend on it:"
                                    },
                                ))),
                            |col, txid| {
                                col.push(
                                    Row::new()
                                        .padding([0, 30])
                                        .spacing(5)
                                        .align_y(Alignment::Center)
                                        .push(text(txid.to_string()))
                                        .push(
                                            Button::new(
                                                icon::clipboard_icon()
                                                    .style(theme::text::secondary),
                                            )
                                            .on_press(Message::Clipboard(txid.to_string()))
                                            .style(theme::button::transparent_border),
                                        ),
                                )
                            },
                        ),
                    )
                })
                .push(
                    Row::new().push(Column::new().width(Length::Fill)).push(
                        button::primary(None, "Broadcast")
                            .on_press(Message::Spend(SpendTxMessage::Confirm)),
                    ),
                ),
        )
        .width(Length::Fixed(if conflicting_txids.is_empty() {
            400.0
        } else {
            800.0
        }))
        .into()
    }
}

pub fn delete_action<'a>(warning: Option<&Error>, deleted: bool) -> Element<'a, Message> {
    if deleted {
        card::simple(
            Column::new()
                .spacing(20)
                .align_x(Alignment::Center)
                .push(text("Successfully deleted this transaction."))
                .push(button::secondary(None, "Go back to PSBTs").on_press(Message::Close)),
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
    let txid = tx.psbt.unsigned_tx.compute_txid().to_string();
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
                        .align_y(Alignment::Center)
                        .push(h3("Miner fee: ").style(theme::text::secondary))
                        .push_maybe(if tx.fee_amount.is_none() {
                            Some(text("Missing information about transaction inputs"))
                        } else {
                            None
                        })
                        .push_maybe(tx.fee_amount.map(|fee| amount_with_size(&fee, H3_SIZE)))
                        .push(text(" ").size(H3_SIZE))
                        .push_maybe(tx.min_feerate_vb().map(|rate| {
                            text(format!("(~{} sats/vbyte)", &rate))
                                .size(H4_SIZE)
                                .style(theme::text::secondary)
                        })),
                ),
        )
        .into()
}

pub fn payjoin_send_success_view<'a>() -> Element<'a, Message> {
    card::simple(text("Payjoin sent successfully")).into()
}

pub fn spend_overview_view<'a>(
    tx: &'a SpendTx,
    desc_info: &'a LianaPolicy,
    key_aliases: &'a HashMap<Fingerprint, String>,
    currently_signing: bool,
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
                                    .align_y(Alignment::Center)
                                    .push(text("PSBT").bold().width(Length::Fill))
                                    .push(
                                        Row::new()
                                            .spacing(5)
                                            .push(
                                                button::secondary(
                                                    Some(icon::backup_icon()),
                                                    "Export",
                                                )
                                                .on_press_maybe(if currently_signing {
                                                    None
                                                } else {
                                                    Some(Message::ExportPsbt)
                                                }),
                                            )
                                            .push(
                                                button::secondary(
                                                    Some(icon::restore_icon()),
                                                    "Import",
                                                )
                                                .on_press_maybe(if currently_signing {
                                                    None
                                                } else {
                                                    Some(Message::ImportPsbt)
                                                }),
                                            ),
                                    )
                                    .align_y(Alignment::Center),
                            )
                            .push(
                                Row::new()
                                    .push(p1_bold("Tx ID").width(Length::Fill))
                                    .push(
                                        p2_regular(tx.psbt.unsigned_tx.compute_txid().to_string())
                                            .style(theme::text::secondary),
                                    )
                                    .push(
                                        Button::new(
                                            icon::clipboard_icon().style(theme::text::secondary),
                                        )
                                        .on_press(Message::Clipboard(
                                            tx.psbt.unsigned_tx.compute_txid().to_string(),
                                        ))
                                        .style(theme::button::transparent_border),
                                    )
                                    .align_y(Alignment::Center),
                            ),
                    )
                    .push(signatures(tx, desc_info, key_aliases)),
            )
            .style(theme::card::simple),
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
                    .push_maybe(if tx.path_ready().is_some() {
                        if let Some(payjoin_status) = &tx.payjoin_status {
                            if *payjoin_status == PayjoinStatus::Pending {
                                Some(
                                    button::secondary(None, "Send Payjoin")
                                        .on_press(Message::Spend(SpendTxMessage::SendPayjoin))
                                        .width(Length::Fixed(150.0)),
                                )
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    } else {
                        None
                    })
                    .align_y(Alignment::Center)
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
        .push(if tx.status == SpendStatus::PayjoinInitiated {
            Container::new(
                scrollable(
                    Row::new()
                        .spacing(5)
                        .align_y(Alignment::Center)
                        .spacing(10)
                        .push(p1_bold("Status"))
                        .push(icon::circle_check_icon().style(theme::text::payjoin))
                        .push(text("Payjoin Initiated").bold().style(theme::text::payjoin)),
                )
                .direction(scrollable::Direction::Horizontal(
                    scrollable::Scrollbar::new().width(2).scroller_width(2),
                )),
            )
            .padding(15)
        } else if tx.status == SpendStatus::PayjoinProposalReady {
            Container::new(
                scrollable(
                    Row::new()
                        .spacing(5)
                        .align_y(Alignment::Center)
                        .spacing(10)
                        .push(p1_bold("Status"))
                        .push(icon::circle_check_icon().style(theme::text::payjoin))
                        .push(
                            text("Payjoin Proposal Ready For Signing")
                                .bold()
                                .style(theme::text::payjoin),
                        ),
                )
                .direction(scrollable::Direction::Horizontal(
                    scrollable::Scrollbar::new().width(2).scroller_width(2),
                )),
            )
            .padding(15)
        } else if let Some(sigs) = tx.path_ready() {
            Container::new(
                scrollable(
                    Row::new()
                        .spacing(5)
                        .align_y(Alignment::Center)
                        .spacing(10)
                        .push(p1_bold("Status"))
                        .push(icon::circle_check_icon().style(theme::text::success))
                        .push(text("Ready").bold().style(theme::text::success))
                        .push(text("  signed by"))
                        .push(sigs.signed_pubkeys.keys().fold(
                            Row::new().spacing(5),
                            |row, value| {
                                row.push(if let Some(alias) = keys_aliases.get(value) {
                                    Container::new(tooltip::Tooltip::new(
                                        Container::new(text(alias))
                                            .padding(10)
                                            .style(theme::pill::simple),
                                        text(value.to_string()),
                                        tooltip::Position::Bottom,
                                    ))
                                } else {
                                    Container::new(text(value.to_string()))
                                        .padding(10)
                                        .style(theme::pill::simple)
                                })
                            },
                        )),
                )
                .direction(scrollable::Direction::Horizontal(
                    scrollable::Scrollbar::new().width(2).scroller_width(2),
                )),
            )
            .padding(15)
        } else {
            Container::new(Collapse::new(
                move || {
                    Button::new(
                        Row::new()
                            .align_y(Alignment::Center)
                            .spacing(20)
                            .push(p1_bold("Status"))
                            .push(
                                Row::new()
                                    .spacing(5)
                                    .align_y(Alignment::Center)
                                    .push(icon::circle_cross_icon().style(theme::text::error))
                                    .push(text("Not ready").style(theme::text::error))
                                    .width(Length::Fill),
                            )
                            .push(icon::collapse_icon()),
                    )
                    .padding(15)
                    .width(Length::Fill)
                    .style(theme::button::transparent_border)
                },
                move || {
                    Button::new(
                        Row::new()
                            .align_y(Alignment::Center)
                            .spacing(20)
                            .push(p1_bold("Status"))
                            .push(
                                Row::new()
                                    .spacing(5)
                                    .align_y(Alignment::Center)
                                    .push(icon::circle_cross_icon().style(theme::text::error))
                                    .push(text("Not ready").style(theme::text::error))
                                    .width(Length::Fill),
                            )
                            .push(icon::collapsed_icon()),
                    )
                    .padding(15)
                    .width(Length::Fill)
                    .style(theme::button::transparent_border)
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

// Display a fingerprint first by its alias if there is any, or in hex otherwise.
fn container_from_fg(
    fg: Fingerprint,
    aliases: &HashMap<Fingerprint, String>,
) -> Container<Message> {
    if let Some(alias) = aliases.get(&fg) {
        Container::new(
            tooltip::Tooltip::new(
                Container::new(text(alias))
                    .padding(10)
                    .style(theme::pill::simple),
                liana_ui::widget::Text::new(fg.to_string()),
                tooltip::Position::Bottom,
            )
            .style(theme::card::simple),
        )
    } else {
        Container::new(text(fg.to_string()))
            .padding(10)
            .style(theme::pill::simple)
    }
}

pub fn path_view<'a>(
    path: &'a PathInfo,
    sigs: &'a PathSpendInfo,
    key_aliases: &'a HashMap<Fingerprint, String>,
) -> Element<'a, Message> {
    // We get a sorted list of all the fingerprints (which correspond to a signer) from this
    // spending path, and from it get an iterator on those of these fingerprints for which a
    // signature was provided in the PSBT, and those for which there isn't any.
    let mut all_fgs: Vec<Fingerprint> = path.thresh_origins().1.into_keys().collect();
    all_fgs.sort();
    let signed_fgs = sigs.signed_pubkeys.keys();
    let non_signed_fgs = all_fgs
        .into_iter()
        .filter(|fg| !sigs.signed_pubkeys.contains_key(fg));
    let missing_signatures = sigs.threshold.saturating_sub(sigs.sigs_count);

    // From these iterators, create the appropriate rows to be displayed.
    let row_unsigned = non_signed_fgs.into_iter().fold(None, |row, fg| {
        Some(
            row.unwrap_or_else(|| Row::new().spacing(5))
                .push(container_from_fg(fg, key_aliases)),
        )
    });
    let row_signed = signed_fgs
        .into_iter()
        .fold(Row::new().spacing(5), |row, fg| {
            row.push(container_from_fg(*fg, key_aliases))
        });

    scrollable(
        Row::new()
            .align_y(Alignment::Center)
            .push(
                Row::new()
                    .push(if missing_signatures == 0 {
                        icon::circle_check_icon().style(theme::text::success)
                    } else {
                        icon::circle_cross_icon().style(theme::text::secondary)
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
                .style(theme::text::secondary),
            )
            .push_maybe(row_unsigned)
            .push_maybe(
                (!sigs.signed_pubkeys.is_empty())
                    .then_some(p1_regular(", already signed by ").style(theme::text::secondary)),
            )
            .push(row_signed),
    )
    .direction(scrollable::Direction::Horizontal(
        scrollable::Scrollbar::new().width(2).scroller_width(2),
    ))
    .into()
}

pub fn inputs_view<'a>(
    coins: &'a HashMap<OutPoint, Coin>,
    tx: &'a Transaction,
    labels: &'a HashMap<String, String>,
    labels_editing: &'a HashMap<String, form::Value<String>>,
) -> Element<'a, Message> {
    Container::new(Collapse::new(
        move || {
            Button::new(
                Row::new()
                    .align_y(Alignment::Center)
                    .push(
                        h4_bold(format!(
                            "{} coin{} spent",
                            tx.input.len(),
                            if tx.input.len() == 1 { "" } else { "s" }
                        ))
                        .width(Length::Fill),
                    )
                    .push(icon::collapse_icon()),
            )
            .padding(20)
            .width(Length::Fill)
            .style(theme::button::transparent_border)
        },
        move || {
            Button::new(
                Row::new()
                    .align_y(Alignment::Center)
                    .push(
                        h4_bold(format!(
                            "{} coin{} spent",
                            tx.input.len(),
                            if tx.input.len() == 1 { "" } else { "s" }
                        ))
                        .width(Length::Fill),
                    )
                    .push(icon::collapsed_icon()),
            )
            .padding(20)
            .width(Length::Fill)
            .style(theme::button::transparent_border)
        },
        move || {
            tx.input
                .iter()
                .fold(
                    Column::new().spacing(10).padding(20),
                    |col: Column<'a, Message>, input| {
                        col.push(input_view(
                            &input.previous_output,
                            coins.get(&input.previous_output),
                            labels,
                            labels_editing,
                        ))
                    },
                )
                .into()
        },
    ))
    .style(theme::card::simple)
    .into()
}

pub fn outputs_view<'a>(
    tx: &'a Transaction,
    network: Network,
    change_indexes: Option<Vec<usize>>,
    labels: &'a HashMap<String, String>,
    labels_editing: &'a HashMap<String, form::Value<String>>,
    is_single_payment: bool,
) -> Element<'a, Message> {
    let change_indexes_copy = change_indexes.clone();
    Column::new()
        .spacing(20)
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
                                .align_y(Alignment::Center)
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
                        .style(theme::button::transparent_border)
                    },
                    move || {
                        Button::new(
                            Row::new()
                                .align_y(Alignment::Center)
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
                        .style(theme::button::transparent_border)
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
                                        tx.compute_txid(),
                                        output,
                                        network,
                                        labels,
                                        labels_editing,
                                        is_single_payment,
                                    ))
                                },
                            )
                            .into()
                    },
                ))
                .style(theme::card::simple)
            } else {
                Container::new(h4_bold("0 payment").style(|t| {
                    theme::text::custom(t.colors.buttons.transparent_border.active.text)
                }))
                .padding(20)
                .width(Length::Fill)
                .style(theme::card::simple)
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
                                    .align_y(Alignment::Center)
                                    .push(h4_bold("Change").width(Length::Fill))
                                    .push(icon::collapse_icon()),
                            )
                            .padding(20)
                            .width(Length::Fill)
                            .style(theme::button::transparent_border)
                        },
                        move || {
                            Button::new(
                                Row::new()
                                    .align_y(Alignment::Center)
                                    .push(h4_bold("Change").width(Length::Fill))
                                    .push(icon::collapsed_icon()),
                            )
                            .padding(20)
                            .width(Length::Fill)
                            .style(theme::button::transparent_border)
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
                    .style(theme::card::simple),
                )
            } else {
                None
            },
        )
        .into()
}

fn input_view<'a>(
    outpoint: &'a OutPoint,
    coin: Option<&'a Coin>,
    labels: &'a HashMap<String, String>,
    labels_editing: &'a HashMap<String, form::Value<String>>,
) -> Element<'a, Message> {
    let outpoint = outpoint.to_string();
    Column::new()
        .width(Length::Fill)
        .push(
            Row::new()
                .spacing(5)
                .align_y(Alignment::Center)
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
                .push_maybe(coin.map(|c| amount(&c.amount))),
        )
        .push(
            Column::new()
                .push(
                    Row::new()
                        .align_y(Alignment::Center)
                        .spacing(5)
                        .push(p1_bold("Outpoint:").style(theme::text::secondary))
                        .push(p2_regular(outpoint.clone()).style(theme::text::secondary))
                        .push(
                            Button::new(icon::clipboard_icon().style(theme::text::secondary))
                                .on_press(Message::Clipboard(outpoint.clone()))
                                .style(theme::button::transparent_border),
                        ),
                )
                .push_maybe(coin.map(|c| {
                    let addr = c.address.to_string();
                    Row::new()
                        .align_y(Alignment::Center)
                        .width(Length::Fill)
                        .push(
                            Row::new()
                                .align_y(Alignment::Center)
                                .width(Length::Fill)
                                .spacing(5)
                                .push(p1_bold("Address:").style(theme::text::secondary))
                                .push(p2_regular(addr.clone()).style(theme::text::secondary))
                                .push(
                                    Button::new(
                                        icon::clipboard_icon().style(theme::text::secondary),
                                    )
                                    .on_press(Message::Clipboard(addr))
                                    .style(theme::button::transparent_border),
                                ),
                        )
                }))
                .push_maybe(coin.and_then(|c| {
                    labels.get(&c.address.to_string()).map(|label| {
                        Row::new()
                            .align_y(Alignment::Center)
                            .width(Length::Fill)
                            .push(
                                Row::new()
                                    .align_y(Alignment::Center)
                                    .width(Length::Fill)
                                    .spacing(5)
                                    .push(p1_bold("Address label:").style(theme::text::secondary))
                                    .push(p2_regular(label).style(theme::text::secondary)),
                            )
                    })
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
    is_single: bool,
) -> Element<'a, Message> {
    let addr = Address::from_script(&output.script_pubkey, network)
        .ok()
        .map(|a| a.to_string());
    let outpoint = OutPoint {
        txid,
        vout: i as u32,
    }
    .to_string();
    // if the payment is single in the transaction, then the label of the txid
    // is attached to the label of the payment.
    let change_labels = if is_single {
        vec![outpoint.clone(), txid.to_string()]
    } else {
        vec![outpoint.clone()]
    };
    Column::new()
        .width(Length::Fill)
        .spacing(5)
        .push(
            Row::new()
                .spacing(5)
                .align_y(Alignment::Center)
                .push(
                    Container::new(if let Some(label) = labels_editing.get(&outpoint) {
                        label::label_editing(change_labels, label, text::P1_SIZE)
                    } else {
                        label::label_editable(change_labels, labels.get(&outpoint), text::P1_SIZE)
                    })
                    .width(Length::Fill),
                )
                .push(amount(&output.value)),
        )
        .push_maybe(addr.map(|addr| {
            Column::new()
                .push(
                    Row::new()
                        .align_y(Alignment::Center)
                        .width(Length::Fill)
                        .push(
                            Row::new()
                                .align_y(Alignment::Center)
                                .width(Length::Fill)
                                .spacing(5)
                                .push(p1_bold("Address:").style(theme::text::secondary))
                                .push(p2_regular(addr.clone()).style(theme::text::secondary))
                                .push(
                                    Button::new(
                                        icon::clipboard_icon().style(theme::text::secondary),
                                    )
                                    .on_press(Message::Clipboard(addr.clone()))
                                    .style(theme::button::transparent_border),
                                ),
                        ),
                )
                .push_maybe(labels.get(&addr).map(|label| {
                    Row::new()
                        .align_y(Alignment::Center)
                        .width(Length::Fill)
                        .push(
                            Row::new()
                                .align_y(Alignment::Center)
                                .width(Length::Fill)
                                .spacing(5)
                                .push(p1_bold("Address label:").style(theme::text::secondary))
                                .push(p2_regular(label).style(theme::text::secondary)),
                        )
                }))
        }))
        .into()
}

fn change_view(output: &TxOut, network: Network) -> Element<Message> {
    let addr = Address::from_script(&output.script_pubkey, network)
        .unwrap()
        .to_string();
    Column::new()
        .width(Length::Fill)
        .spacing(5)
        .push(
            Row::new()
                .push(Space::with_width(Length::Fill))
                .push(amount(&output.value)),
        )
        .push(
            Row::new()
                .align_y(Alignment::Center)
                .width(Length::Fill)
                .push(
                    Row::new()
                        .align_y(Alignment::Center)
                        .width(Length::Fill)
                        .spacing(5)
                        .push(p1_bold("Address:").style(theme::text::secondary))
                        .push(p2_regular(addr.clone()).style(theme::text::secondary))
                        .push(
                            Button::new(icon::clipboard_icon().style(theme::text::secondary))
                                .on_press(Message::Clipboard(addr))
                                .style(theme::button::transparent_border),
                        ),
                ),
        )
        .into()
}

#[allow(clippy::too_many_arguments)]
pub fn sign_action<'a>(
    warning: Option<&Error>,
    hws: &'a [HardwareWallet],
    descriptor: &LianaDescriptor,
    signer: Option<Fingerprint>,
    signer_alias: Option<&'a String>,
    signed: &HashSet<Fingerprint>,
    signing: &HashSet<Fingerprint>,
    recovery_timelock: Option<u16>,
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
                                let (signed, signing, can_sign) =
                                    hw.fingerprint().map_or((false, false, false), |f| {
                                        (
                                            signed.contains(&f),
                                            signing.contains(&f),
                                            descriptor
                                                .contains_fingerprint_in_path(f, recovery_timelock),
                                        )
                                    });
                                col.push(hw_list_view(i, hw, signed, signing, can_sign))
                            },
                        ))
                        .push_maybe({
                            signer.map(|fingerprint| {
                                let can_sign = descriptor
                                    .contains_fingerprint_in_path(fingerprint, recovery_timelock);
                                let btn = Button::new(if signed.contains(&fingerprint) {
                                    hw::sign_success_hot_signer(fingerprint, signer_alias)
                                } else {
                                    hw::hot_signer(fingerprint, signer_alias, can_sign)
                                })
                                .padding(10)
                                .style(theme::button::secondary)
                                .width(Length::Fill);
                                if can_sign {
                                    btn.on_press(Message::Spend(SpendTxMessage::SelectHotSigner))
                                } else {
                                    btn
                                }
                            })
                        })
                        .width(Length::Fill),
                )
                .spacing(20)
                .width(Length::Fill)
                .align_x(Alignment::Center),
        ))
        .width(Length::Fixed(500.0))
        .into()
}

pub fn sign_action_toasts<'a>(
    error: Option<&Error>,
    hws: &'a [HardwareWallet],
    signing: &HashSet<Fingerprint>,
) -> Vec<Element<'a, Message>> {
    let mut vec: Vec<Element<'a, Message>> = hws
        .iter()
        .filter_map(|hw| {
            if let HardwareWallet::Supported {
                kind,
                fingerprint,
                version,
                alias,
                ..
            } = &hw
            {
                if signing.contains(fingerprint) {
                    Some(
                        liana_ui::component::notification::processing_hardware_wallet(
                            kind,
                            version.as_ref(),
                            fingerprint,
                            alias.as_ref().map(|x| x.as_str()),
                        )
                        .max_width(400.0)
                        .into(),
                    )
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect();
    if let Some(e) = error {
        vec.push(
            liana_ui::component::notification::processing_hardware_wallet_error(
                "Device failed to sign".to_string(),
                e.to_string(),
            )
            .max_width(400.0)
            .into(),
        )
    }

    vec
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
                            button::secondary(Some(icon::clipboard_icon()), "Copy")
                                .on_press(Message::Clipboard(psbt)),
                        )
                        .align_y(Alignment::Center),
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
                            .size(P1_SIZE)
                            .padding(10),
                        )
                        .push(Row::new().push(Space::with_width(Length::Fill)).push(
                            if updated.valid && !updated.value.is_empty() && !processing {
                                button::secondary(None, "Update")
                                    .on_press(Message::ImportSpend(ImportSpendMessage::Confirm))
                            } else if processing {
                                button::secondary(None, "Processing...")
                            } else {
                                button::secondary(None, "Update")
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
                text("Spend transaction is updated").style(theme::text::secondary),
            ))
            .padding(50),
        )
        .width(Length::Fixed(400.0))
        .align_x(Alignment::Center)
        .into()
}
