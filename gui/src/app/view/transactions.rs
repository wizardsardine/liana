use chrono::NaiveDateTime;

use iced::{alignment, Alignment, Length};

use liana_ui::{
    color,
    component::{amount::*, badge, card, text::*},
    icon, theme,
    util::Collection,
    widget::*,
};

use crate::{
    app::{
        cache::Cache,
        error::Error,
        menu::Menu,
        view::{dashboard, message::Message},
    },
    daemon::model::HistoryTransaction,
};

pub const HISTORY_EVENT_PAGE_SIZE: u64 = 20;

pub fn transactions_view<'a>(
    cache: &'a Cache,
    pending_txs: &[HistoryTransaction],
    txs: &Vec<HistoryTransaction>,
    warning: Option<&'a Error>,
) -> Element<'a, Message> {
    dashboard(
        &Menu::Transactions,
        cache,
        warning,
        Column::new()
            .push(Container::new(h3("Transactions")).width(Length::Fill))
            .push(
                Column::new()
                    .spacing(10)
                    .push_maybe(if !pending_txs.is_empty() {
                        Some(
                            pending_txs
                                .iter()
                                .enumerate()
                                .fold(Column::new().spacing(10), |col, (i, tx)| {
                                    col.push(tx_list_view(i, tx))
                                }),
                        )
                    } else {
                        None
                    })
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
            .spacing(30),
    )
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
                                    NaiveDateTime::from_timestamp_opt(t as i64, 0)
                                        .unwrap()
                                        .format("%b. %d, %Y - %T"),
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
                } else if tx.outgoing_amount != Amount::from_sat(0) {
                    Row::new()
                        .spacing(5)
                        .push(text("-"))
                        .push(amount(&tx.outgoing_amount))
                        .align_items(Alignment::Center)
                } else {
                    Row::new().push(text("Self send"))
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

pub fn tx_view<'a>(
    cache: &'a Cache,
    tx: &'a HistoryTransaction,
    warning: Option<&'a Error>,
) -> Element<'a, Message> {
    dashboard(
        &Menu::Transactions,
        cache,
        warning,
        Column::new()
            .push(if tx.is_self_send() {
                Container::new(h3("Transaction")).width(Length::Fill)
            } else if tx.is_external() {
                Container::new(h3("Incoming transaction")).width(Length::Fill)
            } else {
                Container::new(h3("Outgoing transaction")).width(Length::Fill)
            })
            .push(
                Column::new().spacing(20).push(
                    Column::new()
                        .push(if tx.is_self_send() {
                            Container::new(h1("Self send"))
                        } else if tx.is_external() {
                            Container::new(amount_with_size(&tx.incoming_amount, H1_SIZE))
                        } else {
                            Container::new(amount_with_size(&tx.outgoing_amount, H1_SIZE))
                        })
                        .push_maybe(tx.fee_amount.map(|fee_amount| {
                            Row::new()
                                .align_items(Alignment::Center)
                                .push(h3("Miner fee: ").style(color::GREY_3))
                                .push(amount_with_size(&fee_amount, H3_SIZE))
                        })),
                ),
            )
            .push(card::simple(
                Column::new()
                    .push_maybe(tx.time.map(|t| {
                        let date = NaiveDateTime::from_timestamp_opt(t as i64, 0)
                            .unwrap()
                            .format("%b. %d, %Y - %T");
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
            .push(super::psbt::inputs_and_outputs_view(
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
            .spacing(20),
    )
    .into()
}
