use iced::{
    widget::{column, row, Space},
    Alignment,
};
use liana_ui::{
    component::{
        button::{btn_export, btn_see_more},
        panels::home::payment,
        text::new,
    },
    spacing::{HSpacing, VSpacing},
    widget::{Column, Element, SpaceExt},
};

use crate::{
    app::{
        menu::Menu,
        view::{label, message::Message},
    },
    daemon::model::HistoryTransaction,
    export::ImportExportMessage,
};

pub fn transactions_view(
    txs: &[HistoryTransaction],
    is_last_page: bool,
    processing: bool,
) -> Element<'_, Message> {
    let export = btn_export(Some(ImportExportMessage::Open.into()));
    let header = row![
        new::d2(Menu::Transactions.title()),
        Space::fill_width(),
        export
    ]
    .align_y(Alignment::Center)
    .spacing(HSpacing::M);

    let list = txs
        .iter()
        .enumerate()
        .fold(Column::new().spacing(VSpacing::M), |col, (i, tx)| {
            col.push(tx_list_entry(i, tx))
        });

    let see_more =
        (!is_last_page && !txs.is_empty()).then(|| btn_see_more(processing, Message::Next));

    column![header, list, see_more]
        .align_x(Alignment::Center)
        .spacing(VSpacing::XL)
        .into()
}

fn tx_list_entry(i: usize, tx: &HistoryTransaction) -> Element<'_, Message> {
    let label = tx.label().text(label::prefixed);
    payment::list_entry(
        label,
        tx.datetime(),
        tx.wallet_tx.kind().payment_kind(),
        tx.is_batch(),
        tx.is_payjoin(),
        tx.wallet_tx.amount(),
        None,
        Some(Message::Select(i)),
    )
}
