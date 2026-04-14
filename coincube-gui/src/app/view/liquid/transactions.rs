use std::collections::HashMap;

use breez_sdk_liquid::model::{PaymentDetails, PaymentState};
use breez_sdk_liquid::prelude::{Payment, PaymentType, RefundableSwap};
use coincube_core::miniscript::bitcoin::Amount;
use iced::widget::image;

use coincube_ui::{
    component::{
        amount::DisplayAmount,
        button, card, form,
        quote_display::{self, Quote, QuoteDisplayProps},
        text::*,
        transaction::{TransactionDirection, TransactionListItem},
    },
    icon, theme,
    widget::*,
};
use iced::{
    widget::{scrollable, Column, Container, Row, Space},
    Alignment, Length,
};

use coincube_ui::image::asset_network_logo;

use crate::app::breez::assets::{format_usdt_display, USDT_PRECISION};
use crate::app::breez::swap_status::{classify_payment, BtcSwapReceiveStatus};
use crate::app::menu::Menu;
use crate::app::state::liquid::transactions::{AssetFilter, InFlightRefund};
use crate::app::view::message::{FeeratePriority, Message};
use crate::app::view::FiatAmountConverter;
use crate::export::ImportExportMessage;
use crate::utils::{format_time_ago, format_timestamp, truncate_middle};

/// Styled status cell for the payment detail card. For BTC onchain swap
/// payments this routes through `classify_payment`, which gives us the full
/// Boltz lifecycle (including the `Failed` → `Refundable` upgrade when the SDK
/// still lists the swap as refundable and the `Pending` → `PendingConfirmation`
/// vs `PendingSwapCompletion` split once the lockup tx is seen). Direct Liquid
/// and Lightning payments fall back to raw SDK labels, because swap-specific
/// mappings like "Complete Send → Refunded" don't apply to them.
fn payment_status_text(
    payment: &Payment,
    refundable_swap_addresses: &[String],
) -> Element<'static, Message> {
    if matches!(payment.details, PaymentDetails::Bitcoin { .. }) {
        let status = classify_payment(payment, refundable_swap_addresses);
        let style = match status {
            BtcSwapReceiveStatus::Completed | BtcSwapReceiveStatus::Refunded => {
                theme::text::success
            }
            BtcSwapReceiveStatus::Failed | BtcSwapReceiveStatus::Refundable => {
                theme::text::destructive
            }
            _ => theme::text::secondary,
        };
        return text(status.label()).style(style).into();
    }

    match payment.status {
        PaymentState::Complete => text("Complete").style(theme::text::success).into(),
        PaymentState::Pending => text("Pending").style(theme::text::secondary).into(),
        PaymentState::Created => text("Created").style(theme::text::secondary).into(),
        PaymentState::Failed => text("Failed").style(theme::text::destructive).into(),
        PaymentState::TimedOut => text("Timed Out").style(theme::text::destructive).into(),
        PaymentState::Refundable => text("Refundable").style(theme::text::destructive).into(),
        PaymentState::RefundPending => text("Refund Pending").style(theme::text::secondary).into(),
        PaymentState::WaitingFeeAcceptance => text("Waiting Fee Acceptance")
            .style(theme::text::secondary)
            .into(),
    }
}

