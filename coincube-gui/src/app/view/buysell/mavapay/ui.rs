use crate::{
    app::view::{
        buysell::{panel::BuyOrSell, MavapayFlowStep, MavapayState},
        BuySellMessage, Message as ViewMessage,
    },
    services::{coincube::Country, mavapay::*},
};

use chrono::{DateTime, Local, Utc};
use iced::{widget::*, Alignment, Length};

use coincube_ui::component::{button, card, text};
use coincube_ui::{color, icon::*, theme, widget::Column};

struct CheckoutDetails {
    reference: String,
    reference_label: &'static str,
    total_fiat: f64,
    btc_amount: f64,
    currency_symbol: &'static str,
}

impl CheckoutDetails {
    fn from_quote(quote: &GetQuoteResponse, sats: u64, country: &Country) -> Self {
        let (reference, reference_label) = match &quote.order_id {
            Some(order_id) => (order_id.clone(), "Order Ref"),
            None => (quote.id.clone(), "Quote Ref"),
        };

        Self {
            reference,
            reference_label,
            total_fiat: quote.total_amount_in_source_currency as f64 / 100.0,
            btc_amount: sats as f64 / 100_000_000.0,
            currency_symbol: country.currency.symbol,
        }
    }
}

fn status_color(status: &TransactionStatus) -> iced::Color {
    match status {
        TransactionStatus::Success | TransactionStatus::Paid => color::GREEN,
        TransactionStatus::Pending => color::ORANGE,
        TransactionStatus::Expired | TransactionStatus::Failed => color::RED,
    }
}

fn order_type_info(order: &GetOrderResponse) -> (&'static str, iced::Color) {
    if matches!(order.currency, MavapayCurrency::Bitcoin) {
        ("BUY", color::GREEN)
    } else {
        ("SELL", color::RED)
    }
}

fn badge<'a>(
    label: impl ToString,
    badge_color: iced::Color,
) -> iced::widget::Container<'a, BuySellMessage, theme::Theme> {
    container(text::p2_bold(label.to_string()).color(badge_color))
        .padding(iced::Padding::from([4, 8]))
        .style(move |_| container::Style {
            background: Some(iced::Background::Color(iced::Color {
                a: 0.15,
                ..badge_color
            })),
            border: iced::Border {
                radius: 8.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
}

fn success_icon_badge() -> iced::widget::Container<'static, BuySellMessage, theme::Theme> {
    container(check_icon().size(16).style(theme::text::success))
        .padding(8)
        .style(|_| container::Style {
            background: Some(iced::Background::Color(iced::color!(0x2FC455, 0.18))),
            border: iced::Border {
                radius: 25.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
}

pub fn form<'a>(state: &'a MavapayState) -> iced::Element<'a, ViewMessage, theme::Theme> {
    let form = match &state.step {
        MavapayFlowStep::Transaction { .. } => transactions_form,
        MavapayFlowStep::Checkout { .. } => checkout_form,
        MavapayFlowStep::History { .. } => history_view,
        MavapayFlowStep::OrderDetail { .. } => order_detail_view,
    };

    let element: iced::Element<'a, BuySellMessage, theme::Theme> = form(state).into();
    element.map(ViewMessage::BuySell)
}

fn checkout_form<'a>(state: &'a MavapayState) -> Column<'a, BuySellMessage> {
    let MavapayFlowStep::Checkout {
        buy_or_sell,
        fulfilled_order,
        quote,
        sat_amount,
        country,
        ..
    } = &state.step
    else {
        unreachable!()
    };

    let details = CheckoutDetails::from_quote(quote, *sat_amount, country);

    match fulfilled_order {
        None => {
            iced::widget::column![
                text::h4_bold("Checkout"),
                match buy_or_sell {
                    BuyOrSell::Buy { address: _ } => {
                        container(
                        iced::widget::column![
                            text::p1_bold("Complete Your Order"),
                            text::p1_medium("Review your order details carefully before confirming your purchase. Once confirmed, your Bitcoin will be delivered to your wallet.")
                                .color(color::GREY_2),
                            Space::new().height(15),
                            container(
                                iced::widget::column![
                                    summary_card(&details),
                                    instructions_card(quote, &details),
                                    notes_card()
                                ].spacing(10)
                            )
                            .padding(10)
                            .style(|t| container::Style {
                                border: iced::Border {
                                    radius: 25.0.into(),
                                    ..Default::default()
                                },
                                ..theme::container::background(t)
                            })
                        ])
                        .padding(20)
                        .style(theme::card::simple)
                    }
                    BuyOrSell::Sell => {
                        // TODO: display bitcoin address or lightning invoice for deposit, and beneficiary input forms
                        container(text::p1_italic(
                            "Display lightning invoice or bitcoin address for deposit here...",
                        ))
                    },
                }
            ]
            .push(Space::new().height(10))
            .push(
                (cfg!(debug_assertions) && option_env!("MAVAPAY_API_KEY").is_none()).then(|| {
                    button::primary(Some(wrench_icon()), "Simulate Pay-In (Developer Option)")
                        .on_press_maybe(
                            fulfilled_order
                                .is_none()
                                .then_some(BuySellMessage::Mavapay(MavapayMessage::SimulatePayIn)),
                        )
                }),
            )
        }
        Some(order) => {
            order_success_view(buy_or_sell, order, &details)
        }
    }
    .align_x(Alignment::Center)
    .width(600)
}

