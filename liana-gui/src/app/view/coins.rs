use std::collections::HashMap;

use iced::{
    widget::{column, row, Space},
    Alignment, Length,
};

use liana_ui::{
    component::{
        address::address as address_view, amount::amount, badge, button, card, form,
        label::display_label, pill, text::new,
    },
    spacing::{HSpacing, VSpacing},
    theme,
    widget::{Column, Container, Element, Row, SpaceExt},
};

use crate::{
    app::{
        cache::Cache,
        menu::Menu,
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
    let title = Container::new(new::d2(Menu::Coins.title())).width(Length::Fill);

    let list =
        coins
            .iter()
            .enumerate()
            .fold(Column::new().spacing(VSpacing::S), |col, (i, coin)| {
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

    column![title, list]
        .align_x(Alignment::Center)
        .spacing(VSpacing::XXL)
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
    let txid = coin.outpoint.txid.to_string();
    let seq = remaining_sequence(coin, blockheight, timelock);

    let coin_label = liana::label::resolve(
        labels.get(&outpoint).map(String::as_str),
        &coin.default_label,
    );

    // The label is edited in the details, so the header only shows it while folded.
    let label = (!expanded).then(|| display_label(&coin_label, new::CAPTION_SPEC, None));

    let status = if coin.spend_info.is_some() {
        pill::spent()
    } else if coin.block_height.is_none() {
        pill::unconfirmed()
    } else {
        pill::coin_sequence(seq)
    };
    let summary = row![badge::coin(), label, Space::fill_width(), status]
        .spacing(HSpacing::M)
        .align_y(Alignment::Center)
        .width(Length::Fill);
    let header = row![summary, amount(&coin.amount)]
        .align_y(Alignment::Center)
        .spacing(HSpacing::XL);

    let label_editor = label::label_field(
        vec![outpoint.clone()],
        labels_editing.get(&outpoint),
        &coin_label,
        LabelSize::Body,
    );
    let recovery = match (coin.spend_info, coin.block_height) {
        (None, Some(b)) if blockheight > b as u32 + timelock as u32 => {
            Some(new::b5_bold(t!("coins-recovery-available")).style(theme::text::error))
        }
        (None, Some(b)) => Some(new::b5_bold(t!(
            "coins-first-recovery-in-blocks",
            blocks = b as u32 + timelock as u32 - blockheight
        ))),
        _ => None,
    };

    let address_label = info_row(
        t!("coins-address-label"),
        new::small_caption(
            labels
                .get(&address)
                .cloned()
                .unwrap_or_else(|| t!("common-no-label")),
        )
        .style(theme::text::secondary),
    )
    .align_y(Alignment::Center);
    let copy_address = button::btn_copy(Some(Message::Clipboard(address.clone())));
    let address = info_row(
        t!("common-address-label"),
        row![address_view(address), copy_address].align_y(Alignment::Center),
    )
    .align_y(Alignment::Center);
    let deposit = info_row(
        t!("coins-deposit-transaction-label"),
        new::small_caption(
            labels
                .get(&txid)
                .cloned()
                .unwrap_or_else(|| t!("common-no-label")),
        )
        .style(theme::text::secondary),
    )
    .align_y(Alignment::Center);
    let copy_outpoint = button::btn_copy(Some(Message::Clipboard(outpoint.clone())));
    let outpoint_row = info_row(
        t!("coins-outpoint"),
        row![
            new::small_caption(outpoint).style(theme::text::secondary),
            copy_outpoint
        ]
        .align_y(Alignment::Center),
    )
    .align_y(Alignment::Center);
    let block_height = coin.block_height.map(|b| {
        info_row(
            t!("coins-block-height"),
            new::small_caption(b.to_string()).style(theme::text::secondary),
        )
    });
    let coin_info = column![address_label, address, deposit, outpoint_row, block_height];

    let spend: Element<'a, Message> = match coin.spend_info {
        Some(info) => {
            let spend_txid = info_row(
                t!("coins-spend-txid"),
                new::small_caption(info.txid.to_string()),
            );
            let spend_height = match info.height {
                Some(height) => info_row(
                    t!("coins-spend-block-height"),
                    new::small_caption(height.to_string()),
                ),
                None => {
                    row![new::b5_bold(t!("coins-not-in-block")).style(theme::text::secondary)]
                }
            };
            column![spend_txid, spend_height]
                .spacing(VSpacing::XS)
                .into()
        }
        None => {
            let message = Some(Message::Menu(Menu::RefreshCoins(vec![coin.outpoint])));
            let refresh = button::btn_refresh_coin(message, seq == 0);
            column![row![Space::fill_width(), refresh]].into()
        }
    };

    let details = column![label_editor, recovery, coin_info, spend]
        .padding(10)
        .spacing(VSpacing::XS);

    card::foldable::FoldableCard::new(None, header, Some(details.into()))
        .expanded(expanded)
        .on_toggle(move || Message::Select(index))
        .padding(card::CardPadding::Soft)
        .into()
}

fn info_row<'a>(key: String, value: impl Into<Element<'a, Message>>) -> Row<'a, Message> {
    row![
        new::b5_bold(key).style(theme::text::secondary),
        value.into()
    ]
    .spacing(HSpacing::S)
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
