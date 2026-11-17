use crate::{
    app::view::{
        buysell::{mavapay::SellFulfilment, panel::BuyOrSell, MavapayFlowStep, MavapayState},
        BuySellMessage, Message as ViewMessage,
    },
    services::{coincube::Country, mavapay::*},
};
use coincube_ui::locale::copy as localized_copy;
use coincube_ui::localized_format;

use iced::{widget, Alignment, Length};

use coincube_ui::component::amount::{format_f64_as_string, format_u64_as_string};
use coincube_ui::component::{button, card, text};
use coincube_ui::{color, icon::*, theme};

pub fn form<'a>(
    state: &'a MavapayState,
    mask_amounts: bool,
) -> iced::Element<'a, ViewMessage, theme::Theme> {
    // The eye covers saved orders and completed checkout. Entry and an
    // unfulfilled checkout show amounts for active payment review.
    let form = match state
        .steps
        .last()
        .expect("`MavapayState` must have at least one flow-step")
    {
        MavapayFlowStep::BuyInputFrom { .. } => buy_input_form(state),
        MavapayFlowStep::SellInputForm { .. } => sell_input_form(state),
        MavapayFlowStep::Checkout { .. } => checkout_form(state, mask_amounts),
        MavapayFlowStep::History { .. } => history_view(state, mask_amounts),
        MavapayFlowStep::OrderDetail { .. } => order_detail_view(state, mask_amounts),
    };

    let element: iced::Element<'a, BuySellMessage, theme::Theme> = widget::column![
        form,
        widget::Space::new().height(Length::Fixed(5.0)),
        text::caption(localized_copy("Powered by Mavapay")).style(theme::text::secondary)
    ]
    .align_x(iced::Alignment::Center)
    .into();

    element.map(ViewMessage::BuySell)
}

fn buy_input_form<'a>(state: &'a MavapayState) -> widget::Column<'a, BuySellMessage, theme::Theme> {
    let Some(MavapayFlowStep::BuyInputFrom {
        getting_invoice,
        sending_quote,
        ..
    }) = state.steps.last()
    else {
        unreachable!()
    };

    let form = match state.btc_price {
        Some(price) => widget::container(
            widget::column![
                widget::row![
                    widget::column![
                        widget::text(format!(
                            "{} ({})",
                            state.country.currency.name, state.country.currency.code
                        ))
                        .size(14)
                        .style(theme::text::secondary),
                        widget::Space::new().height(5),
                        iced_aw::number_input(
                            &{ state.sat_amount as f64 * (price / 100_000_000.0) }.round(),
                            ..,
                            |a| { BuySellMessage::Mavapay(MavapayMessage::FiatAmountChanged(a,)) }
                        )
                        .ignore_buttons(true)
                        .ignore_scroll(true)
                        .on_submit(BuySellMessage::Mavapay(MavapayMessage::NormalizeAmounts))
                        .align_x(Alignment::Center)
                        .step(500.0)
                        .width(150)
                        .set_size(18)
                        .padding(10)
                    ]
                    .align_x(Alignment::Center),
                    widget::container(left_right_icon().size(20).center()).padding(12),
                    widget::column![
                        widget::text(localized_copy("Satoshis (BTCSAT)"))
                            .size(14)
                            .style(theme::text::secondary),
                        widget::space().height(5),
                        iced_aw::number_input(&state.sat_amount, .., |a| {
                            BuySellMessage::Mavapay(MavapayMessage::SatAmountChanged(a as _))
                        })
                        .ignore_buttons(true)
                        .ignore_scroll(true)
                        .on_submit(BuySellMessage::Mavapay(MavapayMessage::NormalizeAmounts))
                        .align_x(Alignment::Center)
                        .step(1000)
                        .width(150)
                        .set_size(18)
                        .padding(10)
                    ]
                    .align_x(Alignment::Center)
                ]
                .align_y(Alignment::End)
                .spacing(20)
                .padding(0),
                match getting_invoice {
                    true => button::secondary(
                        Some(clock_icon()),
                        localized_copy("[1] Getting Invoice..."),
                    ),
                    false => match sending_quote {
                        true => button::secondary(
                            Some(clock_icon()),
                            localized_copy("[2] Getting Quote..."),
                        ),
                        false =>
                            button::primary(Some(card_icon()), localized_copy("Generate Invoice"))
                                .on_press(BuySellMessage::Mavapay(
                                    MavapayMessage::GenerateLightningInvoice
                                )),
                    },
                }
                .width(Length::Fill)
            ]
            .spacing(10)
            .align_x(iced::Alignment::Center)
            .width(Length::Fill),
        ),
        None => widget::container(
            text::p1_italic(localized_copy(
                "Getting recent conversion rates, please wait",
            ))
            .width(Length::Fill)
            .center(),
        )
        .align_y(Alignment::Center)
        .align_x(Alignment::Center),
    }
    .padding(15)
    .style(theme::card::simple)
    .width(Length::Fixed(600.0));

    let previous_btn = button::secondary(Some(previous_icon()), localized_copy("Back"))
        .width(Length::Fixed(150.0))
        .on_press(BuySellMessage::Mavapay(MavapayMessage::NavigateBack));

    widget::column![
        widget::container(previous_btn).width(Length::Fill),
        // header text
        text::h4_bold(localized_copy("Buy Bitcoin using Fiat Money")).center(),
        widget::Space::new().height(Length::Fixed(20.0)),
        form,
    ]
    .align_x(Alignment::Center)
}

