use bitcoin::Amount;
use chrono::{DateTime, Utc};

use iced::{
    widget::{column, row},
    Alignment,
};
use liana::transaction::PaymentKind;
use liana_i18n::t;

use crate::{
    component::{
        amount::{amount_with_fiat_tooltip, AmountSize, FiatAmount},
        panels::{home::payment::kind_icon, txid_row},
        text::{format_datetime, new},
    },
    spacing::{HSpacing, VSpacing},
    theme,
    widget::Element,
};

pub fn amount_row<'a, M: 'a>(kind: PaymentKind, amount: Amount) -> Element<'a, M> {
    let amount: Element<'a, M> = if kind == PaymentKind::SendToSelf {
        new::d2(t!("common-self-transfer")).into()
    } else {
        amount_with_fiat_tooltip(
            &amount,
            None::<fn(Amount) -> FiatAmount>,
            AmountSize::L,
            true,
            None,
        )
    };
    row![kind_icon(kind), amount]
        .spacing(HSpacing::S)
        .align_y(Alignment::Center)
        .into()
}

pub fn date_row<'a, M: 'a>(time: Option<DateTime<Utc>>) -> Element<'a, M> {
    let date = time
        .map(format_datetime)
        .unwrap_or_else(|| t!("transactions-date-unconfirmed"));
    row![
        new::b2_medium(t!("transactions-date")).style(theme::text::primary),
        new::b2(date).style(theme::text::secondary)
    ]
    .spacing(HSpacing::S)
    .align_y(Alignment::Center)
    .into()
}

/// Date and txid rows.
pub fn overview<'a, M: Clone + 'static>(
    time: Option<DateTime<Utc>>,
    txid: String,
    copy_txid: M,
) -> Element<'a, M> {
    column![date_row(time), txid_row(txid, copy_txid)]
        .spacing(VSpacing::SM)
        .into()
}