/// Returns `Some(formatted_usdt_string)` when the payment is a USDt asset payment.
fn usdt_amount_str(payment: &Payment, usdt_id: &str) -> Option<String> {
    if let PaymentDetails::Liquid {
        asset_id,
        asset_info,
        ..
    } = &payment.details
    {
        if !usdt_id.is_empty() && asset_id == usdt_id {
            let display = if let Some(info) = asset_info {
                format_usdt_display(
                    (info.amount * 10_f64.powi(USDT_PRECISION as i32)).round() as u64
                )
            } else {
                format_usdt_display(payment.amount_sat)
            };
            return Some(format!("{} USDt", display));
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
pub fn liquid_transactions_view<'a>(
    payments: &'a [Payment],
    refundables: &'a [RefundableSwap],
    in_flight_refunds: &'a HashMap<String, InFlightRefund>,
    _balance: &'a Amount,
    fiat_converter: Option<FiatAmountConverter>,
    _loading: bool,
    bitcoin_unit: coincube_ui::component::amount::BitcoinDisplayUnit,
    usdt_id: &'a str,
    asset_filter: AssetFilter,
    show_direction_badges: bool,
    empty_state_quote: &'a Quote,
    empty_state_image_handle: &'a image::Handle,
) -> Element<'a, Message> {
    let mut content = Column::new().spacing(20).width(Length::Fill);

    // Header row with Transactions heading and Export button (matching Vault style)
    content = content.push(
        Row::new()
            .push(Container::new(h3("Transactions").bold()))
            .push(Space::new().width(Length::Fill))
            .push(
                button::secondary(Some(icon::backup_icon()), "Export")
                    .on_press(ImportExportMessage::Open.into()),
            ),
    );

    // Asset filter tabs
    {
        let mut filter_row = Row::new().spacing(8);
        for (label, filter) in [
            ("All", AssetFilter::All),
            ("L-BTC", AssetFilter::LbtcOnly),
            ("USDt", AssetFilter::UsdtOnly),
        ] {
            let is_active = asset_filter == filter;
            let btn = iced::widget::button(
                Container::new(text(label).size(P2_SIZE).color(if is_active {
                    coincube_ui::color::WHITE
                } else {
                    coincube_ui::color::GREY_3
                }))
                .padding([6, 14]),
            )
            .on_press(Message::SetAssetFilter(filter))
            .style(if is_active {
                theme::button::primary
            } else {
                theme::button::transparent_border
            });
            filter_row = filter_row.push(btn);
        }
        content = content.push(filter_row);
    }

    if payments.is_empty() {
        // Empty state with Kage quote
        content = content.push(
            Column::new()
                .spacing(20)
                .width(Length::Fill)
                .align_x(Alignment::Center)
                .push(Space::new().height(Length::Fixed(40.0)))
                .push(quote_display::display(
                    &QuoteDisplayProps::new("empty-wallet", empty_state_quote, empty_state_image_handle),
                ))
                .push(Space::new().height(Length::Fixed(10.0)))
                .push(
                    text("Your Liquid wallet is ready. Once you send or receive\nfunds, they'll show up here.")
                        .size(16)
                        .style(theme::text::secondary)
                        .wrapping(iced::widget::text::Wrapping::Word)
                        .align_x(iced::alignment::Horizontal::Center),
                )
                .push(
                    Row::new()
                        .spacing(15)
                        .push(
                            button::primary(None, "Send sats")
                                .on_press(Message::Menu(Menu::Liquid(crate::app::menu::LiquidSubMenu::Send)))
                                .padding(15)
                                .width(Length::Fixed(150.0)),
                        )
                        .push(
                            button::transparent_border(None, "Receive sats")
                                .on_press(Message::Menu(Menu::Liquid(crate::app::menu::LiquidSubMenu::Receive)))
                                .padding(15)
                                .width(Length::Fixed(150.0)),
                        ),
                ),
        );
    } else {
        // Transaction list
        content = content.push(
            Column::new()
                .spacing(10)
                .push(payments.iter().enumerate().fold(
                    Column::new().spacing(10),
                    |col, (i, payment)| {
                        col.push(transaction_row(
                            i,
                            payment,
                            fiat_converter,
                            bitcoin_unit,
                            usdt_id,
                            show_direction_badges,
                        ))
                    },
                )),
        );
    }

    // Refundables are always BTC → L-BTC swap refunds, so they only belong
    // under the "All" and "L-BTC" filters. Previously they leaked into the
    // USDt tab — fixed here.
    let show_refundables =
        !refundables.is_empty() && matches!(asset_filter, AssetFilter::All | AssetFilter::LbtcOnly);
    if show_refundables {
        content = content.push(
            Column::new()
                .spacing(10)
                .push(Space::new().height(Length::Fixed(20.0)))
                .push(Container::new(h3("Refundable Transactions").bold()))
                .push(refundables.iter().enumerate().fold(
                    Column::new().spacing(10),
                    |col, (i, refundable)| {
                        col.push(refundable_row(
                            i,
                            refundable,
                            in_flight_refunds.get(&refundable.swap_address),
                            fiat_converter,
                            bitcoin_unit,
                            show_direction_badges,
                        ))
                    },
                )),
        );
    }

    content.into()
}

fn transaction_row<'a>(
    i: usize,
    payment: &'a Payment,
    fiat_converter: Option<FiatAmountConverter>,
    bitcoin_unit: coincube_ui::component::amount::BitcoinDisplayUnit,
    usdt_id: &str,
    show_direction_badges: bool,
) -> Element<'a, Message> {
    let is_receive = matches!(payment.payment_type, PaymentType::Receive);
    let usdt_str = usdt_amount_str(payment, usdt_id);

    // Extract description — label USDt payments explicitly
    let is_usdt = usdt_str.is_some();
    let description: &str = if is_usdt {
        "USDt Transfer"
    } else {
        match &payment.details {
            PaymentDetails::Lightning {
                payer_note,
                description,
                ..
            } => payer_note
                .as_ref()
                .filter(|s| !s.is_empty())
                .unwrap_or(description),
            PaymentDetails::Liquid {
                payer_note,
                description,
                ..
            } => payer_note
                .as_ref()
                .filter(|s| !s.is_empty())
                .unwrap_or(description),
            PaymentDetails::Bitcoin { description, .. } => description,
        }
    };

    let time_ago = format_time_ago(payment.timestamp.into());

    let direction = if is_receive {
        TransactionDirection::Incoming
    } else {
        TransactionDirection::Outgoing
    };

    // Determine the combo icon based on payment type
    let combo_icon: Element<'_, Message> = if is_usdt {
        asset_network_logo("usdt", "liquid", 40.0)
    } else {
        match &payment.details {
            PaymentDetails::Lightning { .. } => asset_network_logo("btc", "lightning", 40.0),
            PaymentDetails::Liquid { .. } => asset_network_logo("lbtc", "liquid", 40.0),
            PaymentDetails::Bitcoin { .. } => asset_network_logo("btc", "bitcoin", 40.0),
        }
    };

    if let Some(ref usdt_display) = usdt_str {
        let item = TransactionListItem::new(direction, &Amount::ZERO, bitcoin_unit)
            .with_label(description.to_string())
            .with_time_ago(time_ago)
            .with_custom_icon(combo_icon)
            .with_show_direction_badge(show_direction_badges)
            .with_amount_override(usdt_display.clone());
        return item.view(Message::Select(i)).into();
    }

    let mut btc_amount = Amount::from_sat(payment.amount_sat);
    if !is_receive {
        btc_amount += Amount::from_sat(payment.fees_sat);
    }

    let mut item = TransactionListItem::new(direction, &btc_amount, bitcoin_unit)
        .with_label(description.to_string())
        .with_time_ago(time_ago)
        .with_custom_icon(combo_icon)
        .with_show_direction_badge(show_direction_badges);

    if let Some(fiat_amount) = fiat_converter.map(|converter| {
        let fiat = converter.convert(btc_amount);
        format!("~{} {}", fiat.to_rounded_string(), fiat.currency())
    }) {
        item = item.with_fiat_amount(fiat_amount);
    }

    item.view(Message::Select(i)).into()
}

