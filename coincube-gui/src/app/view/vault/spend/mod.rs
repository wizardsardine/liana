use std::collections::HashMap;
use std::time::Duration;

use iced::{
    alignment,
    widget::{checkbox, scrollable, tooltip, Column, Container, Row, Space},
    Alignment, Length,
};

use coincube_core::{
    descriptors::CoincubePolicy,
    miniscript::bitcoin::{bip32::Fingerprint, Amount, Denomination, Network},
};

use coincube_ui::{
    color,
    component::{amount::*, badge, button, form, spinner, text::*},
    icon::{self},
    theme,
    widget::*,
};

use crate::{
    app::{
        cache::Cache,
        error::Error,
        menu::Menu,
        view::{dashboard, message::*, vault::coins, vault::psbt, FiatAmountConverter},
        wallet::SyncStatus,
    },
    daemon::model::{remaining_sequence, Coin, SpendTx},
};

#[allow(clippy::too_many_arguments)]
pub fn spend_view<'a>(
    menu: &'a Menu,
    cache: &'a Cache,
    tx: &'a SpendTx,
    spend_warnings: &'a [String],
    saved: bool,
    desc_info: &'a CoincubePolicy,
    key_aliases: &'a HashMap<Fingerprint, String>,
    labels_editing: &'a HashMap<String, form::Value<String>>,
    network: Network,
    currently_signing: bool,
    warning: Option<&Error>,
) -> Element<'a, Message> {
    let is_recovery = tx
        .psbt
        .unsigned_tx
        .input
        .iter()
        .any(|txin| txin.sequence.is_relative_lock_time());
    dashboard(
        menu,
        cache,
        warning,
        Column::new()
            .spacing(20)
            .push(
                Container::new(h3(if is_recovery { "Recovery" } else { "Send" }))
                    .width(Length::Fill),
            )
            .push(psbt::spend_header(tx, labels_editing))
            .push_maybe(if spend_warnings.is_empty() || saved {
                None
            } else {
                Some(spend_warnings.iter().fold(
                    Column::new().padding(15).spacing(5),
                    |col, warning| {
                        col.push(
                            Row::new()
                                .spacing(5)
                                .push(icon::warning_icon().style(theme::text::warning))
                                .push(text(warning).style(theme::text::warning)),
                        )
                    },
                ))
            })
            .push(psbt::spend_overview_view(
                tx,
                desc_info,
                key_aliases,
                currently_signing,
            ))
            .push(
                Column::new()
                    .spacing(20)
                    .push(psbt::inputs_view(
                        &tx.coins,
                        &tx.psbt.unsigned_tx,
                        &tx.labels,
                        labels_editing,
                    ))
                    .push(psbt::outputs_view(
                        &tx.psbt.unsigned_tx,
                        network,
                        Some(tx.change_indexes.clone()),
                        &tx.labels,
                        labels_editing,
                        tx.is_single_payment().is_some(),
                    )),
            )
            .push(if saved {
                Row::new()
                    .push(
                        button::secondary(None, "Delete")
                            .width(Length::Fixed(200.0))
                            .on_press_maybe(if currently_signing {
                                None
                            } else {
                                Some(Message::Spend(SpendTxMessage::Delete))
                            }),
                    )
                    .width(Length::Fill)
            } else {
                Row::new()
                    .push(
                        button::secondary(None, "< Previous")
                            .width(Length::Fixed(150.0))
                            .on_press_maybe(if currently_signing {
                                None
                            } else {
                                Some(Message::Previous)
                            }),
                    )
                    .push(Space::new().width(Length::Fill))
                    .push(
                        button::secondary(None, "Save")
                            .width(Length::Fixed(150.0))
                            .on_press_maybe(if currently_signing {
                                None
                            } else {
                                Some(Message::Spend(SpendTxMessage::Save))
                            }),
                    )
                    .width(Length::Fill)
            }),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn create_spend_tx<'a>(
    balance: &'a Amount,
    unconfirmed_balance: &'a Amount,
    menu: &'a Menu,
    cache: &'a Cache,
    fiat_converter: Option<&FiatAmountConverter>,
    recipients: Vec<Element<'a, Message>>,
    is_valid: bool,
    duplicate: bool,
    timelock: u16,
    recovery_timelock: Option<u16>,
    coins: &[(Coin, bool)],
    coins_labels: &'a HashMap<String, String>,
    batch_label: &form::Value<String>,
    amount_left: Option<&Amount>,
    feerate: &form::Value<String>,
    fee_amount: Option<&Amount>,
    sync_status: &SyncStatus,
    error: Option<&Error>,
    is_first_step: bool,
    loading_fee_estimate: Option<usize>,
    bitcoin_unit: BitcoinDisplayUnit,
) -> Element<'a, Message> {
    let is_self_send = recipients.is_empty();
    let fiat_balance = fiat_converter.as_ref().map(|c| c.convert(*balance));
    let fiat_unconfirmed = fiat_converter.map(|c| c.convert(*unconfirmed_balance));
    dashboard(
        menu,
        cache,
        error,
        Column::new()
            .push(h3("Balance"))
            .push(
                Column::new()
                    .push(
                        if sync_status.is_synced() {
                            Column::new()
                                .spacing(5)
                                .push(amount_with_size_and_unit(balance, H1_SIZE, bitcoin_unit))
                                .push_maybe(
                                    fiat_balance.map(|fiat| {
                                        fiat.to_text().size(P2_SIZE).color(color::GREY_2)
                                    }),
                                )
                        } else {
                            Column::new().push(Row::new().push(spinner::Carousel::new(
                                Duration::from_millis(1000),
                                vec![
                                    amount_with_size_and_unit(balance, H1_SIZE, bitcoin_unit),
                                    amount_with_size_colors_and_unit(
                                        balance,
                                        H1_SIZE,
                                        color::GREY_4,
                                        Some(color::GREY_2),
                                        bitcoin_unit,
                                    ),
                                ],
                            )))
                        }
                        .wrap(),
                    )
                    .push_maybe(if !sync_status.is_synced() {
                        Some(
                            Row::new()
                                .push(
                                    match sync_status {
                                        SyncStatus::BlockchainSync(progress) => text(format!(
                                            "Syncing blockchain ({:.2}%)",
                                            100.0 * *progress
                                        )),
                                        SyncStatus::WalletFullScan => text("Syncing"),
                                        _ => text("Checking for new transactions"),
                                    }
                                    .style(theme::text::secondary),
                                )
                                .push(spinner::typing_text_carousel(
                                    "...",
                                    true,
                                    Duration::from_millis(2000),
                                    |content| text(content).style(theme::text::secondary),
                                )),
                        )
                    } else {
                        None
                    })
                    .push(
                        Column::new()
                            .spacing(10)
                            .push(h3(if recovery_timelock.is_some() {
                                "Recovery"
                            } else if is_self_send {
                                "Self-transfer"
                            } else {
                                "Send"
                            }))
                            .push_maybe(if recipients.len() > 1 {
                                Some(
                                    form::Form::new("Batch label", batch_label, |s| {
                                        Message::CreateSpend(CreateSpendMessage::BatchLabelEdited(
                                            s,
                                        ))
                                    })
                                    .warning("Invalid label length, cannot be superior to 100")
                                    .size(30)
                                    .padding(10),
                                )
                            } else {
                                None
                            })
                            .push_maybe(
                                if unconfirmed_balance.to_sat() != 0 && sync_status.is_synced() {
                                    Some(
                                        Row::new()
                                            .spacing(10)
                                            .align_y(Alignment::Center)
                                            .push(
                                                text("+")
                                                    .size(H3_SIZE)
                                                    .style(theme::text::secondary),
                                            )
                                            .push(unconfirmed_amount_with_size_and_unit(
                                                unconfirmed_balance,
                                                H3_SIZE,
                                                bitcoin_unit,
                                            ))
                                            .push(
                                                text("unconfirmed")
                                                    .size(H3_SIZE)
                                                    .style(theme::text::secondary),
                                            )
                                            .push_maybe(fiat_unconfirmed.map(|fiat| {
                                                Row::new()
                                                    .align_y(Alignment::Center)
                                                    .push(Space::new().width(10)) // total spacing = 20 including row spacing
                                                    .push(
                                                        fiat.to_text()
                                                            .size(H4_SIZE)
                                                            .color(color::GREY_3),
                                                    )
                                            }))
                                            .wrap(),
                                    )
                                } else {
                                    None
                                },
                            ),
                    ),
            )
            .push(
                Column::new()
                    .push(Column::with_children(recipients).spacing(10))
                    .push(
                        Row::new()
                            .push_maybe(if duplicate {
                                Some(
                                    Container::new(
                                        text("Two payment addresses are the same")
                                            .style(theme::text::warning),
                                    )
                                    .padding(10),
                                )
                            } else {
                                None
                            })
                            .push(Space::new().width(Length::Fill))
                            .push_maybe(if is_self_send || recovery_timelock.is_some() {
                                // Recipients cannot be added for self-send (zero recipients) and recovery (exactly one recipient).
                                None
                            } else {
                                Some(
                                    button::secondary(Some(icon::plus_icon()), "Add payment")
                                        .on_press(Message::CreateSpend(
                                            CreateSpendMessage::AddRecipient,
                                        )),
                                )
                            }),
                    )
                    .spacing(20),
            )
            .push(
                Row::new()
                    .spacing(10)
                    .align_y(Alignment::Center)
                    .push(Container::new(p1_bold("Feerate:")).padding(10))
                    .push(
                        button::secondary(None, "Fast (~10m)")
                            .width(Length::Fixed(130.0))
                            .on_press_maybe((!matches!(loading_fee_estimate, Some(1))).then(
                                || Message::CreateSpend(CreateSpendMessage::FetchFeeEstimate(1)),
                            )),
                    )
                    .push(
                        button::secondary(None, "Normal (~1h)")
                            .width(Length::Fixed(130.0))
                            .on_press_maybe((!matches!(loading_fee_estimate, Some(6))).then(
                                || Message::CreateSpend(CreateSpendMessage::FetchFeeEstimate(6)),
                            )),
                    )
                    .push(
                        button::secondary(None, "Slow (~4h)")
                            .width(Length::Fixed(130.0))
                            .on_press_maybe((!matches!(loading_fee_estimate, Some(24))).then(
                                || Message::CreateSpend(CreateSpendMessage::FetchFeeEstimate(24)),
                            )),
                    )
                    .push(
                        Container::new(
                            form::Form::new_trimmed("42 (in sats/vbyte)", feerate, move |msg| {
                                Message::CreateSpend(CreateSpendMessage::FeerateEdited(msg))
                            })
                            .warning(
                                "Feerate must be an integer less than or equal to 1000 sats/vbyte",
                            )
                            .size(P1_SIZE)
                            .padding(10),
                        )
                        .width(Length::Fixed(150.0)),
                    )
                    .push_maybe(fee_amount.map(|fee| {
                        Row::new()
                            .spacing(10)
                            .align_y(Alignment::Center)
                            .push(p1_regular("Fee:").style(theme::text::secondary))
                            .push(amount_with_size(fee, P1_SIZE))
                            .push_maybe(fiat_converter.map(|conv| {
                                Row::new().spacing(10).align_y(Alignment::Center).push(
                                    conv.convert(*fee)
                                        .to_text()
                                        .size(P2_SIZE)
                                        .style(theme::text::secondary),
                                )
                            }))
                    }))
                    .wrap(),
            )
            .push(
                Container::new(
                    Column::new()
                        .spacing(10)
                        .push(
                            Row::new()
                                .align_y(Alignment::Center)
                                .push(p1_bold("Coins selection").width(Length::Fill))
                                .push(if is_self_send || recovery_timelock.is_some() {
                                    Row::new()
                                        .spacing(5)
                                        .push(amount_with_size(
                                            &Amount::from_sat(
                                                coins
                                                    .iter()
                                                    .filter_map(|(coin, selected)| {
                                                        if *selected {
                                                            Some(coin.amount.to_sat())
                                                        } else {
                                                            None
                                                        }
                                                    })
                                                    .sum(),
                                            ),
                                            P2_SIZE,
                                        ))
                                        .push(p2_regular("selected").style(theme::text::secondary))
                                } else if let Some(amount_left) = amount_left {
                                    if amount_left.to_sat() == 0 && !is_valid {
                                        // If amount left is set, the current configuration must be redraftable.
                                        // If it's not valid, either no coins are selected or there's a recipient
                                        // with max selected and invalid amount.
                                        if coins.iter().all(|(_, selected)| !selected) {
                                            // This can happen if we have a single recipient
                                            // and it has the max selected.
                                            Row::new().push(
                                                text("Select at least one coin.")
                                                    .style(theme::text::secondary),
                                            )
                                        } else {
                                            // There must be a recipient with max selected and value 0.
                                            Row::new().push(
                                                text("Check max amount for recipient.")
                                                    .style(theme::text::secondary),
                                            )
                                        }
                                    } else {
                                        Row::new()
                                            .spacing(5)
                                            .push(amount_with_size(amount_left, P2_SIZE))
                                            .push(
                                                p2_regular("left to select")
                                                    .style(theme::text::secondary),
                                            )
                                    }
                                } else {
                                    Row::new().push(
                                        text(if feerate.value.is_empty() || !feerate.valid {
                                            "Feerate needs to be set."
                                        } else {
                                            "Add recipient details."
                                        })
                                        .style(theme::text::secondary),
                                    )
                                })
                                .width(Length::Fill),
                        )
                        .push(
                            Container::new(scrollable(coins.iter().enumerate().fold(
                                Column::new().spacing(10),
                                |col, (i, (coin, selected))| {
                                    col.push(coin_list_view(
                                        i,
                                        coin,
                                        coins_labels,
                                        timelock,
                                        cache.blockheight() as u32,
                                        *selected,
                                    ))
                                },
                            )))
                            .max_height(300),
                        ),
                )
                .padding(20)
                .style(theme::card::simple),
            )
            .push(
                Row::new()
                    .spacing(20)
                    .align_y(Alignment::Center)
                    .push_maybe(
                        (!is_first_step).then_some(
                            button::secondary(None, "< Previous")
                                .width(Length::Fixed(150.0))
                                .on_press(Message::Previous),
                        ),
                    )
                    .push(Space::new().width(Length::Fill))
                    .push(
                        button::secondary(None, "Clear")
                            .on_press(Message::CreateSpend(CreateSpendMessage::Clear))
                            .width(Length::Fixed(100.0)),
                    )
                    .push(
                        if is_valid
                            && !duplicate
                            && error.is_none()
                            && (is_self_send
                                || recovery_timelock.is_some()
                                || Some(&Amount::from_sat(0)) == amount_left)
                        {
                            button::primary(None, "Next")
                                .on_press(Message::CreateSpend(CreateSpendMessage::Generate))
                                .width(Length::Fixed(100.0))
                        } else {
                            button::secondary(None, "Next").width(Length::Fixed(100.0))
                        },
                    ),
            )
            .push(Space::new().height(Length::Fixed(20.0)))
            .spacing(20),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn recipient_view<'a>(
    index: usize,
    address: &'a form::Value<String>,
    amount: &'a form::Value<String>,
    fiat_form_value: Option<&'a form::Value<String>>,
    fiat_converter: Option<&FiatAmountConverter>,
    label: &'a form::Value<String>,
    is_max_selected: bool,
    is_recovery: bool,
) -> Element<'a, CreateSpendMessage> {
    let btc_amt = Amount::from_str_in(&amount.value, Denomination::Bitcoin).ok();

    Container::new(
        Column::new()
            .spacing(10)
            .push_maybe(
                // Recipient for recovery cannot be deleted.
                (!is_recovery).then_some(
                    Row::new().push(Space::new().width(Length::Fill)).push(
                        Button::new(icon::cross_icon())
                            .style(theme::button::transparent)
                            .on_press(CreateSpendMessage::DeleteRecipient(index))
                            .width(Length::Shrink),
                    ),
                ),
            )
            .push(
                Row::new()
                    .align_y(Alignment::Start)
                    .spacing(10)
                    .push(
                        Container::new(p1_bold("Address"))
                            .align_x(alignment::Horizontal::Right)
                            .padding(10)
                            .width(Length::Fixed(130.0)),
                    )
                    .push(
                        form::Form::new_trimmed("Address", address, move |msg| {
                            CreateSpendMessage::RecipientEdited(index, "address", msg)
                        })
                        .warning("Invalid address (maybe it is for another network?)")
                        .size(P1_SIZE)
                        .padding(10),
                    ),
            )
            .push(
                Row::new()
                    .align_y(Alignment::Start)
                    .spacing(10)
                    .push(
                        Container::new(p1_bold("Description"))
                            .align_x(alignment::Horizontal::Right)
                            .padding(10)
                            .width(Length::Fixed(130.0)),
                    )
                    .push(
                        form::Form::new("Payment label", label, move |msg| {
                            CreateSpendMessage::RecipientEdited(index, "label", msg)
                        })
                        .warning("Label length is too long (> 100 char)")
                        .size(P1_SIZE)
                        .padding(10),
                    ),
            )
            .push(
                Row::new()
                    .align_y(Alignment::Center)
                    .spacing(10)
                    .push(
                        Container::new(p1_bold("Amount (BTC)"))
                            .padding(10)
                            .align_x(alignment::Horizontal::Right)
                            .width(Length::Fixed(130.0)),
                    )
                    .push(
                        Row::new()
                            .align_y(Alignment::Center)
                            .spacing(5)
                            .push(if is_max_selected {
                                let amount_txt = btc_amt
                                    .map(|a| a.to_formatted_string())
                                    .unwrap_or(amount.value.clone());
                                Container::new(
                                    text(amount_txt).size(P1_SIZE).style(theme::text::secondary),
                                )
                                .width(Length::Fill)
                            } else {
                                form::Form::new_amount_btc("0.001 (in BTC)", amount, move |msg| {
                                    CreateSpendMessage::RecipientEdited(index, "amount", msg)
                                })
                                .warning(
                                    "Invalid amount. (Note amounts lower than 0.00005 BTC are invalid.)",
                                )
                                .size(P1_SIZE)
                                .padding(10)
                                .into_container()
                            })
                            .push_maybe(fiat_converter.map(|conv| {
                                Row::new()
                                    .align_y(Alignment::Center)
                                    .spacing(5)
                                    .push(Space::new().width(Length::Fixed(20.0))) // add some space between BTC and fiat amounts
                                    .push(p1_bold(format!("~{}", conv.currency())))
                                    .push(Space::new().width(Length::Fixed(5.0)))
                                    .push(if is_max_selected {
                                        let fiat_from_btc = btc_amt
                                            .map(|a| conv.convert(a))
                                            .map(|fa| fa.to_formatted_string())
                                            .unwrap_or_default();
                                        Container::new(
                                            text(fiat_from_btc)
                                                .size(P1_SIZE)
                                                .style(theme::text::secondary),
                                        )
                                        .width(Length::Fill)
                                    } else {
                                        let conv = *conv;
                                        // The particular form shown depends on whether the user has entered a fiat amount or
                                        // if we are instead converting the BTC amount.
                                        let fiat_form = if let Some(val) = fiat_form_value {
                                            val
                                        } else if let Some(btc_amt) = btc_amt {
                                            let fa = conv.convert(btc_amt);
                                            &form::Value {
                                                value: fa.to_rounded_string(), // required decimal places for currency
                                                warning: None,
                                                valid: true,
                                            }
                                        } else {
                                            &form::Value::default()
                                        };
                                        form::Form::new_trimmed(
                                            &format!("Enter amount in {}", conv.currency()),
                                            fiat_form,
                                            move |msg| {
                                                CreateSpendMessage::RecipientFiatAmountEdited(
                                                    index, msg, conv,
                                                )
                                            },
                                        )
                                        .size(P1_SIZE)
                                        .padding(10)
                                        .into_container()
                                    })
                                    .push(tooltip::Tooltip::new(
                                        icon::tooltip_icon(),
                                        conv.to_container_summary(),
                                        tooltip::Position::Bottom,
                                    ))
                                    .push(Space::new().width(Length::Fixed(10.0)))
                            })),
                    )
                    .push_maybe(
                        // The MAX option cannot be edited for recovery recipients.
                        (!is_recovery).then_some(tooltip::Tooltip::new(
                            checkbox(is_max_selected).label("MAX")
                                .on_toggle(move |_| CreateSpendMessage::SendMaxToRecipient(index)),
                            // Add spaces at end so that text is padded at screen edge.
                            "Total amount remaining after paying fee and any other recipients     ",
                            tooltip::Position::Bottom,
                        )),
                    )
                    .width(Length::Fill),
            ),
    )
    .padding(20)
    .style(theme::card::simple)
    .into()
}

