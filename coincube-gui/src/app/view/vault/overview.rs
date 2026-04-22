use chrono::{DateTime, Local, Utc};
use std::{collections::HashMap, time::Duration, vec};

use iced::{
    alignment,
    widget::{Container, Row, Space},
    Alignment::{self, Center},
    Length,
};

use coincube_core::miniscript::bitcoin;
use coincube_ui::{
    color,
    component::{
        amount::*,
        button, card, form, spinner,
        text::*,
        transaction::{TransactionBadge, TransactionDirection, TransactionListItem},
    },
    icon::{self, cross_icon},
    theme,
    widget::{Button, Column, ColumnExt, Element},
};

use crate::{
    app::{
        cache::Cache,
        menu::{self, Menu, VaultSubMenu},
        settings::display::DisplayMode,
        view::{
            balance_header_card, dashboard,
            message::Message,
            vault::coins,
            vault::label,
            wallet_header::{
                wallet_header, HeaderVariant, SyncState, UnconfirmedBalance, WalletHeaderProps,
            },
            FiatAmountConverter,
        },
        wallet::SyncStatus,
    },
    daemon::model::{HistoryTransaction, Payment, PaymentKind, TransactionKind},
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
                    .push(text(RESCAN_WARNING).style(theme::text::warning))
                    .align_y(Center),
            )
            .push(
                Row::new()
                    .spacing(5)
                    .push(Space::new().width(Length::Fill))
                    .push(
                        button::secondary(None, "Go to rescan").on_press(Message::Menu(
                            Menu::Vault(menu::VaultSubMenu::Settings(Some(
                                menu::SettingsOption::Node,
                            ))),
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
pub fn vault_overview_view<'a>(
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
    bitcoin_unit: BitcoinDisplayUnit,
    node_bitcoind_sync_progress: Option<f64>,
    node_bitcoind_ibd: Option<bool>,
    show_direction_badges: bool,
    display_mode: DisplayMode,
) -> Element<'a, Message> {
    let fiat_balance = fiat_converter.as_ref().map(|c| c.convert(*balance));
    let fiat_unconfirmed = fiat_converter
        .as_ref()
        .map(|c| c.convert(*unconfirmed_balance));
    let sync = match sync_status {
        SyncStatus::Synced => SyncState::Synced,
        SyncStatus::BlockchainSync(progress) => SyncState::Syncing {
            progress: Some(*progress),
            label: "Syncing blockchain".to_string(),
        },
        SyncStatus::WalletFullScan => SyncState::Syncing {
            progress: None,
            label: "Syncing".to_string(),
        },
        SyncStatus::LatestWalletSync => SyncState::Checking,
    };
    let unconfirmed = (unconfirmed_balance.to_sat() != 0).then_some(UnconfirmedBalance {
        amount: *unconfirmed_balance,
        fiat: fiat_unconfirmed,
    });
    let btc_fiat_str = fiat_balance
        .as_ref()
        .map(|f| format!("{} {}", f.to_rounded_string(), f.currency()))
        .unwrap_or_default();
    let vault_btc_row = Row::new()
        .spacing(10)
        .align_y(Alignment::Center)
        .push(coincube_ui::image::asset_network_logo::<Message>(
            "btc", "bitcoin", 40.0,
        ))
        .push(text("BTC").size(P1_SIZE).bold().width(Length::Fixed(60.0)))
        .push(amount_with_size_and_unit(balance, P1_SIZE, bitcoin_unit))
        .push(
            text(btc_fiat_str)
                .size(P2_SIZE)
                .style(theme::text::secondary)
                .width(Length::Fill),
        )
        .push(
            button::primary(None, "Send")
                .on_press(Message::Menu(Menu::Vault(VaultSubMenu::Send)))
                .width(Length::Fixed(90.0)),
        )
        .push(
            button::orange_outline(None, "Receive")
                .on_press(Message::Menu(Menu::Vault(VaultSubMenu::Receive)))
                .width(Length::Fixed(90.0)),
        );
    Column::new()
        .push(balance_header_card(
            Column::new()
                .spacing(16)
                .push(
                    Column::new().spacing(8).push(h4_bold("Balance")).push(
                        wallet_header::<Message>(WalletHeaderProps {
                            sats: *balance,
                            fiat: fiat_balance,
                            balance_masked: false,
                            bitcoin_unit,
                            variant: HeaderVariant::Overview,
                            sync,
                            unconfirmed,
                            pending_send_sats: 0,
                            pending_receive_sats: 0,
                            display_mode,
                            on_swap: Some(Message::FlipDisplayMode),
                        }),
                    ),
                )
                .push(vault_btc_row),
        ))
        .push(show_rescan_warning.then_some(rescan_warning()))
        .push(match (node_bitcoind_ibd, node_bitcoind_sync_progress) {
            (Some(true), Some(progress)) => Some(
                Container::new(
                    Row::new()
                        .spacing(10)
                        .align_y(Alignment::Center)
                        .push(
                            text(format!(
                                "Your local node is syncing — {:.1}% complete",
                                100.0 * progress
                            ))
                            .style(theme::text::secondary)
                            .width(Length::Fill),
                        )
                        .push(spinner::typing_text_carousel(
                            "...",
                            true,
                            Duration::from_millis(2000),
                            |content| text(content).style(theme::text::secondary),
                        ))
                        .width(Length::Fill),
                )
                .padding(15)
                .style(theme::card::border),
            ),
            _ => None,
        })
        .push(if expiring_coins.is_empty() {
            remaining_sequence.map(|sequence| {
                Container::new(
                    Row::new()
                        .spacing(15)
                        .align_y(Alignment::Center)
                        .push(
                            h4_regular(format!(
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
                        .width(Length::Fill),
                )
                .padding(25)
                .style(theme::card::border)
            })
        } else {
            Some(
                Container::new(
                    Row::new()
                        .spacing(15)
                        .align_y(Alignment::Center)
                        .push(
                            h4_regular(format!(
                                "Recovery path is or will soon be available for {} coin(s).",
                                expiring_coins.len(),
                            ))
                            .width(Length::Fill),
                        )
                        .push(
                            button::primary(Some(icon::arrow_repeat()), "Refresh coins").on_press(
                                Message::Menu(Menu::Vault(crate::app::menu::VaultSubMenu::Coins(
                                    Some(expiring_coins.to_owned()),
                                ))),
                            ),
                        ),
                )
                .padding(25)
                .style(theme::card::invalid),
            )
        })
        .push(
            Column::new()
                .spacing(10)
                .push(h4_bold("Last transactions"))
                .push(events.iter().fold(Column::new().spacing(10), |col, event| {
                    if event.kind != PaymentKind::SendToSelf {
                        col.push(event_list_view(
                            event,
                            bitcoin_unit,
                            fiat_converter,
                            show_direction_badges,
                        ))
                    } else {
                        col
                    }
                }))
                .push(if !is_last_page && !events.is_empty() {
                    Some(
                        Container::new(
                            Button::new(
                                text(if processing {
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
                }),
        )
        .push_maybe(if !events.is_empty() {
            Some(
                Container::new({
                    let tx_icon = icon::history_icon()
                        .size(18)
                        .style(|_theme: &theme::Theme| iced::widget::text::Style {
                            color: Some(color::ORANGE),
                        });
                    let tx_label =
                        text("View All Transactions")
                            .size(15)
                            .style(|_theme: &theme::Theme| iced::widget::text::Style {
                                color: Some(color::ORANGE),
                            });
                    iced::widget::button(
                        Container::new(
                            Row::new()
                                .spacing(8)
                                .align_y(iced::Alignment::Center)
                                .push(tx_icon)
                                .push(tx_label),
                        )
                        .padding([10, 20])
                        .style(|_theme: &theme::Theme| {
                            iced::widget::container::Style {
                                background: Some(iced::Background::Color(color::TRANSPARENT)),
                                border: iced::Border {
                                    color: color::ORANGE,
                                    width: 1.5,
                                    radius: 20.0.into(),
                                },
                                ..Default::default()
                            }
                        }),
                    )
                    .style(|_theme: &theme::Theme, _| iced::widget::button::Style {
                        background: Some(iced::Background::Color(color::TRANSPARENT)),
                        text_color: color::ORANGE,
                        border: iced::Border {
                            radius: 20.0.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    })
                    .on_press(Message::Menu(Menu::Vault(VaultSubMenu::Transactions(None))))
                })
                .width(Length::Fill)
                .center_x(Length::Fill),
            )
        } else {
            None
        })
        .push(Space::new().height(Length::Fixed(40.0)))
        .spacing(20)
        .into()
}

fn event_list_view(
    event: &Payment,
    bitcoin_unit: BitcoinDisplayUnit,
    fiat_converter: Option<FiatAmountConverter>,
    show_direction_badges: bool,
) -> Element<'_, Message> {
    let direction = if event.kind == PaymentKind::Incoming {
        TransactionDirection::Incoming
    } else {
        TransactionDirection::Outgoing
    };

    let label = if let Some(label) = &event.label {
        Some(label.clone())
    } else {
        event
            .address_label
            .as_ref()
            .map(|label| format!("address label: {}", label))
    };

    let mut item = TransactionListItem::new(direction, &event.amount, bitcoin_unit)
        .with_custom_icon(coincube_ui::image::asset_network_logo(
            "btc", "bitcoin", 40.0,
        ))
        .with_show_direction_badge(show_direction_badges);

    if let Some(label) = label {
        item = item.with_label(label);
    }

    if let Some(timestamp) = event.time {
        item = item.with_timestamp(timestamp);
    } else {
        item = item.with_badge(TransactionBadge::Unconfirmed);
    }

    if let Some(fiat_amount) = fiat_converter.map(|converter| {
        let fiat = converter.convert(event.amount);
        format!("{} {}", fiat.to_rounded_string(), fiat.currency())
    }) {
        item = item.with_fiat_amount(fiat_amount);
    }

    item.view(Message::Menu(Menu::Vault(VaultSubMenu::Transactions(
        Some(event.outpoint.txid),
    ))))
    .into()
}

pub fn payment_view<'a>(
    menu: &'a Menu,
    cache: &'a Cache,
    tx: &'a HistoryTransaction,
    output_index: usize,
    labels_editing: &'a HashMap<String, form::Value<String>>,
) -> Element<'a, Message> {
    let txid = tx.tx.compute_txid().to_string();
    let outpoint = bitcoin::OutPoint {
        txid: tx.tx.compute_txid(),
        vout: output_index as u32,
    }
    .to_string();
    dashboard(
        menu,
        cache,
        Column::new()
            .push(match tx.kind {
                TransactionKind::OutgoingSinglePayment(_)
                | TransactionKind::OutgoingPaymentBatch(_) => {
                    Container::new(h3("Outgoing payment")).width(Length::Fill)
                }
                TransactionKind::IncomingSinglePayment(_)
                | TransactionKind::IncomingPaymentBatch(_) => {
                    Container::new(h3("Incoming payment")).width(Length::Fill)
                }
                _ => Container::new(h3("Payment")).width(Length::Fill),
            })
            .push(if tx.is_single_payment().is_some() {
                // if the payment is a payment of a single payment transaction then
                // the label of the transaction is attached to the label of the payment outpoint
                if let Some(label) = labels_editing.get(&outpoint) {
                    label::label_editing(vec![outpoint.clone(), txid.clone()], label, H3_SIZE)
                } else {
                    label::label_editable(
                        vec![outpoint.clone(), txid.clone()],
                        tx.labels.get(&outpoint),
                        H3_SIZE,
                    )
                }
            } else if let Some(label) = labels_editing.get(&outpoint) {
                label::label_editing(vec![outpoint.clone()], label, H3_SIZE)
            } else {
                label::label_editable(vec![outpoint.clone()], tx.labels.get(&outpoint), H3_SIZE)
            })
            .push(Container::new(amount_with_size(
                &tx.tx.output[output_index].value,
                H3_SIZE,
            )))
            .push(Space::new().height(H3_SIZE))
            .push(Container::new(h3("Transaction")).width(Length::Fill))
            .push(if tx.is_batch() {
                if let Some(label) = labels_editing.get(&txid) {
                    Some(label::label_editing(vec![txid.clone()], label, H3_SIZE))
                } else {
                    Some(label::label_editable(
                        vec![txid.clone()],
                        tx.labels.get(&txid),
                        H3_SIZE,
                    ))
                }
            } else {
                None
            })
            .push(tx.fee_amount.map(|fee_amount| {
                Row::new()
                    .align_y(Alignment::Center)
                    .push(h3("Miner fee: ").style(theme::text::secondary))
                    .push(amount_with_size(&fee_amount, H3_SIZE))
                    .push(text(" ").size(H3_SIZE))
                    .push(
                        text(format!(
                            "({} sats/vbyte)",
                            fee_amount.to_sat() / tx.tx.vsize() as u64
                        ))
                        .size(H4_SIZE)
                        .style(theme::text::secondary),
                    )
            }))
            .push(card::simple(
                Column::new()
                    .push(tx.time.map(|t| {
                        let date = DateTime::<Utc>::from_timestamp(t as i64, 0)
                            .unwrap()
                            .with_timezone(&Local)
                            .format("%b. %d, %Y - %T");
                        Row::new()
                            .width(Length::Fill)
                            .push(Container::new(text("Date:").bold()).width(Length::Fill))
                            .push(Container::new(text(format!("{}", date))).width(Length::Shrink))
                    }))
                    .push(
                        Row::new()
                            .width(Length::Fill)
                            .align_y(Alignment::Center)
                            .push(Container::new(text("Txid:").bold()).width(Length::Fill))
                            .push(
                                Row::new()
                                    .align_y(Alignment::Center)
                                    .push(Container::new(
                                        text(format!("{}", tx.tx.compute_txid())).small(),
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
                    Menu::Vault(VaultSubMenu::Transactions(Some(tx.tx.compute_txid()))),
                )),
            )
            .spacing(20),
    )
}

/// Full-screen celebration view when a vault payment is received.
pub fn received_celebration_page<'a>(
    context: &str,
    amount_display: &'a str,
    quote: &'a coincube_ui::component::quote_display::Quote,
    image_handle: &'a iced::widget::image::Handle,
) -> Element<'a, Message> {
    coincube_ui::component::received_celebration_page(
        context,
        amount_display,
        quote,
        image_handle,
        Message::DismissReceivedCelebration,
    )
}