fn transactions_form<'a>(state: &'a MavapayState) -> Column<'a, BuySellMessage> {
    let MavapayFlowStep::Transaction {
        sat_amount,
        btc_price: current_price,
        buy_or_sell,
        country,
        transfer_speed,
        sending_quote,
        ..
    } = &state.step
    else {
        unreachable!()
    };

    let input_form = match current_price {
        Some(price) => Container::new(
            iced::widget::column![
                Space::new().height(17),
                iced::widget::row![
                    iced::widget::column![
                        text(format!(
                            "{} ({})",
                            country.currency.name, country.currency.code
                        ))
                        .size(14)
                        .color(color::GREY_2),
                        Space::new().height(5),
                        iced_aw::number_input(
                            &{
                                *sat_amount as f64
                                    * (price.btc_price_in_unit_currency / 100_000_000.0)
                            }
                            .round(),
                            ..,
                            |a| { BuySellMessage::Mavapay(MavapayMessage::FiatAmountChanged(a)) }
                        )
                        .on_submit(BuySellMessage::Mavapay(MavapayMessage::NormalizeAmounts))
                        .align_x(Alignment::Center)
                        .step(500.0)
                        .width(150)
                        .set_size(18)
                        .padding(10)
                    ]
                    .align_x(Alignment::Center),
                    container(left_right_icon().size(20).center()).padding(12),
                    iced::widget::column![
                        text("Satoshis (BTCSAT)").size(14).color(color::GREY_2),
                        Space::new().height(5),
                        iced_aw::number_input(&(*sat_amount as f64), .., |a| {
                            BuySellMessage::Mavapay(MavapayMessage::SatAmountChanged(a))
                        })
                        .on_submit(BuySellMessage::Mavapay(MavapayMessage::NormalizeAmounts))
                        .align_x(Alignment::Center)
                        .step(1000.0)
                        .width(150)
                        .set_size(18)
                        .padding(10)
                    ]
                    .align_x(Alignment::Center)
                ]
                .align_y(Alignment::End)
                .spacing(20)
                .padding(0),
                iced::widget::row![
                    Space::new().width(Length::Fill),
                    text::p1_medium("Select an onchain transfer speed: ")
                        .width(Length::Shrink)
                        .center(),
                    iced::widget::pick_list(
                        OnchainTransferSpeed::all(),
                        Some(transfer_speed),
                        |s| { BuySellMessage::Mavapay(MavapayMessage::TransferSpeedChanged(s)) }
                    )
                    .padding(10)
                    .width(100),
                    Space::new().width(Length::Fill),
                ]
                .width(Length::Fill)
                .align_y(Alignment::Center),
                Space::new().height(20),
                match buy_or_sell {
                    // TODO: ensure user has BTC balance to satisfy quote
                    BuyOrSell::Sell => {
                        // TODO: onchain sell currently unsupported, lightning integration will be required to proceed
                        button::primary(Some(send_icon()), "Send Bitcoin (Currently Unsupported)")
                    }
                    BuyOrSell::Buy { .. } => match sending_quote {
                        true => button::primary(Some(reload_icon()), "Processing Quote..."),
                        false => {
                            button::primary(Some(card_icon()), "Proceed to Payment")
                                .on_press(BuySellMessage::Mavapay(MavapayMessage::CreateQuote))
                        }
                    },
                }
                .width(Length::Fill)
            ]
            .spacing(10)
            .align_x(Alignment::Center)
            .width(Length::Fill),
        ),
        None => Container::new(
            text::p1_italic("Currently loading BTC price, please wait")
                .width(Length::Fill)
                .center(),
        )
        .align_y(Alignment::Center)
        .align_x(Alignment::Center),
    }
    .padding(15)
    .style(theme::card::simple)
    .width(Length::Fixed(600.0));

    // combine UI, render beneficiary input form using card styling
    iced::widget::column![
        // separator
        Space::new().height(Length::Fixed(5.0)),
        container(Space::new().height(Length::Fixed(4.0)).width(Length::Fill))
            .style(theme::card::simple),
        Space::new().height(Length::Fixed(5.0)),
        // header text
        text::h4_bold(match buy_or_sell {
            BuyOrSell::Sell => "Sell Bitcoin to Fiat Money",
            BuyOrSell::Buy { .. } => "Buy Bitcoin using Fiat Money",
        })
        .color(color::WHITE)
        .center(),
        Space::new().height(Length::Fixed(20.0)),
        input_form,
        Space::new().height(Length::Fixed(5.0)),
        text::p2_medium("Powered by Mavapay").color(color::GREY_3)
    ]
    .align_x(Alignment::Center)
}

