pub mod detail;
pub mod step;

use iced::{
    widget::{Button, Column, Container, Row, Space},
    Alignment, Element, Length,
};

use crate::{
    app::{error::Error, menu::Menu, view::util::*},
    daemon::model::{SpendStatus, SpendTx},
    ui::{
        color,
        component::{badge, button, card, form, text::*},
        icon,
        util::Collection,
    },
};

use super::{message::*, warning::warn};

pub fn import_spend_view<'a>(
    imported: &form::Value<String>,
    error: Option<&Error>,
    processing: bool,
) -> Element<'a, Message> {
    Column::new()
        .push(warn(error))
        .push(card::simple(
            Column::new()
                .spacing(10)
                .push(text("Insert PSBT:").bold())
                .push(
                    form::Form::new("PSBT", imported, move |msg| {
                        Message::ImportSpend(ImportSpendMessage::PsbtEdited(msg))
                    })
                    .warning("Please enter a base64 encoded PSBT")
                    .size(20)
                    .padding(10),
                )
                .push(Row::new().push(Space::with_width(Length::Fill)).push(
                    if imported.valid && !imported.value.is_empty() && !processing {
                        button::primary(None, "Import")
                            .on_press(Message::ImportSpend(ImportSpendMessage::Confirm))
                    } else if processing {
                        button::primary(None, "Processing...")
                    } else {
                        button::primary(None, "Import")
                    },
                )),
        ))
        .max_width(400)
        .into()
}

pub fn import_spend_success_view<'a>() -> Element<'a, Message> {
    Column::new()
        .push(
            card::simple(Container::new(
                text("PSBT is imported").style(color::SUCCESS),
            ))
            .padding(50),
        )
        .width(Length::Units(400))
        .align_items(Alignment::Center)
        .into()
}

pub fn spend_view<'a>(spend_txs: &[SpendTx]) -> Element<'a, Message> {
    Column::new()
        .push(
            Row::new()
                .spacing(10)
                .push(Column::new().width(Length::Fill))
                .push(
                    button::border(Some(icon::import_icon()), "Import")
                        .on_press(Message::ImportSpend(ImportSpendMessage::Import)),
                )
                .push(
                    button::primary(Some(icon::plus_icon()), "New")
                        .on_press(Message::Menu(Menu::CreateSpendTx)),
                ),
        )
        .push(
            Container::new(
                Row::new()
                    .push(text(format!(" {}", spend_txs.len())).bold())
                    .push(text(" draft transactions")),
            )
            .width(Length::Fill),
        )
        .push(
            Column::new().spacing(10).push(
                spend_txs
                    .iter()
                    .enumerate()
                    .fold(Column::new().spacing(10), |col, (i, tx)| {
                        col.push(spend_tx_list_view(i, tx))
                    }),
            ),
        )
        .align_items(Alignment::Center)
        .spacing(20)
        .into()
}

fn spend_tx_list_view<'a>(i: usize, tx: &SpendTx) -> Element<'a, Message> {
    Container::new(
        Button::new(
            Row::new()
                .push(
                    Row::new()
                        .push(badge::spend())
                        .push(if let Some(sigs) = tx.sigs.recovery_path() {
                            Row::new()
                                .spacing(10)
                                .align_items(Alignment::Center)
                                .push(
                                    Row::new()
                                        .spacing(5)
                                        .align_items(Alignment::Center)
                                        .push(text(format!(
                                            "{}/{}",
                                            if sigs.sigs_count <= sigs.threshold {
                                                sigs.sigs_count
                                            } else {
                                                sigs.threshold
                                            },
                                            sigs.threshold
                                        )))
                                        .push(icon::key_icon()),
                                )
                                .push(
                                    Container::new(text(" Recovery ").small())
                                        .padding(3)
                                        .style(badge::PillStyle::Simple),
                                )
                        } else {
                            let sigs = tx.sigs.primary_path();
                            Row::new()
                                .spacing(5)
                                .align_items(Alignment::Center)
                                .push(text(format!(
                                    "{}/{}",
                                    if sigs.sigs_count <= sigs.threshold {
                                        sigs.sigs_count
                                    } else {
                                        sigs.threshold
                                    },
                                    sigs.threshold
                                )))
                                .push(icon::key_icon())
                        })
                        .spacing(10)
                        .align_items(Alignment::Center)
                        .width(Length::Fill),
                )
                .push_maybe(match tx.status {
                    SpendStatus::Deprecated => Some(badge::deprecated()),
                    SpendStatus::Broadcast => Some(badge::unconfirmed()),
                    SpendStatus::Spent => Some(badge::spent()),
                    _ => None,
                })
                .push(
                    Column::new()
                        .push(amount(&tx.spend_amount))
                        .push(text(format!("fee: {:8}", tx.fee_amount.to_btc())).small())
                        .width(Length::Shrink),
                )
                .align_items(Alignment::Center)
                .spacing(20),
        )
        .padding(10)
        .on_press(Message::Select(i))
        .style(button::Style::TransparentBorder.into()),
    )
    .style(card::SimpleCardStyle)
    .into()
}
