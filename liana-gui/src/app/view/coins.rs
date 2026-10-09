use std::collections::HashMap;

use iced::{
    widget::{column, row, Space},
    Alignment,
};

use liana_ui::{
    component::{
        button::btn_map,
        form,
        label::{display_label, LABEL_DISPLAY_MAX_CHARS},
        panels::coins,
        text::new,
    },
    spacing::{HSpacing, VSpacing},
    widget::{Column, Element, SpaceExt},
};

use crate::{
    app::{
        cache::Cache,
        menu::{MapFocus, Menu},
        view::{
            label::{self, LabelSize},
            message::Message,
        },
    },
    daemon::model::{remaining_sequence, Coin},
    t,
};

pub fn coins_view<'a>(
    cache: &Cache,
    coins: &'a [Coin],
    timelock: u16,
    selected: &[usize],
    labels: &'a HashMap<String, String>,
    labels_editing: &'a HashMap<String, form::Value<String>>,
) -> Element<'a, Message> {
    let map = btn_map(Some(Message::Menu(Menu::Map(None))));
    let header = row![new::d2(Menu::Coins.title()), Space::fill_width(), map]
        .align_y(Alignment::Center)
        .spacing(HSpacing::M);

    let list =
        coins
            .iter()
            .enumerate()
            .fold(Column::new().spacing(VSpacing::M), |col, (i, coin)| {
                col.push(coin_list_view(
                    coin,
                    timelock,
                    cache.blockheight() as u32,
                    i,
                    selected.contains(&i),
                    labels,
                    labels_editing,
                ))
            });

    column![header, list]
        .align_x(Alignment::Center)
        .spacing(VSpacing::XL)
        .into()
}

fn coin_list_view<'a>(
    coin: &'a Coin,
    timelock: u16,
    blockheight: u32,
    index: usize,
    expanded: bool,
    labels: &'a HashMap<String, String>,
    labels_editing: &'a HashMap<String, form::Value<String>>,
) -> Element<'a, Message> {
    let outpoint = coin.outpoint.to_string();
    let address = coin.address.to_string();
    let seq = remaining_sequence(coin, blockheight, timelock);

    let coin_label = liana::label::resolve(
        labels.get(&outpoint).map(String::as_str),
        &coin.default_label,
    );
    let label = if expanded {
        label::label_field(
            vec![outpoint.clone()],
            labels_editing.get(&outpoint),
            &coin_label,
            LabelSize::Entry,
        )
    } else {
        display_label(
            &coin_label,
            LabelSize::Entry.spec(),
            Some(LABEL_DISPLAY_MAX_CHARS),
        )
    };

    coins::coin_entry(
        coin.amount,
        coin.outpoint,
        &coin.address,
        labels.get(&address).map(String::as_str),
        coin.block_height,
        coin.spend_info.map(|info| coins::CoinSpend {
            txid: info.txid,
            height: info.height,
        }),
        seq,
        label,
        Message::Clipboard(address),
        Message::Clipboard(outpoint),
        Message::Menu(Menu::RefreshCoins(vec![coin.outpoint])),
        Message::Menu(Menu::Map(Some(MapFocus::Coin(coin.outpoint)))),
        expanded,
        Message::Select(index),
    )
}

/// returns y,m,d
pub fn expire_message_units(sequence: u32) -> Vec<String> {
    let mut n_minutes = sequence * 10;
    let n_years = n_minutes / 525960;
    n_minutes -= n_years * 525960;
    let n_months = n_minutes / 43830;
    n_minutes -= n_months * 43830;
    let n_days = n_minutes / 1440;

    #[allow(clippy::nonminimal_bool)]
    if n_years != 0 || n_months != 0 || n_days != 0 {
        let mut units = Vec::new();
        if n_years != 0 {
            units.push(t!("duration-years", count = n_years));
        }
        if n_months != 0 {
            units.push(t!("duration-months", count = n_months));
        }
        if n_days != 0 {
            units.push(t!("duration-days", count = n_days));
        }
        units
    } else {
        n_minutes -= n_days * 1440;
        let n_hours = n_minutes / 60;
        n_minutes -= n_hours * 60;
        let mut units = Vec::new();
        if n_hours != 0 {
            units.push(t!("duration-hours", count = n_hours));
        }
        if n_minutes != 0 {
            units.push(t!("duration-minutes", count = n_minutes));
        }
        units
    }
}

#[cfg(test)]
mod tests {
    use super::expire_message_units;
    #[test]
    fn test_expire_message_units() {
        let testcases = [
            (
                61,
                vec![
                    "\u{2068}10\u{2069} hours".to_string(),
                    "\u{2068}10\u{2069} minutes".to_string(),
                ],
            ),
            (1112, vec!["\u{2068}7\u{2069} days".to_string()]),
            (52600, vec!["1 year".to_string()]),
        ];

        for (seq, result) in testcases {
            assert_eq!(expire_message_units(seq), result);
        }
    }
}