fn detail_row<'a>(
    label: &'a str,
    value: String,
    text_color: Option<iced::Color>,
) -> iced::widget::Row<'a, BuySellMessage, theme::Theme> {
    iced::widget::row![
        iced::widget::column![
            text::p2_medium(label).color(color::GREY_2),
            text::p2_bold(&value).color(text_color.unwrap_or(color::WHITE))
        ]
        .width(Length::Fill),
        Button::new(clipboard_icon().style(theme::text::secondary))
            .on_press(BuySellMessage::Clipboard(value))
            .style(theme::button::transparent)
    ]
    .spacing(10)
    .align_y(Alignment::Center)
}

fn summary_card<'a>(
    details: &CheckoutDetails,
) -> iced::widget::Container<'a, BuySellMessage, theme::Theme> {
    let CheckoutDetails {
        reference,
        reference_label,
        total_fiat,
        btc_amount,
        currency_symbol,
    } = details;

    card::simple(
        iced::widget::column![
            iced::widget::row![
                success_icon_badge(),
                Space::new().width(10),
                iced::widget::column![
                    text::p1_bold("Order Created Successfully"),
                    text::p2_medium(format!("{}: {}", reference_label, reference))
                        .color(color::GREY_2)
                ]
            ]
            .align_y(Alignment::Center),
            Space::new().height(15),
            iced::widget::row![
                iced::widget::column![
                    text::p2_medium("Order Amount").color(color::GREY_2),
                    text::p1_bold(format!("{}{}", currency_symbol, total_fiat))
                ]
                .width(Length::Fill),
                iced::widget::column![
                    text::p2_medium("Bitcoin Expected").color(color::GREY_2),
                    text::p1_bold(format!("{:.8} BTC", btc_amount))
                ]
                .width(Length::Fill),
            ],
            Space::new().height(10),
            iced::widget::row![iced::widget::column![
                text::p2_medium("Order Status").color(color::GREY_2),
                iced::widget::row![clock_icon(), text::p1_bold("PENDING")]
            ]]
        ]
        .width(Length::Fill)
        .padding(15),
    )
    .width(Length::Fill)
}

