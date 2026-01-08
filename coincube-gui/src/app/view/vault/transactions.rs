use std::collections::{HashMap, HashSet};
use std::convert::TryInto;

use chrono::{DateTime, Local, Utc};
use iced::{
    alignment,
    widget::{tooltip, Space},
    Alignment, Length,
};

use coincube_ui::{
    component::{
        amount::*, button, card, form, text::*,
        transaction::{TransactionBadge, TransactionDirection, TransactionListItem, TransactionType},
    },
    icon::{self, receipt_icon},
    theme,
    widget::*,
};

use crate::{
    app::{
        cache::Cache,
        error::Error,
        menu::Menu,
        view::{
            dashboard,
            message::{CreateRbfMessage, Message},
            placeholder,
            vault::{label, warning::warn},
            FiatAmountConverter,
        },
    },
    daemon::model::{HistoryTransaction, Txid},
    export::ImportExportMessage,
};

pub fn transactions_view<'a>(
    menu: &'a Menu,
    cache: &'a Cache,
    txs: &'a [HistoryTransaction],
    warning: Option<&'a Error>,
    is_last_page: bool,
    processing: bool,
) -> Element<'a, Message> {
    let fiat_converter = cache.fiat_price.as_ref().and_then(|p| p.try_into().ok());
    
    dashboard(
        menu,
        cache,
        warning,
        Column::new()
            .push(
                Row::new()
                    .push(Container::new(h3("Transactions")))
                    .push(Space::new().width(Length::Fill))
                    .push(
                        button::secondary(Some(icon::backup_icon()), "Export")
                            .on_press(ImportExportMessage::Open.into()),
                    ),
            )
            .push_maybe(txs.is_empty().then(|| {
                placeholder(
                    receipt_icon().size(80),
                    "No transactions yet",
                    "Your transaction history will appear here once you send or receive coins.",
                )
            }))
            .push(
                Column::new()
                    .spacing(10)
                    .push(
                        txs.iter()
                            .enumerate()
                            .fold(Column::new().spacing(10), |col, (i, tx)| {
                                col.push(tx_list_view(i, tx, cache.bitcoin_unit.into(), fiat_converter))
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
            .spacing(25),
    )
}

fn tx_list_view(i: usize, tx: &HistoryTransaction, bitcoin_unit: BitcoinDisplayUnit, fiat_converter: Option<FiatAmountConverter>) -> Element<'_, Message> {
    let direction = if tx.is_external() {
        TransactionDirection::Incoming
    } else if tx.is_send_to_self() {
        TransactionDirection::SelfTransfer
    } else {
        TransactionDirection::Outgoing
    };

    let amount = if tx.is_external() {
        &tx.incoming_amount
    } else {
        &tx.outgoing_amount
    };

    let label = if let Some(outpoint) = tx.is_single_payment() {
        tx.labels.get(&outpoint.to_string()).cloned()
    } else {
        tx.labels.get(&tx.tx.compute_txid().to_string()).cloned()
    };

    let timestamp = tx.time.and_then(|t| {
        DateTime::<Utc>::from_timestamp(t as i64, 0)
    });

    let mut badges = Vec::new();
    if tx.time.is_none() {
        badges.push(TransactionBadge::Unconfirmed);
    }
    if tx.is_batch() {
        badges.push(TransactionBadge::Batch);
    }

    let mut item = TransactionListItem::new(direction, amount, bitcoin_unit)
        .with_type(TransactionType::Bitcoin)
        .with_badges(badges);

    if let Some(label) = label {
        item = item.with_label(label);
    }

    if let Some(timestamp) = timestamp {
        item = item.with_timestamp(timestamp);
    }

    if let Some(fiat_amount) = fiat_converter.map(|converter| {
        let fiat = converter.convert(*amount);
        format!("~{} {}", fiat.to_rounded_string(), fiat.currency())
    }) {
        item = item.with_fiat_amount(fiat_amount);
    }

    item.view(Message::Select(i)).into()
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
            confirm_button.on_press(Message::CreateRbf(super::super::CreateRbfMessage::Confirm));
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
                    .push(icon::square_check_icon().style(theme::text::secondary))
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

pub fn transaction_detail_view<'a>(
    menu: &'a Menu,
    cache: &'a Cache,
    tx: &'a HistoryTransaction,
    labels_editing: &'a HashMap<String, form::Value<String>>,
    warning: Option<&'a Error>,
    bitcoin_unit: BitcoinDisplayUnit,
) -> Element<'a, Message> {
    let txid = tx.tx.compute_txid().to_string();
    dashboard(
        menu,
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
                            Container::new(amount_with_size_and_unit(&tx.incoming_amount, H1_SIZE, bitcoin_unit))
                        } else {
                            Container::new(amount_with_size_and_unit(&tx.outgoing_amount, H1_SIZE, bitcoin_unit))
                        })
                        .push_maybe(tx.fee_amount.map(|fee_amount| {
                            Row::new()
                                .align_y(Alignment::Center)
                                .push(h3("Miner fee: ").style(theme::text::secondary))
                                .push(amount_with_size_and_unit(&fee_amount, H3_SIZE, bitcoin_unit))
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
                                .on_press(Message::CreateRbf(super::super::CreateRbfMessage::New(false))),
                        )
                        .push(
                            tooltip::Tooltip::new(
                                button::secondary(None, "Cancel transaction")
                                .width(Length::Fixed(200.0))
                                .on_press(Message::CreateRbf(super::super::CreateRbfMessage::New(true))),
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