fn coin_list_view<'a>(
    i: usize,
    coin: &Coin,
    coins_labels: &'a HashMap<String, String>,
    timelock: u16,
    blockheight: u32,
    selected: bool,
) -> Element<'a, Message> {
    Row::new()
        .push(
            Row::new()
                .push(
                    checkbox(selected).on_toggle(move |_| {
                        Message::CreateSpend(CreateSpendMessage::SelectCoin(i))
                    }),
                )
                .push(
                    if let Some(label) = coins_labels.get(&coin.outpoint.to_string()) {
                        Container::new(p1_regular(label)).width(Length::Fill)
                    } else if let Some(label) = coins_labels.get(&coin.outpoint.txid.to_string()) {
                        Container::new(
                            Row::new()
                                .spacing(5)
                                .push(
                                    // It is not possible to know if a coin is a
                                    // change coin or not so for now, From is
                                    // enough
                                    p1_regular("From").style(theme::text::secondary),
                                )
                                .push(p1_regular(label)),
                        )
                        .width(Length::Fill)
                    } else {
                        Container::new(p1_regular("")).width(Length::Fill)
                    },
                )
                .push(if coin.spend_info.is_some() {
                    badge::spent()
                } else if coin.block_height.is_none() {
                    badge::unconfirmed()
                } else {
                    let seq = remaining_sequence(coin, blockheight, timelock);
                    coins::coin_sequence_label(seq, timelock as u32)
                })
                .spacing(10)
                .align_y(Alignment::Center)
                .width(Length::Fill),
        )
        .push(amount(&coin.amount))
        .align_y(Alignment::Center)
        .spacing(20)
        .into()
}