fn instructions_card<'a>(
    quote: &GetQuoteResponse,
    details: &CheckoutDetails,
) -> iced::widget::Container<'a, BuySellMessage, theme::Theme> {
    let CheckoutDetails {
        reference,
        total_fiat,
        currency_symbol,
        ..
    } = details;
    let account_number = quote.ngn_bank_account_number.clone();

    card::simple(
        iced::widget::column![
            iced::widget::row![
                container(
                    cash_icon().size(16).color(iced::color![0x000DFF]),
                ).padding(8).style(|_| container::Style {
                    background: Some(iced::Background::Color(iced::color![0x000DFF, 0.14])),
                    border: iced::Border {
                        radius: 25.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
                Space::new().width(10),
                iced::widget::column![
                text::p1_bold("Payment Instructions"),
                text::p2_medium("Follow these steps to complete your order").color(color::GREY_2)
            ]
            ]
            .align_y(Alignment::Center),
            Space::new().height(15),
            text::p2_medium("STEP 1: TRANSFER FUNDS TO OUR ACCOUNT")
            .color(color::GREY_2),
            Space::new().height(10),
            quote.bank_name.clone().map(|bank_name|
                detail_row("Bank Name", bank_name, None)
            ),
            Space::new().height(20),
            account_number.clone().map(|account_number|
                detail_row("Account Number", account_number, None)
            ),
            Space::new().height(20),
            quote.ngn_account_name.clone().map(|account_name|
                detail_row("Account Name", account_name, None)
            ),
            Space::new().height(20),
            detail_row(
                "Amount to Send",
                format!("{}{}", currency_symbol, total_fiat),
                Some(color::GREEN),
            ),
            Space::new().height(20),
            text::p2_medium("STEP 2: INCLUDE THIS REFERENCE IN YOUR TRANSFER")
                .color(color::GREY_2),
            Space::new().height(10),
            card::simple(
                iced::widget::column![
                iced::widget::row![
                    warning_icon().size(20).style(theme::text::warning),
                    Space::new().width(10),
                    text::p2_medium("Critical: Include this reference number").style(theme::text::warning),
                ].align_y(Alignment::Center),
                Space::new().height(20),
                iced::widget::row![
                    text::h4_bold(reference.clone()),
                    Button::new(clipboard_icon().style(theme::text::secondary))
                        .on_press(BuySellMessage::Clipboard(reference.clone()))
                        .style(theme::button::transparent),
                ].align_y(Alignment::Center),
                Space::new().height(20),
                text::p2_medium("This helps us match your payment to your order. Without this reference, your order may be delayed.")
                .color(color::GREY_2)
                ].width(Length::Fill)
            ).style(theme::card::modal),
            Space::new().height(20),
            text::p2_medium("STEP 3: WAIT FOR CONFIRMATION"),
            Space::new().height(10),
            iced::widget::row![
                reload_icon().size(16).style(theme::text::secondary),
                Space::new().width(10),
                text::p2_medium("Waiting for payment confirmation...")
                    .color(color::GREY_2)
            ].align_y(Alignment::Center),
            Space::new().height(10),
            button::primary(Some(reload_icon()), "Start Over")
                .on_press(BuySellMessage::ResetWidget)
        ].width(Length::Fill).padding(15)
    ).width(Length::Fill)
}

fn notes_card<'a>() -> iced::widget::Container<'a, BuySellMessage, theme::Theme> {
    card::simple(
        iced::widget::column![
            text::p1_bold("Important Notes"),
            Space::new().height(10),
            note_item("Your order will begin execution once we confirm receipt of funds"),
            note_item("Execution time will depend on market liquidity"),
            note_item("You will receive real-time updates on trade execution progress"),
            note_item("Final Bitcoin price may vary based on actual execution prices"),
            note_item("Our commission (1-2%) will be deducted from the final Bitcoin amount"),
        ]
        .width(Length::Fill)
        .padding(15),
    )
}

fn note_item<'a>(content: &str) -> iced::widget::Row<'a, BuySellMessage, theme::Theme> {
    iced::widget::row![
        dot_icon().size(4).color(color::ORANGE),
        Space::new().width(8),
        text::p2_medium(content)
    ]
    .align_y(Alignment::Center)
}

fn order_success_view<'a>(
    buy_or_sell: &BuyOrSell,
    order: &GetOrderResponse,
    details: &CheckoutDetails,
) -> Column<'a, BuySellMessage> {
    let (title, subtitle) = match buy_or_sell {
        BuyOrSell::Sell => (
            "Withdrawal Complete",
            "Your Bitcoin has been successfully sent to your wallet.",
        ),
        BuyOrSell::Buy { .. } => (
            "Purchase Complete",
            "Your Bitcoin has been successfully sent to your wallet",
        ),
    };

    iced::widget::column![
        text::h4_bold("Order Confirmation"),
        Space::new().height(10),
        container(iced::widget::column![
            card::simple(
                iced::widget::column![iced::widget::row![
                    success_icon_badge(),
                    Space::new().width(15),
                    iced::widget::column![
                        text::h4_bold(title),
                        text::p2_medium(subtitle).color(color::GREY_2)
                    ]
                ]
                .align_y(Alignment::Center)]
                .width(Length::Fill)
                .padding(20)
            )
            .width(Length::Fill),
            Space::new().height(10),
            card::simple(
                iced::widget::column![
                    text::p1_bold("Order Details"),
                    Space::new().height(15),
                    detail_row(details.reference_label, order.id.clone(), None),
                    Space::new().height(15),
                    iced::widget::row![
                        iced::widget::column![
                            text::p2_medium("Amount Paid").color(color::GREY_2),
                            text::p1_bold(format!(
                                "{}{}",
                                details.currency_symbol, details.total_fiat
                            ))
                        ]
                        .width(Length::Fill),
                        iced::widget::column![
                            text::p2_medium("Bitcoin Received").color(color::GREY_2),
                            text::p1_bold(format!("{:.8} BTC", details.btc_amount))
                        ]
                        .width(Length::Fill)
                    ],
                    Space::new().height(15),
                    iced::widget::row![
                        iced::widget::column![
                            text::p2_medium("Order Status").color(color::GREY_2),
                            text::p2_bold(&order.status).color(status_color(&order.status))
                        ]
                        .width(Length::Fill),
                        iced::widget::column![
                            text::p2_medium("Payment Method").color(color::GREY_2),
                            text::p1_bold(format!("{}", order.payment_method))
                        ]
                        .width(Length::Fill)
                    ],
                    iced::widget::column![
                        Space::new().height(15),
                        text::p2_medium("Order Date").color(color::GREY_2),
                        text::p2_bold(pretty_timestamp(&order.created_at))
                    ]
                ]
                .width(Length::Fill)
                .padding(20)
            )
            .width(Length::Fill),
            Space::new().height(10),
            card::simple(
                iced::widget::column![
                    text::p2_medium("Thank you for using Mavapay!")
                        .color(color::GREY_2)
                        .center()
                        .width(Length::Fill),
                    Space::new().height(15),
                    button::primary(Some(reload_icon()), "Start New Transaction")
                        .on_press(BuySellMessage::ResetWidget)
                        .width(Length::Fill)
                ]
                .width(Length::Fill)
                .padding(20)
            )
            .width(Length::Fill)
        ])
        .padding(10)
    ]
}

