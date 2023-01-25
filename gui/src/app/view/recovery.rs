use iced::{
    widget::{Button, Column, Container, Row, Space},
    Alignment, Element, Length,
};

use liana::miniscript::bitcoin::{util::psbt::Psbt, Amount};

use crate::{
    app::view::{
        hw::hw_list_view,
        message::{CreateSpendMessage, Message},
    },
    hw::HardwareWallet,
    ui::{
        component::{button, card, form, text::*},
        icon,
        util::Collection,
    },
};

#[allow(clippy::too_many_arguments)]
pub fn recovery<'a>(
    locked_coins: &(usize, Amount),
    recoverable_coins: &(usize, Amount),
    feerate: &form::Value<String>,
    address: &'a form::Value<String>,
    generated: Option<&Psbt>,
    hws: &[HardwareWallet],
    chosen_hw: Option<usize>,
    done: bool,
) -> Element<'a, Message> {
    Column::new()
        .push(Space::with_height(Length::Units(100)))
        .push(
            Row::new()
                .push(Container::new(
                    icon::recovery_icon().width(Length::Units(100)).size(50),
                ))
                .push(text("Recover the funds").size(50).bold())
                .align_items(Alignment::Center)
                .spacing(1),
        )
        .push(
            Container::new(Row::new().push(text(format!(
                "{} ({} coins) will be spendable through the recovery path in the next block",
                recoverable_coins.1, recoverable_coins.0
            ))))
            .center_x(),
        )
        .push_maybe(if *locked_coins != (0, Amount::from_sat(0)) {
            Some(
                Container::new(Row::new().push(text(format!(
                    "{} ({} coins) are not yet spendable through the recovery path",
                    locked_coins.1, locked_coins.0
                ))))
                .center_x(),
            )
        } else {
            None
        })
        .push(Space::with_height(Length::Units(20)))
        .push(if let Some(psbt) = generated {
            if done {
                Column::new()
                    .spacing(20)
                    .align_items(Alignment::Center)
                    .push(text("Funds were sweeped"))
                    .push(card::simple(
                        Column::new()
                            .push(
                                Row::new()
                                    .spacing(5)
                                    .align_items(Alignment::Center)
                                    .push(
                                        text(format!(
                                            "{}",
                                            Amount::from_sat(psbt.unsigned_tx.output[0].value)
                                        ))
                                        .small()
                                        .bold(),
                                    )
                                    .push(text(" to ").small())
                                    .push(text(&address.value).small().bold()),
                            )
                            .push(
                                Row::new()
                                    .spacing(5)
                                    .align_items(Alignment::Center)
                                    .push(
                                        text(format!("Txid: {}", psbt.unsigned_tx.txid())).small(),
                                    )
                                    .push(
                                        Button::new(icon::clipboard_icon().small())
                                            .on_press(Message::Clipboard(
                                                psbt.unsigned_tx.txid().to_string(),
                                            ))
                                            .style(button::Style::Border.into()),
                                    ),
                            )
                            .push_maybe(
                                if recoverable_coins.1.to_sat() > psbt.unsigned_tx.output[0].value {
                                    Some(
                                        Row::new().push(
                                            text(format!(
                                                "Fees: {}",
                                                recoverable_coins.1
                                                    - Amount::from_sat(
                                                        psbt.unsigned_tx.output[0].value
                                                    )
                                            ))
                                            .small(),
                                        ),
                                    )
                                } else {
                                    None
                                },
                            ),
                    ))
            } else {
                Column::new()
                    .spacing(20)
                    .align_items(Alignment::Center)
                    .push_maybe(if chosen_hw.is_none() {
                        Some(button::border(None, "< Previous").on_press(Message::Previous))
                    } else {
                        None
                    })
                    .push(text("Sign the transaction to sweep the funds").bold())
                    .push(card::simple(
                        Column::new()
                            .push(
                                Row::new()
                                    .spacing(5)
                                    .align_items(Alignment::Center)
                                    .push(
                                        text(format!(
                                            "{}",
                                            Amount::from_sat(psbt.unsigned_tx.output[0].value)
                                        ))
                                        .small()
                                        .bold(),
                                    )
                                    .push(text(" to ").small())
                                    .push(text(&address.value).small().bold()),
                            )
                            .push(
                                Row::new()
                                    .spacing(5)
                                    .align_items(Alignment::Center)
                                    .push(
                                        text(format!("Txid: {}", psbt.unsigned_tx.txid())).small(),
                                    )
                                    .push(
                                        Button::new(icon::clipboard_icon().small())
                                            .on_press(Message::Clipboard(
                                                psbt.unsigned_tx.txid().to_string(),
                                            ))
                                            .style(button::Style::Border.into()),
                                    ),
                            )
                            .push_maybe(
                                if recoverable_coins.1.to_sat() > psbt.unsigned_tx.output[0].value {
                                    Some(
                                        Row::new().push(
                                            text(format!(
                                                "Fees: {}",
                                                recoverable_coins.1
                                                    - Amount::from_sat(
                                                        psbt.unsigned_tx.output[0].value
                                                    )
                                            ))
                                            .small(),
                                        ),
                                    )
                                } else {
                                    None
                                },
                            ),
                    ))
                    .push(if !hws.is_empty() {
                        Column::new()
                            .push(
                                Row::new()
                                    .align_items(Alignment::Center)
                                    .push(
                                        text("Select hardware wallet to sign with:")
                                            .bold()
                                            .width(Length::Fill),
                                    )
                                    .push_maybe(if chosen_hw.is_none() {
                                        Some(
                                            button::border(None, "Refresh")
                                                .on_press(Message::Reload),
                                        )
                                    } else {
                                        None
                                    }),
                            )
                            .spacing(10)
                            .push(hws.iter().enumerate().fold(
                                Column::new().spacing(10),
                                |col, (i, hw)| {
                                    col.push(hw_list_view(
                                        i,
                                        hw,
                                        Some(i) == chosen_hw,
                                        chosen_hw.is_some(),
                                        false,
                                    ))
                                },
                            ))
                            .max_width(500)
                    } else {
                        Column::new()
                            .push(
                                Column::new()
                                    .spacing(20)
                                    .width(Length::Fill)
                                    .push("Please connect a hardware wallet")
                                    .push(
                                        button::primary(None, "Refresh").on_press(Message::Reload),
                                    )
                                    .align_items(Alignment::Center),
                            )
                            .width(Length::Fill)
                    })
            }
        } else {
            Column::new()
                .push(text("Enter destination address and feerate:").bold())
                .push(
                    Container::new(
                        form::Form::new("Address", address, move |msg| {
                            Message::CreateSpend(CreateSpendMessage::RecipientEdited(
                                0, "address", msg,
                            ))
                        })
                        .warning("Please enter correct bitcoin address")
                        .size(20)
                        .padding(10),
                    )
                    .width(Length::Units(250)),
                )
                .push(
                    Container::new(
                        form::Form::new("Feerate (sat/vbyte)", feerate, move |msg| {
                            Message::CreateSpend(CreateSpendMessage::FeerateEdited(msg))
                        })
                        .warning("Please enter correct feerate (sat/vbyte)")
                        .size(20)
                        .padding(10),
                    )
                    .width(Length::Units(250)),
                )
                .push(
                    if feerate.valid
                        && !feerate.value.is_empty()
                        && address.valid
                        && !address.value.is_empty()
                        && recoverable_coins.0 != 0
                    {
                        button::primary(None, "Next")
                            .on_press(Message::Next)
                            .width(Length::Units(200))
                    } else {
                        button::primary(None, "Next").width(Length::Units(200))
                    },
                )
                .spacing(20)
                .align_items(Alignment::Center)
        })
        .align_items(Alignment::Center)
        .spacing(20)
        .into()
}
