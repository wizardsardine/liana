use chrono::{DateTime, Local, Utc};
use std::{collections::HashMap, vec};

use iced::{
    widget::{Container, Row, Space},
    Alignment, Length,
};

use liana::miniscript::bitcoin;
use liana_ui::{
    component::{
        amount::*,
        button, card, form,
        home::{self, rescan_warning, SyncProgress},
        payment::{self, payment_card, PaymentKind, UIPayment},
        text::{legacy, Text},
    },
    icon, theme,
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

fn recovery_hint<'a>(sequence: u32) -> Element<'a, Message> {
    home::recovery_hint(coins::expire_message_units(sequence).join(", "))
}

fn recovery_warning<'a>(expiring_coins: &[bitcoin::OutPoint]) -> Element<'a, Message> {
    home::recovery_warning(
        expiring_coins.len(),
        Message::Menu(Menu::RefreshCoins(expiring_coins.to_owned())),
    )
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
    let sync = (!sync_status.is_synced()).then_some(match sync_status {
        SyncStatus::BlockchainSync(progress) => SyncProgress::Blockchain(*progress),
        SyncStatus::WalletFullScan => SyncProgress::FullScan,
        _ => SyncProgress::Transactions,
    });
    let balance = Column::new()
        .push(home::balance(
            balance,
            fiat_balance.map(|fiat| fiat.to_display_string()),
            sync.is_some(),
        ))
        .push_maybe(sync.map(home::syncing))
        .push_maybe(
            (unconfirmed_balance.to_sat() != 0 && sync_status.is_synced()).then(|| {
                home::unconfirmed_balance(
                    unconfirmed_balance,
                    fiat_unconfirmed.map(|fiat| fiat.to_display_string()),
                )
            }),
        );

    let expire_warning = if expiring_coins.is_empty() {
        remaining_sequence.map(recovery_hint)
    } else {
        Some(recovery_warning(expiring_coins))
    };

    let history = events.iter().fold(Column::new().spacing(10), |col, event| {
        if event.kind != PaymentKind::SendToSelf {
            col.push(payment_card(
                UIPayment {
                    label: event.label.as_deref().or(event.address_label.as_deref()),
                    kind: event.kind,
                    time: event.time,
                    amount: event.amount,
                    fiat_price: None,
                },
                Some(Message::SelectPayment(event.outpoint)),
            ))
        } else {
            col
        }
    });

    let see_more =
        (!is_last_page && !events.is_empty()).then(|| payment::see_more(processing, Message::Next));
    Column::new()
        .push(legacy::panel_title("Balance"))
        .push(balance)
        .push_maybe(show_rescan_warning.then(|| {
            rescan_warning(
                Message::Menu(Menu::SettingsPreSelected(menu::SettingsOption::Node)),
                Message::HideRescanWarning,
            )
        }))
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
