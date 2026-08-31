use iced::{
    widget::{column, row},
    Alignment, Length,
};

use liana_ui::{
    component::{
        amount::{amount, amount_with_font},
        badge,
        button::{btn_import, btn_new},
        pill,
        text::{legacy::panel_title, new},
    },
    icon, theme,
    widget::*,
};

use crate::{
    app::menu::Menu,
    daemon::model::{SpendStatus, SpendTx},
    t,
};

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
            col.push(spend_tx_list_view(i, tx))
        });

    column![header, list]
        .align_x(Alignment::Center)
        .spacing(25)
        .into()
}

fn spend_tx_list_view(i: usize, tx: &SpendTx) -> Element<'_, Message> {
    let badge = if tx.is_send_to_self() {
        badge::cycle()
    } else {
        badge::spend()
    };

    let sigs = if tx.sigs.recovery_paths().is_empty() {
        let sigs = tx.sigs.primary_path();
        let count = sigs.sigs_count.min(sigs.threshold);
        let counter =
            new::caption(format!("{count}/{}", sigs.threshold)).style(theme::text::secondary);
        let key = icon::key_icon().style(theme::text::secondary);
        Container::new(row![counter, key].spacing(5).align_y(Alignment::Center))
    } else {
        pill::recovery()
    };

    let label = tx
        .labels
        .get(&tx.psbt.unsigned_tx.compute_txid().to_string())
        .map(new::b5_medium);

    let left = row![badge, sigs, label]
        .spacing(10)
        .align_y(Alignment::Center)
        .width(Length::Fill);

    let batch = tx.is_batch().then_some(pill::batch());

    let status = match tx.status {
        SpendStatus::Deprecated => Some(pill::deprecated().width(120.0)),
        SpendStatus::Broadcast => Some(pill::unconfirmed().width(120.0)),
        SpendStatus::Spent => Some(pill::spent().width(120.0)),
        _ => None,
    };

    let spent = if tx.is_send_to_self() {
        Container::new(new::b5_medium(t!("common-self-transfer")))
    } else {
        Container::new(amount(&tx.spend_amount))
    };
    let fee = tx
        .fee_amount
        .map(|fee| amount_with_font(&fee, new::CAPTION_SPEC));
    let amounts = column![spent, fee].align_x(Alignment::End).width(140);

    let content = row![left, batch, status, amounts]
        .align_y(Alignment::Center)
        .spacing(20);

    let entry = Button::new(content)
        .padding(10)
        .on_press(Message::Select(i))
        .style(theme::button::transparent_border);

    Container::new(entry)
        .style(theme::card::button_simple)
        .into()
}
