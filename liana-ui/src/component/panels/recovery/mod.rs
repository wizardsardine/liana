use std::collections::{HashMap, HashSet};

use bitcoin::{
    bip32::{DerivationPath, Fingerprint},
    Amount,
};
use iced::{
    widget::{checkbox, column, row},
    Alignment, Length,
};
use liana_i18n::t;

use crate::{
    component::{
        amount::amount,
        pill,
        text::{legacy, Text},
    },
    widget::{Element, Row},
};

pub fn path_entry<'a, M: Clone + 'static>(
    threshold: usize,
    origins: &'a [(Fingerprint, HashSet<DerivationPath>)],
    total_amount: Amount,
    number_of_coins: usize,
    key_aliases: &'a HashMap<Fingerprint, String>,
    selected: bool,
    on_select: M,
) -> Element<'a, M> {
    let select = checkbox(selected).on_toggle(move |_| on_select.clone());
    let keys = origins.iter().fold(
        Row::new().align_y(Alignment::Center).spacing(5),
        |row, (fg, _)| {
            row.push(pill::fingerprint(
                fg.to_string(),
                key_aliases.get(fg).map(String::as_str),
            ))
        },
    );
    let signatures = row![
        legacy::text(t!("recovery-signatures-from", count = threshold)).bold(),
        keys
    ]
    .align_y(Alignment::Center)
    .spacing(10);
    let coins = row![
        legacy::text(t!("recovery-coins-total", count = number_of_coins)),
        amount(&total_amount)
    ]
    .spacing(5);
    let description = column![signatures, coins].spacing(5);

    row![select, description]
        .width(Length::Fill)
        .align_y(Alignment::Center)
        .spacing(20)
        .into()
}
