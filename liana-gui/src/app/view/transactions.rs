use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Local, Utc};
use iced::{
    alignment,
    widget::{tooltip, Space},
    Alignment, Length,
};

use liana_ui::{
    component::{amount::*, badge, button, card, form, pill, text::*},
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
    t,
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
                    .push(Container::new(panel_title(Menu::Transactions.title())))
                    .push(Space::with_width(Length::Fill))
                    .push(
                        button::secondary(Some(icon::backup_icon()), t!("common-export"))
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
                                        t!("common-fetching")
                                    } else {
                                        t!("common-see-more")
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
                    Some(pill::unconfirmed())
                } else {
                    None
                })
                .push_maybe(if tx.is_batch() {
                    Some(pill::batch())
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
                    Row::new().push(text(t!("common-self-transfer")))
                })
                .align_y(Alignment::Center)
                .spacing(20),
        )
        .padding(10)
        .on_press(Message::Select(i))
        .style(theme::button::transparent_border),
    )
    .style(theme::card::button_simple)
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
    let mut confirm_button =
        button::secondary(None, t!("common-confirm")).width(Length::Fixed(200.0));
    if feerate.valid || is_cancel {
        confirm_button =
            confirm_button.on_press(Message::CreateRbf(super::CreateRbfMessage::Confirm));
    }
    let help_text = if is_cancel {
        t!("transactions-rbf-cancel-help")
    } else {
        t!("transactions-rbf-bump-help")
    };
    card::simple(
        Column::new()
            .spacing(10)
            .push(Container::new(h4_bold(t!("transactions-replacement"))).width(Length::Fill))
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
                                    t!("transactions-rbf-invalidates-some")
                                } else {
                                    t!("transactions-rbf-invalidates-one")
                                },
                            )))
                            .push(Row::new().padding([0, 30]).push(text(
                                if descendant_txids.len() > 1 {
                                    t!("transactions-rbf-descendants-some")
                                } else {
                                    t!("transactions-rbf-descendants-one")
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
                        .push(Container::new(p1_bold(t!("common-feerate"))).padding(10))
                        .spacing(10)
                        .push(
                            if replacement_txid.is_none() {
                                form::Form::new_trimmed("", feerate, move |msg| {
                                    Message::CreateRbf(CreateRbfMessage::FeerateEdited(msg))
                                })
                                .warning(t!("transactions-rbf-feerate-warning"))
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
                    .push(text(t!("transactions-rbf-created")).style(theme::text::success))
            }))
            .push_maybe(replacement_txid.map(|id| {
                Row::new().push(
                    button::primary(None, t!("transactions-go-to-replacement"))
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
                Container::new(h3(t!("transactions-transaction"))).width(Length::Fill)
            } else if tx.is_external() {
                Container::new(h3(t!("transactions-incoming"))).width(Length::Fill)
            } else {
                Container::new(h3(t!("transactions-outgoing"))).width(Length::Fill)
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
                            Container::new(h1(t!("common-self-transfer")))
                        } else if tx.is_external() {
                            Container::new(amount_with_font(&tx.incoming_amount, H1_SPEC))
                        } else {
                            Container::new(amount_with_font(&tx.outgoing_amount, H1_SPEC))
                        })
                        .push_maybe(tx.fee_amount.map(|fee_amount| {
                            Row::new()
                                .align_y(Alignment::Center)
                                .push(
                                    h3(t!("transactions-miner-fee")).style(theme::text::secondary),
                                )
                                .push(amount_with_font(&fee_amount, H3_SPEC))
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
                            button::secondary(None, t!("transactions-bump-fee"))
                                .width(Length::Fixed(200.0))
                                .on_press(Message::CreateRbf(super::CreateRbfMessage::New(false))),
                        )
                        .push(tooltip::Tooltip::new(
                            button::secondary(None, t!("transactions-cancel"))
                                .width(Length::Fixed(200.0))
                                .on_press(Message::CreateRbf(super::CreateRbfMessage::New(true))),
                            text(t!("transactions-cancel-tooltip")),
                            tooltip::Position::Top,
                        ))
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
                            .push(
                                Container::new(text(t!("transactions-date")).bold())
                                    .width(Length::Fill),
                            )
                            .push(Container::new(text(format!("{date}"))).width(Length::Shrink))
                    }))
                    .push(
                        Row::new()
                            .width(Length::Fill)
                            .align_y(Alignment::Center)
                            .push(
                                Container::new(text(t!("transactions-txid")).bold())
                                    .width(Length::Fill),
                            )
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
                        &tx.change_indexes,
                        &tx.labels,
                        labels_editing,
                        tx.is_single_payment().is_some(),
                        tx.is_external(),
                    )),
            )
            .spacing(20),
    )
}
