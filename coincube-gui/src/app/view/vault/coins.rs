use coincube_ui::locale::copy as localized_copy;
use coincube_ui::localized_format;
use std::collections::HashMap;

use iced::{
    widget::{checkbox, Space},
    Alignment, Length,
};

use coincube_ui::component::amount::BitcoinDisplayUnit;
use coincube_ui::{
    component::{
        amount::*,
        badge, button, form,
        tenshu::{
            action::ActionButton,
            field::{text_field, FieldSize},
        },
        text::*,
    },
    icon::{self, coins_icon},
    theme,
    widget::{
        action::Action, floating::Floating, horizontal_scroll, Column, Container, Element, Row,
        RowExt,
    },
};

use crate::{
    app::{
        cache::Cache,
        menu::Menu,
        view::{
            message::{CoinControlMessage, Message},
            placeholder,
            vault::split_coins,
        },
    },
    chain::ChainId,
    daemon::model::{remaining_sequence, Coin},
    services::chain_status::ChainStatus,
};

/// The Chains badge. Only positive evidence reads as *verified*; see
/// [`crate::services::chain_status`].
pub fn chain_badge<'a, T: 'a>(status: ChainStatus, here: ChainId) -> Container<'a, T> {
    Container::new(iced::widget::tooltip::Tooltip::new(
        Container::new(p2_regular(status.label(here)))
            .padding([5, 8])
            .center_x(Length::Shrink)
            .style(theme::pill::simple),
        Container::new(p1_regular(status.explanation(here)))
            .padding(10)
            .style(theme::card::simple),
        iced::widget::tooltip::Position::Top,
    ))
}

/// A saved coin's block height proves it was confirmed, even while the
/// daemon's chain tip has not been loaded (or is briefly behind the coin).
fn confirmation_status(block_height: Option<i32>, tip: u32) -> String {
    match block_height {
        None => "Unconfirmed".to_string(),
        Some(height) if height >= 0 && tip > 0 && (height as u32) <= tip => {
            let confirmations = tip - height as u32 + 1;
            format!(
                "{} confirmation{}",
                confirmations,
                if confirmations == 1 { "" } else { "s" }
            )
        }
        Some(_) => "Confirmed · confirmations unavailable".to_string(),
    }
}

fn recovery_timing_known(coin: &Coin, tip: u32) -> bool {
    tip > 0
        && coin
            .block_height
            .is_some_and(|height| height >= 0 && (height as u32) <= tip)
}

const AMOUNT_COLUMN_WIDTH: f32 = 160.0;
const CONFIRMATIONS_COLUMN_WIDTH: f32 = 100.0;
const DETAILS_COLUMN_WIDTH: f32 = 85.0;
const CHECK_COLUMN_WIDTH: f32 = 24.0;
const CHAINS_COLUMN_WIDTH: f32 = 110.0;
const STATUS_COLUMN_WIDTH: f32 = 216.0;
const TABLE_MIN_WIDTH: f32 = 1024.0;
pub const COIN_LABEL_INPUT_ID: &str = "vault.coin-label.input";

pub fn coin_action_id(outpoint: &str, action: &str) -> iced::advanced::widget::Id {
    iced::advanced::widget::Id::from(format!("vault.coin.{outpoint}.{action}"))
}

fn coin_copy<'a>(
    outpoint: &str,
    value: String,
    kind: &'static str,
    location: &str,
) -> Element<'a, Message> {
    iced::widget::tooltip(
        Action::new(
            coin_action_id(outpoint, &format!("copy-{kind}-{location}")),
            icon::clipboard_icon().size(16),
            Message::FocusAction,
        )
        .padding(6)
        .style(theme::button::transparent_border)
        .on_press(Message::CopyWithToast(
            value,
            format!("{kind} copied to clipboard"),
        )),
        Container::new(p2_regular(localized_format!(
            "Copy {}",
            "Copiar {}",
            kind.to_lowercase()
        )))
        .padding(8)
        .style(theme::card::simple),
        iced::widget::tooltip::Position::Top,
    )
    .into()
}

fn coin_label_popover<'a>(
    outpoint: &str,
    value: &'a form::Value<String>,
    has_label: bool,
    saving: bool,
) -> Element<'a, Message> {
    use theme::tenshu::button::ActionKind;
    let items = vec![outpoint.to_owned()];
    let confirm = Message::Label(items.clone(), crate::app::view::LabelMessage::Confirm);
    let cancel = Message::Label(items.clone(), crate::app::view::LabelMessage::Cancel);
    let input_items = items.clone();
    let mut field = text_field(
        COIN_LABEL_INPUT_ID,
        "Label",
        &value.value,
        Message::FocusAction,
    )
    .placeholder(localized_copy("e.g. Savings"))
    .size(FieldSize::Compact)
    .error_maybe((!value.valid).then_some("This label is too long. Try a shorter label."));
    if !saving {
        field = field.on_input(move |value| {
            Message::Label(
                input_items.clone(),
                crate::app::view::LabelMessage::Edited(value),
            )
        });
        if value.valid {
            field = field.on_submit(confirm.clone());
        }
    }
    let panel = Container::new(
        Column::new()
            .spacing(16)
            .push(
                Column::new()
                    .spacing(3)
                    .push(p1_bold(if has_label {
                        localized_copy("Edit coin label")
                    } else {
                        localized_copy("Add coin label")
                    }))
                    .push(
                        p2_regular(localized_copy("For this coin only"))
                            .style(theme::text::secondary),
                    ),
            )
            .push(field)
            .push(
                Row::new()
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .push(Space::new().width(Length::Fill))
                    .push(
                        ActionButton::new(
                            coin_action_id(outpoint, "label-cancel"),
                            localized_copy("Cancel"),
                            ActionKind::Secondary,
                            Message::FocusAction,
                        )
                        .compact()
                        .on_press_maybe((!saving).then_some(cancel.clone())),
                    )
                    .push(
                        ActionButton::new(
                            coin_action_id(outpoint, "label-save"),
                            if saving { "Saving…" } else { "Save label" },
                            ActionKind::Primary,
                            Message::FocusAction,
                        )
                        .compact()
                        .on_press_maybe((value.valid && !saving).then_some(confirm)),
                    ),
            ),
    )
    .padding(20)
    .width(320)
    .style(theme::tenshu::surface::panel);
    let floating = Floating::new(panel).gap(8.0).fit_to_window();
    if saving {
        floating.into()
    } else {
        floating.on_dismiss(cancel).dismiss_on_escape().into()
    }
}

fn coin_label(coin: &Coin, labels: &HashMap<String, String>) -> String {
    let outpoint = coin.outpoint.to_string();
    let txid = coin.outpoint.txid.to_string();
    if let Some(label) = labels.get(&outpoint).filter(|label| !label.is_empty()) {
        label.clone()
    } else if let Some(label) = labels.get(&txid).filter(|label| !label.is_empty()) {
        format!("From {label}")
    } else {
        "—".to_string()
    }
}

