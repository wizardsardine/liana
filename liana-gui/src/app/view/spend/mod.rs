use std::collections::HashMap;

use iced::{
    alignment,
    widget::{checkbox, scrollable, tooltip, Column, Container, Row, Space},
    Alignment, Length,
};

use liana::{
    descriptors::LianaPolicy,
    miniscript::bitcoin::{bip32::Fingerprint, Amount, Denomination, Network},
};

use liana_ui::{
    component::{amount::*, badge, button, form, text::*},
    icon, theme,
    widget::*,
};

use crate::{
    app::{
        cache::Cache,
        error::Error,
        menu::Menu,
        view::{coins, dashboard, message::*, psbt, FiatAmountConverter},
    },
    daemon::model::{remaining_sequence, Coin, SpendTx},
};

#[allow(clippy::too_many_arguments)]
pub fn spend_view<'a>(
    cache: &'a Cache,
    tx: &'a SpendTx,
    spend_warnings: &'a [String],
    saved: bool,
    desc_info: &'a LianaPolicy,
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

    let title =
        Container::new(h3(if is_recovery { "Recovery" } else { "Send" })).width(Length::Fill);

    let warnings = (!(spend_warnings.is_empty() || saved)).then_some(spend_warnings.iter().fold(
        Column::new().padding(15).spacing(5),
        |col, warning| {
            col.push(
                Row::new()
                    .spacing(5)
                    .push(icon::warning_icon().style(theme::text::warning))
                    .push(text(warning).style(theme::text::warning)),
            )
        },
    ));

    let spend_overview = psbt::spend_overview_view(tx, desc_info, key_aliases, currently_signing);

    let inputs_outputs = Column::new()
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
            &tx.change_indexes,
            &tx.labels,
            labels_editing,
            tx.is_single_payment().is_some(),
            false,
        ));

    let bottom_row = if saved {
        Row::new()
            .push(button::secondary(None, "Delete").width(200).on_press_maybe(
                (!currently_signing).then_some(Message::Spend(SpendTxMessage::Delete)),
            ))
            .width(Length::Fill)
    } else {
        Row::new()
            .push(
                button::secondary(None, "< Previous")
                    .width(150)
                    .on_press_maybe((!currently_signing).then_some(Message::Previous)),
            )
            .push(Space::with_width(Length::Fill))
            .push(button::secondary(None, "Save").width(150).on_press_maybe(
                (!currently_signing).then_some(Message::Spend(SpendTxMessage::Save)),
            ))
            .width(Length::Fill)
    };

    let content = Column::new()
        .spacing(20)
        .push(title)
        .push(psbt::spend_header(tx, labels_editing))
        .push_maybe(warnings)
        .push(spend_overview)
        .push(inputs_outputs)
        .push(bottom_row);

    dashboard(
        if is_recovery {
            &Menu::Recovery
        } else {
            &Menu::CreateSpendTx
        },
        cache,
        warning,
        content,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn create_spend_tx<'a>(
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
    error: Option<&Error>,
    is_first_step: bool,
) -> Element<'a, Message> {
    let is_self_send = recipients.is_empty();

    // Title
    let title = h3(if recovery_timelock.is_some() {
        "Recovery"
    } else if is_self_send {
        "Self-transfer"
    } else {
        "Send"
    });

    // Optional batch label
    let batch_label = (recipients.len() > 1).then_some(
        form::Form::new("Batch label", batch_label, |s| {
            Message::CreateSpend(CreateSpendMessage::BatchLabelEdited(s))
        })
        .warning("Invalid label length, cannot be superior to 100")
        .size(30)
        .padding(10),
    );

    // Recipients
    let recipients_cards = Column::new()
        .push(Column::with_children(recipients).spacing(10))
        .spacing(20);

    // Add payment row
    let duplicates_warning = duplicate.then_some(
        Container::new(text("Two payment addresses are the same").style(theme::text::warning))
            .padding(10),
    );
    let add_payment_btn = (!(is_self_send || recovery_timelock.is_some())).then_some(
        button::secondary(Some(icon::plus_icon()), "Add payment")
            .on_press(Message::CreateSpend(CreateSpendMessage::AddRecipient)),
    );
    let add_payment_row = Row::new()
        .push_maybe(duplicates_warning)
        .push(Space::with_width(Length::Fill))
        .push_maybe(add_payment_btn);

    // Fee-rate row
    let fee_input = Container::new(
        form::Form::new_trimmed("42 (in sats/vbyte)", feerate, move |msg| {
            Message::CreateSpend(CreateSpendMessage::FeerateEdited(msg))
        })
        .warning("Feerate must be an integer less than or equal to 1000 sats/vbyte")
        .size(P1_SIZE)
        .padding(10),
    )
    .width(150);
    let fee_amount = fee_amount.map(|fee| {
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
    });
    let fee_rate_row = Row::new()
        .spacing(10)
        .align_y(Alignment::Center)
        .push(Container::new(p1_bold("Feerate:")).padding(10))
        .push(fee_input)
        .push_maybe(fee_amount)
        .wrap();

    // Coin selection
    let selected_amount = (is_self_send || recovery_timelock.is_some()).then_some(
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
            .push(p2_regular("selected").style(theme::text::secondary)),
    );
    let hint = selected_amount
        .is_none()
        .then_some(if let Some(amount_left) = amount_left {
            if amount_left.to_sat() == 0 && !is_valid {
                // If amount left is set, the current configuration must be redraftable.
                // If it's not valid, either no coins are selected or there's a recipient
                // with max selected and invalid amount.
                if coins.iter().all(|(_, selected)| !selected) {
                    // This can happen if we have a single recipient
                    // and it has the max selected.
                    Row::new().push(text("Select at least one coin.").style(theme::text::secondary))
                } else {
                    // There must be a recipient with max selected and value 0.
                    Row::new()
                        .push(text("Check max amount for recipient.").style(theme::text::secondary))
                }
            } else {
                Row::new()
                    .spacing(5)
                    .push(amount_with_size(amount_left, P2_SIZE))
                    .push(p2_regular("left to select").style(theme::text::secondary))
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
        });
    let coin_selection_header = Row::new()
        .align_y(Alignment::Center)
        .push(p1_bold("Coins selection").width(Length::Fill))
        .push_maybe(selected_amount)
        .push_maybe(hint)
        .width(Length::Fill);
    let coins = Container::new(scrollable(coins.iter().enumerate().fold(
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
    .max_height(300);
    let coin_selection = Container::new(
        Column::new()
            .push(coin_selection_header)
            .push(coins)
            .spacing(10),
    )
    .padding(20)
    .style(theme::card::simple);

    // Bottom row
    let previous = (!is_first_step).then_some(
        button::secondary(None, "< Previous")
            .width(Length::Fixed(150.0))
            .on_press(Message::Previous),
    );
    let clear = button::secondary(None, "Clear")
        .on_press(Message::CreateSpend(CreateSpendMessage::Clear))
        .width(Length::Fixed(100.0));
    let next = if is_valid
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
    };
    let bottom_row = Row::new()
        .spacing(20)
        .align_y(Alignment::Center)
        .push_maybe(previous)
        .push(Space::with_width(Length::Fill))
        .push(clear)
        .push(next);
    let content = Column::new()
        .push(title)
        .push_maybe(batch_label)
        .push(recipients_cards)
        .push(add_payment_row)
        .push(fee_rate_row)
        .push(coin_selection)
        .push(bottom_row)
        .push(Space::with_height(Length::Fixed(20.0)))
        .spacing(20);

    dashboard(
        if recovery_timelock.is_some() {
            &Menu::Recovery
        } else {
            &Menu::CreateSpendTx
        },
        cache,
        error,
        content,
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

    // Recipient for recovery cannot be deleted.
    let header = (!is_recovery).then_some(
        Row::new().push(Space::with_width(Length::Fill)).push(
            Button::new(icon::cross_icon())
                .style(theme::button::transparent)
                .on_press(CreateSpendMessage::DeleteRecipient(index))
                .width(Length::Shrink),
        ),
    );

    let address_row = Row::new()
        .align_y(Alignment::Start)
        .spacing(10)
        .push(
            Container::new(p1_bold("Address"))
                .align_x(alignment::Horizontal::Right)
                .padding(10)
                .width(130),
        )
        .push(
            form::Form::new_trimmed("Address", address, move |msg| {
                CreateSpendMessage::RecipientEdited(index, "address", msg)
            })
            .warning("Invalid address (maybe it is for another network?)")
            .size(P1_SIZE)
            .padding(10),
        );

    let label_row = Row::new()
        .align_y(Alignment::Start)
        .spacing(10)
        .push(
            Container::new(p1_bold("Description"))
                .align_x(alignment::Horizontal::Right)
                .padding(10)
                .width(130),
        )
        .push(
            form::Form::new("Payment label", label, move |msg| {
                CreateSpendMessage::RecipientEdited(index, "label", msg)
            })
            .warning("Label length is too long (> 100 char)")
            .size(P1_SIZE)
            .padding(10),
        );

    // Amount row
    let btc_label = Container::new(p1_bold("Amount (BTC)"))
        .padding(10)
        .align_x(alignment::Horizontal::Right)
        .width(130);
    let btc_input = if is_max_selected {
        let amount_txt = btc_amt
            .map(|a| a.to_formatted_string())
            .unwrap_or(amount.value.clone());
        Container::new(text(amount_txt).size(P1_SIZE).style(theme::text::secondary))
            .width(Length::Fill)
    } else {
        form::Form::new_amount_btc("0.001 (in BTC)", amount, move |msg| {
            CreateSpendMessage::RecipientEdited(index, "amount", msg)
        })
        .warning("Invalid amount. (Note amounts lower than 0.000005 BTC are invalid.)")
        .size(P1_SIZE)
        .padding(10)
        .into_container()
    };

    let fiat_price = fiat_converter.map(|conv| {
        Row::new()
            .align_y(Alignment::Center)
            .spacing(5)
            .push(Space::with_width(20)) // add some space between BTC and fiat amounts
            .push_maybe(
                (!is_max_selected || btc_amt.is_some())
                    .then_some(p1_bold(format!("~{}", conv.currency()))),
            )
            .push(Space::with_width(5))
            .push_maybe(if is_max_selected {
                // fiat is processed from btc
                btc_amt.map(|a| {
                    let a = conv.convert(a).to_formatted_string();
                    Container::new(text(a).size(P1_SIZE).style(theme::text::secondary))
                        .width(Length::Fill)
                })
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
                (!is_max_selected || btc_amt.is_some()).then_some(
                    form::Form::new_trimmed(
                        &format!("Enter amount in {}", conv.currency()),
                        fiat_form,
                        move |msg| CreateSpendMessage::RecipientFiatAmountEdited(index, msg, conv),
                    )
                    .size(P1_SIZE)
                    .padding(10)
                    .into_container(),
                )
            })
            .push_maybe(
                (!is_max_selected || btc_amt.is_some()).then_some(tooltip::Tooltip::new(
                    icon::tooltip_icon(),
                    conv.to_container_summary(),
                    tooltip::Position::Bottom,
                )),
            )
            .push(Space::with_width(10))
    });

    // The MAX option cannot be edited for recovery recipients.
    let max = (!is_recovery).then_some(tooltip::Tooltip::new(
        checkbox("MAX", is_max_selected)
            .on_toggle(move |_| CreateSpendMessage::SendMaxToRecipient(index)),
        // Add spaces at end so that text is padded at screen edge.
        "Total amount remaining after paying fee and any other recipients     ",
        tooltip::Position::Bottom,
    ));

    let amount_row = Row::new()
        .align_y(Alignment::Center)
        .spacing(10)
        .push(btc_label)
        .push(btc_input)
        .push_maybe(fiat_price)
        .push_maybe(max)
        .width(Length::Fill);

    Container::new(
        Column::new()
            .push_maybe(header)
            .push(address_row)
            .push(label_row)
            .push(amount_row)
            .spacing(10),
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
                    checkbox("", selected).on_toggle(move |_| {
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
        // give some space for the scroll bar without using padding
        .push(Space::with_width(Length::Fixed(0.0)))
        .align_y(Alignment::Center)
        .spacing(20)
        .into()
}
