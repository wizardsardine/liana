use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Local, Utc};
use iced::{
    alignment,
    widget::{tooltip, Space},
    Alignment, Length,
};

use liana_ui::{
    component::{amount::*, badge, button, card, form, text::*},
    icon, theme,
    widget::*,
};

use crate::{
    app::{
        cache::Cache,
        error::Error,
        menu::Menu,
        view::{
            dashboard, label,
            message::{CreateRbfMessage, Message},
            warning::warn,
        },
    },
    daemon::model::{HistoryTransaction, Txid},
    export::ImportExportMessage,
};

pub fn transactions_view<'a>(
    cache: &'a Cache,
    txs: &'a [HistoryTransaction],
    warning: Option<&'a Error>,
    is_last_page: bool,
    processing: bool,
) -> Element<'a, Message> {
    dashboard(
        &Menu::Transactions,
        cache,
        warning,
        Column::new()
            .push(
                Row::new()
                    .push(Container::new(h3("Transactions")))
                    .push(Space::with_width(Length::Fill))
                    .push(
                        button::secondary(Some(icon::backup_icon()), "Export")
                            .on_press(ImportExportMessage::Open.into()),
                    ),
            )
            .push(
                Column::new()
                    .spacing(10)
                    .push(
                        txs.iter()
                            .enumerate()
                            .fold(Column::new().spacing(10), |col, (i, tx)| {
                                col.push(tx_list_view(i, tx))
                            }),
                    )
                    .push_maybe(if !is_last_page && !txs.is_empty() {
                        Some(
                            Container::new(
                                Button::new(
                                    text(if processing {
                                        "Fetching ..."
                                    } else {
                                        "See more"
                                    })
                                    .width(Length::Fill)
                                    .align_x(alignment::Horizontal::Center),
                                )
                                .width(Length::Fill)
                                .padding(15)
                                .style(theme::button::transparent_border)
                                .on_press_maybe(if !processing {
                                    Some(Message::Next)
                                } else {
                                    None
                                }),
                            )
                            .width(Length::Fill)
                            .style(theme::card::simple),
                        )
                    } else {
                        None
                    }),
            )
            .align_x(Alignment::Center)
            .spacing(30),
    )
}

fn tx_list_view(i: usize, tx: &HistoryTransaction) -> Element<'_, Message> {
    Container::new(
        Button::new(
            Row::new()
                .push(
                    Row::new()
                        .push(if tx.is_external() {
                            badge::receive()
                        } else if tx.is_send_to_self() {
                            badge::cycle()
                        } else {
                            badge::spend()
                        })
                        .push(
                            Column::new()
                                .push_maybe(if let Some(outpoint) = tx.is_single_payment() {
                                    tx.labels.get(&outpoint.to_string()).map(p1_regular)
                                } else {
                                    tx.labels
                                        .get(&tx.tx.compute_txid().to_string())
                                        .map(p1_regular)
                                })
                                .push_maybe(tx.time.map(|t| {
                                    Container::new(
                                        text(
                                            DateTime::<Utc>::from_timestamp(t as i64, 0)
                                                .expect("Correct unix timestamp")
                                                .with_timezone(&Local)
                                                .format("%b. %d, %Y - %T")
                                                .to_string(),
                                        )
                                        .style(theme::text::secondary)
                                        .small(),
                                    )
                                })),
                        )
                        .spacing(10)
                        .align_y(Alignment::Center)
                        .width(Length::Fill),
                )
                .push_maybe(if tx.time.is_none() {
                    Some(badge::unconfirmed())
                } else {
                    None
                })
                .push_maybe(if tx.is_batch() {
                    Some(badge::batch())
                } else {
                    None
                })
                .push(if tx.is_external() {
                    Row::new()
                        .spacing(5)
                        .push(text("+"))
                        .push(amount(&tx.incoming_amount))
                        .align_y(Alignment::Center)
                } else if tx.outgoing_amount != Amount::from_sat(0) {
                    Row::new()
                        .spacing(5)
                        .push(text("-"))
                        .push(amount(&tx.outgoing_amount))
                        .align_y(Alignment::Center)
                } else {
                    Row::new().push(text("Self-transfer"))
                })
                .align_y(Alignment::Center)
                .spacing(20),
        )
        .padding(10)
        .on_press(Message::Select(i))
        .style(theme::button::transparent_border),
    )
    .style(theme::card::simple)
    .into()
}

