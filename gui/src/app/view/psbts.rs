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
        .width(Length::Fixed(400.0))
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
                        .push(if tx.is_self_send() {
                            badge::cycle()
                        } else {
                            badge::spend()
                        })
                        .push(if !tx.sigs.recovery_paths().is_empty() {
                            badge::recovery()
                        } else {
                            let sigs = tx.sigs.primary_path();
                            Container::new(
                                Row::new()
                                    .spacing(5)
                                    .align_items(Alignment::Center)
                                    .push(
                                        p2_regular(format!(
                                            "{}/{}",
                                            if sigs.sigs_count <= sigs.threshold {
                                                sigs.sigs_count
                                            } else {
                                                sigs.threshold
                                            },
                                            sigs.threshold
                                        ))
                                        .style(color::GREY_3),
                                    )
                                    .push(icon::key_icon().style(color::GREY_3)),
                            )
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
                        .align_items(Alignment::End)
                        .push(if !tx.is_self_send() {
                            Container::new(amount(&tx.spend_amount))
                        } else {
                            Container::new(p1_regular("Self-transfer"))
                        })
                        .push(amount_with_size(&tx.fee_amount, P2_SIZE))
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