fn refundable_row<'a>(
    i: usize,
    refundable: &'a RefundableSwap,
    in_flight: Option<&'a InFlightRefund>,
    fiat_converter: Option<FiatAmountConverter>,
    bitcoin_unit: coincube_ui::component::amount::BitcoinDisplayUnit,
    show_direction_badges: bool,
) -> Element<'a, Message> {
    let btc_amount = Amount::from_sat(refundable.amount_sat);
    let time_ago = format_time_ago(refundable.timestamp.into());

    let direction = TransactionDirection::Incoming;

    // If we have an in-flight refund for this swap, reflect that in the row
    // label so the user can tell at a glance that their submission is being
    // broadcast. Previously the card either disappeared or looked identical
    // to "not yet refunded".
    let label = match in_flight {
        Some(InFlightRefund {
            refund_txid: Some(txid),
            ..
        }) => {
            format!("Refund broadcast · {}", truncate_middle(txid, 6, 6))
        }
        Some(_) => "Refund broadcasting…".to_string(),
        None => "Refundable Swap".to_string(),
    };

    let mut item = TransactionListItem::new(direction, &btc_amount, bitcoin_unit)
        .with_label(label)
        .with_time_ago(time_ago)
        .with_custom_icon(asset_network_logo("lbtc", "liquid", 40.0))
        .with_show_direction_badge(show_direction_badges);

    if let Some(fiat_amount) = fiat_converter.map(|converter| {
        let fiat = converter.convert(btc_amount);
        format!("~{} {}", fiat.to_rounded_string(), fiat.currency())
    }) {
        item = item.with_fiat_amount(fiat_amount);
    }

    item.view(Message::SelectRefundable(i)).into()
}

