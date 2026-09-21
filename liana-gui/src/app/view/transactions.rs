use chrono::{DateTime, Local, Utc};
use iced::{
    widget::{column, row},
    Alignment, Length,
};

use liana_ui::{
    component::{button::btn_export_transactions, list, panels::transactions, text::legacy},
    widget::*,
};

use crate::{
    app::{
        cache::Cache,
        error::Error,
        menu::Menu,
        view::{dashboard, message::Message},
    },
    daemon::model::HistoryTransaction,
    export::ImportExportMessage,
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
    let label_key = tx
        .single_payment()
        .map(|outpoint| outpoint.to_string())
        .unwrap_or_else(|| tx.txid.to_string());
    let label = tx.labels.get(&label_key).map(String::as_str);
    let date = tx.time.map(|t| {
        DateTime::<Utc>::from_timestamp(t as i64, 0)
            .expect("Correct unix timestamp")
            .with_timezone(&Local)
            .format("%b. %d, %Y - %T")
            .to_string()
    });

    transactions::list_entry(
        label,
        date,
        tx.is_incoming(),
        tx.is_send_to_self(),
        tx.is_batch(),
        tx.wallet_tx.amount(),
        Message::Select(i),
    )
}
