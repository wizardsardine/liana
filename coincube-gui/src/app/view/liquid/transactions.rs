use breez_sdk_liquid::model::{PaymentDetails, PaymentState};
use breez_sdk_liquid::prelude::{Payment, PaymentType, RefundableSwap};
use coincube_core::miniscript::bitcoin::Amount;
use coincube_ui::{
    component::{
        amount::DisplayAmount,
        button, card, form,
        text::*,
        transaction::{TransactionDirection, TransactionListItem, TransactionType},
    },
    icon, theme,
    widget::*,
};
use iced::{
    widget::{Column, Container, Row, Space},
    Alignment, Length,
};

use crate::app::breez::assets::{format_usdt_display, USDT_PRECISION};
use crate::app::menu::Menu;
use crate::app::view::message::{FeeratePriority, Message};
use crate::app::view::FiatAmountConverter;
use crate::export::ImportExportMessage;
use crate::utils::{format_time_ago, format_timestamp};

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
                    (info.amount * 10_f64.powi(USDT_PRECISION as i32)).round() as u64,
                )
            } else {
                format_usdt_display(payment.amount_sat)
            };
            return Some(format!("{} USDt", display));
        }
    }
    None
}

pub fn liquid_transactions_view<'a>(
    payments: &'a [Payment],
    refundables: &'a [RefundableSwap],
    _balance: &'a Amount,
    fiat_converter: Option<FiatAmountConverter>,
    _loading: bool,
    bitcoin_unit: coincube_ui::component::amount::BitcoinDisplayUnit,
    usdt_id: &'a str,
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

    if payments.is_empty() {
        // Empty state
        content = content.push(
            Column::new()
                .spacing(20)
                .width(Length::Fill)
                .align_x(Alignment::Center)
                .push(Space::new().height(Length::Fixed(100.0)))
                .push(h2("No transactions yet").style(theme::text::primary))
                .push(
                    text("Your Lightning wallet is ready. Once you send or receive\nsats, they'll show up here.")
                        .size(16)
                        .style(theme::text::secondary)
                        .wrapping(iced::widget::text::Wrapping::Word)
                        .align_x(iced::alignment::Horizontal::Center),
                )
                .push(Space::new().height(Length::Fixed(20.0)))
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
                        ))
                    },
                )),
        );
    }

    if !refundables.is_empty() {
        content = content.push(
            Column::new()
                .spacing(10)
                .push(Space::new().height(Length::Fixed(20.0)))
                .push(Container::new(h3("Refundable Transactions").bold()))
                .push(refundables.iter().enumerate().fold(
                    Column::new().spacing(10),
                    |col, (i, refundable)| {
                        col.push(refundable_row(i, refundable, fiat_converter, bitcoin_unit))
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

    let tx_type = match &payment.details {
        PaymentDetails::Lightning { .. } => TransactionType::Lightning,
        PaymentDetails::Liquid { .. } | PaymentDetails::Bitcoin { .. } => TransactionType::Bitcoin,
    };

    if let Some(ref usdt_display) = usdt_str {
        let item = TransactionListItem::new(direction, &Amount::ZERO, bitcoin_unit)
            .with_label(description.to_string())
            .with_time_ago(time_ago)
            .with_type(tx_type)
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
        .with_type(tx_type);

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
    fiat_converter: Option<FiatAmountConverter>,
    bitcoin_unit: coincube_ui::component::amount::BitcoinDisplayUnit,
) -> Element<'a, Message> {
    let btc_amount = Amount::from_sat(refundable.amount_sat);
    let time_ago = format_time_ago(refundable.timestamp.into());

    let direction = TransactionDirection::Incoming;
    let tx_type = TransactionType::Bitcoin;

    let mut item = TransactionListItem::new(direction, &btc_amount, bitcoin_unit)
        .with_label("Refundable Swap".to_string())
        .with_time_ago(time_ago)
        .with_type(tx_type);

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

    if let Some(ref usdt_display) = usdt_str {
        // USDt detail view: show USDt amount + L-BTC fees
        let usdt_num = match &payment.details {
            PaymentDetails::Liquid { asset_info, .. } => {
                if let Some(info) = asset_info {
                    format_usdt_display(
                        (info.amount * 10_f64.powi(USDT_PRECISION as i32)).round() as u64,
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
                .push(text(&usdt_num).size(H1_SIZE).bold())
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
                        .push(text(&usdt_num).size(H1_SIZE).bold())
                        .push(text("USDt").size(H1_SIZE).color(coincube_ui::color::GREY_3)),
                )
        };
        return Column::new()
            .spacing(20)
            .push(Container::new(h3(title)).width(Length::Fill))
            .push(Column::new().push(p1_regular(description)).spacing(10))
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
                            .push(Column::new().width(Length::FillPortion(2)).push(
                                match payment.status {
                                    PaymentState::Complete => {
                                        text("Complete").style(theme::text::success)
                                    }
                                    PaymentState::Pending => {
                                        text("Pending").style(theme::text::secondary)
                                    }
                                    PaymentState::Created => {
                                        text("Created").style(theme::text::secondary)
                                    }
                                    PaymentState::Failed => {
                                        text("Failed").style(theme::text::destructive)
                                    }
                                    PaymentState::TimedOut => {
                                        text("Timed Out").style(theme::text::destructive)
                                    }
                                    PaymentState::Refundable => {
                                        text("Refundable").style(theme::text::destructive)
                                    }
                                    PaymentState::RefundPending => {
                                        text("Refund Pending").style(theme::text::secondary)
                                    }
                                    PaymentState::WaitingFeeAcceptance => {
                                        text("Waiting Fee Acceptance").style(theme::text::secondary)
                                    }
                                },
                            ))
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
                    .push(
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
                            .spacing(20),
                    )
                    .spacing(15),
            ))
            .into();
    }

    Column::new()
        .spacing(20)
        .push(if is_receive {
            Container::new(h3("Incoming payment")).width(Length::Fill)
        } else {
            Container::new(h3("Outgoing payment")).width(Length::Fill)
        })
        .push(Column::new().push(p1_regular(description)).spacing(10))
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
                        .push(Column::new().width(Length::FillPortion(2)).push(
                            match payment.status {
                                PaymentState::Complete => {
                                    text("Complete").style(theme::text::success)
                                }
                                PaymentState::Pending => {
                                    text("Pending").style(theme::text::secondary)
                                }
                                PaymentState::Created => {
                                    text("Created").style(theme::text::secondary)
                                }
                                PaymentState::Failed => {
                                    text("Failed").style(theme::text::destructive)
                                }
                                PaymentState::TimedOut => {
                                    text("Timed Out").style(theme::text::destructive)
                                }
                                PaymentState::Refundable => {
                                    text("Refundable").style(theme::text::destructive)
                                }
                                PaymentState::RefundPending => {
                                    text("Refund Pending").style(theme::text::secondary)
                                }
                                PaymentState::WaitingFeeAcceptance => {
                                    text("Waiting Fee Acceptance").style(theme::text::secondary)
                                }
                            },
                        ))
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

pub fn refundable_detail_view<'a>(
    refundable: &'a RefundableSwap,
    fiat_converter: Option<FiatAmountConverter>,
    bitcoin_unit: coincube_ui::component::amount::BitcoinDisplayUnit,
    refund_address: &'a form::Value<String>,
    refund_feerate: &'a form::Value<String>,
    refunding: bool,
) -> Element<'a, Message> {
    let btc_amount = Amount::from_sat(refundable.amount_sat);

    let date_text =
        format_timestamp(refundable.timestamp as u64).unwrap_or_else(|| "Unknown".to_string());

    let can_refund = refund_address.valid
        && !refund_address.value.trim().is_empty()
        && refund_feerate.valid
        && !refund_feerate.value.trim().is_empty()
        && refund_feerate.value.parse::<u32>().is_ok();

    Column::new()
        .spacing(20)
        .push(Container::new(h3("Refundable Swap")).width(Length::Fill))
        .push(
            Column::new()
                .push(p1_regular("This swap can be refunded"))
                .spacing(10),
        )
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
                                .push(text(&refundable.swap_address)),
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
                        ),
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
                                .push(
                                    Row::new()
                                        .spacing(5)
                                        .push(
                                            button::secondary(None, "Low")
                                                .on_press(Message::RefundFeeratePrioritySelected(
                                                    FeeratePriority::Low,
                                                ))
                                                .width(Length::Fixed(80.0)),
                                        )
                                        .push(
                                            button::secondary(None, "Medium")
                                                .on_press(Message::RefundFeeratePrioritySelected(
                                                    FeeratePriority::Medium,
                                                ))
                                                .width(Length::Fixed(100.0)),
                                        )
                                        .push(
                                            button::secondary(None, "High")
                                                .on_press(Message::RefundFeeratePrioritySelected(
                                                    FeeratePriority::High,
                                                ))
                                                .width(Length::Fixed(80.0)),
                                        ),
                                ),
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