fn short_address(address: &str) -> String {
    if address.chars().count() <= 15 {
        address.to_string()
    } else {
        let start: String = address.chars().take(5).collect();
        let end: String = address
            .chars()
            .rev()
            .take(4)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        format!("{start}…{end}")
    }
}

fn coin_table_header<'a>(blake2b: bool) -> Container<'a, Message> {
    Container::new(
        Row::new()
            .spacing(12)
            .align_y(Alignment::Center)
            .push(Space::new().width(Length::Fixed(CHECK_COLUMN_WIDTH)))
            .push(
                Container::new(
                    p2_regular(localized_copy("Amount"))
                        .bold()
                        .style(theme::text::secondary),
                )
                .width(Length::Fixed(AMOUNT_COLUMN_WIDTH)),
            )
            .push(
                Container::new(
                    p2_regular(localized_copy("Confirmations"))
                        .bold()
                        .style(theme::text::secondary),
                )
                .width(Length::Fixed(CONFIRMATIONS_COLUMN_WIDTH)),
            )
            .push_maybe(blake2b.then(|| {
                Container::new(
                    p2_regular(localized_copy("Chains"))
                        .bold()
                        .style(theme::text::secondary),
                )
                .width(Length::Fixed(CHAINS_COLUMN_WIDTH))
            }))
            .push(
                Container::new(
                    p2_regular(localized_copy("Status"))
                        .bold()
                        .style(theme::text::secondary),
                )
                .width(Length::Fixed(STATUS_COLUMN_WIDTH)),
            )
            .push(
                Container::new(
                    p2_regular(localized_copy("Label"))
                        .bold()
                        .style(theme::text::secondary),
                )
                .width(Length::FillPortion(1)),
            )
            .push(
                Container::new(
                    p2_regular(localized_copy("Address"))
                        .bold()
                        .style(theme::text::secondary),
                )
                .width(Length::FillPortion(1)),
            )
            .push(
                Container::new(
                    p2_regular(localized_copy("Details"))
                        .bold()
                        .style(theme::text::secondary),
                )
                .width(Length::Fixed(DETAILS_COLUMN_WIDTH)),
            ),
    )
    .padding(14)
    .width(Length::Fill)
}

#[allow(clippy::too_many_arguments)]
pub fn coins_view<'a>(
    cache: &'a Cache,
    coins: &'a [Coin],
    timelock: Option<u16>,
    selected: &'a [usize],
    labels: &'a HashMap<String, String>,
    labels_editing: &'a HashMap<String, form::Value<String>>,
    bitcoin_unit: BitcoinDisplayUnit,
    blake2b: bool,
    control: CoinControl,
) -> Element<'a, Message> {
    let chosen_total: coincube_core::miniscript::bitcoin::Amount = coins
        .iter()
        .filter(|coin| control.chosen.contains(&coin.outpoint))
        .map(|coin| coin.amount)
        .sum();
    let chosen_count = control.chosen.len();
    let selection_bar = control.selection_bar;
    let label_saving = control.label_saving;
    let chosen = control.chosen;
    Column::new()
        .push(Container::new(h3(localized_copy("Coin control")).bold()).width(Length::Fill))
        .push(
            p1_regular(if blake2b {
                localized_copy("Inspect coin balances, labels, confirmations, replay status and recovery timing.")
            } else {
                localized_copy("Inspect coin balances, labels, confirmations and recovery timing.")
            })
            .width(Length::Fill)
            .style(theme::text::secondary),
        )
        .push(control.both_chains.map(|(count, total, can_split)| {
            split_coins::both_chains_banner(
                cache.chain(),
                count,
                total,
                can_split,
                cache.balance_amounts_masked(),
            )
        }))
        .push(coins.is_empty().then(|| {
            placeholder(
                coins_icon().size(80),
                coincube_ui::locale::copy("No coins yet"),
                coincube_ui::locale::copy("Coins will appear here after this Vault receives bitcoin."),
            )
        }))
        .push((!coins.is_empty()).then(|| {
            crate::app::view::responsive(move |size| {
                let table =
                    Container::new(Column::new().push(coin_table_header(blake2b)).push(
                        coins.iter().enumerate().fold(
                            Column::new().spacing(0),
                            |col, (i, coin)| {
                                col.push(iced::widget::rule::horizontal(1))
                                    .push(coin_list_view(
                                        coin,
                                        timelock,
                                        cache.blockheight().max(0) as u32,
                                        i,
                                        selected.contains(&i),
                                        chosen.contains(&coin.outpoint),
                                        labels,
                                        labels_editing,
                                        bitcoin_unit,
                                        cache.balance_amounts_masked(),
                                        blake2b.then(|| {
                                            (cache.chain_status(&coin.outpoint), cache.chain())
                                        }),
                                        label_saving == Some(coin.outpoint),
                                    ))
                            },
                        ),
                    ))
                    .style(theme::card::simple)
                    .width(size.width.max(if blake2b {
                        TABLE_MIN_WIDTH
                    } else {
                        TABLE_MIN_WIDTH - CHAINS_COLUMN_WIDTH - 12.0
                    }));
                horizontal_scroll(table)
            })
            .height(Length::Shrink)
        }))
        .push((selection_bar && !coins.is_empty()).then(|| {
            split_coins::selection_bar(chosen_count, chosen_total, cache.balance_amounts_masked())
        }))
        .align_x(Alignment::Center)
        .spacing(35)
        .into()
}

/// Coin control's selection and split state, as the table shows it.
#[derive(Debug, Default)]
pub struct CoinControl {
    /// Ticked coins, for Send selected.
    pub chosen: std::collections::BTreeSet<coincube_core::miniscript::bitcoin::OutPoint>,
    /// Coins still on both chains (count, total, whether Split coins can
    /// open now), when the Chains column shows and any are.
    pub both_chains: Option<(usize, coincube_core::miniscript::bitcoin::Amount, bool)>,
    /// Whether to show the selection bar.
    pub selection_bar: bool,
    /// The label write in flight; its editor stays visible but cannot resubmit.
    pub label_saving: Option<coincube_core::miniscript::bitcoin::OutPoint>,
}

