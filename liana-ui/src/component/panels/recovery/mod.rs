use std::collections::{HashMap, HashSet};

use bitcoin::{
    bip32::{DerivationPath, Fingerprint},
    Amount,
};
use iced::{
    widget::{column, row},
    Alignment,
};
use liana_i18n::t;

use crate::{
    component::{
        amount::amount,
        button::EntryWidth,
        checkbox::checkbox_button,
        list::{self, EntryAccent},
        pill, scrollable,
        text::new,
    },
    spacing::{HSpacing, VSpacing},
    theme,
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
    let toggle = on_select.clone();
    let select = checkbox_button(selected, move |_| toggle.clone());
    let keys = origins.iter().fold(
        Row::new().align_y(Alignment::Center).spacing(HSpacing::S),
        |row, (fg, _)| {
            row.push(pill::fingerprint(
                fg.to_string(),
                key_aliases.get(fg).map(String::as_str),
            ))
        },
    );
    let signatures = scrollable::horizontal_thin(
        row![
            new::h2(t!("recovery-signatures-from", count = threshold)),
            keys
        ]
        .align_y(Alignment::Center)
        .spacing(HSpacing::M),
    );
    let coins = row![
        new::b4(t!("recovery-coins-total", count = number_of_coins)).style(theme::text::secondary),
        amount(&total_amount)
    ]
    .align_y(Alignment::Center)
    .spacing(HSpacing::S);
    let description = column![signatures, coins].spacing(VSpacing::M);

    let accent = selected.then_some(list::entry_accent(EntryAccent::Success));

    list::list_entry_row(
        None,
        description,
        Some(select.into()),
        accent,
        EntryWidth::Fill,
        Some(on_select),
    )
}
