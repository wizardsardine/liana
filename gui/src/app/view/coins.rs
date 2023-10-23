use std::collections::HashMap;

use iced::{widget::Space, Alignment, Length};

use liana_ui::{
    color,
    component::{amount::*, badge, button, form, text::*},
    icon, theme,
    util::Collection,
    widget::*,
};

use crate::{
    app::{
        cache::Cache,
        menu::Menu,
        view::{label, message::Message},
    },
    daemon::model::{remaining_sequence, Coin},
};

pub fn coins_view<'a>(
    cache: &Cache,
    coins: &'a [Coin],
    timelock: u16,
    selected: &[usize],
    labels: &'a HashMap<String, String>,
    labels_editing: &'a HashMap<String, form::Value<String>>,
) -> Element<'a, Message> {
    Column::new()
        .push(Container::new(h3("Coins")).width(Length::Fill))
        .push(
            Column::new()
                .spacing(10)
                .push(coins.iter().enumerate().fold(
                    Column::new().spacing(10),
                    |col, (i, coin)| {
                        col.push(coin_list_view(
                            coin,
                            timelock,
                            cache.blockheight as u32,
                            i,
                            selected.contains(&i),
                            labels,
                            labels_editing,
                        ))
                    },
                )),
        )
        .align_items(Alignment::Center)
        .spacing(30)
        .into()
}

#[allow(clippy::collapsible_else_if)]
fn coin_list_view<'a>(
    coin: &'a Coin,
    timelock: u16,
    blockheight: u32,
    index: usize,
    collapsed: bool,
    labels: &'a HashMap<String, String>,
    labels_editing: &'a HashMap<String, form::Value<String>>,
) -> Container<'a, Message> {
    let outpoint = coin.outpoint.to_string();
    let address = coin.address.to_string();
    let txid = coin.outpoint.txid.to_string();
    Container::new(
        Column::new()
            .push(
                Button::new(
                    Row::new()
                        .push(
                            Row::new()
                                .push(badge::coin())
                                .push(if !collapsed {
                                    if let Some(label) = labels.get(&outpoint) {
                                        if !label.is_empty() {
                                            Container::new(p1_regular(label)).width(Length::Fill)
                                        } else if let Some(label) = labels.get(&txid) {
                                            Container::new(
                                                Row::new()
                                                    .spacing(5)
                                                    .push(
                                                        // It it not possible to know if a coin is a
                                                        // change coin or not so for now, From is
                                                        // enough
                                                        p1_regular("From").style(color::GREY_3),
                                                    )
                                                    .push(p1_regular(label)),
                                            )
                                            .width(Length::Fill)
                                        } else {
                                            Container::new(Space::with_width(Length::Fill))
                                                .width(Length::Fill)
                                        }
                                    } else if let Some(label) = labels.get(&txid) {
                                        Container::new(
                                            Row::new()
                                                .spacing(5)
                                                .push(
                                                    // It it not possible to know if a coin is a
                                                    // change coin or not so for now, From is
                                                    // enough
                                                    p1_regular("From").style(color::GREY_3),
                                                )
                                                .push(p1_regular(label)),
                                        )
                                        .width(Length::Fill)
                                    } else {
                                        Container::new(Space::with_width(Length::Fill))
                                            .width(Length::Fill)
                                    }
                                } else {
                                    Container::new(Space::with_width(Length::Fill))
                                        .width(Length::Fill)
                                })
                                .push(if coin.spend_info.is_some() {
                                    badge::spent()
                                } else if coin.block_height.is_none() {
                                    badge::unconfirmed()
                                } else {
                                    let seq = remaining_sequence(coin, blockheight, timelock);
                                    coin_sequence_label(seq, timelock as u32)
                                })
                                .spacing(10)
                                .align_items(Alignment::Center)
                                .width(Length::Fill),
                        )
                        .push(amount(&coin.amount))
                        .align_items(Alignment::Center)
                        .spacing(20),
                )
                .style(theme::Button::TransparentBorder)
                .padding(10)
                .on_press(Message::Select(index)),
            )
            .push_maybe(if collapsed {
                Some(
                    Column::new()
                        .padding(10)
                        .spacing(5)
                        .push(
                            Container::new(if let Some(label) = labels_editing.get(&outpoint) {
                                label::label_editing(vec![outpoint.clone()], label, P1_SIZE)
                            } else {
                                label::label_editable(
                                    vec![outpoint.clone()],
                                    labels.get(&outpoint),
                                    P1_SIZE,
                                )
                            })
                            .width(Length::Fill),
                        )
                        .push_maybe(if coin.spend_info.is_none() {
                            if let Some(b) = coin.block_height {
                                if blockheight > b as u32 + timelock as u32 {
                                    Some(Container::new(
                                        p1_bold("One of the recovery path is available")
                                            .style(color::RED),
                                    ))
                                } else {
                                    Some(Container::new(p1_bold(format!(
                                        "One of the recovery path will be available in {} blocks",
                                        b as u32 + timelock as u32 - blockheight
                                    ))))
                                }
                            } else {
                                None
                            }
                        } else {
                            None
                        })
                        .push(
                            Column::new()
                                .push(
                                    Row::new()
                                        .align_items(Alignment::Center)
                                        .push(
                                            p2_regular("Address label:")
                                                .bold()
                                                .style(color::GREY_2),
                                        )
                                        .push(if let Some(label) = labels.get(&address) {
                                            p2_regular(label).style(color::GREY_2)
                                        } else {
                                            p2_regular("No label").style(color::GREY_2)
                                        })
                                        .spacing(5),
                                )
                                .push(
                                    Row::new()
                                        .align_items(Alignment::Center)
                                        .push(p2_regular("Address:").bold().style(color::GREY_2))
                                        .push(
                                            Row::new()
                                                .align_items(Alignment::Center)
                                                .push(
                                                    p2_regular(address.clone())
                                                        .style(color::GREY_2),
                                                )
                                                .push(
                                                    Button::new(icon::clipboard_icon())
                                                        .on_press(Message::Clipboard(
                                                            address.clone(),
                                                        ))
                                                        .style(theme::Button::TransparentBorder),
                                                ),
                                        )
                                        .spacing(5),
                                )
                                .push(
                                    Row::new()
                                        .align_items(Alignment::Center)
                                        .push(
                                            p2_regular("Deposit transaction label:")
                                                .bold()
                                                .style(color::GREY_2),
                                        )
                                        .push(if let Some(label) = labels.get(&txid) {
                                            p2_regular(label).style(color::GREY_2)
                                        } else {
                                            p2_regular("No label").style(color::GREY_2)
                                        })
                                        .spacing(5),
                                )
                                .push(
                                    Row::new()
                                        .align_items(Alignment::Center)
                                        .push(p2_regular("Outpoint:").bold().style(color::GREY_2))
                                        .push(
                                            Row::new()
                                                .align_items(Alignment::Center)
                                                .push(
                                                    p2_regular(format!("{}", coin.outpoint))
                                                        .style(color::GREY_2),
                                                )
                                                .push(
                                                    Button::new(icon::clipboard_icon())
                                                        .on_press(Message::Clipboard(
                                                            coin.outpoint.to_string(),
                                                        ))
                                                        .style(theme::Button::TransparentBorder),
                                                ),
                                        )
                                        .spacing(5),
                                )
                                .push_maybe(coin.block_height.map(|b| {
                                    Row::new()
                                        .push(
                                            p2_regular("Block height:").bold().style(color::GREY_2),
                                        )
                                        .push(p2_regular(format!("{}", b)).style(color::GREY_2))
                                        .spacing(5)
                                })),
                        )
                        .push(if let Some(info) = coin.spend_info {
                            Column::new()
                                .push(
                                    Row::new()
                                        .push(p2_regular("Spend txid:").bold().style(color::GREY_2))
                                        .push(p2_regular(format!("{}", info.txid)))
                                        .spacing(5),
                                )
                                .push(if let Some(height) = info.height {
                                    Row::new()
                                        .push(
                                            p2_regular("Spend block height:")
                                                .bold()
                                                .style(color::GREY_2),
                                        )
                                        .push(p2_regular(format!("{}", height)))
                                        .spacing(5)
                                } else {
                                    Row::new().push(
                                        p2_regular("Not in a block").bold().style(color::GREY_2),
                                    )
                                })
                                .spacing(5)
                        } else {
                            Column::new().push(
                                Row::new().push(Space::with_width(Length::Fill)).push(
                                    button::primary(Some(icon::arrow_repeat()), "Refresh coin")
                                        .on_press(Message::Menu(Menu::RefreshCoins(vec![
                                            coin.outpoint,
                                        ]))),
                                ),
                            )
                        }),
                )
            } else {
                None
            }),
    )
    .style(theme::Container::Card(theme::Card::Simple))
}

