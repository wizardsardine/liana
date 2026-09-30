use chrono::{DateTime, Local, Utc};
use std::{collections::HashMap, vec};

use iced::{
    widget::{column, row, Container, Space},
    Alignment, Length,
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
        view::{
            dashboard,
            label::{self, LabelSize},
            message::Message,
        },
    },
    daemon::model::{HistoryTransaction, Payment},
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
    let title = Container::new(legacy::h3(title)).width(Length::Fill);
    // if the payment is a payment of a single payment transaction then
    // the label of the transaction is attached to the label of the payment outpoint
    let labelled = if tx.single_payment().is_some() {
        vec![outpoint.clone(), txid.clone()]
    } else {
        vec![outpoint.clone()]
    };
    let payment_label = label::label_field(
        labelled,
        labels_editing.get(&outpoint),
        Payment::from_tx_output(tx, output_index)
            .map(|payment| payment.label())
            .unwrap_or_default(),
        LabelSize::Title,
    );
    let amount = amount_with_font(&tx.tx.output[output_index].value, spec);
    let tx_title = Container::new(legacy::h3(t!("transactions-transaction"))).width(Length::Fill);
    let tx_label = tx.is_batch().then(|| {
        label::label_field(
            vec![txid.clone()],
            labels_editing.get(&txid),
            tx.label(),
            LabelSize::Title,
        )
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
            Container::new(legacy::text(t!("transactions-date")).bold()).width(Length::Fill),
            Container::new(legacy::text(format!("{date}"))).width(Length::Shrink)
        ]
        .width(Length::Fill)
    });
    let txid_row = row![
        Container::new(legacy::text(t!("transactions-txid")).bold()).width(Length::Fill),
        row![
            Container::new(legacy::text(txid.clone()).small()),
            button::btn_copy(Some(Message::Clipboard(txid.clone())))
        ]
        .align_y(Alignment::Center)
        .width(Length::Shrink)
    ]
    .width(Length::Fill)
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
