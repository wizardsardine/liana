use chrono::NaiveDateTime;

use iced::{alignment, Alignment, Length};

use liana_ui::{
    component::{amount::*, badge, card, text::*},
    icon, theme,
    util::Collection,
    widget::*,
};

use crate::{
    app::{cache::Cache, view::message::Message},
    daemon::model::HistoryTransaction,
};

pub const HISTORY_EVENT_PAGE_SIZE: u64 = 20;

pub fn transactions_view<'a>(
    pending_txs: &[HistoryTransaction],
    txs: &Vec<HistoryTransaction>,
) -> Element<'a, Message> {
    Column::new()
        .push(Container::new(h3("Transactions")).width(Length::Fill))
        .push(
            Column::new()
                .spacing(10)
                .push(
                    pending_txs
                        .iter()
                        .enumerate()
                        .fold(Column::new().spacing(10), |col, (i, tx)| {
                            col.push(tx_list_view(i, tx))
                        }),
                )
                .push(
                    txs.iter()
                        .enumerate()
                        .fold(Column::new().spacing(10), |col, (i, tx)| {
                            col.push(tx_list_view(i + pending_txs.len(), tx))
                        }),
                )
                .push_maybe(
                    if txs.len() % HISTORY_EVENT_PAGE_SIZE as usize == 0 && !txs.is_empty() {
                        Some(
                            Container::new(
                                Button::new(
                                    text("See more")
                                        .width(Length::Fill)
                                        .horizontal_alignment(alignment::Horizontal::Center),
                                )
                                .width(Length::Fill)
                                .padding(15)
                                .style(theme::Button::TransparentBorder)
                                .on_press(Message::Next),
                            )
                            .width(Length::Fill)
                            .style(theme::Container::Card(theme::Card::Simple)),
                        )
                    } else {
                        None
                    },
                ),
        )
        .align_items(Alignment::Center)
        .spacing(20)
        .into()
}

fn tx_list_view<'a>(i: usize, tx: &HistoryTransaction) -> Element<'a, Message> {
    Container::new(
        Button::new(
            Row::new()
                .push(
                    Row::new()
                        .push(if tx.is_external() {
                            badge::receive()
                        } else {
                            badge::spend()
                        })
                        .push(if let Some(t) = tx.time {
                            Container::new(
                                text(format!(
                                    "{}",
                                    NaiveDateTime::from_timestamp_opt(t as i64, 0).unwrap(),
                                ))
                                .small(),
                            )
                        } else {
                            badge::unconfirmed()
                        })
                        .spacing(10)
                        .align_items(Alignment::Center)
                        .width(Length::Fill),
                )
                .push(if tx.is_external() {
                    Row::new()
                        .spacing(5)
                        .push(text("+"))
                        .push(amount(&tx.incoming_amount))
                        .align_items(Alignment::Center)
                } else {
                    Row::new()
                        .spacing(5)
                        .push(text("-"))
                        .push(amount(&tx.outgoing_amount))
                        .align_items(Alignment::Center)
                })
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

pub fn tx_view<'a>(cache: &Cache, tx: &'a HistoryTransaction) -> Element<'a, Message> {
    Column::new()
        .push(
            Row::new()
                .push(if tx.is_external() {
                    badge::receive()
                } else {
                    badge::spend()
                })
                .spacing(10)
                .align_items(Alignment::Center),
        )
        .push(if tx.is_external() {
            amount_with_size(&tx.incoming_amount, 50)
        } else {
            amount_with_size(&tx.outgoing_amount, 50)
        })
        .push_maybe(
            tx.fee_amount
                .map(|fee| Row::new().push(text("Miner Fee: ")).push(amount(&fee))),
        )
        .push(card::simple(
            Column::new()
                .push_maybe(tx.time.map(|t| {
                    let date = NaiveDateTime::from_timestamp_opt(t as i64, 0).unwrap();
                    Row::new()
                        .width(Length::Fill)
                        .push(Container::new(text("Date:").bold()).width(Length::Fill))
                        .push(Container::new(text(format!("{}", date))).width(Length::Shrink))
                }))
                .push(
                    Row::new()
                        .width(Length::Fill)
                        .align_items(Alignment::Center)
                        .push(Container::new(text("Txid:").bold()).width(Length::Fill))
                        .push(
                            Row::new()
                                .align_items(Alignment::Center)
                                .push(Container::new(text(format!("{}", tx.tx.txid())).small()))
                                .push(
                                    Button::new(icon::clipboard_icon())
                                        .on_press(Message::Clipboard(tx.tx.txid().to_string()))
                                        .style(theme::Button::TransparentBorder),
                                )
                                .width(Length::Shrink),
                        ),
                )
                .spacing(5),
        ))
        .push(super::spend::detail::inputs_and_outputs_view(
            &tx.coins,
            &tx.tx,
            cache.network,
            if tx.is_external() {
                None
            } else {
                Some(tx.change_indexes.clone())
            },
            if tx.is_external() {
                Some(tx.change_indexes.clone())
            } else {
                None
            },
        ))
        .align_items(Alignment::Center)
        .spacing(20)
        .max_width(800)
        .into()
}