#[allow(clippy::collapsible_else_if)]
#[allow(clippy::too_many_arguments)]
fn coin_list_view<'a>(
    coin: &'a Coin,
    timelock: Option<u16>,
    blockheight: u32,
    index: usize,
    collapsed: bool,
    chosen: bool,
    labels: &'a HashMap<String, String>,
    labels_editing: &'a HashMap<String, form::Value<String>>,
    bitcoin_unit: BitcoinDisplayUnit,
    mask_amounts: bool,
    chain: Option<(ChainStatus, ChainId)>,
    label_saving: bool,
) -> Container<'a, Message> {
    let outpoint = coin.outpoint.to_string();
    let address = coin.address.to_string();
    let txid = coin.outpoint.txid.to_string();
    let amount: Element<'a, Message> = if mask_amounts {
        text("••••••").into()
    } else {
        amount_with_unit(&coin.amount, bitcoin_unit).into()
    };
    let status = coin_status_badge(coin, timelock, blockheight);
    let label = coin_label(coin, labels);
    let has_label = labels.get(&outpoint).is_some_and(|label| !label.is_empty());
    let label_trigger = Action::new(
        coin_action_id(&outpoint, "label"),
        Row::new()
            .spacing(5)
            .align_y(Alignment::Center)
            .push(
                p2_regular(if label == "—" && !has_label {
                    "Add label".to_owned()
                } else {
                    label
                })
                .wrapping(iced::widget::text::Wrapping::WordOrGlyph),
            )
            .push(
                if has_label {
                    icon::pencil_icon()
                } else {
                    icon::plus_icon()
                }
                .size(14),
            ),
        Message::FocusAction,
    )
    .padding([4, 0])
    .style(theme::button::transparent_border)
    .on_press_maybe((!label_saving).then_some(Message::CoinControl(
        CoinControlMessage::EditLabel(coin.outpoint),
    )));
    // No enclosing row button: copy, labels, checkboxes and details each own
    // their activation, so a copy or edit never expands the coin by accident.
    let row = Row::new()
        .spacing(12)
        .align_y(Alignment::Center)
        .push(
            Container::new(checkbox(chosen).on_toggle(move |_| {
                Message::CoinControl(CoinControlMessage::Choose(coin.outpoint))
            }))
            .width(Length::Fixed(CHECK_COLUMN_WIDTH)),
        )
        .push(Container::new(amount).width(Length::Fixed(AMOUNT_COLUMN_WIDTH)))
        .push(
            Container::new(
                p2_regular(confirmation_status(coin.block_height, blockheight))
                    .style(theme::text::secondary),
            )
            .width(Length::Fixed(CONFIRMATIONS_COLUMN_WIDTH)),
        )
        .push_maybe(chain.map(|(status, here)| {
            Container::new(chain_badge(status, here)).width(Length::Fixed(CHAINS_COLUMN_WIDTH))
        }))
        .push(
            Container::new(
                status
                    .map(Element::from)
                    .unwrap_or_else(|| p2_regular("—").into()),
            )
            .width(Length::Fixed(STATUS_COLUMN_WIDTH)),
        )
        .push(Container::new(label_trigger).width(Length::FillPortion(1)))
        .push(
            Container::new(
                Row::new()
                    .spacing(3)
                    .align_y(Alignment::Center)
                    .push(p2_regular(short_address(&address)).style(theme::text::secondary))
                    .push(coin_copy(
                        &outpoint,
                        address.clone(),
                        coincube_ui::locale::copy("Address"),
                        "row",
                    )),
            )
            .width(Length::FillPortion(1)),
        )
        .push(
            Container::new(
                Action::new(
                    coin_action_id(&outpoint, "details"),
                    p2_regular(if collapsed {
                        localized_copy("Hide details")
                    } else {
                        localized_copy("Details ▾")
                    })
                    .width(Length::Fill)
                    .style(theme::text::secondary),
                    Message::FocusAction,
                )
                .padding([4, 0])
                .style(theme::button::transparent_border)
                .on_press(Message::Select(index)),
            )
            .width(Length::Fixed(DETAILS_COLUMN_WIDTH)),
        );
    let editor = labels_editing.get(&outpoint).map(|value| {
        // The trailing region is wider than the Label cell, allowing the
        // anchored editor to overlap subsequent columns without moving them.
        let prefix = CHECK_COLUMN_WIDTH
            + AMOUNT_COLUMN_WIDTH
            + CONFIRMATIONS_COLUMN_WIDTH
            + STATUS_COLUMN_WIDTH
            + 4.0 * 12.0
            + if chain.is_some() {
                CHAINS_COLUMN_WIDTH + 12.0
            } else {
                0.0
            };
        Row::new()
            .push(Space::new().width(prefix))
            .push(coin_label_popover(
                &outpoint,
                value,
                has_label,
                label_saving,
            ))
    });
    Container::new(
        Column::new()
            .push(
                Container::new(Column::new().push(row).push(editor))
                    .padding(14)
                    .width(Length::Fill),
            )
            .push(if collapsed {
                Some(
                    Column::new()
                        .padding(10)
                        .spacing(5)
                        .push(coin_recovery_detail(coin, timelock, blockheight))
                        .push(coin_identifiers(coin, labels, &outpoint, &address, &txid))
                        .push(coin_spend_detail(coin, timelock, blockheight)),
                )
            } else {
                None
            }),
    )
    .width(Length::Fill)
}

fn coin_status_badge<'a>(
    coin: &Coin,
    timelock: Option<u16>,
    blockheight: u32,
) -> Option<Container<'a, Message>> {
    if coin.spend_info.is_some() {
        Some(badge::spent())
    } else if coin.block_height.is_none() {
        Some(badge::unconfirmed())
    } else if let Some(timelock) = timelock {
        Some(if recovery_timing_known(coin, blockheight) {
            coin_sequence_label(
                remaining_sequence(coin, blockheight, timelock),
                timelock as u32,
            )
        } else {
            badge::badge_pill(
                coincube_ui::locale::copy("Recovery timing unavailable"),
                coincube_ui::locale::copy(
                    "The chain tip is not available yet. Recovery timing will appear after sync.",
                ),
            )
        })
    } else if !coin.is_immature {
        Some(badge::badge_pill(
            coincube_ui::locale::copy("Spendable"),
            coincube_ui::locale::copy("This confirmed coin can use the primary spending path with its required signatures."),
        ))
    } else {
        None
    }
}

fn coin_recovery_detail<'a>(
    coin: &Coin,
    timelock: Option<u16>,
    blockheight: u32,
) -> Option<Container<'a, Message>> {
    if coin.spend_info.is_none() {
        if let (Some(_), Some(timelock)) = (coin.block_height, timelock) {
            if !recovery_timing_known(coin, blockheight) {
                Some(Container::new(
                    p1_regular(localized_copy("Recovery timing unavailable"))
                        .style(theme::text::secondary),
                ))
            } else if remaining_sequence(coin, blockheight, timelock) == 0 {
                Some(Container::new(
                    p1_bold(localized_copy("One or more recovery paths are available"))
                        .style(theme::text::error),
                ))
            } else {
                Some(Container::new(p1_bold(localized_format!(
                    "First recovery path will be available in {} blocks",
                    "La primera ruta de recuperación estará disponible en bloques {}",
                    remaining_sequence(coin, blockheight, timelock)
                ))))
            }
        } else {
            None
        }
    } else {
        None
    }
}

