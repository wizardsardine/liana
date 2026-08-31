use iced::{
    widget::{column, row},
    Alignment, Length,
};

use liana_ui::{
    component::{
        button::{btn_import, btn_new},
        panels::psbts::{self, PsbtSigs},
        text::legacy::panel_title,
    },
    widget::*,
};

use crate::{app::menu::Menu, daemon::model::SpendTx};

use super::message::*;

pub fn psbts_view(spend_txs: &[SpendTx]) -> Element<'_, Message> {
    let title = Container::new(panel_title(Menu::PSBTs.title())).width(Length::Fill);
    let import = btn_import(Some(Message::ImportPsbt));
    let new_tx = btn_new(Some(Message::Menu(Menu::CreateSpendTx)));
    let header = row![title, import, new_tx]
        .align_y(Alignment::Center)
        .spacing(10);

    let list = spend_txs
        .iter()
        .enumerate()
        .fold(Column::new().spacing(10), |col, (i, tx)| {
            col.push(psbt_list_entry(i, tx))
        });

    column![header, list]
        .align_x(Alignment::Center)
        .spacing(25)
        .into()
}

fn psbt_list_entry(i: usize, tx: &SpendTx) -> Element<'_, Message> {
    let sigs = if tx.sigs.recovery_paths().is_empty() {
        let sigs = tx.sigs.primary_path();
        PsbtSigs::Primary {
            count: sigs.sigs_count,
            threshold: sigs.threshold,
        }
    } else {
        PsbtSigs::Recovery
    };

    let label = tx
        .labels
        .get(&tx.psbt.unsigned_tx.compute_txid().to_string())
        .map(String::as_str);

    psbts::list_entry(
        label,
        tx.is_send_to_self(),
        tx.is_batch(),
        tx.status,
        sigs,
        tx.spend_amount,
        tx.fee_amount,
        Some(Message::Select(i)),
    )
}