fn sell_input_form<'a>(
    state: &'a MavapayState,
) -> widget::Column<'a, BuySellMessage, theme::Theme> {
    let Some(MavapayFlowStep::SellInputForm {
        banks,
        beneficiary,
        sending_quote,
        ..
    }) = state.steps.last()
    else {
        unreachable!()
    };

    let input_field = |value: &'a str, caption: &'static str, field: &'static str| {
        widget::column![
            text::caption(localized_copy(caption)),
            widget::text_input("...", value)
                .size(20)
                .padding(10)
                .on_input(move |b| {
                    BuySellMessage::Mavapay(MavapayMessage::BeneficiaryFieldUpdate(field, b))
                })
        ]
        .spacing(0)
    };

    let sat_amount_input = |sat_amount: u64,
                            price: Option<f64>|
     -> Option<widget::Column<'a, BuySellMessage, theme::Theme>> {
        price.map(|p| {
            widget::column![
                text::caption(localized_copy("Input Transfer Amount (in Satoshis)")),
                widget::row![
                    iced_aw::number_input(&sat_amount, .., |a| BuySellMessage::Mavapay(
                        MavapayMessage::SatAmountChanged(a as _)
                    ))
                    .ignore_buttons(true)
                    .ignore_scroll(true)
                    .on_submit(BuySellMessage::Mavapay(MavapayMessage::NormalizeAmounts))
                    .align_x(Alignment::Center)
                    .width(150)
                    .set_size(20)
                    .font(iced::Font::MONOSPACE),
                    widget::text("≈"),
                    widget::text(format!(
                        "{} {}",
                        state.country.currency.symbol,
                        (sat_amount as f64 * (p / 100_000_000.0)).round()
                    ))
                    .center()
                    .font(iced::Font::MONOSPACE),
                ]
                .spacing(7)
                .align_y(iced::Alignment::Center)
            ]
            .spacing(0)
        })
    };

    let mut validation_message = None;

    let form = match beneficiary {
        Beneficiary::NGN {
            bank_account_number,
            bank_account_name,
            bank_code,
            ..
        } => {
            if bank_account_number.parse::<usize>().is_err() {
                validation_message = Some("Bank Account Number MUST be a number");
            } else if bank_code.is_empty() {
                validation_message = Some("Select a recipient bank");
            } else if bank_account_name.is_none() {
                validation_message = Some("Verify your bank account details to continue");
            }

            widget::column![
                text::h3(localized_copy("Setup Bank Details (Nigeria)")),
                widget::container(widget::Space::default().width(iced::Length::Fill).height(2))
                    .style(theme::card::border),
                widget::space().width(iced::Length::Fill),
                sat_amount_input(state.sat_amount, state.btc_price),
                input_field(
                    bank_account_number,
                    "Enter Recipient Bank Account Number",
                    "NGN.bank_account_number"
                ),
                match banks {
                    Some(MavapayBanks::Nigerian(banks)) => {
                        widget::column![
                            text::caption(localized_copy("Select Recipient Bank")),
                            widget::row![
                                widget::pick_list(
                                    banks.as_slice(),
                                    banks.iter().find(|b| b.nip_bank_code == *bank_code),
                                    |b| {
                                        BuySellMessage::Mavapay(
                                            MavapayMessage::BeneficiaryFieldUpdate(
                                                "NGN.bank_code",
                                                b.nip_bank_code,
                                            ),
                                        )
                                    },
                                )
                                .width(iced::Length::Fill)
                                .text_size(16)
                                .padding(10),
                                widget::button(localized_copy("Verify Details"))
                                    .padding(10)
                                    .on_press_maybe(
                                        (!(bank_account_number.is_empty() || bank_code.is_empty()))
                                            .then_some(BuySellMessage::Mavapay(
                                                MavapayMessage::VerifyNgnBankDetails
                                            ))
                                    )
                            ]
                            .align_y(iced::Alignment::Center)
                            .spacing(5),
                        ]
                    }
                    Some(MavapayBanks::SouthAfrican(_)) => unreachable!(),
                    None => widget::column![localized_copy("loading banks...")]
                        .align_x(iced::Alignment::Center),
                }
                .spacing(0),
                bank_account_name.as_ref().map(|s| {
                    widget::column![
                        text::caption(localized_copy("Is this the recipient's registered name?"))
                            .style(theme::text::success),
                        widget::container(widget::text(s).size(20))
                            .padding(8)
                            .style(|th| {
                                theme::card::modal(th)
                                    .border(iced::Border::default().width(2).rounded(2))
                            })
                    ]
                })
            ]
        }
        Beneficiary::ZAR {
            name,
            bank_name,
            bank_account_number,
        } => {
            if bank_account_number.parse::<usize>().is_err() {
                validation_message = Some("Bank Account Number MUST be a number");
            } else if bank_name.is_empty() {
                validation_message = Some("Select the recipient's bank");
            } else if name.is_empty() {
                validation_message = Some("Set the recipient's legal name");
            }

            widget::column![
                text::h3(localized_copy("Setup Bank Details (South Africa)")),
                widget::container(widget::Space::default().width(iced::Length::Fill).height(2))
                    .style(theme::card::border),
                widget::space().width(iced::Length::Fill),
                sat_amount_input(state.sat_amount, state.btc_price),
                input_field(name, "Enter the Recipient's Name", "ZAR.name"),
                input_field(
                    bank_account_number,
                    "Enter Recipient Bank Account Number",
                    "ZAR.bank_account_number"
                ),
                widget::space().height(5),
                match banks {
                    Some(MavapayBanks::SouthAfrican(banks)) => {
                        widget::column![
                            text::caption(localized_copy("Select Recipient Bank")),
                            widget::pick_list(
                                banks.as_slice(),
                                banks.iter().find(|b| *b == bank_name),
                                |b| {
                                    BuySellMessage::Mavapay(MavapayMessage::BeneficiaryFieldUpdate(
                                        "ZAR.bank_name",
                                        b,
                                    ))
                                },
                            )
                            .padding(10)
                            .text_size(16)
                        ]
                    }
                    Some(MavapayBanks::Nigerian(_)) => unreachable!(),
                    None => widget::column![localized_copy("loading banks...")]
                        .align_x(iced::Alignment::Center),
                }
                .spacing(0),
            ]
        }
        Beneficiary::KES(KenyanBeneficiary::PayToPhone {
            account_name,
            phone_number,
        }) => {
            if account_name.is_empty() {
                validation_message = Some("Set the recipient's legal name");
            } else if phone_number.is_empty() {
                validation_message = Some("Set the recipient's phone number");
            }

            widget::column![
                text::h3(localized_copy("Setup Mobile Money Details (Kenya-MPESA)")),
                widget::container(widget::Space::default().width(iced::Length::Fill).height(2))
                    .style(theme::card::border),
                widget::space().width(iced::Length::Fill),
                sat_amount_input(state.sat_amount, state.btc_price),
                input_field(
                    account_name,
                    "Enter Recipient Account Name",
                    "KES.account_name"
                ),
                input_field(
                    phone_number,
                    "Enter Recipient Phone Number",
                    "KES.phone_number"
                ),
            ]
        }

        b => unreachable!("Beneficiary currently not supported: {:?}", b),
    }
    .spacing(10)
    .width(iced::Length::Fill);

    let previous_btn = button::secondary(Some(previous_icon()), localized_copy("Back"))
        .width(Length::Fixed(150.0))
        .on_press(BuySellMessage::Mavapay(MavapayMessage::NavigateBack));

    widget::column![
        widget::container(previous_btn).width(Length::Fill),
        widget::Space::new().height(6),
        widget::container(form).padding(20).style(|th| {
            theme::card::simple(th).border(iced::Border {
                radius: 25.0.into(),
                width: 1.0,
                color: color::TRANSPARENT_ORANGE,
            })
        }),
        widget::Space::new().height(12),
        widget::row![
            widget::row![
                card::simple(widget::space().height(iced::Length::Fill).width(5)).padding(1),
                widget::space().width(10),
                text::p2_medium(localized_copy("Sell Bitcoin to Fiat Money")).center(),
            ]
            .align_y(iced::Alignment::Center),
            widget::space().width(iced::Length::Fill),
            match validation_message {
                None => match sending_quote {
                    true =>
                        button::secondary(Some(clock_icon()), localized_copy("Fetching Quote.."))
                            .style(|th, st| {
                                let mut base = theme::button::secondary(th, st);
                                base.border = iced::Border::default().rounded(3);
                                base
                            }),
                    false => button::primary(Some(enter_box_icon()), localized_copy("Get Quote"))
                        .on_press_maybe(
                            (banks.is_some() || state.country.code == "KE")
                                .then_some(BuySellMessage::Mavapay(MavapayMessage::CreateQuote))
                        )
                        .style(|th, st| {
                            let mut base = theme::button::primary(th, st);
                            base.border = iced::Border::default().rounded(3);
                            base
                        }),
                },
                Some(m) => {
                    widget::button(widget::text(localized_copy(m)).size(14))
                        .padding(12)
                        .style(|th, st| {
                            let mut base = theme::button::secondary(th, st);
                            base.border = iced::Border::default().rounded(2).width(1);
                            base
                        })
                }
            }
        ]
        .height(50)
        .align_y(iced::Alignment::Center)
    ]
    .width(600)
    .align_x(Alignment::Center)
}

/// `label` is an English literal, localized here at render time.
fn detail_row<'a>(
    label: &'static str,
    value: impl Into<std::borrow::Cow<'a, str>>,
    text_style: Option<fn(&theme::Theme) -> widget::text::Style>,
) -> widget::Row<'a, BuySellMessage, theme::Theme> {
    let value = value.into().into_owned();

    widget::row![
        widget::column![
            text::p2_medium(localized_copy(label)).color(color::GREY_2),
            text::p2_bold(value.clone()).style(move |th| match text_style {
                Some(f) => f(th),
                None => theme::text::primary(th),
            })
        ]
        .width(Length::Fill),
        widget::Button::new(clipboard_icon().style(theme::text::secondary))
            .on_press(BuySellMessage::Clipboard(value))
            .style(theme::button::transparent)
    ]
    .spacing(10)
    .align_y(Alignment::Center)
}

fn summary_card<'a>(
    quote: &'a GetQuoteResponse,
) -> widget::Column<'a, BuySellMessage, theme::Theme> {
    let (reference, reference_label) = match &quote.order_id {
        Some(order_id) => (order_id.as_str(), "Order Ref"),
        None => (quote.id.as_str(), "Quote Ref"),
    };

    let source_currency: MavapayCurrency = (&quote.source_currency).into();
    let source_amount = quote.total_amount_in_source_currency as f64
        / match source_currency {
            MavapayCurrency::KenyanShilling
            | MavapayCurrency::SouthAfricanRand
            | MavapayCurrency::NigerianNaira => 100.0,
            MavapayCurrency::Bitcoin => 100_000_000.0,
        };

    let target_currency: MavapayCurrency = (&quote.target_currency).into();
    let target_amount = quote.amount_in_target_currency as f64
        / match target_currency {
            MavapayCurrency::KenyanShilling
            | MavapayCurrency::SouthAfricanRand
            | MavapayCurrency::NigerianNaira => 100.0,
            MavapayCurrency::Bitcoin => 100_000_000.0,
        };

    widget::column![
        widget::container(
            widget::row![
                success_icon_badge(),
                widget::Space::new().width(10),
                widget::column![
                    text::p1_bold(localized_copy("Order Created Successfully")),
                    text::p2_medium(format!(
                        "{}: {}",
                        localized_copy(reference_label),
                        reference
                    ))
                    .style(theme::text::secondary)
                ]
            ]
            .align_y(Alignment::Center)
        )
        .width(iced::Length::Fill)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center),
        widget::container(widget::Space::default().width(iced::Length::Fill).height(2))
            .style(theme::card::border),
        widget::row![
            widget::space().width(iced::Length::Fill),
            widget::column![
                text::caption(localized_copy("You Send")).style(theme::text::success),
                text::p1_bold(format!(
                    "{} {}",
                    source_amount,
                    localized_copy(currency_label(&source_currency))
                ))
            ]
            .spacing(1),
            widget::column![
                text::caption(localized_copy("You Receive")).color(color::ORANGE),
                text::p1_bold(format!(
                    "{} {}",
                    target_amount,
                    localized_copy(currency_label(&target_currency))
                ))
            ]
            .spacing(1),
            widget::space().width(iced::Length::Fill)
        ]
        .spacing(10),
    ]
    .spacing(15)
    .width(Length::Fill)
    .padding(7)
}