pub fn coin_sequence_label<'a, T: 'a>(seq: u32, timelock: u32) -> Container<'a, T> {
    if seq == 0 {
        Container::new(
            Row::new()
                .spacing(5)
                .push(icon::clock_icon().width(Length::Fixed(20.0)))
                .push(p2_regular("Expired"))
                .align_items(Alignment::Center),
        )
        .padding(10)
        .style(theme::Container::Pill(theme::Pill::Warning))
    } else if seq < timelock * 10 / 100 {
        Container::new(
            Row::new()
                .spacing(5)
                .push(icon::clock_icon().width(Length::Fixed(20.0)))
                .push(p2_regular(expire_message(seq)))
                .align_items(Alignment::Center),
        )
        .padding(10)
        .style(theme::Container::Pill(theme::Pill::Simple))
    } else {
        Container::new(
            Row::new()
                .spacing(5)
                .push(icon::clock_icon().width(Length::Fixed(20.0)))
                .push(p2_regular(expire_message(seq)).style(color::GREY_3))
                .align_items(Alignment::Center),
        )
        .padding(10)
        .style(theme::Container::Pill(theme::Pill::Simple))
    }
}

pub fn expire_message(sequence: u32) -> String {
    if sequence <= 144 {
        "Expires today".to_string()
    } else if sequence <= 2 * 144 {
        "Expires in ≈ 2 days".to_string()
    } else {
        format!("Expires in {}", expire_message_units(sequence).join(","))
    }
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
    if n_days != 0 || n_months != 0 || n_days != 0 {
        [(n_years, "year"), (n_months, "month"), (n_days, "day")]
            .iter()
            .filter_map(|(n, u)| {
                if *n != 0 {
                    Some(format!("{} {}{}", n, u, if *n > 1 { "s" } else { "" }))
                } else {
                    None
                }
            })
            .collect()
    } else {
        n_minutes -= n_days * 1440;
        let n_hours = n_minutes / 60;
        n_minutes -= n_hours * 60;
        [(n_hours, "hour"), (n_minutes, "minute")]
            .iter()
            .filter_map(|(n, u)| {
                if *n != 0 {
                    Some(format!("{} {}{}", n, u, if *n > 1 { "s" } else { "" }))
                } else {
                    None
                }
            })
            .collect()
    }
}