fn history_view<'a>(state: &'a MavapayState) -> Column<'a, BuySellMessage> {
    let MavapayFlowStep::History {
        orders,
        loading,
        error,
    } = &state.step
    else {
        unreachable!()
    };

    let content: iced::Element<'a, BuySellMessage, theme::Theme> = match (loading, orders, error) {
        (true, _, _) => container(
            iced::widget::column![
                reload_icon().size(24).style(theme::text::secondary),
                Space::new().height(10),
                text::p2_medium("Loading order history...").color(color::GREY_2)
            ]
            .align_x(Alignment::Center),
        )
        .padding(40)
        .width(Length::Fill)
        .align_x(Alignment::Center)
        .into(),
        (false, _, Some(err)) => card::simple(
            iced::widget::column![
                iced::widget::row![
                    warning_icon().size(20).style(theme::text::warning),
                    Space::new().width(10),
                    text::p1_bold("Failed to load order history")
                ]
                .align_y(Alignment::Center),
                Space::new().height(10),
                text::p2_medium(err).color(color::GREY_2),
                Space::new().height(15),
                button::primary(Some(reload_icon()), "Retry")
                    .on_press(BuySellMessage::Mavapay(MavapayMessage::FetchOrders))
            ]
            .padding(20)
            .align_x(Alignment::Center)
            .width(Length::Fill),
        )
        .width(Length::Fill)
        .into(),
        (false, Some(order_list), None) if order_list.is_empty() => card::simple(
            iced::widget::column![
                history_icon().size(48).style(theme::text::secondary),
                Space::new().height(15),
                text::p1_bold("No orders found"),
                Space::new().height(5),
                text::p2_medium("Your orders will appear here once you buy or sell bitcoin.")
                    .color(color::GREY_2)
            ]
            .padding(40)
            .align_x(Alignment::Center)
            .width(Length::Fill),
        )
        .width(Length::Fill)
        .into(),
        (false, Some(order_list), None) => order_list
            .iter()
            .fold(iced::widget::column![].spacing(10), |col, order| {
                col.push(order_row(order))
            })
            .width(Length::Fill)
            .into(),
        (false, None, None) => container(text::p2_medium("Unexpected state")).into(),
    };

    iced::widget::column![
        iced::widget::button(
            iced::widget::row![
                previous_icon().size(16).color(color::GREY_2),
                Space::new().width(5),
                text::p2_medium("Back").color(color::GREY_2)
            ]
            .align_y(Alignment::Center)
        )
        .style(theme::button::transparent)
        .on_press(BuySellMessage::ResetWidget),
        Space::new().height(10),
        text::h4_bold("Order History"),
        content
    ]
    .padding(20)
}

