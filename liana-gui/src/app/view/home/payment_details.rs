use chrono::{DateTime, Local, Utc};
use std::{collections::HashMap, vec};

use iced::{
    widget::{Container, Row, Space},
    Alignment, Length,
};

use liana::miniscript::bitcoin;
use liana_ui::{
    component::{
        amount::amount_with_font,
        button, card, form,
        text::{legacy, Text},
    },
    theme,
    widget::{Column, ColumnExt, Element, SpaceExt},
};

use crate::{
    app::{
        cache::Cache,
        error::Error,
        menu::Menu,
        view::{dashboard, label, message::Message},
    },
    daemon::model::{HistoryTransaction, TransactionKind},
};

pub fn payment_details_view<'a>(
    cache: &'a Cache,
    tx: &'a HistoryTransaction,
    output_index: usize,
    labels_editing: &'a HashMap<String, form::Value<String>>,
    warning: Option<&'a Error>,
) -> Element<'a, Message> {
    let txid = tx.tx.compute_txid().to_string();
    let outpoint = bitcoin::OutPoint {
        txid: tx.tx.compute_txid(),
        vout: output_index as u32,
    }
    .to_string();
    dashboard(
        &Menu::Home,
        cache,
        warning,
        Column::new()
            .push(match tx.kind {
                TransactionKind::OutgoingSinglePayment(_)
                | TransactionKind::OutgoingPaymentBatch(_) => {
                    Container::new(legacy::h3("Outgoing payment")).width(Length::Fill)
                }
                TransactionKind::IncomingSinglePayment(_)
                | TransactionKind::IncomingPaymentBatch(_) => {
                    Container::new(legacy::h3("Incoming payment")).width(Length::Fill)
                }
                _ => Container::new(legacy::h3("Payment")).width(Length::Fill),
            })
            .push(if tx.is_single_payment().is_some() {
                // if the payment is a payment of a single payment transaction then
                // the label of the transaction is attached to the label of the payment outpoint
                if let Some(label) = labels_editing.get(&outpoint) {
                    label::label_editing(
                        vec![outpoint.clone(), txid.clone()],
                        label,
                        legacy::H3_SIZE,
                    )
                } else {
                    label::label_editable(
                        vec![outpoint.clone(), txid.clone()],
                        tx.labels.get(&outpoint),
                        legacy::H3_SIZE,
                    )
                }
            } else if let Some(label) = labels_editing.get(&outpoint) {
                label::label_editing(vec![outpoint.clone()], label, legacy::H3_SIZE)
            } else {
                label::label_editable(
                    vec![outpoint.clone()],
                    tx.labels.get(&outpoint),
                    legacy::H3_SIZE,
                )
            })
            .push(Container::new(amount_with_font(
                &tx.tx.output[output_index].value,
                legacy::H3_SPEC,
            )))
            .push(Space::with_height(legacy::H3_SIZE))
            .push(Container::new(legacy::h3("Transaction")).width(Length::Fill))
            .push_maybe(if tx.is_batch() {
                if let Some(label) = labels_editing.get(&txid) {
                    Some(label::label_editing(
                        vec![txid.clone()],
                        label,
                        legacy::H3_SIZE,
                    ))
                } else {
                    Some(label::label_editable(
                        vec![txid.clone()],
                        tx.labels.get(&txid),
                        legacy::H3_SIZE,
                    ))
                }
            } else {
                None
            })
            .push_maybe(tx.fee_amount.map(|fee_amount| {
                Row::new()
                    .align_y(Alignment::Center)
                    .push(legacy::h3("Miner fee: ").style(theme::text::secondary))
                    .push(amount_with_font(&fee_amount, legacy::H3_SPEC))
                    .push(legacy::text(" ").size(legacy::H3_SIZE))
                    .push(
                        legacy::text(format!(
                            "({} sats/vbyte)",
                            fee_amount.to_sat() / tx.tx.vsize() as u64
                        ))
                        .size(legacy::H4_SIZE)
                        .style(theme::text::secondary),
                    )
            }))
            .push(card::simple(
                Column::new()
                    .push_maybe(tx.time.map(|t| {
                        let date = DateTime::<Utc>::from_timestamp(t as i64, 0)
                            .unwrap()
                            .with_timezone(&Local)
                            .format("%b. %d, %Y - %T");
                        Row::new()
                            .width(Length::Fill)
                            .push(Container::new(legacy::text("Date:").bold()).width(Length::Fill))
                            .push(
                                Container::new(legacy::text(format!("{date}")))
                                    .width(Length::Shrink),
                            )
                    }))
                    .push(
                        Row::new()
                            .width(Length::Fill)
                            .align_y(Alignment::Center)
                            .push(Container::new(legacy::text("Txid:").bold()).width(Length::Fill))
                            .push(
                                Row::new()
                                    .align_y(Alignment::Center)
                                    .push(Container::new(
                                        legacy::text(format!("{}", tx.tx.compute_txid())).small(),
                                    ))
                                    .push(button::btn_copy(Some(Message::Clipboard(
                                        tx.tx.compute_txid().to_string(),
                                    ))))
                                    .width(Length::Shrink),
                            ),
                    )
                    .spacing(5),
            ))
            .push(
                button::tertiary(None, "See transaction details").on_press(Message::Menu(
                    Menu::TransactionPreSelected(tx.tx.compute_txid()),
                )),
            )
            .spacing(20),
    )
}