fn instructions_card<'a>(
    quote: &'a GetQuoteResponse,
    country: &'static Country,
) -> widget::Container<'a, BuySellMessage, theme::Theme> {
    let account_number = quote.ngn_bank_account_number.as_deref();
    let reference = match &quote.order_id {
        Some(order_id) => order_id.as_str(),
        None => quote.id.as_str(),
    };

    // TODO: Generally rework this, for light mode too
    card::simple(
        widget::column![
            widget::row![
                widget::container(
                    cash_icon().size(16).color(iced::color![0x000DFF]),
                ).padding(8).style(|_| widget::container::Style {
                    background: Some(iced::Background::Color(iced::color![0x000DFF, 0.14])),
                    border: iced::Border {
                        radius: 25.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
                widget::Space::new().width(10),
                widget::column![
                text::p1_bold(localized_copy("Payment Instructions")),
                text::p2_medium(localized_copy("Follow these steps to complete your order")).style(theme::text::secondary)
            ]
            ]
            .align_y(Alignment::Center),
            widget::Space::new().height(15),
            text::p2_medium(localized_copy("STEP 1: TRANSFER FUNDS TO OUR ACCOUNT"))
            .style(theme::text::secondary),
            widget::Space::new().height(10),
            quote.bank_name.as_deref().map(|bn|
                detail_row("Bank Name", bn, None)
            ),
            widget::Space::new().height(20),
            account_number.map(|an|
                detail_row("Account Number", an, None)
            ),
            widget::Space::new().height(20),
            quote.ngn_account_name.as_deref().map(|an|
                detail_row("Account Name", an, None)
            ),
            widget::Space::new().height(20),
            detail_row(
                "Amount to Send",
                format!("{}{}", country.currency.symbol, quote.total_amount_in_source_currency as f64 / 100.0),
                Some(theme::text::success),
            ),
            widget::Space::new().height(20),
            text::p2_medium(localized_copy("STEP 2: INCLUDE THIS REFERENCE IN YOUR TRANSFER"))
                .style(theme::text::secondary),
            widget::Space::new().height(10),
            card::simple(
                widget::column![
                    widget::row![
                        warning_icon().size(20).style(theme::text::warning),
                        widget::Space::new().width(10),
                        text::p2_medium(localized_copy("Critical: Include this reference number")).style(theme::text::warning),
                    ].align_y(Alignment::Center),
                    widget::Space::new().height(20),
                    widget::row![
                        text::h4_bold(reference),
                        widget::Button::new(clipboard_icon().style(theme::text::secondary))
                            .on_press(BuySellMessage::Clipboard(reference.to_string()))
                            .style(theme::button::transparent),
                    ].align_y(Alignment::Center),
                    widget::Space::new().height(20),
                    text::p2_medium(localized_copy("This helps us match your payment to your order. Without this reference, your order may be delayed."))
                    .style(theme::text::secondary)
                ].width(Length::Fill)
            ).style(theme::card::modal),
            widget::Space::new().height(20),
            text::p2_medium(localized_copy("STEP 3: WAIT FOR CONFIRMATION")),
            widget::Space::new().height(10),
            widget::row![
                reload_icon().size(16).style(theme::text::secondary),
                widget::Space::new().width(10),
                text::p2_medium(localized_copy("Waiting for payment confirmation..."))
                    .style(theme::text::secondary)
            ].align_y(Alignment::Center),
            widget::Space::new().height(10),
            button::primary(Some(reload_icon()), localized_copy("Start Over"))
                .on_press(BuySellMessage::ResetWidget)
        ].width(Length::Fill).padding(15)
    ).width(Length::Fill)
}

fn notes_card<'a>() -> widget::Container<'a, BuySellMessage, theme::Theme> {
    card::simple(
        widget::column![
            text::p1_bold(localized_copy("Important Notes")),
            widget::Space::new().height(10),
            note_item("Your order will begin execution once we confirm receipt of funds"),
            note_item("Execution time will depend on market liquidity"),
            note_item("You will receive real-time updates on trade execution progress"),
            note_item("Final Bitcoin price may vary based on actual execution prices"),
            note_item("Our commission (1-2%) will be deducted from the final Bitcoin amount"),
        ]
        .width(Length::Fill),
    )
}

fn note_item<'a>(content: &'static str) -> widget::Row<'a, BuySellMessage, theme::Theme> {
    widget::row![
        dot_icon().size(4).color(color::ORANGE),
        widget::Space::new().width(8),
        text::p2_medium(localized_copy(content))
    ]
    .align_y(Alignment::Center)
}

// TODO: This widget needs a general touch-up
fn order_success_view<'a>(
    order: &'a GetOrderResponse,
    sats: u64,
    buy_or_sell: &'a BuyOrSell,
    _country: &'static Country,
    mask_amounts: bool,
) -> widget::Column<'a, BuySellMessage, theme::Theme> {
    let (title, subtitle) = match buy_or_sell {
        BuyOrSell::Sell => (
            "Withdrawal Complete",
            "Your Bitcoin has been successfully sent to your wallet.",
        ),
        BuyOrSell::Buy => (
            "Purchase Complete",
            "Your Bitcoin has been successfully sent to your wallet",
        ),
    };

    widget::column![
        text::h4_bold(localized_copy("Order Confirmation")),
        widget::Space::new().height(10),
        widget::container(widget::column![
            card::simple(
                widget::row![
                    success_icon_badge(),
                    widget::Space::new().width(15),
                    widget::column![
                        text::h4_bold(localized_copy(title)),
                        text::p2_medium(localized_copy(subtitle)).style(theme::text::secondary)
                    ]
                ]
                .align_y(Alignment::Center)
            )
            .width(Length::Fill),
            widget::Space::new().height(10),
            card::simple(
                widget::column![
                    text::p1_bold(localized_copy("Order Details")),
                    widget::Space::new().height(15),
                    detail_row("Order Id", &order.order_id, None),
                    widget::Space::new().height(15),
                    widget::row![
                        widget::column![
                            text::p2_medium(localized_copy("Amount Paid"))
                                .style(theme::text::secondary),
                            text::p1_bold(passive_amount(mask_amounts, || {
                                format_amount(order.amount, &order.currency)
                            }))
                        ]
                        .width(Length::Fill),
                        widget::column![
                            text::p2_medium(localized_copy("Bitcoin Received"))
                                .style(theme::text::secondary),
                            text::p1_bold(passive_amount(mask_amounts, || format!(
                                "{} BTC",
                                format_f64_as_string(sats as f64 / 100_000_000.0, ",", 8, false)
                            )))
                        ]
                        .width(Length::Fill)
                    ],
                    widget::Space::new().height(15),
                    widget::row![
                        widget::column![
                            text::p2_medium(localized_copy("Order Status"))
                                .style(theme::text::secondary),
                            text::p2_bold(localized_copy(order_status_text(&order.status)))
                                .style(status_style(&order.status))
                        ]
                        .width(Length::Fill),
                        widget::column![
                            text::p2_medium(localized_copy("Payment Method"))
                                .style(theme::text::secondary),
                            text::p1_bold(localized_copy(order.payment_method.as_str()))
                        ]
                        .width(Length::Fill)
                    ],
                    widget::column![
                        widget::Space::new().height(15),
                        text::p2_medium(localized_copy("Order Date")).style(theme::text::secondary),
                        text::p2_bold(display_timestamp(&order.created_at))
                    ],
                    // separator
                    widget::container(
                        widget::Space::new()
                            .height(Length::Fixed(3.0))
                            .width(Length::Fill)
                    )
                    .style(theme::container::border_grey),
                    button::primary(Some(reload_icon()), localized_copy("Start New Transaction"))
                        .on_press(BuySellMessage::ResetWidget)
                        .width(Length::Fill)
                ]
                .spacing(15)
                .width(Length::Fill)
                .padding(20)
            ),
        ])
        .padding(10)
    ]
}

fn history_view<'a>(
    state: &'a MavapayState,
    mask_amounts: bool,
) -> widget::Column<'a, BuySellMessage, theme::Theme> {
    let Some(MavapayFlowStep::History {
        transactions,
        loading,
    }) = state.steps.last()
    else {
        unreachable!()
    };

    let content: iced::Element<'a, BuySellMessage, theme::Theme> = match (loading, transactions) {
        (true, _) => widget::container(
            widget::column![
                reload_icon().size(24).style(theme::text::secondary),
                widget::Space::new().height(10),
                text::p2_medium(localized_copy("Loading transaction history..."))
                    .style(theme::text::secondary),
            ]
            .align_x(Alignment::Center),
        )
        .padding(40)
        .width(Length::Fill)
        .align_x(Alignment::Center)
        .into(),
        (false, Some(transaction_list)) if transaction_list.is_empty() => card::simple(
            widget::column![
                history_icon().size(48).style(theme::text::secondary),
                widget::Space::new().height(15),
                text::p1_bold(localized_copy("No transactions found")),
                widget::Space::new().height(5),
                text::p2_medium(localized_copy(
                    "Your transactions will appear here once you buy or sell bitcoin.",
                ))
                .style(theme::text::primary)
            ]
            .padding(40)
            .align_x(Alignment::Center)
            .width(Length::Fill),
        )
        .width(Length::Fill)
        .into(),
        (false, Some(transaction_list)) => transaction_list
            .iter()
            .enumerate()
            .fold(widget::column![].spacing(10), |col, (idx, transaction)| {
                col.push(transaction_row(idx, transaction, mask_amounts))
            })
            .width(Length::Fill)
            .into(),
        (false, None) => button::primary(Some(reload_icon()), localized_copy("Retry"))
            .on_press(BuySellMessage::Mavapay(MavapayMessage::FetchTransactions))
            .into(),
    };

    widget::column![
        button::secondary(Some(previous_icon()), localized_copy("Back"))
            .width(Length::Fixed(150.0))
            .on_press(BuySellMessage::ResetWidget),
        widget::Space::new().height(10),
        text::h4_bold(localized_copy("Order History")),
        content
    ]
    .padding(20)
    .width(Length::Fill)
}

fn pretty_timestamp(ts: &str) -> String {
    ts.parse::<chrono::DateTime<chrono::Utc>>()
        .ok()
        .map(|dt| {
            let local = dt
                .with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S %:z")
                .to_string();
            local
        })
        .unwrap_or_else(|| "unknown".into())
}