/// Return the modal view for a new RBF transaction.
///
/// `descendant_txids` contains the IDs of any transactions from this wallet that are
/// direct descendants of the transaction to be replaced.
pub fn create_rbf_modal<'a>(
    is_cancel: bool,
    descendant_txids: &HashSet<Txid>,
    feerate: &form::Value<String>,
    replacement_txid: Option<Txid>,
    warning: Option<&'a Error>,
) -> Element<'a, Message> {
    let mut confirm_button = button::secondary(None, "Confirm").width(Length::Fixed(200.0));
    if feerate.valid || is_cancel {
        confirm_button =
            confirm_button.on_press(Message::CreateRbf(super::CreateRbfMessage::Confirm));
    }
    let help_text = if is_cancel {
        "Replace the transaction with one paying a higher feerate \
        that sends the coins back to your wallet. There is no guarantee \
        the original transaction won't get mined first. New inputs may \
        be used for the replacement transaction."
    } else {
        "Replace the transaction with one paying a higher feerate \
        to incentivize faster confirmation. New inputs may be used \
        for the replacement transaction."
    };
    card::simple(
        Column::new()
            .spacing(10)
            .push(Container::new(h4_bold("Transaction replacement")).width(Length::Fill))
            .push(Row::new().push(text(help_text)))
            .push_maybe(if descendant_txids.is_empty() {
                None
            } else {
                Some(
                    descendant_txids.iter().fold(
                        Column::new()
                            .spacing(5)
                            .push(Row::new().spacing(10).push(icon::warning_icon()).push(text(
                                if descendant_txids.len() > 1 {
                                    "WARNING: Replacing this transaction \
                                    will invalidate some later payments."
                                } else {
                                    "WARNING: Replacing this transaction \
                                    will invalidate a later payment."
                                },
                            )))
                            .push(Row::new().padding([0, 30]).push(text(
                                if descendant_txids.len() > 1 {
                                    "The following transactions are \
                                    spending one or more outputs \
                                    from the transaction to be replaced \
                                    and will be dropped when the replacement \
                                    is broadcast, along with any other \
                                    transactions that depend on them:"
                                } else {
                                    "The following transaction is \
                                    spending one or more outputs \
                                    from the transaction to be replaced \
                                    and will be dropped when the replacement \
                                    is broadcast, along with any other \
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
                                            icon::clipboard_icon().style(theme::text::secondary),
                                        )
                                        .on_press(Message::Clipboard(txid.to_string()))
                                        .style(theme::button::transparent_border),
                                    ),
                            )
                        },
                    ),
                )
            })
            .push_maybe(if !is_cancel {
                Some(
                    Row::new()
                        .push(Container::new(p1_bold("Feerate")).padding(10))
                        .spacing(10)
                        .push(
                            if replacement_txid.is_none() {
                                form::Form::new_trimmed("", feerate, move |msg| {
                                    Message::CreateRbf(CreateRbfMessage::FeerateEdited(msg))
                                })
                                .warning(
                                    "Feerate must be greater than previous value and \
                                    less than or equal to 1000 sats/vbyte",
                                )
                            } else {
                                form::Form::new_disabled("", feerate)
                            }
                            .size(P1_SIZE)
                            .padding(10),
                        )
                        .width(Length::Fill),
                )
            } else {
                None
            })
            .push(warn(warning))
            .push(Row::new().push(if replacement_txid.is_none() {
                Row::new().push(confirm_button)
            } else {
                Row::new()
                    .spacing(10)
                    .align_y(Alignment::Center)
                    .push(icon::circle_check_icon().style(theme::text::secondary))
                    .push(
                        text("Replacement PSBT created successfully and ready to be signed")
                            .style(theme::text::success),
                    )
            }))
            .push_maybe(replacement_txid.map(|id| {
                Row::new().push(
                    button::primary(None, "Go to replacement")
                        .width(Length::Fixed(200.0))
                        .on_press(Message::Menu(Menu::PsbtPreSelected(id))),
                )
            })),
    )
    .width(Length::Fixed(800.0))
    .into()
}

