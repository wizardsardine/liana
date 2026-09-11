use std::collections::HashMap;

use iced::{
    widget::{column, row, Space},
    Alignment, Length,
};

use liana_ui::{
    component::{
        address::address as address_view,
        amount::*,
        badge, button, card, form, pill,
        text::{new, *},
    },
    icon, theme,
    widget::*,
};

use crate::{
    app::{
        cache::Cache,
        menu::Menu,
        view::{label, message::Message},
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
    let title = Container::new(panel_title(Menu::Coins.title())).width(Length::Fill);

    let list = coins
        .iter()
        .enumerate()
        .fold(Column::new().spacing(10), |col, (i, coin)| {
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
        .spacing(30)
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

    // The label is edited in the details, so the header only shows it while folded.
    let label: Option<Element<'a, Message>> = if expanded {
        None
    } else if let Some(label) = labels.get(&outpoint).filter(|label| !label.is_empty()) {
        Some(new::caption(label).into())
    } else {
        labels.get(&txid).map(|label| {
            // It is not possible to know if a coin is a change coin or not so for now, From is
            // enough
            let from = new::caption(t!("common-from")).style(theme::text::secondary);
            row![from, new::caption(label)].spacing(5).into()
        })
    };
    let label =
        Container::new(label.unwrap_or_else(|| Space::fill_width().into())).width(Length::Fill);

    let status = if coin.spend_info.is_some() {
        pill::spent()
    } else if coin.block_height.is_none() {
        pill::unconfirmed()
    } else {
        pill::coin_sequence(seq)
    };
    let summary = row![badge::coin(), label, status]
        .spacing(10)
        .align_y(Alignment::Center)
        .width(Length::Fill);
    let header = row![summary, amount(&coin.amount)]
        .align_y(Alignment::Center)
        .spacing(20);

    let details = {
        let label_editor = if let Some(label) = labels_editing.get(&outpoint) {
            label::label_editing(vec![outpoint.clone()], label, P1_SIZE)
        } else {
            label::label_editable(vec![outpoint.clone()], labels.get(&outpoint), P1_SIZE)
        };
        let label_editor = Container::new(label_editor).width(Length::Fill);

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

        let address_label = row![
            p2_regular(t!("coins-address-label"))
                .bold()
                .style(theme::text::secondary),
            p2_regular(
                labels
                    .get(&address)
                    .cloned()
                    .unwrap_or_else(|| t!("common-no-label"))
            )
            .style(theme::text::secondary)
        ]
        .align_y(Alignment::Center)
        .spacing(5);
        let copy_address = button::btn_copy(Some(Message::Clipboard(address.clone())));
        let address = row![
            p2_regular(t!("common-address-label"))
                .bold()
                .style(theme::text::secondary),
            row![address_view(address), copy_address].align_y(Alignment::Center)
        ]
        .align_y(Alignment::Center)
        .spacing(5);
        let deposit = row![
            p2_regular(t!("coins-deposit-transaction-label"))
                .bold()
                .style(theme::text::secondary),
            p2_regular(
                labels
                    .get(&txid)
                    .cloned()
                    .unwrap_or_else(|| t!("common-no-label"))
            )
            .style(theme::text::secondary)
        ]
        .align_y(Alignment::Center)
        .spacing(5);
        let copy_outpoint = button::btn_copy(Some(Message::Clipboard(outpoint.clone())));
        let outpoint_row = row![
            p2_regular(t!("coins-outpoint"))
                .bold()
                .style(theme::text::secondary),
            row![
                p2_regular(outpoint).style(theme::text::secondary),
                copy_outpoint
            ]
            .align_y(Alignment::Center)
        ]
        .align_y(Alignment::Center)
        .spacing(5);
        let block_height = coin.block_height.map(|b| {
            row![
                p2_regular(t!("coins-block-height"))
                    .bold()
                    .style(theme::text::secondary),
                p2_regular(b.to_string()).style(theme::text::secondary)
            ]
            .spacing(5)
        });
        let coin_info = column![address_label, address, deposit, outpoint_row, block_height];

        let spend = match coin.spend_info {
            Some(info) => {
                let spend_txid = row![
                    p2_regular(t!("coins-spend-txid"))
                        .bold()
                        .style(theme::text::secondary),
                    p2_regular(info.txid.to_string())
                ]
                .spacing(5);
                let spend_height = match info.height {
                    Some(height) => row![
                        p2_regular(t!("coins-spend-block-height"))
                            .bold()
                            .style(theme::text::secondary),
                        p2_regular(height.to_string())
                    ]
                    .spacing(5),
                    None => row![p2_regular(t!("coins-not-in-block"))
                        .bold()
                        .style(theme::text::secondary)],
                };
                column![spend_txid, spend_height].spacing(5)
            }
            None => {
                let icon = Some(icon::arrow_repeat());
                let label = t!("coins-refresh-coin");
                let refresh = if seq == 0 {
                    button::primary(icon, label)
                } else {
                    button::secondary(icon, label)
                }
                .on_press(Message::Menu(Menu::RefreshCoins(vec![coin.outpoint])));
                column![row![Space::fill_width(), refresh]]
            }
        };

        column![label_editor, recovery, coin_info, spend]
            .padding(10)
            .spacing(5)
    };

    card::foldable::FoldableCard::new(None, header, Some(details.into()))
        .expanded(expanded)
        .on_toggle(move || Message::Select(index))
        .padding(10)
        .into()
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
    use super::*;
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