fn coin_identifiers<'a>(
    coin: &Coin,
    labels: &'a HashMap<String, String>,
    outpoint: &str,
    address: &str,
    txid: &str,
) -> Column<'a, Message> {
    Column::new()
        .push(
            Row::new()
                .align_y(Alignment::Center)
                .push(
                    p2_regular(localized_copy("Address label:"))
                        .bold()
                        .style(theme::text::secondary),
                )
                .push(if let Some(label) = labels.get(address) {
                    p2_regular(label).style(theme::text::secondary)
                } else {
                    p2_regular(localized_copy("No label")).style(theme::text::secondary)
                })
                .spacing(5),
        )
        .push(
            Row::new()
                .align_y(Alignment::Center)
                .push(
                    p2_regular(localized_copy("Address:"))
                        .bold()
                        .style(theme::text::secondary),
                )
                .push(
                    Row::new()
                        .align_y(Alignment::Center)
                        .push(p2_regular(address.to_owned()).style(theme::text::secondary))
                        .push(coin_copy(
                            outpoint,
                            address.to_owned(),
                            coincube_ui::locale::copy("Address"),
                            "details",
                        )),
                )
                .spacing(5),
        )
        .push(
            Row::new()
                .align_y(Alignment::Center)
                .push(
                    p2_regular(localized_copy("Deposit transaction label:"))
                        .bold()
                        .style(theme::text::secondary),
                )
                .push(if let Some(label) = labels.get(txid) {
                    p2_regular(label).style(theme::text::secondary)
                } else {
                    p2_regular(localized_copy("No label")).style(theme::text::secondary)
                })
                .spacing(5),
        )
        .push(
            Row::new()
                .align_y(Alignment::Center)
                .spacing(5)
                .push(
                    p2_regular(localized_copy("Transaction ID:"))
                        .bold()
                        .style(theme::text::secondary),
                )
                .push(
                    p2_regular(txid.to_owned())
                        .style(theme::text::secondary)
                        .wrapping(iced::widget::text::Wrapping::WordOrGlyph),
                )
                .push(coin_copy(
                    outpoint,
                    txid.to_owned(),
                    coincube_ui::locale::copy("Transaction ID"),
                    "details",
                )),
        )
        .push(
            Row::new()
                .align_y(Alignment::Center)
                .push(
                    p2_regular(localized_copy("Outpoint:"))
                        .bold()
                        .style(theme::text::secondary),
                )
                .push(
                    Row::new()
                        .align_y(Alignment::Center)
                        .push(
                            p2_regular(format!("{}", coin.outpoint)).style(theme::text::secondary),
                        )
                        .push(coin_copy(
                            outpoint,
                            outpoint.to_owned(),
                            coincube_ui::locale::copy("Outpoint"),
                            "details",
                        )),
                )
                .spacing(5),
        )
        .push(coin.block_height.map(|b| {
            Row::new()
                .push(
                    p2_regular(localized_copy("Block height:"))
                        .bold()
                        .style(theme::text::secondary),
                )
                .push(p2_regular(format!("{}", b)).style(theme::text::secondary))
                .spacing(5)
        }))
}

fn coin_spend_detail<'a>(
    coin: &Coin,
    timelock: Option<u16>,
    blockheight: u32,
) -> Column<'a, Message> {
    if let Some(info) = coin.spend_info {
        Column::new()
            .push(
                Row::new()
                    .push(
                        p2_regular(localized_copy("Spend txid:"))
                            .bold()
                            .style(theme::text::secondary),
                    )
                    .push(p2_regular(format!("{}", info.txid)))
                    .spacing(5),
            )
            .push(if let Some(height) = info.height {
                Row::new()
                    .push(
                        p2_regular(localized_copy("Spend block height:"))
                            .bold()
                            .style(theme::text::secondary),
                    )
                    .push(p2_regular(format!("{}", height)))
                    .spacing(5)
            } else {
                Row::new().push(
                    p2_regular(localized_copy("Not in a block"))
                        .bold()
                        .style(theme::text::secondary),
                )
            })
            .spacing(5)
    } else {
        Column::new().push(Row::new().push(Space::new().width(Length::Fill)).push({
            let (icon, label) = (Some(icon::arrow_repeat()), "Refresh coin");
            let refresh_btn = if recovery_timing_known(coin, blockheight)
                && timelock
                    .is_some_and(|timelock| remaining_sequence(coin, blockheight, timelock) == 0)
            {
                button::primary_compact(icon, label)
            } else {
                button::secondary_compact(icon, label)
            };
            refresh_btn.on_press(Message::Menu(Menu::Vault(
                crate::app::menu::VaultSubMenu::Coins(Some(vec![coin.outpoint])),
            )))
        }))
    }
}

pub fn coin_sequence_label<'a, T: 'a>(seq: u32, timelock: u32) -> Container<'a, T> {
    if seq == 0 {
        Container::new(
            Row::new()
                .spacing(5)
                .push(icon::clock_icon().width(Length::Fixed(20.0)))
                .push(p2_regular(localized_copy("Recovery open")))
                .align_y(Alignment::Center),
        )
        .padding(10)
        .style(theme::pill::warning)
    } else if seq < timelock * 10 / 100 {
        Container::new(
            Row::new()
                .spacing(5)
                .push(icon::clock_icon().width(Length::Fixed(20.0)))
                .push(p2_regular(recovery_status_copy(seq)))
                .align_y(Alignment::Center),
        )
        .padding(10)
        .style(theme::pill::simple)
    } else {
        Container::new(
            Row::new()
                .spacing(5)
                .push(icon::clock_icon().width(Length::Fixed(20.0)))
                .push(p2_regular(recovery_status_copy(seq)).style(theme::text::secondary))
                .align_y(Alignment::Center),
        )
        .padding(10)
        .style(theme::pill::simple)
    }
}

fn recovery_status_copy(remaining_blocks: u32) -> String {
    if remaining_blocks == 0 {
        "Recovery open".to_string()
    } else if remaining_blocks < 144 {
        format!(
            "Recovery in {} block{}",
            remaining_blocks,
            if remaining_blocks == 1 { "" } else { "s" }
        )
    } else {
        let days = remaining_blocks.saturating_add(143) / 144;
        format!(
            "Recovery in ≈ {} day{}",
            days,
            if days == 1 { "" } else { "s" }
        )
    }
}

/// Human duration used by the Vault overview's recovery summary.
/// Ten minutes a block, as [`expire_message_units`] counts.
const MINUTES_PER_BLOCK: u32 = 10;
const MINUTES_PER_DAY: u32 = 1440;

/// Days until `sequence` blocks have passed, rounded up: a path one block
/// away is "1 day" away, not "0 days". [`expire_message_units`] floors, for
/// its "≈ N left" breakdown.
pub fn blocks_to_days_ceil(sequence: u32) -> u32 {
    (sequence.saturating_mul(MINUTES_PER_BLOCK)).div_ceil(MINUTES_PER_DAY)
}