/// `pretty_timestamp` for display: the English fallback stays in the helper
/// (tests assert it); the localized one is chosen here, never by comparing.
fn display_timestamp(ts: &str) -> String {
    match ts.parse::<chrono::DateTime<chrono::Utc>>() {
        Ok(_) => pretty_timestamp(ts),
        Err(_) => localized_copy("unknown").to_string(),
    }
}

fn transaction_row<'a>(
    idx: usize,
    transaction: &'a OrderTransaction,
    mask_amounts: bool,
) -> widget::Container<'a, BuySellMessage, theme::Theme> {
    let (tx_status_text, tx_status_color) = transaction_status_info(transaction);
    let (order_type, order_type_color) =
        order_type_from_payment(transaction.payment_method.as_ref());

    card::simple(
        widget::column![
            widget::row![
                widget::column![
                    text::p2_medium(localized_copy("Order ID")).style(theme::text::secondary),
                    text::p2_bold(&transaction.order_id)
                ]
                .width(Length::Fill),
                badge(localized_copy(order_type), order_type_color),
                widget::Space::new().width(8),
                badge(localized_copy(tx_status_text), tx_status_color)
            ]
            .align_y(Alignment::Center),
            widget::Space::new().height(12),
            widget::row![
                widget::column![
                    text::p2_medium(localized_copy("Amount")).style(theme::text::secondary),
                    text::p2_bold(passive_amount(mask_amounts, || {
                        format_amount(transaction.amount, &transaction.currency)
                    }))
                ]
                .width(Length::Fill),
                transaction
                    .payment_method
                    .as_ref()
                    .map(|pm| widget::column![
                        text::p2_medium(localized_copy("Payment Method"))
                            .style(theme::text::secondary),
                        text::p2_bold(localized_copy(pm.as_str()))
                    ]
                    .width(Length::Fill)),
                widget::column![
                    text::p2_medium(localized_copy("Date")).style(theme::text::secondary),
                    text::p2_bold(display_timestamp(&transaction.created_at))
                ]
                .width(Length::Fill),
                button::secondary(None, localized_copy("View"))
                    .on_press(BuySellMessage::Mavapay(MavapayMessage::SelectTransaction(
                        idx
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

/// `label` is an English literal, localized here at render time.
fn info_field<'a>(
    label: &'static str,
    value: impl ToString,
) -> widget::Column<'a, BuySellMessage, theme::Theme> {
    widget::column![
        text::p2_medium(localized_copy(label)).style(theme::text::secondary),
        text::p2_bold(value.to_string())
    ]
    .width(Length::Fill)
}

/// Format a minor-unit (cents) fiat amount with two decimal places and comma
/// grouping, e.g. `1234567` -> `12,345.67`. Uses integer division so large
/// amounts stay exact (an `f64` cast would lose precision above 2^53).
fn format_fiat_cents(cents: u64) -> String {
    format!(
        "{}.{:02}",
        format_u64_as_string(cents / 100, ","),
        cents % 100
    )
}

fn format_currency_amount(amount: u64, currency: &MavapayUnitCurrency) -> String {
    match currency {
        MavapayUnitCurrency::KenyanShillingCent => format!("{} KES", format_fiat_cents(amount)),
        MavapayUnitCurrency::SouthAfricanRandCent => format!("{} ZAR", format_fiat_cents(amount)),
        MavapayUnitCurrency::NigerianNairaKobo => format!("{} NGN", format_fiat_cents(amount)),
        MavapayUnitCurrency::BitcoinSatoshi => format!(
            "{} BTC",
            format_f64_as_string(amount as f64 / 100_000_000.0, ",", 8, false)
        ),
    }
}

fn order_detail_view<'a>(
    state: &'a MavapayState,
    mask_amounts: bool,
) -> widget::Column<'a, BuySellMessage, theme::Theme> {
    let Some(MavapayFlowStep::OrderDetail {
        transaction,
        order,
        loading,
    }) = state.steps.last()
    else {
        unreachable!()
    };

    let (order_type, order_type_color) =
        order_type_from_payment(transaction.payment_method.as_ref());
    let (tx_status_text, tx_status_color) = transaction_status_info(transaction);

    let back_button = widget::button(
        widget::row![
            previous_icon().size(16).style(theme::text::secondary),
            widget::Space::new().width(5),
            text::p2_medium(localized_copy("Back to History")).style(theme::text::secondary)
        ]
        .align_y(Alignment::Center),
    )
    .style(theme::button::transparent)
    .on_press(BuySellMessage::Mavapay(MavapayMessage::NavigateBack));

    let header = widget::row![
        text::h4_bold(localized_copy("Order Details")),
        widget::Space::new().width(Length::Fill),
        badge(localized_copy(order_type), order_type_color),
        widget::Space::new().width(8),
        badge(localized_copy(tx_status_text), tx_status_color)
    ]
    .align_y(Alignment::Center);

    // Get total fees from all quotes if available, otherwise use transaction fees
    let fees_display = passive_amount(mask_amounts, || {
        order
            .as_ref()
            .map(|o| {
                let quotes = o.quotes();
                if quotes.is_empty() {
                    return format_fees(transaction.fees, &transaction.currency);
                }

                // Sum up fees from all quotes (use target currency fees)
                let total_fees: u64 = quotes
                    .iter()
                    .map(|q| q.transaction_fees_in_target_currency)
                    .sum();

                // Use the target currency from the first quote for formatting
                if let Some(first_quote) = quotes.first() {
                    format_currency_amount(total_fees, &first_quote.target_currency)
                } else {
                    format_fees(transaction.fees, &transaction.currency)
                }
            })
            .unwrap_or_else(|| format_fees(transaction.fees, &transaction.currency))
    });

    // Transaction summary card (always shown)
    let transaction_card = card::simple(
        widget::column![
            text::p1_bold(localized_copy("Transaction Summary")),
            widget::Space::new().height(15),
            detail_row("Transaction ID", transaction.transaction_id.clone(), None),
            widget::Space::new().height(8),
            detail_row("Order ID", transaction.order_id.clone(), None),
            widget::Space::new().height(8),
            widget::row![
                info_field(
                    "Amount",
                    passive_amount(mask_amounts, || {
                        format_amount(transaction.amount, &transaction.currency)
                    })
                ),
                info_field("Fees", fees_display),
            ],
            widget::Space::new().height(8),
            widget::row![
                transaction
                    .payment_method
                    .as_ref()
                    .map(|pm| info_field("Payment Method", localized_copy(pm.as_str()))),
                info_field("Date", display_timestamp(&transaction.created_at)),
            ]
        ]
        .padding(20)
        .width(Length::Fill),
    )
    .width(Length::Fill);

    // Order details (shown when order is loaded)
    let order_details: iced::Element<'a, BuySellMessage, theme::Theme> = if *loading {
        widget::container(
            widget::column![
                reload_icon().size(24).style(theme::text::secondary),
                widget::Space::new().height(10),
                text::p2_medium(localized_copy("Loading order details..."))
                    .style(theme::text::secondary)
            ]
            .align_x(Alignment::Center),
        )
        .padding(40)
        .width(Length::Fill)
        .align_x(Alignment::Center)
        .into()
    } else if let Some(order) = order {
        let quotes = order.quotes();

        if quotes.is_empty() {
            return widget::column![
                back_button,
                widget::Space::new().height(15),
                header,
                widget::Space::new().height(20),
                transaction_card,
                widget::Space::new().height(10),
                card::simple(
                    widget::column![
                        text::p1_bold(localized_copy("Order Information")),
                        widget::Space::new().height(15),
                        detail_row("Order ID", order.order_id.clone(), None),
                        widget::Space::new().height(8),
                        widget::row![
                            info_field(
                                "Amount",
                                passive_amount(mask_amounts, || {
                                    format_amount(order.amount, &order.currency)
                                })
                            ),
                            info_field("Status", localized_copy(order_status_text(&order.status))),
                        ],
                        widget::Space::new().height(8),
                        widget::row![
                            info_field("Currency", localized_copy(currency_label(&order.currency))),
                            info_field(
                                "Payment Method",
                                localized_copy(order.payment_method.as_str())
                            ),
                        ],
                    ]
                    .padding(20)
                    .width(Length::Fill),
                )
                .width(Length::Fill)
            ]
            .padding(20)
            .width(Length::Fill);
        }

        // Build quote cards for all quotes
        let quote_cards: Vec<iced::Element<'a, BuySellMessage, theme::Theme>> = quotes
            .iter()
            .enumerate()
            .map(|(idx, quote)| {
                let paid_amount = passive_amount(mask_amounts, || {
                    format_currency_amount(quote.total_amount, &quote.source_currency)
                });
                let received_amount = passive_amount(mask_amounts, || {
                    format_currency_amount(quote.equivalent_amount, &quote.target_currency)
                });

                let title = if quotes.len() > 1 {
                    localized_format!("Quote #{}", "Cotización #{}", idx + 1)
                } else {
                    localized_copy("Quote Details").to_string()
                };

                card::simple(
                    widget::column![
                        text::p1_bold(title),
                        widget::Space::new().height(15),
                        widget::row![
                            info_field("Amount Paid", &paid_amount),
                            info_field("Amount Received", &received_amount),
                        ],
                        widget::Space::new().height(8),
                        widget::row![
                            info_field(
                                "Source Fee",
                                passive_amount(mask_amounts, || format_currency_amount(
                                    quote.transaction_fees_in_source_currency,
                                    &quote.source_currency
                                ))
                            ),
                            info_field(
                                "Target Fee",
                                passive_amount(mask_amounts, || format_currency_amount(
                                    quote.transaction_fees_in_target_currency,
                                    &quote.target_currency
                                ))
                            ),
                            info_field(
                                "Fee (USD)",
                                passive_amount(mask_amounts, || format!(
                                    "${}",
                                    format_fiat_cents(quote.transaction_fees_in_usd_cent)
                                ))
                            ),
                        ],
                        widget::Space::new().height(8),
                        detail_row("Bitcoin Address", quote.payment_btc_detail.clone(), None),
                    ]
                    .padding(20)
                    .width(Length::Fill),
                )
                .width(Length::Fill)
                .into()
            })
            .collect();

        let mut content = widget::column![].width(Length::Fill);
        for (i, card) in quote_cards.into_iter().enumerate() {
            if i > 0 {
                content = content.push(widget::Space::new().height(10));
            }
            content = content.push(card);
        }

        content.into()
    } else {
        card::simple(
            widget::container(
                widget::row![
                    warning_icon().size(20).style(theme::text::warning),
                    widget::Space::new().width(10),
                    text::p1_bold(localized_copy("Failed to load order details"))
                ]
                .align_y(Alignment::Center),
            )
            .padding(20)
            .width(Length::Fill),
        )
        .width(Length::Fill)
        .into()
    };

    widget::column![
        back_button,
        widget::Space::new().height(15),
        header,
        widget::Space::new().height(20),
        transaction_card,
        widget::Space::new().height(10),
        order_details,
    ]
    .padding(20)
    .width(Length::Fill)
}

fn checkout_form<'a>(
    state: &'a MavapayState,
    mask_amounts: bool,
) -> widget::Column<'a, BuySellMessage, theme::Theme> {
    let Some(MavapayFlowStep::Checkout {
        fulfilled_order,
        quote,
        invoice_qr_code_data,
        spark_balance,
        sell,
        ..
    }) = state.steps.last()
    else {
        unreachable!()
    };

    match fulfilled_order {
        None => {
            widget::column![
                widget::container(match state.buy_or_sell {
                    BuyOrSell::Buy => {
                        widget::column![
                            summary_card(quote),
                            instructions_card(quote, state.country),
                            notes_card()
                        ]
                    }
                    BuyOrSell::Sell => {
                        widget::column![
                            summary_card(quote),
                            // Once a Spark payment of this invoice is under
                            // way (or done), paying it again from another
                            // wallet would pay twice: the QR goes away.
                            invoice_qr_code_data
                                .as_ref()
                                .filter(|_| sell.fulfilment == SellFulfilment::Idle)
                                .map(|data| invoice_qr_code_display(
                                    localized_copy("Deposit into the following address to proceed"),
                                    quote.invoice.as_str(),
                                    data
                                )),
                            // The deposit QR above always works; paying from
                            // Spark is offered on top of it once the balance
                            // is known.
                            spark_fulfilment(
                                &sell.fulfilment,
                                *spark_balance,
                                // Mavapay's routing-fee estimate on top: a
                                // balance that covers only the total would
                                // fail at send.
                                quote
                                    .total_amount_in_source_currency
                                    .saturating_add(quote.estimated_routing_fee.unwrap_or(0))
                            ),
                        ]
                        .spacing(10)
                    }
                })
                .padding(10)
                .style(theme::card::simple),
                widget::Space::new().height(15),
                (fulfilled_order.is_none() && cfg!(debug_assertions)).then(|| {
                    button::primary(Some(wrench_icon()), "Simulate Pay-In (Developer Option)")
                        .on_press(BuySellMessage::Mavapay(MavapayMessage::SimulatePayIn))
                }),
            ]
        }
        Some(order) => order_success_view(
            order,
            state.sat_amount,
            &state.buy_or_sell,
            state.country,
            mask_amounts,
        ),
    }
    .align_x(Alignment::Center)
    .width(600)
}