fn pretty_timestamp(ts: &str) -> String {
    ts.parse::<DateTime<Utc>>()
        .ok()
        .map(|dt| {
            let local = dt
                .with_timezone(&Local)
                .format("%Y-%m-%d %H:%M:%S %:z")
                .to_string();
            local
        })
        .unwrap_or_else(|| "unknown".into())
}

fn format_order_amount(order: &GetOrderResponse) -> String {
    match order.currency {
        MavapayCurrency::Bitcoin => format!("{:.8} BTC", order.amount as f64 / 100_000_000.0),
        MavapayCurrency::KenyanShilling => format!("{:.2} KSh", order.amount as f64 / 100.0),
        MavapayCurrency::SouthAfricanRand => format!("{:.2} ZAR", order.amount as f64 / 100.0),
        MavapayCurrency::NigerianNaira => format!("{:.2} NGN", order.amount as f64 / 100.0),
    }
}

fn order_row<'a>(
    order: &'a GetOrderResponse,
) -> iced::widget::Container<'a, BuySellMessage, theme::Theme> {
    let (order_type, order_type_color) = order_type_info(order);
    let order_status_color = status_color(&order.status);

    card::simple(
        iced::widget::column![
            iced::widget::row![
                iced::widget::column![
                    text::p2_medium("Order ID").color(color::GREY_2),
                    text::p2_bold(&order.id)
                ]
                .width(Length::Fill),
                badge(order_type, order_type_color),
                Space::new().width(8),
                badge(&order.status, order_status_color)
            ]
            .align_y(Alignment::Center),
            Space::new().height(12),
            iced::widget::row![
                iced::widget::column![
                    text::p2_medium("Amount").color(color::GREY_2),
                    text::p2_bold(format_order_amount(order))
                ]
                .width(Length::Fill),
                iced::widget::column![
                    text::p2_medium("Payment").color(color::GREY_2),
                    text::p2_bold(&order.payment_method)
                ]
                .width(Length::Fill),
                iced::widget::column![
                    text::p2_medium("Order Date").color(color::GREY_2),
                    text::p2_bold(pretty_timestamp(&order.created_at))
                ]
                .width(Length::Fill),
                button::secondary(None, "View")
                    .on_press(BuySellMessage::Mavapay(MavapayMessage::SelectOrder(
                        order.clone()
                    )))
                    .width(80)
            ]
            .align_y(Alignment::Center)
        ]
        .padding(15)
        .width(Length::Fill),
    )
    .width(Length::Fill)
}

fn info_field<'a>(
    label: &'a str,
    value: impl ToString,
) -> iced::widget::Column<'a, BuySellMessage, theme::Theme> {
    iced::widget::column![
        text::p2_medium(label).color(color::GREY_2),
        text::p2_bold(value.to_string())
    ]
    .width(Length::Fill)
}

fn format_currency_amount(amount: u64, currency: &MavapayUnitCurrency) -> String {
    match currency {
        MavapayUnitCurrency::KenyanShillingCent => format!("{:.2} KES", amount as f64 / 100.0),
        MavapayUnitCurrency::SouthAfricanRandCent => format!("{:.2} ZAR", amount as f64 / 100.0),
        MavapayUnitCurrency::NigerianNairaKobo => format!("{:.2} NGN", amount as f64 / 100.0),
        MavapayUnitCurrency::BitcoinSatoshi => format!("{:.8} BTC", amount as f64 / 100_000_000.0),
    }
}