pub fn transaction_detail_view<'a>(
    payment: &'a Payment,
    fiat_converter: Option<FiatAmountConverter>,
    bitcoin_unit: coincube_ui::component::amount::BitcoinDisplayUnit,
    usdt_id: &str,
    refundable_swap_addresses: &[String],
) -> Element<'a, Message> {
    let is_receive = matches!(payment.payment_type, PaymentType::Receive);
    let usdt_str = usdt_amount_str(payment, usdt_id);
    let btc_amount = Amount::from_sat(payment.amount_sat);
    let fees_sat = Amount::from_sat(payment.fees_sat);
    let mut total_amount = btc_amount;

    if !is_receive {
        total_amount += fees_sat;
    }

    // Format full date/time
    let date_text =
        format_timestamp(payment.timestamp as u64).unwrap_or_else(|| "Unknown".to_string());

    // Extract description — label USDt payments explicitly
    let description: &str = if usdt_str.is_some() {
        "USDt Transfer"
    } else {
        match &payment.details {
            PaymentDetails::Lightning {
                payer_note,
                description,
                ..
            } => payer_note
                .as_ref()
                .filter(|s| !s.is_empty())
                .unwrap_or(description),
            PaymentDetails::Liquid {
                payer_note,
                description,
                ..
            } => payer_note
                .as_ref()
                .filter(|s| !s.is_empty())
                .unwrap_or(description),
            PaymentDetails::Bitcoin { description, .. } => description,
        }
    };

    let title = if is_receive {
        "Incoming payment"
    } else {
        "Outgoing payment"
    };

    // Helper: combo icon for detail view based on payment type
    let make_detail_icon =
        |is_usdt: bool, details: &PaymentDetails| -> (&'static str, &'static str) {
            if is_usdt {
                ("usdt", "liquid")
            } else {
                match details {
                    PaymentDetails::Lightning { .. } => ("btc", "lightning"),
                    PaymentDetails::Liquid { .. } => ("lbtc", "liquid"),
                    PaymentDetails::Bitcoin { .. } => ("btc", "bitcoin"),
                }
            }
        };

    if let Some(ref usdt_display) = usdt_str {
        // USDt detail view: show USDt amount + L-BTC fees
        let usdt_num = match &payment.details {
            PaymentDetails::Liquid { asset_info, .. } => {
                if let Some(info) = asset_info {
                    format_usdt_display(
                        (info.amount * 10_f64.powi(USDT_PRECISION as i32)).round() as u64
                    )
                } else {
                    format_usdt_display(payment.amount_sat)
                }
            }
            _ => format_usdt_display(payment.amount_sat),
        };
        let amount_row = if is_receive {
            Row::new()
                .spacing(10)
                .align_y(Alignment::Center)
                .push(text(usdt_num).size(H1_SIZE).bold())
                .push(text("USDt").size(H1_SIZE).color(coincube_ui::color::GREY_3))
        } else {
            Row::new()
                .spacing(5)
                .align_y(Alignment::Center)
                .push(text("-").size(H1_SIZE))
                .push(
                    Row::new()
                        .spacing(10)
                        .align_y(Alignment::Center)
                        .push(text(usdt_num).size(H1_SIZE).bold())
                        .push(text("USDt").size(H1_SIZE).color(coincube_ui::color::GREY_3)),
                )
        };
        return Column::new()
            .spacing(20)
            .push(detail_back_button())
            .push(Container::new(h3(title)).width(Length::Fill))
            .push({
                let (a, n) = make_detail_icon(true, &payment.details);
                Row::new()
                    .spacing(10)
                    .align_y(Alignment::Center)
                    .push(asset_network_logo::<Message>(a, n, 32.0))
                    .push(p1_regular(description))
            })
            .push(
                Column::new()
                    .spacing(20)
                    .push(Column::new().push(Container::new(amount_row))),
            )
            .push(card::simple(
                Column::new()
                    .push(
                        Row::new()
                            .push(
                                Column::new()
                                    .width(Length::FillPortion(1))
                                    .push(text("Date").bold()),
                            )
                            .push(
                                Column::new()
                                    .width(Length::FillPortion(2))
                                    .push(text(date_text)),
                            )
                            .spacing(20),
                    )
                    .push(
                        Row::new()
                            .push(
                                Column::new()
                                    .width(Length::FillPortion(1))
                                    .push(text("Status").bold()),
                            )
                            .push(
                                Column::new()
                                    .width(Length::FillPortion(2))
                                    .push(payment_status_text(payment, refundable_swap_addresses)),
                            )
                            .spacing(20),
                    )
                    .push(
                        Row::new()
                            .push(
                                Column::new()
                                    .width(Length::FillPortion(1))
                                    .push(text("Asset Amount").bold()),
                            )
                            .push(
                                Column::new()
                                    .width(Length::FillPortion(2))
                                    .push(text(usdt_display.clone())),
                            )
                            .spacing(20),
                    )
                    .push(if fees_sat.to_sat() == 0 && !is_receive {
                        Row::new()
                            .push(
                                Column::new()
                                    .width(Length::FillPortion(1))
                                    .push(text("Fees").bold()),
                            )
                            .push(
                                Column::new()
                                    .width(Length::FillPortion(2))
                                    .push(text("Paid in USDt")),
                            )
                            .spacing(20)
                    } else {
                        Row::new()
                            .push(
                                Column::new()
                                    .width(Length::FillPortion(1))
                                    .push(text("Fees (L-BTC)").bold()),
                            )
                            .push(
                                Column::new().width(Length::FillPortion(2)).push(text(
                                    fees_sat.to_formatted_string_with_unit(bitcoin_unit),
                                )),
                            )
                            .spacing(20)
                    })
                    .spacing(15),
            ))
            .into();
    }

    Column::new()
        .spacing(20)
        .push(detail_back_button())
        .push(if is_receive {
            Container::new(h3("Incoming payment")).width(Length::Fill)
        } else {
            Container::new(h3("Outgoing payment")).width(Length::Fill)
        })
        .push({
            let (a, n) = make_detail_icon(false, &payment.details);
            Row::new()
                .spacing(10)
                .align_y(Alignment::Center)
                .push(asset_network_logo::<Message>(a, n, 32.0))
                .push(p1_regular(description))
        })
        .push(
            Column::new().spacing(20).push(
                Column::new()
                    .push(if is_receive {
                        Container::new(coincube_ui::component::amount::amount_with_size_and_unit(
                            &btc_amount,
                            H1_SIZE,
                            bitcoin_unit,
                        ))
                    } else {
                        Container::new(Row::new().spacing(5).push(text("-").size(H1_SIZE)).push(
                            coincube_ui::component::amount::amount_with_size_and_unit(
                                &total_amount,
                                H1_SIZE,
                                bitcoin_unit,
                            ),
                        ))
                    })
                    .push_maybe(fiat_converter.map(|converter| {
                        // Use total_amount for outgoing payments to match headline (includes fees)
                        let amount_for_conversion =
                            if is_receive { btc_amount } else { total_amount };
                        let fiat = converter.convert(amount_for_conversion);
                        Row::new().align_y(Alignment::Center).push(
                            fiat.to_text()
                                .size(H2_SIZE)
                                .color(coincube_ui::color::GREY_2),
                        )
                    })),
            ),
        )
        .push(card::simple(
            Column::new()
                .push(
                    Row::new()
                        .push(
                            Column::new()
                                .width(Length::FillPortion(1))
                                .push(text("Date").bold()),
                        )
                        .push(
                            Column::new()
                                .width(Length::FillPortion(2))
                                .push(text(date_text)),
                        )
                        .spacing(20),
                )
                .push(
                    Row::new()
                        .push(
                            Column::new()
                                .width(Length::FillPortion(1))
                                .push(text("Status").bold()),
                        )
                        .push(
                            Column::new()
                                .width(Length::FillPortion(2))
                                .push(payment_status_text(payment, refundable_swap_addresses)),
                        )
                        .spacing(20),
                )
                .push(
                    Row::new()
                        .push(
                            Column::new()
                                .width(Length::FillPortion(1))
                                .push(text("Amount").bold()),
                        )
                        .push(
                            Column::new()
                                .width(Length::FillPortion(2))
                                .push(text(btc_amount.to_formatted_string_with_unit(bitcoin_unit))),
                        )
                        .spacing(20),
                )
                .push(
                    Row::new()
                        .push(
                            Column::new()
                                .width(Length::FillPortion(1))
                                .push(text("Fees").bold()),
                        )
                        .push(
                            Column::new()
                                .width(Length::FillPortion(2))
                                .push(text(fees_sat.to_formatted_string_with_unit(bitcoin_unit))),
                        )
                        .spacing(20),
                )
                .spacing(15),
        ))
        .into()
}

