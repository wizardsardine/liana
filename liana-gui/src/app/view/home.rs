use chrono::{DateTime, Local, Utc};
use std::{collections::HashMap, time::Duration, vec};

use iced::{
    alignment,
    widget::{Container, Row, Space},
    Alignment::{self, Center},
    Length,
};

use liana::miniscript::bitcoin;
use liana_ui::{
    color,
    component::{
        amount::*,
        button,
        card::{self, home_hint, home_warning},
        form,
        payment::{payment_card, PaymentKind, UIPayment},
        spinner,
        text::{legacy, Text},
    },
    font::MANROPE_MEDIUM,
    icon::{self, cross_icon, ICON_SIZE_M},
    theme,
    widget::*,
};

use crate::{
    app::{
        cache::Cache,
        error::Error,
        menu::{self, Menu},
        view::{coins, dashboard, label, message::Message, FiatAmountConverter},
        wallet::SyncStatus,
    },
    daemon::model::{HistoryTransaction, Payment, TransactionKind},
};

const RESCAN_WARNING: &str = "As this wallet was restored from a backup, you may need to rescan the blockchain to see past transactions.";

fn rescan_warning<'a>() -> Element<'a, Message> {
    Container::new(
        Column::new()
            .spacing(10)
            .push(
                Row::new()
                    .spacing(5)
                    .push(icon::warning_icon().style(theme::text::warning))
                    .push(legacy::text(RESCAN_WARNING).style(theme::text::warning))
                    .align_y(Center),
            )
            .push(
                Row::new()
                    .spacing(5)
                    .push(Space::with_width(Length::Fill))
                    .push(
                        button::secondary(None, "Go to rescan").on_press(Message::Menu(
                            Menu::SettingsPreSelected(menu::SettingsOption::Node),
                        )),
                    )
                    .push(
                        button::secondary(Some(cross_icon()), "Dismiss")
                            .on_press(Message::HideRescanWarning),
                    ),
            ),
    )
    .padding(25)
    .style(theme::card::border)
    .into()
}

