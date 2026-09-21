use bitcoin::Amount;
use chrono::{DateTime, Utc};
use std::fmt::Display;

use iced::{
    widget::{column, row, Space},
    Alignment,
};
use liana::transaction::PaymentKind;
use liana_i18n::t;

use crate::{
    component::{
        amount::{amount_with_fiat_tooltip, AmountSize, FiatAmount},
        card,
        panels::{fees_row, home::payment::kind_icon, txid_row},
        pill::{self, PillWidth},
        text::{format_datetime, new},
    },
    spacing::{HSpacing, VSpacing},
    theme,
    widget::{Element, SpaceExt},
};

/// Title row with the confirmation status pill.
pub fn title<'a, M: 'a>(title: impl Display, confirmed: bool) -> Element<'a, M> {
    let status = if confirmed {
        pill::confirmed()
    } else {
        pill::unconfirmed()
    };
    row![
        new::h1(title),
        Space::fill_width(),
        status.width(PillWidth::SM)
    ]
    .align_y(Alignment::Center)
    .into()
}

pub fn header<'a, M: 'static>(
    title: impl Display,
    confirmed: bool,
    label: Element<'a, M>,
    kind: PaymentKind,
    amount: Amount,
    fee: Option<Amount>,
    feerate: Option<u64>,
) -> Element<'a, M> {
    let amount: Element<'a, M> = if kind == PaymentKind::SendToSelf {
        new::d2(t!("common-self-transfer")).into()
    } else {
        let amount = amount_with_fiat_tooltip(
            &amount,
            None::<fn(Amount) -> FiatAmount>,
            AmountSize::L,
            true,
            None,
        );
        row![kind_icon(kind), amount]
            .spacing(HSpacing::S)
            .align_y(Alignment::Center)
            .into()
    };

    let feerate = feerate.map(|rate| t!("common-feerate-value", rate = rate));
    let fees = fee.is_some().then(|| fees_row(fee, feerate));

    column![self::title(title, confirmed), label, column![amount, fees]]
        .spacing(VSpacing::L)
        .into()
}

pub fn overview<'a, M: Clone + 'static>(
    timestamp: Option<u32>,
    txid: String,
    copy_txid: M,
    action: Option<Element<'a, M>>,
) -> Element<'a, M> {
    let date = timestamp.map(|timestamp| {
        format_datetime(
            DateTime::<Utc>::from_timestamp(timestamp as i64, 0).expect("Correct unix timestamp"),
        )
    });
    let date = date.map(|date| {
        row![
            new::b5_bold(t!("transactions-date")),
            Space::fill_width(),
            new::caption(date).style(theme::text::secondary)
        ]
        .align_y(Alignment::Center)
    });
    let txid = txid_row(txid, copy_txid);
    let action = action.map(|action| row![Space::fill_width(), action]);

    let card = card::simple(column![date, txid].spacing(VSpacing::S));
    column![card, action].spacing(VSpacing::L).into()
}