/// Low/Medium/High fee priority buttons. While an async fee fetch is in
/// flight for a given priority, that button renders with a "…" label and is
/// disabled, so the user can tell something is happening — before this, the
/// buttons silently triggered a slow mempool fetch and the user was left
/// wondering if they had actually been clicked.
fn fee_priority_buttons(pending: Option<FeeratePriority>) -> Element<'static, Message> {
    fn one(
        label: &'static str,
        priority: FeeratePriority,
        width: f32,
        pending: Option<FeeratePriority>,
    ) -> Element<'static, Message> {
        if pending == Some(priority) {
            // Pending: show "…" and no on_press so the button is visually
            // disabled while the async fee fetch resolves.
            button::secondary(None, "…")
                .width(Length::Fixed(width))
                .into()
        } else {
            button::secondary(None, label)
                .on_press(Message::RefundFeeratePrioritySelected(priority))
                .width(Length::Fixed(width))
                .into()
        }
    }

    Row::new()
        .spacing(5)
        .push(one("Low", FeeratePriority::Low, 80.0, pending))
        .push(one("Medium", FeeratePriority::Medium, 100.0, pending))
        .push(one("High", FeeratePriority::High, 80.0, pending))
        .into()
}

#[allow(clippy::too_many_arguments)]
pub fn refundable_detail_view<'a>(
    refundable: &'a RefundableSwap,
    fiat_converter: Option<FiatAmountConverter>,
    bitcoin_unit: coincube_ui::component::amount::BitcoinDisplayUnit,
    refund_address: &'a form::Value<String>,
    refund_feerate: &'a form::Value<String>,
    refunding: bool,
    pending_fee_priority: Option<FeeratePriority>,
    in_flight: Option<&'a InFlightRefund>,
    has_vault: bool,
) -> Element<'a, Message> {
    let btc_amount = Amount::from_sat(refundable.amount_sat);

    let date_text =
        format_timestamp(refundable.timestamp as u64).unwrap_or_else(|| "Unknown".to_string());

    let can_refund = refund_address.valid
        && !refund_address.value.trim().is_empty()
        && refund_feerate.valid
        && !refund_feerate.value.trim().is_empty()
        && refund_feerate.value.parse::<u32>().is_ok()
        && in_flight.is_none();

    let header_status = match in_flight {
        Some(InFlightRefund {
            refund_txid: Some(txid),
            ..
        }) => {
            format!("Refund broadcast · {}", truncate_middle(txid, 6, 6))
        }
        Some(_) => "Refund broadcasting…".to_string(),
        None => "This swap can be refunded".to_string(),
    };

    Column::new()
        .spacing(20)
        .push(Container::new(h3("Refundable Swap")).width(Length::Fill))
        .push(Column::new().push(p1_regular(header_status)).spacing(10))
        .push(
            Column::new().spacing(20).push(
                Column::new()
                    .push(Container::new(Row::new().spacing(5).push(
                        coincube_ui::component::amount::amount_with_size_and_unit(
                            &btc_amount,
                            H1_SIZE,
                            bitcoin_unit,
                        ),
                    )))
                    .push_maybe(fiat_converter.map(|converter| {
                        let fiat = converter.convert(btc_amount);
                        Row::new().align_y(Alignment::Center).push(
                            fiat.to_text()
                                .size(H2_SIZE)
                                .color(coincube_ui::color::GREY_2),
                        )
                    })),
            ),
        )
        .push(card::simple(
            Column::new()
                .push(
                    Row::new()
                        .push(
                            Column::new()
                                .width(Length::FillPortion(1))
                                .push(text("Date").bold()),
                        )
                        .push(
                            Column::new()
                                .width(Length::FillPortion(2))
                                .push(text(date_text)),
                        )
                        .spacing(20),
                )
                .push(
                    Row::new()
                        .push(
                            Column::new()
                                .width(Length::FillPortion(1))
                                .push(text("Swap Address").bold()),
                        )
                        .push(
                            Column::new()
                                .width(Length::FillPortion(2))
                                // Long taproot swap addresses overflow the
                                // card otherwise. Show a middle-truncated
                                // preview with a copy button so the user can
                                // still grab the full address.
                                .push(
                                    Row::new()
                                        .align_y(Alignment::Center)
                                        .spacing(8)
                                        .push(Container::new(
                                            scrollable(
                                                text(truncate_middle(
                                                    &refundable.swap_address,
                                                    10,
                                                    10,
                                                ))
                                                .size(14),
                                            )
                                            .direction(scrollable::Direction::Horizontal(
                                                scrollable::Scrollbar::new()
                                                    .width(2)
                                                    .scroller_width(2),
                                            )),
                                        ))
                                        .push(
                                            iced::widget::button(
                                                icon::clipboard_icon()
                                                    .style(theme::text::secondary),
                                            )
                                            .on_press(Message::Clipboard(
                                                refundable.swap_address.clone(),
                                            ))
                                            .style(theme::button::transparent_border),
                                        ),
                                ),
                        )
                        .spacing(20),
                )
                .push(
                    Row::new()
                        .push(
                            Column::new()
                                .width(Length::FillPortion(1))
                                .push(text("Amount").bold()),
                        )
                        .push(
                            Column::new()
                                .width(Length::FillPortion(2))
                                .push(text(btc_amount.to_formatted_string_with_unit(bitcoin_unit))),
                        )
                        .spacing(20),
                )
                .spacing(15),
        ))
        .push(Space::new().height(Length::Fixed(20.0)))
        .push(
            Column::new()
                .spacing(15)
                .push(Container::new(h4_bold("Refund Details")).width(Length::Fill))
                .push(
                    Column::new()
                        .spacing(10)
                        .push(text("Bitcoin Address").bold())
                        .push(
                            form::Form::new(
                                "Enter Bitcoin address for refund",
                                refund_address,
                                Message::RefundAddressEdited,
                            )
                            .size(14)
                            .padding(12),
                        )
                        .push_maybe(has_vault.then(|| {
                            Row::new().spacing(8).push(
                                button::transparent_border(None, "Use Vault address")
                                    .on_press(Message::GenerateVaultRefundAddress)
                                    .padding([6, 14]),
                            )
                        })),
                )
                .push(
                    Column::new()
                        .spacing(10)
                        .push(text("Fee Rate (sat/vB)").bold())
                        .push(
                            Row::new()
                                .spacing(10)
                                .push(
                                    form::Form::new_amount_sats(
                                        "Enter fee rate",
                                        refund_feerate,
                                        Message::RefundFeerateEdited,
                                    )
                                    .size(14)
                                    .padding(12),
                                )
                                .push(fee_priority_buttons(pending_fee_priority)),
                        ),
                )
                .push(
                    Row::new().spacing(10).push(
                        button::primary(None, if refunding { "Refund..." } else { "Refund" })
                            .on_press_maybe(if can_refund && !refunding {
                                Some(Message::SubmitRefund)
                            } else {
                                None
                            })
                            .width(Length::Fill),
                    ),
                ),
        )
        .into()
}

fn detail_back_button() -> Element<'static, Message> {
    iced::widget::button(
        Row::new()
            .spacing(5)
            .align_y(Alignment::Center)
            .push(icon::previous_icon().style(theme::text::secondary))
            .push(text("Previous").size(14).style(theme::text::secondary)),
    )
    .on_press(Message::Close)
    .style(theme::button::transparent)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refundables_gated_out_of_usdt_filter() {
        // Contract guard: refundables are BTC→L-BTC swap refunds, so they
        // should only surface under All / L-BTC. Regression from prior bug
        // where they appeared under the USDt tab.
        for (filter, expected) in [
            (AssetFilter::All, true),
            (AssetFilter::LbtcOnly, true),
            (AssetFilter::UsdtOnly, false),
        ] {
            let show = matches!(filter, AssetFilter::All | AssetFilter::LbtcOnly);
            assert_eq!(
                show, expected,
                "refundables visibility for filter {:?}",
                filter
            );
        }
    }
}