#[allow(clippy::too_many_arguments)]
pub fn home_view<'a>(
    balance: &'a bitcoin::Amount,
    unconfirmed_balance: &'a bitcoin::Amount,
    remaining_sequence: &Option<u32>,
    fiat_converter: Option<FiatAmountConverter>,
    expiring_coins: &[bitcoin::OutPoint],
    events: &'a [Payment],
    is_last_page: bool,
    processing: bool,
    sync_status: &SyncStatus,
    show_rescan_warning: bool,
) -> Element<'a, Message> {
    let fiat_balance = fiat_converter.as_ref().map(|c| c.convert(*balance));
    let fiat_unconfirmed = fiat_converter.map(|c| c.convert(*unconfirmed_balance));
    let balance = Column::new()
        .push(
            if sync_status.is_synced() {
                Row::new()
                    .align_y(Alignment::Center)
                    .push(amount_with_font(balance, legacy::H1_SPEC))
                    .push_maybe(fiat_balance.map(|fiat| {
                        Row::new()
                            .align_y(Alignment::Center)
                            .push(Space::with_width(20))
                            .push(
                                fiat.to_text()
                                    .font(MANROPE_MEDIUM)
                                    .size(legacy::H2_SIZE)
                                    .color(color::GREY_2),
                            )
                    }))
            } else {
                Row::new().push(spinner::Carousel::new(
                    Duration::from_millis(1000),
                    vec![
                        amount_with_font(balance, legacy::H1_SPEC),
                        amount_with_font_blink(balance, legacy::H1_SPEC),
                    ],
                ))
            }
            .wrap(),
        )
        .push_maybe(if !sync_status.is_synced() {
            Some(
                Row::new()
                    .push(
                        match sync_status {
                            SyncStatus::BlockchainSync(progress) => legacy::text(format!(
                                "Syncing blockchain ({:.2}%)",
                                100.0 * *progress
                            )),
                            SyncStatus::WalletFullScan => legacy::text("Syncing"),
                            _ => legacy::text("Checking for new transactions"),
                        }
                        .style(theme::text::secondary),
                    )
                    .push(spinner::typing_text_carousel(
                        "...",
                        true,
                        Duration::from_millis(2000),
                        |content| legacy::text(content).style(theme::text::secondary),
                    )),
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
                            legacy::text("+")
                                .size(legacy::H3_SIZE)
                                .style(theme::text::secondary),
                        )
                        .push(unconfirmed_amount_with_size(
                            unconfirmed_balance,
                            legacy::H3_SIZE,
                        ))
                        .push(
                            legacy::text("+")
                                .size(legacy::H3_SIZE)
                                .style(theme::text::secondary),
                        )
                        .push(unconfirmed_amount_with_size(
                            unconfirmed_balance,
                            legacy::H3_SIZE,
                        ))
                        .push(
                            legacy::text("unconfirmed")
                                .size(legacy::H3_SIZE)
                                .style(theme::text::secondary),
                        )
                        .push_maybe(fiat_unconfirmed.map(|fiat| {
                            Row::new()
                                .align_y(Alignment::Center)
                                .push(Space::with_width(10)) // total spacing = 20 including row spacing
                                .push(fiat.to_text().size(legacy::H4_SIZE).color(color::GREY_3))
                        }))
                        .wrap(),
                )
            } else {
                None
            },
        );

    let expire_warning = if expiring_coins.is_empty() {
        remaining_sequence.map(|sequence| {
            let content = Row::new()
                .spacing(15)
                .align_y(Alignment::Center)
                .push(
                    legacy::h4_regular(format!(
                        "≈ {} left before first recovery path becomes available.",
                        coins::expire_message_units(sequence).join(", ")
                    ))
                    .width(Length::Fill),
                )
                .push(
                    icon::tooltip_icon()
                        .size(20)
                        .style(theme::text::secondary)
                        .width(Length::Fixed(20.0)),
                )
                .width(Length::Fill);
            home_hint(content)
        })
    } else {
        let content = Row::new()
            .push(icon::warning_fill_icon().size(ICON_SIZE_M as u32))
            .push(
                legacy::h4_regular(format!(
                    "Recovery path is or will soon be available for {} coin(s).",
                    expiring_coins.len(),
                ))
                .width(Length::Fill),
            )
            .push(
                button::primary(Some(icon::arrow_repeat()), "Reset timelock")
                    .on_press(Message::Menu(Menu::RefreshCoins(expiring_coins.to_owned()))),
            )
            .spacing(15)
            .align_y(Alignment::Center);
        Some(home_warning(content))
    };

    let history = events.iter().fold(Column::new().spacing(10), |col, event| {
        if event.kind != PaymentKind::SendToSelf {
            col.push(event_list_view(event))
        } else {
            col
        }
    });

    let see_more = if !is_last_page && !events.is_empty() {
        Some(
            Container::new(
                Button::new(
                    legacy::text(if processing {
                        "Fetching ..."
                    } else {
                        "See more"
                    })
                    .width(Length::Fill)
                    .align_x(alignment::Horizontal::Center),
                )
                .width(Length::Fill)
                .padding(15)
                .style(theme::button::transparent_border)
                .on_press_maybe(if !processing {
                    Some(Message::Next)
                } else {
                    None
                }),
            )
            .width(Length::Fill)
            .style(theme::card::simple),
        )
    } else {
        None
    };
    Column::new()
        .push(legacy::panel_title("Balance"))
        .push(balance)
        .push_maybe(show_rescan_warning.then_some(rescan_warning()))
        .push_maybe(expire_warning)
        .push(
            Column::new()
                .spacing(10)
                .push(legacy::panel_title("Payments History"))
                .push(history)
                .push_maybe(see_more),
        )
        .spacing(20)
        .into()
}

fn event_list_view(event: &Payment) -> Element<'_, Message> {
    payment_card(
        UIPayment {
            label: event.label.as_deref().or(event.address_label.as_deref()),
            kind: event.kind,
            time: event.time,
            amount: event.amount,
            fiat_price: None,
        },
        Some(Message::SelectPayment(event.outpoint)),
    )
}