fn order_detail_view<'a>(state: &'a MavapayState) -> Column<'a, BuySellMessage> {
    let MavapayFlowStep::OrderDetail { order } = &state.step else {
        unreachable!()
    };

    let (order_type, order_type_color) = order_type_info(order);
    let order_status_color = status_color(&order.status);

    let Some(quote) = &order.quote else {
        unreachable!()
    };
    let (paid_amount, received_amount) = (
        format_currency_amount(quote.total_amount, &quote.source_currency),
        format_currency_amount(quote.equivalent_amount, &quote.target_currency),
    );

    let back_button = iced::widget::button(
        iced::widget::row![
            previous_icon().size(16).color(color::GREY_2),
            Space::new().width(5),
            text::p2_medium("Back to History").color(color::GREY_2)
        ]
        .align_y(Alignment::Center),
    )
    .style(theme::button::transparent)
    .on_press(BuySellMessage::Mavapay(MavapayMessage::BackToHistory));

    let header = iced::widget::row![
        text::h4_bold("Order Details"),
        Space::new().width(Length::Fill),
        badge(order_type, order_type_color),
        Space::new().width(8),
        badge(&order.status, order_status_color)
    ]
    .align_y(Alignment::Center);

    let summary_card = card::simple(
        iced::widget::column![
            text::p1_bold("Transaction Summary"),
            Space::new().height(15),
            detail_row("Order ID", order.id.clone(), None),
            Space::new().height(8),
            iced::widget::row![
                info_field("Amount Paid", &paid_amount),
                info_field("Amount Received", &received_amount),
            ],
            Space::new().height(8),
            iced::widget::row![
                info_field("Payment Method", &order.payment_method),
                info_field("Order Date", pretty_timestamp(&order.created_at)),
            ]
        ]
        .padding(20)
        .width(Length::Fill),
    )
    .width(Length::Fill);

    let fee_card = card::simple(
        iced::widget::column![
            text::p1_bold("Fee Breakdown"),
            Space::new().height(15),
            iced::widget::row![
                info_field(
                    "Source Currency Fee",
                    format_currency_amount(
                        quote.transaction_fees_in_source_currency,
                        &quote.source_currency
                    )
                ),
                info_field(
                    "Target Currency Fee",
                    format_currency_amount(
                        quote.transaction_fees_in_target_currency,
                        &quote.target_currency
                    ),
                ),
                info_field(
                    "Fee (USD)",
                    format!("{:.2}", quote.transaction_fees_in_usd_cent as f64 / 100.0)
                ),
            ],
        ]
        .padding(20)
        .width(Length::Fill),
    )
    .width(Length::Fill);

    let rate_card = card::simple(
        iced::widget::column![
            text::p1_bold("Exchange Rates"),
            Space::new().height(15),
            iced::widget::row![
                info_field("BTC/USD Rate", quote.btc_usd_rate),
                info_field(
                    "USD Equivalent",
                    format!("{:.2}", quote.usd_amount as f64 / 100.0)
                ),
            ]
        ]
        .padding(20)
        .width(Length::Fill),
    )
    .width(Length::Fill);

    let payment_card = card::simple(
        iced::widget::column![
            text::p1_bold("Payment Details"),
            Space::new().height(15),
            detail_row("Bitcoin Address", quote.payment_btc_detail.clone(), None),
        ]
        .padding(20)
        .width(Length::Fill),
    )
    .width(Length::Fill);

    let timestamp_card = card::simple(
        iced::widget::column![
            text::p1_bold("Timestamps"),
            Space::new().height(15),
            iced::widget::row![
                info_field("Created At", pretty_timestamp(&quote.created_at)),
                info_field("Updated At", pretty_timestamp(&quote.updated_at)),
                info_field("Expiry", pretty_timestamp(&quote.expiry)),
            ]
        ]
        .padding(20)
        .width(Length::Fill),
    )
    .width(Length::Fill);

    iced::widget::column![
        back_button,
        Space::new().height(15),
        header,
        Space::new().height(20),
        summary_card,
        Space::new().height(10),
        fee_card,
        Space::new().height(10),
        rate_card,
        Space::new().height(10),
        payment_card,
        Space::new().height(10),
        timestamp_card,
    ]
    .padding(20)
    .width(Length::Fill)
}