/// The "pay from Spark" part of a sell checkout. `None` when the Spark balance
/// isn't known, which leaves the deposit QR as the way to pay.
fn spark_fulfilment<'a>(
    fulfilment: &'a SellFulfilment,
    spark_balance: Option<u64>,
    total_sat: u64,
) -> Option<iced::Element<'a, BuySellMessage, theme::Theme>> {
    let element: iced::Element<'a, BuySellMessage, theme::Theme> = match fulfilment {
        SellFulfilment::Idle => {
            let balance = spark_balance?;
            if balance >= total_sat {
                button::primary(Some(bitcoin_icon()), localized_copy("Fulfil from Spark"))
                    .on_press(BuySellMessage::Mavapay(MavapayMessage::FulfillSellInvoice))
                    .into()
            } else {
                button::secondary(
                    Some(restore_icon()),
                    localized_copy("Spark balance too low. Pay the invoice manually"),
                )
                .into()
            }
        }
        // No `on_press`: a second click must not start a second payment.
        SellFulfilment::Preparing | SellFulfilment::Sending => {
            button::primary(Some(bitcoin_icon()), localized_copy("Paying from Spark…")).into()
        }
        SellFulfilment::Paid => text::p2_regular(localized_copy(
            "Paid from your Spark wallet. Waiting for Mavapay to confirm the order.",
        ))
        .into(),
        SellFulfilment::OutcomeUnknown {
            checking,
            outcome,
            guard,
        } => {
            let mut content =
                widget::column![text::p1_bold(localized_copy("Payment status unconfirmed"))]
                    .spacing(10)
                    .align_x(Alignment::Center);
            match (checking, outcome) {
                (true, _) | (_, None) => {
                    content = content.push(text::p2_regular(localized_copy(
                        "Checking your payment history to find out whether it went through. \
                         Don't send it again until this finishes.",
                    )));
                }
                (false, Some(outcome)) => {
                    content = content.push(text::p2_regular(coincube_ui::locale::notice(
                        outcome.guidance(*guard),
                    )));
                    let mut actions = widget::row![].spacing(10);
                    // Repeats under the original key, never a fresh one.
                    if fulfilment.may_resend() {
                        actions = actions.push(
                            button::primary(None, localized_copy("Send again")).on_press(
                                BuySellMessage::Mavapay(MavapayMessage::RetrySellPayment),
                            ),
                        );
                    }
                    actions = actions.push(
                        button::secondary(None, localized_copy("Check again"))
                            .on_press(BuySellMessage::Mavapay(MavapayMessage::CheckSellPayment)),
                    );
                    content = content.push(actions);
                }
            }
            content.into()
        }
    };
    Some(element)
}

fn invoice_qr_code_display<'a>(
    caption: &'a str,
    invoice: &'a str,
    data: &'a iced::widget::qr_code::Data,
) -> widget::Container<'a, BuySellMessage, theme::Theme> {
    widget::container(
        widget::column![
            widget::column![
                text::caption(caption),
                widget::row![
                    widget::container(
                        widget::text(format!("{}…", &invoice[..45]))
                            .font(iced::font::Font {
                                weight: iced::font::Weight::Medium,
                                ..iced::font::Font::MONOSPACE
                            })
                            .size(15)
                    )
                    .style(|th| {
                        theme::container::background(th).border(
                            iced::Border::default()
                                .color(color::GREY_4)
                                .width(1)
                                .rounded(0),
                        )
                    })
                    .align_x(iced::Alignment::Center)
                    .align_y(iced::Alignment::Center)
                    .width(iced::Length::Fill)
                    .height(iced::Length::Fill),
                    widget::button(
                        clipboard_icon()
                            .size(17)
                            .width(38)
                            .height(iced::Length::Fill)
                            .center()
                    )
                    .on_press(BuySellMessage::Mavapay(
                        MavapayMessage::WriteInvoiceToClipboard
                    ))
                    .style(|th, st| {
                        let mut base = theme::button::secondary(th, st);
                        base.border = iced::Border::default()
                            .rounded(0)
                            .width(1)
                            .color(color::GREY_4);
                        base
                    })
                    .height(iced::Length::Fill)
                ]
                .height(36)
            ]
            .spacing(1),
            widget::container(widget::qr_code(data).style(|_| widget::qr_code::Style {
                background: color::WHITE,
                cell: color::BLACK,
            }))
            .padding(10)
            .width(iced::Length::Fill)
            .align_x(iced::Alignment::Center)
            .align_y(iced::Alignment::Center)
            .style(|th| { theme::container::foreground(th).background(color::WHITE) }),
        ]
        .spacing(7),
    )
    .height(iced::Length::Shrink)
    .width(800)
    .padding(8)
    .style(|th| {
        theme::container::foreground(th).border(
            iced::Border::default()
                .color(color::GREY_4)
                .width(1)
                .rounded(5),
        )
    })
}

fn status_style(status: &TransactionStatus) -> fn(&theme::Theme) -> widget::text::Style {
    match status {
        TransactionStatus::Success | TransactionStatus::Paid => theme::text::success,
        TransactionStatus::Pending => theme::text::warning,
        TransactionStatus::Expired | TransactionStatus::Failed => theme::text::error,
    }
}