pub fn payment_view<'a>(
    cache: &'a Cache,
    tx: &'a HistoryTransaction,
    output_index: usize,
    labels_editing: &'a HashMap<String, form::Value<String>>,
    warning: Option<&'a Error>,
) -> Element<'a, Message> {
    let txid = tx.tx.compute_txid().to_string();
    let outpoint = bitcoin::OutPoint {
        txid: tx.tx.compute_txid(),
        vout: output_index as u32,
    }
    .to_string();
    dashboard(
        &Menu::Home,
        cache,
        warning,
        Column::new()
            .push(match tx.kind {
                TransactionKind::OutgoingSinglePayment(_)
                | TransactionKind::OutgoingPaymentBatch(_) => {
                    Container::new(legacy::h3("Outgoing payment")).width(Length::Fill)
                }
                TransactionKind::IncomingSinglePayment(_)
                | TransactionKind::IncomingPaymentBatch(_) => {
                    Container::new(legacy::h3("Incoming payment")).width(Length::Fill)
                }
                _ => Container::new(legacy::h3("Payment")).width(Length::Fill),
            })
            .push(if tx.is_single_payment().is_some() {
                // if the payment is a payment of a single payment transaction then
                // the label of the transaction is attached to the label of the payment outpoint
                if let Some(label) = labels_editing.get(&outpoint) {
                    label::label_editing(
                        vec![outpoint.clone(), txid.clone()],
                        label,
                        legacy::H3_SIZE,
                    )
                } else {
                    label::label_editable(
                        vec![outpoint.clone(), txid.clone()],
                        tx.labels.get(&outpoint),
                        legacy::H3_SIZE,
                    )
                }
            } else if let Some(label) = labels_editing.get(&outpoint) {
                label::label_editing(vec![outpoint.clone()], label, legacy::H3_SIZE)
            } else {
                label::label_editable(
                    vec![outpoint.clone()],
                    tx.labels.get(&outpoint),
                    legacy::H3_SIZE,
                )
            })
            .push(Container::new(amount_with_font(
                &tx.tx.output[output_index].value,
                legacy::H3_SPEC,
            )))
            .push(Space::with_height(legacy::H3_SIZE))
            .push(Container::new(legacy::h3("Transaction")).width(Length::Fill))
            .push_maybe(if tx.is_batch() {
                if let Some(label) = labels_editing.get(&txid) {
                    Some(label::label_editing(
                        vec![txid.clone()],
                        label,
                        legacy::H3_SIZE,
                    ))
                } else {
                    Some(label::label_editable(
                        vec![txid.clone()],
                        tx.labels.get(&txid),
                        legacy::H3_SIZE,
                    ))
                }
            } else {
                None
            })
            .push_maybe(tx.fee_amount.map(|fee_amount| {
                Row::new()
                    .align_y(Alignment::Center)
                    .push(legacy::h3("Miner fee: ").style(theme::text::secondary))
                    .push(amount_with_font(&fee_amount, legacy::H3_SPEC))
                    .push(legacy::text(" ").size(legacy::H3_SIZE))
                    .push(
                        legacy::text(format!(
                            "({} sats/vbyte)",
                            fee_amount.to_sat() / tx.tx.vsize() as u64
                        ))
                        .size(legacy::H4_SIZE)
                        .style(theme::text::secondary),
                    )
            }))
            .push(card::simple(
                Column::new()
                    .push_maybe(tx.time.map(|t| {
                        let date = DateTime::<Utc>::from_timestamp(t as i64, 0)
                            .unwrap()
                            .with_timezone(&Local)
                            .format("%b. %d, %Y - %T");
                        Row::new()
                            .width(Length::Fill)
                            .push(Container::new(legacy::text("Date:").bold()).width(Length::Fill))
                            .push(
                                Container::new(legacy::text(format!("{date}")))
                                    .width(Length::Shrink),
                            )
                    }))
                    .push(
                        Row::new()
                            .width(Length::Fill)
                            .align_y(Alignment::Center)
                            .push(Container::new(legacy::text("Txid:").bold()).width(Length::Fill))
                            .push(
                                Row::new()
                                    .align_y(Alignment::Center)
                                    .push(Container::new(
                                        legacy::text(format!("{}", tx.tx.compute_txid())).small(),
                                    ))
                                    .push(
                                        Button::new(icon::clipboard_icon())
                                            .on_press(Message::Clipboard(
                                                tx.tx.compute_txid().to_string(),
                                            ))
                                            .style(theme::button::transparent_border),
                                    )
                                    .width(Length::Shrink),
                            ),
                    )
                    .spacing(5),
            ))
            .push(
                button::secondary(None, "See transaction details").on_press(Message::Menu(
                    Menu::TransactionPreSelected(tx.tx.compute_txid()),
                )),
            )
            .spacing(20),
    )
}
