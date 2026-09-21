use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Local, Utc};
use iced::{
    widget::{column, row, tooltip},
    Alignment, Length,
};

use liana_ui::{
    component::{
        amount::{amount, amount_with_font},
        badge,
        button::{
            self, btn_bump_fee, btn_cancel_transaction, btn_confirm, btn_export_transactions,
            btn_go_to_replacement,
        },
        card, form, list,
        modal::ModalWidth,
        pill,
        text::legacy,
    },
    icon, theme,
    widget::*,
};

use crate::{
    app::{
        cache::Cache,
        error::Error,
        menu::Menu,
        view::{
            self, dashboard, label,
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
    let title = Container::new(legacy::panel_title(Menu::Transactions.title())).width(Length::Fill);
    let export = btn_export_transactions(Some(ImportExportMessage::Open.into()));
    let header = row![title, export];

    let list = txs
        .iter()
        .enumerate()
        .fold(Column::new().spacing(10), |col, (i, tx)| {
            col.push(tx_list_view(i, tx))
        });

    let see_more =
        (!is_last_page && !txs.is_empty()).then(|| list::see_more(processing, Message::Next));

    dashboard(
        &Menu::Transactions,
        cache,
        warning,
        column![header, column![list, see_more].spacing(10)]
            .align_x(Alignment::Center)
            .spacing(30),
    )
}

fn tx_list_view(i: usize, tx: &HistoryTransaction) -> Element<'_, Message> {
    let badge = if tx.is_incoming() {
        badge::receive()
    } else if tx.is_send_to_self() {
        badge::cycle()
    } else {
        badge::spend()
    };
    let label_key = tx
        .single_payment()
        .map(|outpoint| outpoint.to_string())
        .unwrap_or_else(|| tx.txid.to_string());
    let label = tx.labels.get(&label_key).map(legacy::p1_regular);
    let date = tx.time.map(|t| {
        DateTime::<Utc>::from_timestamp(t as i64, 0)
            .expect("Correct unix timestamp")
            .with_timezone(&Local)
            .format("%b. %d, %Y - %T")
            .to_string()
    });
    let date = date.map(|date| Container::new(legacy::text(date).style(theme::text::secondary)));
    let description = row![badge, column![label, date]]
        .spacing(10)
        .align_y(Alignment::Center)
        .width(Length::Fill);
    let unconfirmed = tx.time.is_none().then_some(pill::unconfirmed());
    let batch = tx.is_batch().then_some(pill::batch());
    let amount_row = if tx.is_send_to_self() {
        row![legacy::text(t!("common-self-transfer"))]
    } else {
        let sign = if tx.is_incoming() { "+" } else { "-" };
        row![legacy::text(sign), amount(&tx.wallet_tx.amount())]
            .spacing(5)
            .align_y(Alignment::Center)
    };

    let content = row![description, unconfirmed, batch, amount_row]
        .align_y(Alignment::Center)
        .spacing(20);

    list::entry_history(content, Message::Select(i))
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
    let confirm_msg =
        (feerate.valid || is_cancel).then_some(Message::CreateRbf(CreateRbfMessage::Confirm));
    let confirm_button = btn_confirm(confirm_msg);
    let help_text = if is_cancel {
        t!("transactions-rbf-cancel-help")
    } else {
        t!("transactions-rbf-bump-help")
    };

    let descendants = (!descendant_txids.is_empty()).then(|| {
        let invalidates = if descendant_txids.len() > 1 {
            t!("transactions-rbf-invalidates-some")
        } else {
            t!("transactions-rbf-invalidates-one")
        };
        let explanation = if descendant_txids.len() > 1 {
            t!("transactions-rbf-descendants-some")
        } else {
            t!("transactions-rbf-descendants-one")
        };
        (invalidates, explanation)
    });
    let descendants = descendants.map(|(invalidates, explanation)| {
        let init = column![
            row![icon::warning_icon(), legacy::text(invalidates)].spacing(10),
            row![legacy::text(explanation)].padding([0, 30])
        ]
        .spacing(5);
        descendant_txids.iter().fold(init, |col, txid| {
            col.push(
                row![
                    legacy::text(txid.to_string()),
                    button::btn_copy(Some(Message::Clipboard(txid.to_string())))
                ]
                .padding([0, 30])
                .spacing(5)
                .align_y(Alignment::Center),
            )
        })
    });
    let feerate_form = (!is_cancel).then(|| {
        if replacement_txid.is_none() {
            form::Form::new_trimmed("", feerate, move |msg| {
                Message::CreateRbf(CreateRbfMessage::FeerateEdited(msg))
            })
            .warning(t!("transactions-rbf-feerate-warning"))
        } else {
            form::Form::new_disabled("", feerate)
        }
        .size(legacy::P1_SIZE)
        .padding(10)
    });
    let feerate = feerate_form.map(|form| {
        row![
            Container::new(legacy::p1_bold(t!("common-feerate"))).padding(10),
            form
        ]
        .spacing(10)
        .width(Length::Fill)
    });
    let status = if replacement_txid.is_none() {
        row![confirm_button]
    } else {
        row![
            icon::circle_check_icon().style(theme::text::secondary),
            legacy::text(t!("transactions-rbf-created")).style(theme::text::success)
        ]
        .spacing(10)
        .align_y(Alignment::Center)
    };
    let replacement = replacement_txid
        .map(|id| btn_go_to_replacement(Some(Message::Menu(Menu::PsbtPreSelected(id)))));

    card::simple(
        column![
            Container::new(legacy::h4_bold(t!("transactions-replacement"))).width(Length::Fill),
            legacy::text(help_text),
            descendants,
            feerate,
            warn(warning),
            status,
            replacement,
        ]
        .spacing(10),
    )
    .width(ModalWidth::XL)
    .into()
}

pub fn tx_view<'a>(
    cache: &'a Cache,
    tx: &'a HistoryTransaction,
    labels_editing: &'a HashMap<String, form::Value<String>>,
    warning: Option<&'a Error>,
) -> Element<'a, Message> {
    let txid = tx.txid.to_string();
    let title = if tx.is_send_to_self() {
        t!("transactions-transaction")
    } else if tx.is_incoming() {
        t!("transactions-incoming")
    } else {
        t!("transactions-outgoing")
    };
    let title = Container::new(legacy::h3(title)).width(Length::Fill);
    // if the payment is a payment of a single payment transaction then
    // the label of the transaction is attached to the label of the payment outpoint
    let outpoint = tx.single_payment().map(|outpoint| outpoint.to_string());
    let label_key = outpoint.clone().unwrap_or_else(|| txid.clone());
    let (labelled, size) = match outpoint {
        Some(outpoint) => (vec![outpoint, txid.clone()], legacy::H3_SIZE),
        None => (vec![txid.clone()], legacy::H1_SIZE),
    };
    let tx_label = match labels_editing.get(&label_key) {
        Some(editing) => label::label_editing(labelled, editing, size),
        None => label::label_editable(labelled, tx.labels.get(&label_key), size),
    };
    let amount: Element<'a, Message> = if tx.is_send_to_self() {
        legacy::h1(t!("common-self-transfer")).into()
    } else {
        amount_with_font(&tx.wallet_tx.amount(), legacy::H1_SPEC).into()
    };
    let fee = tx.wallet_tx.fee().map(|fee_amount| {
        row![
            legacy::h3(t!("transactions-miner-fee")).style(theme::text::secondary),
            amount_with_font(&fee_amount, legacy::H3_SPEC),
            legacy::text(" ").size(legacy::H3_SIZE),
            legacy::h4_regular(t!(
                "common-feerate-value",
                rate = fee_amount.to_sat() / tx.tx.vsize() as u64
            ))
            .style(theme::text::secondary)
        ]
        .align_y(Alignment::Center)
    });
    let header = column![amount, fee];
    // If unconfirmed, give option to use RBF.
    // Check fee amount is some as otherwise we may be missing coins for this transaction.
    let rbf = (tx.time.is_none() && tx.wallet_tx.fee().is_some()).then(|| {
        row![
            btn_bump_fee(Some(Message::CreateRbf(CreateRbfMessage::New(false)))),
            tooltip::Tooltip::new(
                btn_cancel_transaction(Some(Message::CreateRbf(CreateRbfMessage::New(true)))),
                legacy::text(t!("transactions-cancel-tooltip")),
                tooltip::Position::Top,
            )
        ]
        .spacing(10)
    });
    let date = tx.time.map(|t| {
        DateTime::<Utc>::from_timestamp(t as i64, 0)
            .expect("Correct unix timestamp")
            .with_timezone(&Local)
            .format("%b. %d, %Y - %T")
    });
    let date = date.map(|date| {
        row![
            Container::new(legacy::p1_bold(t!("transactions-date"))).width(Length::Fill),
            Container::new(legacy::text(format!("{date}"))).width(Length::Shrink)
        ]
        .width(Length::Fill)
    });
    let txid_row = row![
        Container::new(legacy::p1_bold(t!("transactions-txid"))).width(Length::Fill),
        row![
            Container::new(legacy::p1_regular(txid.clone())),
            button::btn_copy(Some(Message::Clipboard(txid.clone())))
        ]
        .align_y(Alignment::Center)
        .width(Length::Shrink)
    ]
    .width(Length::Fill)
    .align_y(Alignment::Center);
    let txid_card = card::simple(column![date, txid_row].spacing(5));
    // We do not need to display inputs for external incoming transactions
    let inputs = (!tx.is_incoming())
        .then(|| view::psbt::inputs_view(&tx.coins, &tx.tx, &tx.labels, labels_editing));
    let outputs = view::psbt::outputs_view(
        &tx.tx,
        cache.network,
        &tx.owned_output_indexes(),
        &tx.labels,
        labels_editing,
        tx.single_payment().is_some(),
        tx.is_incoming(),
    );

    dashboard(
        &Menu::Transactions,
        cache,
        warning,
        column![
            title,
            tx_label,
            header,
            rbf,
            txid_card,
            column![inputs, outputs].spacing(20)
        ]
        .spacing(20),
    )
}