/// Determine order type based on payment method.
/// - BankTransfer/USDT = BUY (user paying fiat to receive BTC)
/// - Lightning/Onchain = SELL (user paying BTC to receive fiat)
fn order_type_from_payment(
    payment_method: Option<&MavapayPaymentMethod>,
) -> (&'static str, iced::Color) {
    match payment_method {
        Some(MavapayPaymentMethod::BankTransfer) | Some(MavapayPaymentMethod::USDT) => {
            ("BUY", color::GREEN)
        }
        Some(MavapayPaymentMethod::Lightning) | Some(MavapayPaymentMethod::Onchain) => {
            ("SELL", color::ORANGE)
        }
        None => ("N/A", color::BLUE),
    }
}

/// Translate order status to user-friendly display text
fn order_status_text(status: &TransactionStatus) -> &'static str {
    match status {
        TransactionStatus::Success | TransactionStatus::Paid => "Complete",
        TransactionStatus::Pending => "Processing",
        TransactionStatus::Expired => "Expired",
        TransactionStatus::Failed => "Failed",
    }
}

/// Translate transaction status to user-friendly display text and color.
/// For DEPOSIT transactions, even SUCCESS means "Processing" since the order
/// isn't complete until the WITHDRAWAL succeeds.
fn transaction_status_info(transaction: &OrderTransaction) -> (&'static str, iced::Color) {
    match transaction.status {
        TransactionStatus::Pending => ("Processing", color::ORANGE),
        TransactionStatus::Success | TransactionStatus::Paid => {
            match transaction.transaction_type {
                // WITHDRAWAL success means order is actually complete
                TransactionType::Withdrawal => ("Complete", color::GREEN),
                // DEPOSIT success just means payment received, order still processing
                TransactionType::Deposit => ("Processing", color::ORANGE),
            }
        }
        TransactionStatus::Expired => ("Expired", color::RED),
        TransactionStatus::Failed => ("Failed", color::RED),
    }
}

/// English name of a Mavapay currency for display; the same text as its
/// `Display`, as a literal so the view can localize it.
fn currency_label(currency: &MavapayCurrency) -> &'static str {
    match currency {
        MavapayCurrency::KenyanShilling => "Kenyan Shilling (KES)",
        MavapayCurrency::SouthAfricanRand => "South African Rand (ZAR)",
        MavapayCurrency::NigerianNaira => "Nigerian Naira (NGN)",
        MavapayCurrency::Bitcoin => "Bitcoin (BTC)",
    }
}

fn passive_amount(mask_amounts: bool, format: impl FnOnce() -> String) -> String {
    if mask_amounts {
        "••••••".into()
    } else {
        format()
    }
}

fn format_amount(amount: u64, currency: &MavapayCurrency) -> String {
    match currency {
        MavapayCurrency::Bitcoin => format!(
            "{} BTC",
            format_f64_as_string(amount as f64 / 100_000_000.0, ",", 8, false)
        ),
        MavapayCurrency::KenyanShilling => format!("{} KSh", format_fiat_cents(amount)),
        MavapayCurrency::SouthAfricanRand => format!("{} ZAR", format_fiat_cents(amount)),
        MavapayCurrency::NigerianNaira => format!("{} NGN", format_fiat_cents(amount)),
    }
}

fn format_fees(fees: u64, currency: &MavapayCurrency) -> String {
    match currency {
        MavapayCurrency::Bitcoin => format!("{} sats", format_u64_as_string(fees, ",")),
        _ => format_amount(fees, currency),
    }
}

