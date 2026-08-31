use iced::{
    widget::{column, row},
    Alignment, Length,
};

use liana_ui::{
    component::{
        button::{btn_import, btn_new},
        panels::psbts::{self, PsbtSigs},
        text::new,
    },
    spacing::{HSpacing, VSpacing},
    widget::*,
};

use crate::{app::menu::Menu, daemon::model::SpendTx};

use super::message::*;

pub fn psbts_view(spend_txs: &[SpendTx], available_width: f32) -> Element<'_, Message> {
    let title = Container::new(new::d2(Menu::PSBTs.title())).width(Length::Fill);
    let import = btn_import(Some(Message::ImportPsbt));
    let new_tx = btn_new(Some(Message::Menu(Menu::CreateSpendTx)));
    let header = row![title, import, new_tx]
        .align_y(Alignment::Center)
        .spacing(HSpacing::M);

    let list = spend_txs
        .iter()
        .enumerate()
        .fold(Column::new().spacing(VSpacing::M), |col, (i, tx)| {
            col.push(psbt_list_entry(i, tx, available_width))
        });

    column![header, list].spacing(VSpacing::XL).into()
}

fn psbt_list_entry(i: usize, tx: &SpendTx, available_width: f32) -> Element<'_, Message> {
    let info = if tx.sigs.recovery_paths().is_empty() {
        tx.sigs.primary_path()
    } else {
        tx.sigs
            .recovery_paths()
            .last_key_value()
            .expect("not empty")
            .1
    };
    let sigs = PsbtSigs {
        count: info.sigs_count,
        threshold: info.threshold,
    };

    let label = tx
        .labels
        .get(&tx.psbt.unsigned_tx.compute_txid().to_string())
        .map(String::as_str);

    psbts::list_entry(
        label,
        tx.is_send_to_self(),
        tx.is_batch(),
        !tx.sigs.recovery_paths().is_empty(),
        tx.status,
        sigs,
        tx.moved_amount(),
        None,
        available_width,
        Some(Message::Select(i)),
    )
}
