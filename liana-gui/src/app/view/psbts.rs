use iced::{widget::Space, Alignment, Length};

use liana_ui::{
    component::{amount::*, badge, button, card, form, text::*},
    icon, theme,
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
                    form::Form::new_trimmed("PSBT", imported, move |msg| {
                        Message::ImportSpend(ImportSpendMessage::PsbtEdited(msg))
                    })
                    .warning("Please enter a base64 encoded PSBT")
                    .size(P1_SIZE)
                    .padding(10),
                )
                .push(Row::new().push(Space::with_width(Length::Fill)).push(
                    if imported.valid && !imported.value.is_empty() && !processing {
                        button::secondary(None, "Import")
                            .on_press(Message::ImportSpend(ImportSpendMessage::Confirm))
                    } else if processing {
                        button::secondary(None, "Processing...")
                    } else {
                        button::secondary(None, "Import")
                    },
                )),
        ))
        .max_width(400)
        .into()
}

pub fn import_psbt_success_view<'a>() -> Element<'a, Message> {
    Column::new()
        .push(
            card::simple(Container::new(
                text("PSBT is imported").style(theme::text::success),
            ))
            .padding(50),
        )
        .width(Length::Fixed(400.0))
        .align_x(Alignment::Center)
        .into()
}

pub fn psbts_view(spend_txs: &[SpendTx]) -> Element<'_, Message> {
    Column::new()
        .push(
            Row::new()
                .align_y(Alignment::Center)
                .spacing(10)
                .push(Container::new(h3("PSBTs")).width(Length::Fill))
                .push(
                    button::secondary(Some(icon::restore_icon()), "Import")
                        .on_press(Message::ImportPsbt),
                )
                .push(
                    button::secondary(Some(icon::plus_icon()), "New")
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
        .align_x(Alignment::Center)
        .spacing(25)
        .into()
}

fn spend_tx_list_view(i: usize, tx: &SpendTx) -> Element<'_, Message> {
    Container::new(
        Button::new(
            Row::new()
                .push(
                    Row::new()
                        .push(if tx.is_send_to_self() {
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
                                    .align_y(Alignment::Center)
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
                                        .style(theme::text::secondary),
                                    )
                                    .push(icon::key_icon().style(theme::text::secondary)),
                            )
                        })
                        .push_maybe(
                            tx.labels
                                .get(&tx.psbt.unsigned_tx.compute_txid().to_string())
                                .map(p1_regular),
                        )
                        .spacing(10)
                        .align_y(Alignment::Center)
                        .width(Length::Fill),
                )
                .push_maybe(if tx.is_batch() {
                    Some(badge::batch())
                } else {
                    None
                })
                .push_maybe(match tx.status {
                    SpendStatus::Deprecated => Some(badge::deprecated().width(120.0)),
                    SpendStatus::Broadcast => Some(badge::unconfirmed().width(120.0)),
                    SpendStatus::Spent => Some(badge::spent().width(120.0)),
                    _ => None,
                })
                .push(
                    Column::new()
                        .align_x(Alignment::End)
                        .push(if !tx.is_send_to_self() {
                            Container::new(amount(&tx.spend_amount))
                        } else {
                            Container::new(p1_regular("Self-transfer"))
                        })
                        .push_maybe(tx.fee_amount.map(|fee| amount_with_size(&fee, P2_SIZE)))
                        .width(Length::Fixed(140.0)),
                )
                .align_y(Alignment::Center)
                .spacing(20),
        )
        .padding(10)
        .on_press(Message::Select(i))
        .style(theme::button::transparent_border),
    )
    .style(theme::card::simple)
    .into()
}