pub fn expire_message_units(sequence: u32) -> Vec<String> {
    let mut n_minutes = sequence * MINUTES_PER_BLOCK;
    let n_years = n_minutes / 525960;
    n_minutes -= n_years * 525960;
    let n_months = n_minutes / 43830;
    n_minutes -= n_months * 43830;
    let n_days = n_minutes / MINUTES_PER_DAY;

    #[allow(clippy::nonminimal_bool)]
    if n_years != 0 || n_months != 0 || n_days != 0 {
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
        n_minutes -= n_days * MINUTES_PER_DAY;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::preferences::{BalancePrivacy, BalancePrivacyChrome};
    use crate::app::view::shared::landing::capture;
    use crate::services::entangled::Entanglement;
    use coincube_core::miniscript::bitcoin::{bip32::ChildNumber, Address, Amount, OutPoint, Txid};
    use iced::widget::scrollable;
    use std::str::FromStr;

    fn confirmed_coin(height: i32) -> Coin {
        Coin {
            amount: Amount::from_sat(98_765_432),
            outpoint: OutPoint::new(Txid::from_str(&format!("{:064x}", 1)).unwrap(), 0),
            address: Address::from_str("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4")
                .unwrap()
                .assume_checked(),
            block_height: Some(height),
            derivation_index: ChildNumber::from_normal_idx(0).unwrap(),
            spend_info: None,
            is_immature: false,
            is_change: false,
            is_from_self: false,
        }
    }

    #[test]
    fn vertical_wheel_over_the_table_scrolls_the_page_after_horizontal_scrolling() {
        use crate::app::{menu::VaultSubMenu, view::page_head_tests};
        use iced::advanced::{widget::Tree, Layout, Shell};
        use iced::{mouse, Event, Rectangle, Size};

        let renderer = page_head_tests::renderer();
        let cache = Cache::default();
        let menu = Menu::Vault(VaultSubMenu::Coins(None));
        let coins: Vec<_> = (0..24)
            .map(|vout| {
                let mut coin = confirmed_coin(100);
                coin.outpoint.vout = vout;
                coin
            })
            .collect();
        let labels = HashMap::new();
        let editing = HashMap::new();

        for vertical in [
            mouse::ScrollDelta::Lines { x: 0.0, y: -2.0 },
            mouse::ScrollDelta::Pixels { x: -2.0, y: -80.0 },
        ] {
            let mut page = crate::app::view::dashboard(
                &menu,
                &cache,
                coins_view(
                    &cache,
                    &coins,
                    None,
                    &[],
                    &labels,
                    &editing,
                    BitcoinDisplayUnit::Sats,
                    true,
                    CoinControl::default(),
                ),
            );
            let mut tree = Tree::new(page.as_widget());
            let (node, probe) = page_head_tests::frame(&mut tree, &renderer, &mut page);
            let cursor = mouse::Cursor::Available(probe.text("Amount").center());
            let mut wheel = |delta| {
                let mut messages = Vec::new();
                let mut shell = Shell::new(&mut messages);
                page.as_widget_mut().update(
                    &mut tree,
                    &Event::Mouse(mouse::Event::WheelScrolled { delta }),
                    Layout::new(&node),
                    cursor,
                    &renderer,
                    &mut iced::advanced::clipboard::Null,
                    &mut shell,
                    &Rectangle::with_size(Size::new(1024.0, 768.0)),
                );
                (shell.is_event_captured(), messages)
            };
            let (captured, messages) = wheel(mouse::ScrollDelta::Pixels { x: -80.0, y: 0.0 });
            assert!(captured, "the narrow table accepts horizontal scrolling");
            assert!(
                messages.is_empty(),
                "the page stays put during horizontal scrolling"
            );
            let (_, messages) = wheel(vertical);
            assert!(
                messages
                    .iter()
                    .any(|message| matches!(message, Message::Scroll(y) if *y > 0.0)),
                "the page scrolls vertically over the table after {vertical:?}",
                vertical = vertical,
            );
        }
    }

    #[test]
    fn confirmation_copy_distinguishes_unknown_tip_from_unconfirmed_coin() {
        assert_eq!(confirmation_status(None, 0), "Unconfirmed");
        assert_eq!(
            confirmation_status(Some(100), 0),
            "Confirmed · confirmations unavailable"
        );
        assert_eq!(
            confirmation_status(Some(100), 99),
            "Confirmed · confirmations unavailable"
        );
        assert_eq!(confirmation_status(Some(100), 100), "1 confirmation");
        assert_eq!(confirmation_status(Some(100), 101), "2 confirmations");
    }

    #[test]
    fn hidden_coins_list_masks_amount_without_removing_coin() {
        let coins = [confirmed_coin(100)];
        let labels = HashMap::new();
        let selected = [];
        for privacy in [BalancePrivacy::Hidden, BalancePrivacy::Shown] {
            let cache = Cache {
                balance_privacy: Some(BalancePrivacyChrome {
                    privacy,
                    busy: false,
                    interactive: false,
                }),
                ..Default::default()
            };
            let mounted = capture::inspect_full_window(
                "vault-coins-privacy",
                coins_view(
                    &cache,
                    &coins,
                    None,
                    &selected,
                    &labels,
                    &HashMap::new(),
                    BitcoinDisplayUnit::Sats,
                    false,
                    CoinControl::default(),
                ),
                (1024, 768),
                "dark",
                theme::Theme::dark(),
            );
            assert!(mounted.contains("Coin control"));
            assert!(!mounted.contains("replay status and recovery timing"));
            assert!(mounted.contains("Confirmations"));
            assert!(mounted.contains("Address"));
            assert!(mounted.contains("Status"));
            assert!(
                !mounted.contains("Chains"),
                "no chain surface with Blake2b off"
            );
            assert!(mounted.contains("Details ▾"));
            assert!(mounted.contains("Spendable"));
            assert!(mounted.contains("Confirmed · confirmations unavailable"));
            assert_eq!(
                mounted.contains("98,765,432"),
                privacy == BalancePrivacy::Shown
            );
            assert_eq!(
                mounted.contains("••••••"),
                privacy == BalancePrivacy::Hidden
            );
        }
    }

    #[test]
    fn coin_row_uses_outpoint_label_then_deposit_label_and_short_address() {
        let coin = confirmed_coin(100);
        let mut labels = HashMap::new();
        assert_eq!(coin_label(&coin, &labels), "—");
        labels.insert(coin.outpoint.txid.to_string(), "Payroll".into());
        assert_eq!(coin_label(&coin, &labels), "From Payroll");
        labels.insert(coin.outpoint.to_string(), "Reserve".into());
        assert_eq!(coin_label(&coin, &labels), "Reserve");
        assert_eq!(
            short_address("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4"),
            "bc1qw…f3t4"
        );
    }

    #[test]
    fn coin_table_mounts_at_minimum_and_wide_desktop_widths_in_both_themes() {
        let coins = [confirmed_coin(100)];
        let labels = HashMap::new();
        let cache = Cache::default();
        for (size, appearance, theme) in [
            ((1024, 768), "dark", theme::Theme::dark()),
            ((1024, 768), "light", theme::Theme::light()),
            ((1440, 900), "dark", theme::Theme::dark()),
            ((1440, 900), "light", theme::Theme::light()),
        ] {
            let mounted = capture::inspect_full_window(
                "vault-coin-table",
                coins_view(
                    &cache,
                    &coins,
                    None,
                    &[],
                    &labels,
                    &HashMap::new(),
                    BitcoinDisplayUnit::Sats,
                    false,
                    CoinControl::default(),
                ),
                size,
                appearance,
                theme,
            );
            assert!(mounted.contains("Details ▾"));
            assert!(mounted.contains("bc1qw…f3t4"));
        }
    }

    #[test]
    fn unmasked_large_amounts_fit_compact_coin_rows_in_sats_and_btc() {
        let mut one_btc = confirmed_coin(100);
        one_btc.amount = Amount::from_sat(100_000_000);
        let mut twenty_one_btc = confirmed_coin(101);
        twenty_one_btc.amount = Amount::from_sat(2_100_000_000);
        twenty_one_btc.outpoint.vout = 1;
        let coins = [one_btc, twenty_one_btc];
        let cache = Cache {
            balance_privacy: Some(BalancePrivacyChrome {
                privacy: BalancePrivacy::Shown,
                busy: false,
                interactive: false,
            }),
            ..Default::default()
        };
        for unit in [BitcoinDisplayUnit::Sats, BitcoinDisplayUnit::BTC] {
            for (appearance, theme) in [
                ("dark", theme::Theme::dark()),
                ("light", theme::Theme::light()),
            ] {
                let scenario = format!("vault-coin-unmasked-{}", unit);
                let mounted = capture::inspect_full_window(
                    &scenario,
                    coins_view(
                        &cache,
                        &coins,
                        None,
                        &[],
                        &HashMap::new(),
                        &HashMap::new(),
                        unit,
                        false,
                        CoinControl::default(),
                    ),
                    (1024, 768),
                    appearance,
                    theme,
                );
                assert!(!mounted.contains("••••••"));
                assert!(mounted.contains(unit.label()));
                assert!(mounted.contains("bc1qw…f3t4"));
                assert!(mounted.contains("Details ▾"));
            }
        }
    }

    #[test]
    fn coin_table_captures_empty_and_long_label_states() {
        let coins = [confirmed_coin(100)];
        let cache = Cache::default();
        let labels = HashMap::new();
        let empty = capture::inspect_full_window(
            "vault-coin-empty",
            coins_view(
                &cache,
                &[],
                None,
                &[],
                &labels,
                &HashMap::new(),
                BitcoinDisplayUnit::Sats,
                false,
                CoinControl::default(),
            ),
            (1024, 768),
            "light",
            theme::Theme::light(),
        );
        assert!(empty.contains("No coins yet"));
        assert!(!empty.contains("Confirmations"));

        let mut labels = HashMap::new();
        labels.insert(
            coins[0].outpoint.to_string(),
            "Long reserve label for a time-locked deposit held for a shared treasury".into(),
        );
        let long_label = capture::inspect_full_window(
            "vault-coin-long-label",
            coins_view(
                &cache,
                &coins,
                None,
                &[],
                &labels,
                &HashMap::new(),
                BitcoinDisplayUnit::Sats,
                false,
                CoinControl::default(),
            ),
            (1024, 768),
            "dark",
            theme::Theme::dark(),
        );
        assert!(long_label.contains("Long reserve label"));
        assert!(long_label.contains("Details ▾"));

        labels.insert(
            coins[0].outpoint.to_string(),
            "TreasuryReserveDepositWithoutSpacesForCompactWidths".into(),
        );
        let unbroken_label = capture::inspect_full_window(
            "vault-coin-unbroken-label",
            coins_view(
                &cache,
                &coins,
                None,
                &[],
                &labels,
                &HashMap::new(),
                BitcoinDisplayUnit::Sats,
                false,
                CoinControl::default(),
            ),
            (1024, 768),
            "light",
            theme::Theme::light(),
        );
        assert!(unbroken_label.contains("TreasuryReserveDepositWithoutSpacesForCompactWidths"));
    }

    #[test]
    fn coin_table_captures_unknown_sync_and_each_replay_status() {
        let coins = [confirmed_coin(100)];
        let labels = HashMap::new();
        for (answer, expected, scenario) in [
            (
                Entanglement::Unknown,
                "Not checked yet",
                "vault-coin-replay-unknown",
            ),
            (
                Entanglement::Entangled,
                "Both chains",
                "vault-coin-replay-both",
            ),
            (
                Entanglement::NotEntangled,
                "Not seen on Blake2b · checking",
                "vault-coin-replay-unverified",
            ),
        ] {
            let mut cache = Cache::default();
            cache.record_entanglement(coins[0].outpoint.txid, answer, std::time::Instant::now());
            let mounted = capture::inspect_full_window(
                scenario,
                coins_view(
                    &cache,
                    &coins,
                    Some(1008),
                    &[0],
                    &labels,
                    &HashMap::new(),
                    BitcoinDisplayUnit::Sats,
                    true,
                    CoinControl::default(),
                ),
                (1024, 768),
                "light",
                theme::Theme::light(),
            );
            assert!(mounted.contains(expected));
            assert!(mounted.contains("replay status and recovery timing"));
            assert!(mounted.contains("Recovery timing unavailable"));
            assert!(mounted.contains("Hide details"));
        }
    }

    #[test]
    fn recovery_timing_waits_for_a_known_tip_and_details_show_disclosure_state() {
        let coins = [confirmed_coin(100)];
        let labels = HashMap::new();
        let unknown = Cache::default();
        let unknown_view = capture::inspect_full_window(
            "vault-coin-unknown-recovery-tip",
            coins_view(
                &unknown,
                &coins,
                Some(1008),
                &[0],
                &labels,
                &HashMap::new(),
                BitcoinDisplayUnit::Sats,
                false,
                CoinControl::default(),
            ),
            (1024, 768),
            "dark",
            theme::Theme::dark(),
        );
        assert!(unknown_view.contains("Recovery timing unavailable"));
        assert!(!unknown_view.contains("Recovery in"));
        assert!(!unknown_view.contains("First recovery path will be available in"));
        assert!(unknown_view.contains("Hide details"));
        assert!(!unknown_view.contains("Details ▾"));

        let mut known = Cache::default();
        known.daemon_cache.blockheight = 101;
        let known_view = capture::inspect_full_window(
            "vault-coin-known-recovery-tip",
            coins_view(
                &known,
                &coins,
                Some(1008),
                &[],
                &labels,
                &HashMap::new(),
                BitcoinDisplayUnit::Sats,
                false,
                CoinControl::default(),
            ),
            (1024, 768),
            "dark",
            theme::Theme::dark(),
        );
        assert!(known_view.contains("Recovery in ≈ 7 days"));
        assert!(known_view.contains("Details ▾"));
        assert!(!known_view.contains("Hide details"));
    }

    #[test]
    fn chain_badge_never_calls_a_missing_transaction_verified() {
        let mounted = capture::inspect_full_window(
            "vault-chain-status",
            chain_badge::<Message>(ChainStatus::NotSeen, ChainId::Bitcoin).into(),
            (400, 100),
            "dark",
            theme::Theme::dark(),
        );
        assert!(mounted.contains("Not seen on Blake2b · checking"));
        assert!(!mounted.contains("verified"));
        let both = capture::inspect_full_window(
            "vault-chain-status-both",
            chain_badge::<Message>(ChainStatus::BothChains, ChainId::BitcoinBlake2b).into(),
            (400, 100),
            "dark",
            theme::Theme::dark(),
        );
        assert!(both.contains("Both chains"));
    }

    #[test]
    fn blake2b_on_shows_chains_column_banner_and_selection() {
        let coins = [confirmed_coin(100)];
        let mut cache = Cache::default();
        cache.record_entanglement(
            coins[0].outpoint.txid,
            Entanglement::Entangled,
            std::time::Instant::now(),
        );
        let mounted = capture::inspect_full_window(
            "vault-coins-split-surfaces",
            coins_view(
                &cache,
                &coins,
                Some(1008),
                &[],
                &HashMap::new(),
                &HashMap::new(),
                BitcoinDisplayUnit::Sats,
                true,
                CoinControl {
                    chosen: vec![coins[0].outpoint].into_iter().collect(),
                    both_chains: Some((1, coins[0].amount, true)),
                    selection_bar: true,
                    label_saving: None,
                },
            ),
            (1280, 800),
            "light",
            theme::Theme::light(),
        );
        assert!(mounted.contains("Chains"));
        assert!(mounted.contains("Both chains"));
        assert!(mounted.contains("Split coins"));
        assert!(mounted.contains("also exists on Bitcoin Blake2b"));
        assert!(mounted.contains("1 selected · •••••• sats"));
        assert!(!mounted.contains("98,765,432"));
        assert!(mounted.contains("Send selected"));
    }

    #[test]
    fn recovery_status_copy_names_open_and_estimated_wait() {
        assert_eq!(recovery_status_copy(0), "Recovery open");
        assert_eq!(recovery_status_copy(1), "Recovery in 1 block");
        assert_eq!(recovery_status_copy(143), "Recovery in 143 blocks");
        assert_eq!(recovery_status_copy(144), "Recovery in ≈ 1 day");
        assert_eq!(recovery_status_copy(1008), "Recovery in ≈ 7 days");
    }

    #[test]
    fn row_copy_and_label_actions_work_without_expanding_details() {
        let coins = [confirmed_coin(100)];
        let cache = Cache::default();
        let outpoint = coins[0].outpoint.to_string();
        for pointer in [false, true] {
            for (action, expanded, expected, notice) in [
                (
                    "copy-Address-row",
                    false,
                    coins[0].address.to_string(),
                    "Address copied to clipboard",
                ),
                (
                    "copy-Address-details",
                    true,
                    coins[0].address.to_string(),
                    "Address copied to clipboard",
                ),
                (
                    "copy-Transaction ID-details",
                    true,
                    coins[0].outpoint.txid.to_string(),
                    "Transaction ID copied to clipboard",
                ),
                (
                    "copy-Outpoint-details",
                    true,
                    outpoint.clone(),
                    "Outpoint copied to clipboard",
                ),
            ] {
                let selected = if expanded { vec![0] } else { vec![] };
                let messages = capture::activate(
                    coins_view(
                        &cache,
                        &coins,
                        None,
                        &selected,
                        &HashMap::new(),
                        &HashMap::new(),
                        BitcoinDisplayUnit::Sats,
                        true,
                        CoinControl::default(),
                    ),
                    coin_action_id(&outpoint, action),
                    (1920, 1080),
                    pointer,
                );
                assert!(messages.iter().any(|message| matches!(message, Message::CopyWithToast(value, text) if value == &expected && text == notice)), "{:?}", messages);
                assert!(
                    !messages
                        .iter()
                        .any(|message| matches!(message, Message::Select(_))),
                    "copy must not expand the row"
                );
            }
            let messages = capture::activate(
                coins_view(
                    &cache,
                    &coins,
                    None,
                    &[],
                    &HashMap::new(),
                    &HashMap::new(),
                    BitcoinDisplayUnit::Sats,
                    false,
                    CoinControl::default(),
                ),
                coin_action_id(&outpoint, "label"),
                (1920, 1080),
                pointer,
            );
            assert!(messages.iter().any(|message| matches!(message, Message::CoinControl(CoinControlMessage::EditLabel(value)) if *value == coins[0].outpoint)));
            assert!(!messages
                .iter()
                .any(|message| matches!(message, Message::Select(_))));
        }
    }

    #[test]
    fn amount_privacy_masks_both_chain_banner_and_selection_total() {
        let coins = [confirmed_coin(100)];
        // Unresolved privacy must conceal amounts, just like explicitly hidden.
        for privacy in [
            None,
            Some(BalancePrivacy::Hidden),
            Some(BalancePrivacy::Shown),
        ] {
            let cache = Cache {
                balance_privacy: privacy.map(|privacy| BalancePrivacyChrome {
                    privacy,
                    busy: false,
                    interactive: false,
                }),
                ..Default::default()
            };
            let mounted = capture::inspect_full_window(
                "vault-coin-all-amounts-privacy",
                coins_view(
                    &cache,
                    &coins,
                    None,
                    &[],
                    &HashMap::new(),
                    &HashMap::new(),
                    BitcoinDisplayUnit::Sats,
                    true,
                    CoinControl {
                        chosen: std::iter::once(coins[0].outpoint).collect(),
                        both_chains: Some((1, coins[0].amount, true)),
                        selection_bar: true,
                        ..Default::default()
                    },
                ),
                (1280, 900),
                "dark",
                theme::Theme::dark(),
            );
            let shown = privacy == Some(BalancePrivacy::Shown);
            assert_eq!(mounted.contains("98,765,432"), shown);
            assert_eq!(mounted.contains("•••••• sats also exists"), !shown);
            assert_eq!(mounted.contains("1 selected · •••••• sats"), !shown);
            assert!(mounted.contains("Bitcoin Blake2b"));
        }
    }

    #[test]
    fn label_popover_keeps_rows_still_fits_window_and_handles_keyboard_and_dismissal() {
        for bytes in coincube_ui::font::load() {
            iced::advanced::graphics::text::font_system()
                .write()
                .unwrap()
                .load_font(bytes);
        }
        let mut second = confirmed_coin(101);
        second.outpoint.vout = 1;
        let coins = [confirmed_coin(100), second];
        let cache = Cache::default();
        let labels = HashMap::new();
        let outpoint = coins[0].outpoint.to_string();
        let editing = HashMap::from([(outpoint.clone(), form::Value::<String>::default())]);
        let closed_editing = HashMap::new();
        let page = |open| -> Element<'_, Message> {
            scrollable(coins_view(
                &cache,
                &coins,
                Some(1008),
                &[0],
                &labels,
                if open { &editing } else { &closed_editing },
                BitcoinDisplayUnit::Sats,
                true,
                CoinControl::default(),
            ))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
        };

        for (appearance, theme) in [
            ("dark", theme::Theme::dark()),
            ("light", theme::Theme::light()),
        ] {
            // 652px is the real content allocation in a minimum desktop window.
            for (width, height) in [(652, 668), (1280, 900)] {
                assert_label_popover_layout(
                    &page,
                    appearance,
                    &theme,
                    (width, height),
                    std::slice::from_ref(&outpoint),
                );
            }
        }
    }

    fn assert_label_popover_layout<'a>(
        page: &impl Fn(bool) -> Element<'a, Message>,
        appearance: &str,
        theme: &theme::Theme,
        dimensions: (u32, u32),
        label_items: &[String],
    ) {
        use iced::advanced::{
            layout, overlay,
            renderer::Headless,
            widget::{operation, operation::focusable, Id, Operation, Tree},
            Layout, Shell,
        };
        use iced::{keyboard, mouse, Event, Point, Rectangle, Size, Vector};
        #[derive(Default)]
        struct Probe {
            text: Vec<(String, Rectangle)>,
            horizontal_scroll: bool,
        }
        impl Operation for Probe {
            fn traverse(&mut self, visit: &mut dyn FnMut(&mut dyn Operation)) {
                visit(self);
            }
            fn text(&mut self, _: Option<&Id>, bounds: Rectangle, value: &str) {
                self.text.push((value.to_owned(), bounds));
            }
            fn scrollable(
                &mut self,
                _: Option<&Id>,
                bounds: Rectangle,
                content: Rectangle,
                _: Vector,
                _: &mut dyn operation::Scrollable,
            ) {
                self.horizontal_scroll |= content.width > bounds.width + 1.0;
            }
        }
        fn panel_bounds(node: &layout::Node, width: f32) -> Option<Rectangle> {
            if node.bounds().width < width {
                Some(node.bounds())
            } else {
                node.children()
                    .iter()
                    .filter_map(|child| panel_bounds(child, width))
                    .max_by(|a, b| a.height.total_cmp(&b.height))
            }
        }
        let (width, height) = dimensions;
        let size = Size::new(width as f32, height as f32);
        let viewport = Rectangle::with_size(size);
        let limits = layout::Limits::new(Size::ZERO, size);
        let mut renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
            coincube_ui::font::REGULAR,
            iced::Pixels(14.0),
            Some("tiny-skia"),
        ))
        .unwrap();
        let mut closed = page(false);
        let mut closed_tree = Tree::new(closed.as_widget());
        let closed_node = closed
            .as_widget_mut()
            .layout(&mut closed_tree, &renderer, &limits);
        let mut before = Probe::default();
        closed.as_widget_mut().operate(
            &mut closed_tree,
            Layout::new(&closed_node),
            &renderer,
            &mut before,
        );
        let mut element = page(true);
        let mut tree = Tree::new(element.as_widget());
        let node = element
            .as_widget_mut()
            .layout(&mut tree, &renderer, &limits);
        assert!(node.size().width <= size.width);
        let mut after = Probe::default();
        element
            .as_widget_mut()
            .operate(&mut tree, Layout::new(&node), &renderer, &mut after);
        assert_eq!(after.horizontal_scroll, width < TABLE_MIN_WIDTH as u32);
        for entry in &before.text {
            assert!(
                after.text.contains(entry),
                "label editor moved table content: {:?}",
                entry
            );
        }
        let header = after
            .text
            .iter()
            .find(|(text, _)| text == "Status")
            .unwrap()
            .1;
        let status = after
            .text
            .iter()
            .find(|(text, _)| text == "Recovery timing unavailable")
            .unwrap()
            .1;
        assert!(
            status.x >= header.x && status.x < header.x + STATUS_COLUMN_WIDTH,
            "status belongs in its column"
        );
        let mut focus: Box<dyn Operation<()>> =
            Box::new(focusable::focus(Id::from(COIN_LABEL_INPUT_ID)));
        element.as_widget_mut().operate(
            &mut tree,
            Layout::new(&node),
            &renderer,
            &mut operation::black_box(focus.as_mut()),
        );
        let mut popover = element
            .as_widget_mut()
            .overlay(
                &mut tree,
                Layout::new(&node),
                &renderer,
                &viewport,
                Vector::ZERO,
            )
            .map(overlay::Nested::new)
            .unwrap();
        let popup_node = popover.layout(&renderer, size);
        let panel = panel_bounds(&popup_node, size.width).unwrap();
        assert!(panel.width >= 300.0 && panel.width <= 320.0);
        assert!(panel.x >= 0.0 && panel.x + panel.width <= size.width);
        assert!(panel.y >= 0.0 && panel.y + panel.height <= size.height);
        for key in [keyboard::key::Named::Enter, keyboard::key::Named::Escape] {
            let mut messages = Vec::new();
            let event = Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(key),
                modified_key: keyboard::Key::Named(key),
                physical_key: keyboard::key::Physical::Unidentified(
                    keyboard::key::NativeCode::Unidentified,
                ),
                location: keyboard::Location::Standard,
                modifiers: keyboard::Modifiers::default(),
                text: None,
                repeat: false,
            });
            popover.update(
                &event,
                Layout::new(&popup_node),
                mouse::Cursor::Available(panel.center()),
                &renderer,
                &mut iced::advanced::clipboard::Null,
                &mut Shell::new(&mut messages),
            );
            assert!(messages.iter().any(|message| matches!(message, Message::Label(items, intent) if items == label_items && matches!((key, intent), (keyboard::key::Named::Enter, crate::app::view::LabelMessage::Confirm) | (keyboard::key::Named::Escape, crate::app::view::LabelMessage::Cancel)))));
        }
        let mut messages = Vec::new();
        let mut shell = Shell::new(&mut messages);
        popover.update(
            &Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            Layout::new(&popup_node),
            mouse::Cursor::Available(Point::new(2.0, 2.0)),
            &renderer,
            &mut iced::advanced::clipboard::Null,
            &mut shell,
        );
        assert!(shell.is_event_captured());
        assert!(matches!(
            messages.as_slice(),
            [Message::Label(_, crate::app::view::LabelMessage::Cancel)]
        ));
        drop(popover);
        if let Ok(directory) = std::env::var("R06A_CAPTURE_DIR") {
            std::fs::create_dir_all(&directory).unwrap();
            iced::advanced::Renderer::reset(&mut renderer, viewport);
            let style = iced::advanced::renderer::Style {
                text_color: theme.colors.text.primary,
            };
            element.as_widget().draw(
                &tree,
                &mut renderer,
                theme,
                &style,
                Layout::new(&node),
                mouse::Cursor::Unavailable,
                &viewport,
            );
            if let Some(mut popup) = element
                .as_widget_mut()
                .overlay(
                    &mut tree,
                    Layout::new(&node),
                    &renderer,
                    &viewport,
                    Vector::ZERO,
                )
                .map(overlay::Nested::new)
            {
                let popup_node = popup.layout(&renderer, size);
                popup.draw(
                    &mut renderer,
                    theme,
                    &style,
                    Layout::new(&popup_node),
                    mouse::Cursor::Unavailable,
                );
            }
            let pixels = renderer.screenshot(
                Size::new(width, height),
                1.0,
                theme.colors.general.background,
            );
            image::save_buffer(
                std::path::Path::new(&directory).join(format!(
                    "coin-label-popover-{appearance}-{width}x{height}-synthetic.png"
                )),
                &pixels,
                width,
                height,
                image::ColorType::Rgba8,
            )
            .unwrap();
        }
    }

    #[test]
    fn overview_recovery_duration_units_remain_available() {
        assert_eq!(expire_message_units(61), vec!["10 hours", "10 minutes"]);
        assert_eq!(expire_message_units(1112), vec!["7 days"]);
        assert_eq!(expire_message_units(52600), vec!["1 year"]);
    }
}
