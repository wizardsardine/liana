use iced::{widget::Space, Alignment, Length};

use liana_ui::{
    color,
    component::{amount::*, badge, button, card, form, text::*},
    icon, theme,
    util::Collection,
    widget::*,
};

use crate::{
    app::{error::Error, menu::Menu},
    daemon::model::{SpendStatus, SpendTx},
};

use super::{message::*, warning::warn};

pub fn import_psbt_view<'a>(
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

pub fn import_psbt_success_view<'a>() -> Element<'a, Message> {
    Column::new()
        .push(
            card::simple(Container::new(text("PSBT is imported").style(color::GREEN))).padding(50),
        )
        .width(Length::Units(400))
        .align_items(Alignment::Center)
        .into()
}

pub fn psbts_view<'a>(spend_txs: &[SpendTx]) -> Element<'a, Message> {
    Column::new()
        .push(
            Row::new()
                .align_items(Alignment::Center)
                .spacing(10)
                .push(Container::new(h3("PSBTs")).width(Length::Fill))
                .push(
                    button::secondary(Some(icon::import_icon()), "Import")
                        .on_press(Message::ImportSpend(ImportSpendMessage::Import)),
                )
                .push(
                    button::primary(Some(icon::plus_icon()), "New")
                        .on_press(Message::Menu(Menu::CreateSpendTx)),
                ),
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
        .spacing(25)
        .into()
}

fn spend_tx_list_view<'a>(i: usize, tx: &SpendTx) -> Element<'a, Message> {
    Container::new(
        Button::new(
            Row::new()
                .push(
                    Row::new()
                        .push(badge::spend())
                        .push(if !tx.sigs.recovery_paths().is_empty() {
                            Row::new().push(
                                Container::new(p2_regular(" Recovery "))
                                    .padding(10)
                                    .style(theme::Container::Pill(theme::Pill::Simple)),
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
        .style(theme::Button::TransparentBorder),
    )
    .style(theme::Container::Card(theme::Card::Simple))
    .into()
}