fn badge<'a>(
    label: impl widget::text::IntoFragment<'a>,
    badge_color: iced::Color,
) -> widget::Container<'a, BuySellMessage, theme::Theme> {
    widget::container(text::p2_bold(label).color(badge_color))
        .padding(iced::Padding::from([4, 8]))
        .style(move |_| widget::container::Style {
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

fn success_icon_badge() -> widget::Container<'static, BuySellMessage, theme::Theme> {
    widget::container(check_icon().size(16).style(theme::text::success))
        .padding(8)
        .style(|_| widget::container::Style {
            background: Some(iced::Background::Color(iced::color!(0x2FC455, 0.18))),
            border: iced::Border {
                radius: 25.0.into(),
                width: 0.8,
                color: iced::color!(0x2FC455, 0.72),
            },
            ..Default::default()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{
        state::spark::send::{ReconcileOutcome, RetryGuard},
        view::{buysell::mavapay::SellPayment, shared::landing::capture},
        wallets::SparkBackend,
    };
    use std::sync::Arc;

    fn country(code: &str) -> &'static Country {
        crate::services::coincube::get_countries()
            .iter()
            .find(|country| country.code == code)
            .expect("fixture country should exist")
    }

    fn state(buy_or_sell: BuyOrSell, step: MavapayFlowStep, country_code: &str) -> MavapayState {
        let mut state = MavapayState::new(
            buy_or_sell,
            step,
            country(country_code),
            Arc::new(SparkBackend::new(Arc::new(
                crate::app::breez_spark::SparkClient::disconnected_for_test(),
            ))),
        );
        state.sat_amount = 123_456;
        state.btc_price = Some(5_000_000_000.0);
        state
    }

    fn quote(
        id: &str,
        source_currency: MavapayUnitCurrency,
        target_currency: MavapayUnitCurrency,
        payment_method: MavapayPaymentMethod,
    ) -> GetQuoteResponse {
        GetQuoteResponse {
            id: id.to_string(),
            order_id: Some(format!("order-{id}")),
            exchange_rate: 1.0,
            usd_to_target_currency_rate: 1.0,
            source_currency,
            target_currency,
            transaction_fees_in_source_currency: 100,
            transaction_fees_in_target_currency: 10,
            amount_in_source_currency: 12_000,
            amount_in_target_currency: 6_000,
            payment_method,
            expiry: "2026-01-01T00:00:00Z".to_string(),
            is_valid: true,
            invoice: "lnbc1pfixtureinvoicewithmorethanfortyfivecharactersforui".to_string(),
            hash: "hash".to_string(),
            total_amount_in_source_currency: 12_500,
            total_amount_in_target_currency: Some(6_000),
            customer_internal_fee: 0,
            created_at: Some("2026-01-01T00:00:00Z".to_string()),
            updated_at: Some("2026-01-01T00:00:00Z".to_string()),
            estimated_routing_fee: Some(4),
            bank_name: Some("Fixture Bank".to_string()),
            ngn_bank_account_number: Some("1234567890".to_string()),
            ngn_account_name: Some("Mavapay Fixture".to_string()),
            ngn_bank_code: Some("999".to_string()),
        }
    }

    fn transaction(
        status: TransactionStatus,
        transaction_type: TransactionType,
        payment_method: Option<MavapayPaymentMethod>,
    ) -> OrderTransaction {
        OrderTransaction {
            order_id: "order-1".to_string(),
            transaction_id: "tx-1".to_string(),
            amount: 12_345,
            fees: 250,
            currency: MavapayCurrency::NigerianNaira,
            transaction_type,
            status,
            payment_method,
            created_at: "2026-01-01T12:00:00Z".to_string(),
        }
    }

    fn order(status: TransactionStatus, quotes: Vec<OrderQuote>) -> GetOrderResponse {
        GetOrderResponse {
            id: 1,
            order_id: "order-1".to_string(),
            quote_id: Some("quote-1".to_string()),
            amount: 12_345,
            status,
            currency: MavapayCurrency::NigerianNaira,
            payment_method: MavapayPaymentMethod::BankTransfer,
            is_valid: Some(true),
            payment_btc_detail: Some("btc-address".to_string()),
            created_at: "2026-01-01T12:00:00Z".to_string(),
            updated_at: "2026-01-01T12:05:00Z".to_string(),
            order_data: Some(OrderDataWrapper {
                status: "ok".to_string(),
                data: OrderDataInner { quotes },
            }),
        }
    }

    fn order_quote(
        source_currency: MavapayUnitCurrency,
        target_currency: MavapayUnitCurrency,
    ) -> OrderQuote {
        OrderQuote {
            transaction_fees_in_source_currency: 111,
            transaction_fees_in_target_currency: 22,
            transaction_fees_in_usd_cent: 33,
            payment_btc_detail: "bc1qfixtureaddress".to_string(),
            total_amount: 12_345,
            equivalent_amount: 6_789,
            source_currency,
            target_currency,
        }
    }

    #[test]
    fn formatting_and_status_helpers_cover_display_cases() {
        assert_eq!(format_fiat_cents(0), "0.00");
        assert_eq!(format_fiat_cents(5), "0.05");
        assert_eq!(format_fiat_cents(1_234_567), "12,345.67");

        assert_eq!(
            format_currency_amount(12_345, &MavapayUnitCurrency::KenyanShillingCent),
            "123.45 KES"
        );
        assert_eq!(
            format_currency_amount(12_345, &MavapayUnitCurrency::SouthAfricanRandCent),
            "123.45 ZAR"
        );
        assert_eq!(
            format_currency_amount(12_345, &MavapayUnitCurrency::NigerianNairaKobo),
            "123.45 NGN"
        );
        assert_eq!(
            format_currency_amount(123_456_789, &MavapayUnitCurrency::BitcoinSatoshi),
            "1.23456789 BTC"
        );

        assert_eq!(
            format_amount(123_456_789, &MavapayCurrency::Bitcoin),
            "1.23456789 BTC"
        );
        assert_eq!(
            format_amount(12_345, &MavapayCurrency::KenyanShilling),
            "123.45 KSh"
        );
        assert_eq!(
            format_amount(12_345, &MavapayCurrency::SouthAfricanRand),
            "123.45 ZAR"
        );
        assert_eq!(
            format_amount(12_345, &MavapayCurrency::NigerianNaira),
            "123.45 NGN"
        );
        assert_eq!(
            format_fees(12_345, &MavapayCurrency::Bitcoin),
            "12,345 sats"
        );
        assert_eq!(
            format_fees(12_345, &MavapayCurrency::NigerianNaira),
            "123.45 NGN"
        );

        assert_eq!(order_status_text(&TransactionStatus::Success), "Complete");
        assert_eq!(order_status_text(&TransactionStatus::Paid), "Complete");
        assert_eq!(order_status_text(&TransactionStatus::Pending), "Processing");
        assert_eq!(order_status_text(&TransactionStatus::Expired), "Expired");
        assert_eq!(order_status_text(&TransactionStatus::Failed), "Failed");

        assert_eq!(
            transaction_status_info(&transaction(
                TransactionStatus::Pending,
                TransactionType::Deposit,
                None,
            ))
            .0,
            "Processing"
        );
        assert_eq!(
            transaction_status_info(&transaction(
                TransactionStatus::Success,
                TransactionType::Deposit,
                Some(MavapayPaymentMethod::BankTransfer),
            ))
            .0,
            "Processing"
        );
        assert_eq!(
            transaction_status_info(&transaction(
                TransactionStatus::Success,
                TransactionType::Withdrawal,
                Some(MavapayPaymentMethod::Lightning),
            ))
            .0,
            "Complete"
        );
        assert_eq!(
            transaction_status_info(&transaction(
                TransactionStatus::Expired,
                TransactionType::Withdrawal,
                None,
            ))
            .0,
            "Expired"
        );
        assert_eq!(
            transaction_status_info(&transaction(
                TransactionStatus::Failed,
                TransactionType::Withdrawal,
                None,
            ))
            .0,
            "Failed"
        );

        assert_eq!(
            order_type_from_payment(Some(&MavapayPaymentMethod::BankTransfer)).0,
            "BUY"
        );
        assert_eq!(
            order_type_from_payment(Some(&MavapayPaymentMethod::USDT)).0,
            "BUY"
        );
        assert_eq!(
            order_type_from_payment(Some(&MavapayPaymentMethod::Lightning)).0,
            "SELL"
        );
        assert_eq!(
            order_type_from_payment(Some(&MavapayPaymentMethod::Onchain)).0,
            "SELL"
        );
        assert_eq!(order_type_from_payment(None).0, "N/A");

        assert_ne!(pretty_timestamp("2026-01-01T12:00:00Z"), "unknown");
        assert_eq!(pretty_timestamp("not a timestamp"), "unknown");
    }

    #[test]
    fn input_forms_build_for_supported_buy_and_sell_countries() {
        let buy = state(
            BuyOrSell::Buy,
            MavapayFlowStep::BuyInputFrom {
                ln_invoice: None,
                getting_invoice: false,
                sending_quote: false,
            },
            "NG",
        );
        let _ = form(&buy, false);

        let mut buy_without_price = state(
            BuyOrSell::Buy,
            MavapayFlowStep::BuyInputFrom {
                ln_invoice: Some("invoice".to_string()),
                getting_invoice: true,
                sending_quote: false,
            },
            "NG",
        );
        buy_without_price.btc_price = None;
        let _ = form(&buy_without_price, false);

        let ngn = state(
            BuyOrSell::Sell,
            MavapayFlowStep::SellInputForm {
                spark_balance: Some(50_000),
                banks: Some(MavapayBanks::Nigerian(vec![NigerianBank {
                    bank_name: "Fixture Bank".to_string(),
                    nip_bank_code: "999".to_string(),
                }])),
                beneficiary: Beneficiary::NGN {
                    bank_account_name: Some("Recipient".to_string()),
                    bank_account_number: "1234567890".to_string(),
                    bank_code: "999".to_string(),
                    bank_name: "Fixture Bank".to_string(),
                },
                sending_quote: false,
            },
            "NG",
        );
        let _ = form(&ngn, false);

        let zar = state(
            BuyOrSell::Sell,
            MavapayFlowStep::SellInputForm {
                spark_balance: Some(50_000),
                banks: Some(MavapayBanks::SouthAfrican(vec![
                    "Capitec".to_string(),
                    "Standard Bank".to_string(),
                ])),
                beneficiary: Beneficiary::ZAR {
                    name: "Recipient".to_string(),
                    bank_name: "Capitec".to_string(),
                    bank_account_number: "1234567890".to_string(),
                },
                sending_quote: true,
            },
            "ZA",
        );
        let _ = form(&zar, false);

        let kes = state(
            BuyOrSell::Sell,
            MavapayFlowStep::SellInputForm {
                spark_balance: Some(50_000),
                banks: None,
                beneficiary: Beneficiary::KES(KenyanBeneficiary::PayToPhone {
                    account_name: "Recipient".to_string(),
                    phone_number: "+254700000000".to_string(),
                }),
                sending_quote: false,
            },
            "KE",
        );
        let _ = form(&kes, false);

        let invalid_ngn = state(
            BuyOrSell::Sell,
            MavapayFlowStep::SellInputForm {
                spark_balance: None,
                banks: None,
                beneficiary: Beneficiary::NGN {
                    bank_account_name: None,
                    bank_account_number: "not-a-number".to_string(),
                    bank_code: String::new(),
                    bank_name: String::new(),
                },
                sending_quote: false,
            },
            "NG",
        );
        let _ = form(&invalid_ngn, false);
    }

    #[test]
    fn checkout_and_history_forms_build_for_result_shapes() {
        let checkout = state(
            BuyOrSell::Buy,
            MavapayFlowStep::Checkout {
                quote: quote(
                    "quote-1",
                    MavapayUnitCurrency::NigerianNairaKobo,
                    MavapayUnitCurrency::BitcoinSatoshi,
                    MavapayPaymentMethod::BankTransfer,
                ),
                fulfilled_order: None,
                invoice_qr_code_data: None,
                spark_balance: None,
                sell: SellPayment::default(),
                stream_order_id: Some("order-quote-1".to_string()),
            },
            "NG",
        );
        let _ = form(&checkout, false);

        let fulfilled = state(
            BuyOrSell::Buy,
            MavapayFlowStep::Checkout {
                quote: quote(
                    "quote-2",
                    MavapayUnitCurrency::NigerianNairaKobo,
                    MavapayUnitCurrency::BitcoinSatoshi,
                    MavapayPaymentMethod::BankTransfer,
                ),
                fulfilled_order: Some(order(TransactionStatus::Success, Vec::new())),
                invoice_qr_code_data: None,
                spark_balance: None,
                sell: SellPayment::default(),
                stream_order_id: None,
            },
            "NG",
        );
        let _ = form(&fulfilled, false);

        for step in [
            MavapayFlowStep::History {
                transactions: None,
                loading: true,
            },
            MavapayFlowStep::History {
                transactions: Some(Vec::new()),
                loading: false,
            },
            MavapayFlowStep::History {
                transactions: Some(vec![transaction(
                    TransactionStatus::Success,
                    TransactionType::Withdrawal,
                    Some(MavapayPaymentMethod::Lightning),
                )]),
                loading: false,
            },
            MavapayFlowStep::History {
                transactions: None,
                loading: false,
            },
        ] {
            let history = state(BuyOrSell::Buy, step, "NG");
            let _ = form(&history, false);
        }
    }

    #[test]
    fn order_detail_forms_build_for_loading_success_empty_and_failed_states() {
        let base_transaction = transaction(
            TransactionStatus::Paid,
            TransactionType::Withdrawal,
            Some(MavapayPaymentMethod::BankTransfer),
        );

        for step in [
            MavapayFlowStep::OrderDetail {
                transaction: base_transaction.clone(),
                order: None,
                loading: true,
            },
            MavapayFlowStep::OrderDetail {
                transaction: base_transaction.clone(),
                order: None,
                loading: false,
            },
            MavapayFlowStep::OrderDetail {
                transaction: base_transaction.clone(),
                order: Some(order(TransactionStatus::Pending, Vec::new())),
                loading: false,
            },
            MavapayFlowStep::OrderDetail {
                transaction: base_transaction.clone(),
                order: Some(order(
                    TransactionStatus::Success,
                    vec![
                        order_quote(
                            MavapayUnitCurrency::NigerianNairaKobo,
                            MavapayUnitCurrency::BitcoinSatoshi,
                        ),
                        order_quote(
                            MavapayUnitCurrency::BitcoinSatoshi,
                            MavapayUnitCurrency::NigerianNairaKobo,
                        ),
                    ],
                )),
                loading: false,
            },
        ] {
            let detail = state(BuyOrSell::Buy, step, "NG");
            let _ = form(&detail, false);
        }
    }

    fn inspect(state: &MavapayState, mask_amounts: bool) -> capture::Inspection {
        capture::inspect(
            "mavapay-passive-order-privacy",
            form(state, mask_amounts),
            (1024, 768),
            "dark",
            theme::Theme::dark(),
        )
    }

    #[test]
    fn hidden_order_history_masks_saved_amount_but_preserves_status() {
        let history = state(
            BuyOrSell::Buy,
            MavapayFlowStep::History {
                transactions: Some(vec![transaction(
                    TransactionStatus::Success,
                    TransactionType::Withdrawal,
                    Some(MavapayPaymentMethod::BankTransfer),
                )]),
                loading: false,
            },
            "NG",
        );

        let hidden = inspect(&history, true);
        assert!(hidden.contains("••••••"));
        assert!(!hidden.contains("123.45 NGN"));
        assert!(hidden.contains("Order History"));
        assert!(hidden.contains("Complete"));

        let shown = inspect(&history, false);
        assert!(shown.contains("123.45 NGN"));
    }

    #[test]
    fn hidden_order_detail_masks_transaction_order_quote_and_fee_amounts() {
        let transaction = transaction(
            TransactionStatus::Paid,
            TransactionType::Withdrawal,
            Some(MavapayPaymentMethod::BankTransfer),
        );
        for quotes in [
            Vec::new(),
            vec![order_quote(
                MavapayUnitCurrency::NigerianNairaKobo,
                MavapayUnitCurrency::BitcoinSatoshi,
            )],
        ] {
            let detail = state(
                BuyOrSell::Buy,
                MavapayFlowStep::OrderDetail {
                    transaction: transaction.clone(),
                    order: Some(order(TransactionStatus::Paid, quotes)),
                    loading: false,
                },
                "NG",
            );

            let hidden = inspect(&detail, true);
            for amount in [
                "123.45 NGN",
                "2.50 NGN",
                "1.11 NGN",
                "0.00006789 BTC",
                "0.00000022 BTC",
                "$0.33",
            ] {
                assert!(!hidden.contains(amount), "hidden order showed {}", amount);
            }
            assert!(hidden.contains("••••••"));
            assert!(hidden.contains("Order Details"));
            assert!(hidden.contains("Complete"));

            let shown = inspect(&detail, false);
            assert!(shown.contains("123.45 NGN"));
            assert!(shown.contains("2.50 NGN") || shown.contains("0.00000022 BTC"));
        }
    }

    fn sell_checkout(spark_balance: Option<u64>, fulfilment: SellFulfilment) -> MavapayState {
        let quote = quote(
            "quote-sell",
            MavapayUnitCurrency::BitcoinSatoshi,
            MavapayUnitCurrency::NigerianNairaKobo,
            MavapayPaymentMethod::Lightning,
        );
        let invoice_qr_code_data =
            Some(iced::widget::qr_code::Data::new(quote.invoice.as_bytes()).unwrap());
        state(
            BuyOrSell::Sell,
            MavapayFlowStep::Checkout {
                quote,
                fulfilled_order: None,
                invoice_qr_code_data,
                spark_balance,
                sell: SellPayment {
                    fulfilment,
                    ..SellPayment::default()
                },
                stream_order_id: None,
            },
            "NG",
        )
    }

    fn inspect_sell(name: &str, state: &MavapayState) -> capture::Inspection {
        capture::inspect(
            name,
            form(state, false),
            (1024, 768),
            "dark",
            theme::Theme::dark(),
        )
    }

    /// A Spark balance that covers the order offers paying from Spark, next to
    /// the deposit QR. Nothing mentions Liquid any more.
    #[test]
    fn sell_checkout_offers_spark_fulfilment_when_spark_covers_the_order() {
        // The quote's total (12_500) plus Mavapay's routing-fee estimate (4).
        let checkout = sell_checkout(Some(12_504), SellFulfilment::Idle);
        let shown = inspect_sell("mavapay-sell-checkout-spark-funded", &checkout);
        assert!(shown.contains("Deposit into the following address to proceed"));
        assert!(shown.contains("Fulfil from Spark"));
        assert!(!shown.contains("Spark balance too low"));
        assert!(!shown.contains("Liquid"));
    }

    /// Too little in Spark keeps the manual deposit-QR fallback and offers no
    /// Spark payment.
    #[test]
    fn sell_checkout_falls_back_to_manual_payment_when_spark_is_short() {
        // Covers the total but not the routing-fee estimate on top.
        let checkout = sell_checkout(Some(12_503), SellFulfilment::Idle);
        let shown = inspect_sell("mavapay-sell-checkout-spark-short", &checkout);
        assert!(shown.contains("Deposit into the following address to proceed"));
        assert!(shown.contains("Spark balance too low. Pay the invoice manually"));
        assert!(!shown.contains("Fulfil from Spark"));

        // An unknown balance offers nothing beyond the QR.
        let unknown = inspect_sell(
            "mavapay-sell-checkout-spark-unknown",
            &sell_checkout(None, SellFulfilment::Idle),
        );
        assert!(unknown.contains("Deposit into the following address to proceed"));
        assert!(!unknown.contains("Fulfil from Spark"));
        assert!(!unknown.contains("Spark balance too low"));
    }

    /// While a payment is with the bridge, or its outcome is unknown, the
    /// checkout offers no way to pay again; only a check that found no trace
    /// on a key-guarded rail offers "Send again".
    #[test]
    fn sell_checkout_offers_no_second_payment_while_one_is_unresolved() {
        for fulfilment in [SellFulfilment::Preparing, SellFulfilment::Sending] {
            let shown = inspect_sell(
                "mavapay-sell-checkout-in-flight",
                &sell_checkout(Some(50_000), fulfilment),
            );
            assert!(shown.contains("Paying from Spark"));
            assert!(!shown.contains("Fulfil from Spark"));
            // No QR to pay the same invoice again from another wallet.
            assert!(!shown.contains("Deposit into the following address"));
        }
        let paid = inspect_sell(
            "mavapay-sell-checkout-paid",
            &sell_checkout(Some(50_000), SellFulfilment::Paid),
        );
        assert!(paid.contains("Paid from your Spark wallet"));
        assert!(!paid.contains("Deposit into the following address"));

        let checking = inspect_sell(
            "mavapay-sell-checkout-checking",
            &sell_checkout(
                Some(50_000),
                SellFulfilment::OutcomeUnknown {
                    checking: true,
                    outcome: None,
                    guard: RetryGuard::IdempotencyKey,
                },
            ),
        );
        assert!(checking.contains("Payment status unconfirmed"));
        assert!(!checking.contains("Deposit into the following address"));
        assert!(!checking.contains("Fulfil from Spark"));
        assert!(!checking.contains("Send again"));

        let inconclusive = inspect_sell(
            "mavapay-sell-checkout-inconclusive",
            &sell_checkout(
                Some(50_000),
                SellFulfilment::OutcomeUnknown {
                    checking: false,
                    outcome: Some(ReconcileOutcome::Inconclusive("offline".to_string())),
                    guard: RetryGuard::IdempotencyKey,
                },
            ),
        );
        assert!(inconclusive.contains("Check again"));
        assert!(!inconclusive.contains("Send again"));
        assert!(!inconclusive.contains("Fulfil from Spark"));

        let unguarded = inspect_sell(
            "mavapay-sell-checkout-unguarded",
            &sell_checkout(
                Some(50_000),
                SellFulfilment::OutcomeUnknown {
                    checking: false,
                    outcome: Some(ReconcileOutcome::NoTrace),
                    guard: RetryGuard::None,
                },
            ),
        );
        assert!(!unguarded.contains("Send again"));

        let guarded = inspect_sell(
            "mavapay-sell-checkout-guarded",
            &sell_checkout(
                Some(50_000),
                SellFulfilment::OutcomeUnknown {
                    checking: false,
                    outcome: Some(ReconcileOutcome::NoTrace),
                    guard: RetryGuard::IdempotencyKey,
                },
            ),
        );
        assert!(guarded.contains("Send again"));
        assert!(!guarded.contains("Fulfil from Spark"));
    }

    #[test]
    fn checkout_keeps_amount_visible_for_active_payment_review() {
        let checkout = state(
            BuyOrSell::Buy,
            MavapayFlowStep::Checkout {
                quote: quote(
                    "quote-1",
                    MavapayUnitCurrency::NigerianNairaKobo,
                    MavapayUnitCurrency::BitcoinSatoshi,
                    MavapayPaymentMethod::BankTransfer,
                ),
                fulfilled_order: None,
                invoice_qr_code_data: None,
                spark_balance: None,
                sell: SellPayment::default(),
                stream_order_id: None,
            },
            "NG",
        );

        let hidden = inspect(&checkout, true);
        assert!(hidden.contains("125 Nigerian Naira (NGN)"));
        assert!(hidden.contains("Amount to Send"));
    }

    #[test]
    fn fulfilled_checkout_masks_confirmation_amounts_when_hidden() {
        let fulfilled = state(
            BuyOrSell::Buy,
            MavapayFlowStep::Checkout {
                quote: quote(
                    "quote-2",
                    MavapayUnitCurrency::NigerianNairaKobo,
                    MavapayUnitCurrency::BitcoinSatoshi,
                    MavapayPaymentMethod::BankTransfer,
                ),
                fulfilled_order: Some(order(TransactionStatus::Paid, Vec::new())),
                invoice_qr_code_data: None,
                spark_balance: None,
                sell: SellPayment::default(),
                stream_order_id: None,
            },
            "NG",
        );

        let hidden = inspect(&fulfilled, true);
        assert!(hidden.contains("Order Confirmation"));
        assert!(hidden.contains("Amount Paid"));
        assert!(hidden.contains("Bitcoin Received"));
        assert!(hidden.contains("Start New Transaction"));
        assert!(hidden.contains("••••••"));
        assert!(!hidden.contains("123.45 NGN"));
        assert!(!hidden.contains("0.00123456 BTC"));

        let shown = inspect(&fulfilled, false);
        assert!(shown.contains("123.45 NGN"));
        assert!(shown.contains("0.00123456 BTC"));
    }
}
