use chrono::{DateTime, Utc};

use iced::{
    widget::{column, row},
    Alignment,
};
use liana_i18n::t;

use crate::{
    component::{
        panels::txid_row,
        text::{format_datetime, new},
    },
    spacing::{HSpacing, VSpacing},
    theme,
    widget::Element,
};

/// Date and txid rows.
pub fn overview<'a, M: Clone + 'static>(
    time: Option<DateTime<Utc>>,
    txid: String,
    copy_txid: M,
) -> Element<'a, M> {
    let date = time
        .map(format_datetime)
        .unwrap_or_else(|| t!("transactions-date-unconfirmed"));
    let date = row![
        new::b2_medium(t!("transactions-date")).style(theme::text::primary),
        new::b2(date).style(theme::text::secondary)
    ]
    .spacing(HSpacing::S)
    .align_y(Alignment::Center);
    let txid = txid_row(txid, copy_txid);
    column![date, txid].spacing(VSpacing::SM).into()
}