pub fn tx_view<'a>(
    cache: &'a Cache,
    tx: &'a HistoryTransaction,
    labels_editing: &'a HashMap<String, form::Value<String>>,
    warning: Option<&'a Error>,
) -> Element<'a, Message> {
    let txid = tx.tx.compute_txid().to_string();
    dashboard(
        &Menu::Transactions,
        cache,
        warning,
        Column::new()
            .push(if tx.is_send_to_self() {
                Container::new(h3("Transaction")).width(Length::Fill)
            } else if tx.is_external() {
                Container::new(h3("Incoming transaction")).width(Length::Fill)
            } else {
                Container::new(h3("Outgoing transaction")).width(Length::Fill)
            })
            .push(if let Some(outpoint) = tx.is_single_payment() {
                // if the payment is a payment of a single payment transaction then
                // the label of the transaction is attached to the label of the payment outpoint
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
                label::label_editable(vec![txid.clone()], tx.labels.get(&txid), H1_SIZE)
            })
            .push(
                Column::new().spacing(20).push(
                    Column::new()
                        .push(if tx.is_send_to_self() {
                            Container::new(h1("Self-transfer"))
                        } else if tx.is_external() {
                            Container::new(amount_with_size(&tx.incoming_amount, H1_SIZE))
                        } else {
                            Container::new(amount_with_size(&tx.outgoing_amount, H1_SIZE))
                        })
                        .push_maybe(tx.fee_amount.map(|fee_amount| {
                            Row::new()
                                .align_y(Alignment::Center)
                                .push(h3("Miner fee: ").style(theme::text::secondary))
                                .push(amount_with_size(&fee_amount, H3_SIZE))
                                .push(text(" ").size(H3_SIZE))
                                .push(
                                    text(format!(
                                        "({} sats/vbyte)",
                                        fee_amount.to_sat() / tx.tx.vsize() as u64
                                    ))
                                    .size(H4_SIZE)
                                    .style(theme::text::secondary),
                                )
                        })),
                ),
            )
            // If unconfirmed, give option to use RBF.
            // Check fee amount is some as otherwise we may be missing coins for this transaction.
            .push_maybe(if tx.time.is_none() && tx.fee_amount.is_some() {
                Some(
                    Row::new()
                        .push(
                            button::secondary(None, "Bump fee")
                                .width(Length::Fixed(200.0))
                                .on_press(Message::CreateRbf(super::CreateRbfMessage::New(false))),
                        )
                        .push(
                            tooltip::Tooltip::new(
                                button::secondary(None, "Cancel transaction")
                                .width(Length::Fixed(200.0))
                                .on_press(Message::CreateRbf(super::CreateRbfMessage::New(true))),
                                "Best effort attempt at double spending an unconfirmed outgoing transaction",
                                tooltip::Position::Top,
                            )
                        )
                        .spacing(10),
                )
            } else {
                None
            })
            .push(card::simple(
                Column::new()
                    .push_maybe(tx.time.map(|t| {
                        let date = DateTime::<Utc>::from_timestamp(t as i64, 0)
                                        .expect("Correct unix timestamp")
                                        .with_timezone(&Local)
                                        .format("%b. %d, %Y - %T");
                        Row::new()
                            .width(Length::Fill)
                            .push(Container::new(text("Date:").bold()).width(Length::Fill))
                            .push(Container::new(text(format!("{}", date))).width(Length::Shrink))
                    }))
                    .push(
                        Row::new()
                            .width(Length::Fill)
                            .align_y(Alignment::Center)
                            .push(Container::new(text("Txid:").bold()).width(Length::Fill))
                            .push(
                                Row::new()
                                    .align_y(Alignment::Center)
                                    .push(Container::new(text(txid.clone()).small()))
                                    .push(
                                        Button::new(icon::clipboard_icon())
                                            .on_press(Message::Clipboard(txid.clone()))
                                            .style(theme::button::transparent_border),
                                    )
                                    .width(Length::Shrink),
                            ),
                    )
                    .spacing(5),
            ))
            .push(
                Column::new()
                    .spacing(20)
                    // We do not need to display inputs for external incoming transactions
                    .push_maybe(if tx.is_external() {
                        None
                    } else {
                        Some(super::psbt::inputs_view(
                            &tx.coins,
                            &tx.tx,
                            &tx.labels,
                            labels_editing,
                        ))
                    })
                    .push(super::psbt::outputs_view(
                        &tx.tx,
                        cache.network,
                        if tx.is_external() {
                            None
                        } else {
                            Some(tx.change_indexes.clone())
                        },
                        &tx.labels,
                        labels_editing,
                        tx.is_single_payment().is_some(),
                    )),
            )
            .spacing(20),
    )
}
