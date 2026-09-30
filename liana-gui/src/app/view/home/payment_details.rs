use chrono::{DateTime, Local, Utc};
use std::{collections::HashMap, vec};

use iced::{
    widget::{column, row, Space},
    Alignment,
};

use liana::miniscript::bitcoin;
use liana_ui::{
    component::{
        amount::amount_with_font,
        button::{self, btn_see_transaction_details},
        card, form,
        text::{legacy, Text},
    },
    theme,
    widget::{Element, SpaceExt},
};

use crate::{
    app::{
        cache::Cache,
        error::Error,
        menu::Menu,
        view::{dashboard, label, message::Message},
    },
    daemon::model::HistoryTransaction,
    t,
};

pub fn payment_details_view<'a>(
    cache: &'a Cache,
    tx: &'a HistoryTransaction,
    output_index: usize,
    labels_editing: &'a HashMap<String, form::Value<String>>,
    warning: Option<&'a Error>,
) -> Element<'a, Message> {
    let txid = tx.txid.to_string();
    let outpoint = bitcoin::OutPoint::new(tx.txid, output_index as u32).to_string();
    let size = legacy::H3_SIZE;
    let spec = legacy::H3_SPEC;
    let title = if tx.wallet_tx.is_outgoing() {
        t!("payment-outgoing")
    } else if tx.wallet_tx.is_incoming() {
        t!("payment-incoming")
    } else {
        t!("payment-title")
    };
    let title = legacy::h3(title);
    // if the payment is a payment of a single payment transaction then
    // the label of the transaction is attached to the label of the payment outpoint
    let labelled = if tx.single_payment().is_some() {
        vec![outpoint.clone(), txid.clone()]
    } else {
        vec![outpoint.clone()]
    };
    let payment_label = if let Some(label) = labels_editing.get(&outpoint) {
        label::label_editing(labelled, label, size)
    } else {
        label::label_editable(labelled, tx.labels.get(&outpoint), size)
    };
    let amount = amount_with_font(&tx.tx.output[output_index].value, spec);
    let tx_title = legacy::h3(t!("transactions-transaction"));
    let tx_label = tx.is_batch().then(|| {
        if let Some(label) = labels_editing.get(&txid) {
            label::label_editing(vec![txid.clone()], label, size)
        } else {
            label::label_editable(vec![txid.clone()], tx.labels.get(&txid), size)
        }
    });
    let fee = tx.wallet_tx.fee().map(|fee_amount| {
        row![
            legacy::h3(t!("transactions-miner-fee")).style(theme::text::secondary),
            amount_with_font(&fee_amount, spec),
            legacy::text(" ").size(size),
            legacy::text(t!(
                "common-feerate-value",
                rate = fee_amount.to_sat() / tx.tx.vsize() as u64
            ))
            .size(legacy::H4_SIZE)
            .style(theme::text::secondary)
        ]
        .align_y(Alignment::Center)
    });
    let date = tx.time.map(|t| {
        DateTime::<Utc>::from_timestamp(t as i64, 0)
            .unwrap()
            .with_timezone(&Local)
            .format("%b. %d, %Y - %T")
    });
    let date = date.map(|date| {
        row![
            legacy::text(t!("transactions-date")).bold(),
            Space::fill_width(),
            legacy::text(format!("{date}"))
        ]
    });
    let txid_row = row![
        legacy::text(t!("transactions-txid")).bold(),
        Space::fill_width(),
        legacy::text(txid.clone()).small(),
        button::btn_copy(Some(Message::Clipboard(txid.clone())))
    ]
    .align_y(Alignment::Center);
    let overview = card::simple(column![date, txid_row].spacing(5));
    let see_details =
        btn_see_transaction_details(Message::Menu(Menu::TransactionPreSelected(tx.txid)));

    dashboard(
        &Menu::Home,
        cache,
        warning,
        column![
            title,
            payment_label,
            amount,
            Space::with_height(size),
            tx_title,
            tx_label,
            fee,
            overview,
            see_details
        ]
        .spacing(20),
    )
}
