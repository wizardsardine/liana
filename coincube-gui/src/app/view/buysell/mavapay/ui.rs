use crate::{
    app::view::{
        buysell::{panel::BuyOrSell, MavapayFlowStep, MavapayState},
        BuySellMessage, Message as ViewMessage,
    },
    services::{coincube::Country, mavapay::*},
};

use iced::{widget, Alignment, Length};

use coincube_ui::component::{button, card, text};
use coincube_ui::{color, icon::*, theme};

pub fn form<'a>(state: &'a MavapayState) -> iced::Element<'a, ViewMessage, theme::Theme> {
    let form = match state
        .steps
        .last()
        .expect("`MavapayState` must have at least one flow-step")
    {
        MavapayFlowStep::BuyInputFrom { .. } => buy_input_form,
        MavapayFlowStep::SellInputForm { .. } => sell_input_form,
        MavapayFlowStep::Checkout { .. } => checkout_form,
        MavapayFlowStep::History { .. } => history_view,
        MavapayFlowStep::OrderDetail { .. } => order_detail_view,
    };

    let element: iced::Element<'a, BuySellMessage, theme::Theme> = form(state).into();
    element.map(ViewMessage::BuySell)
}

fn buy_input_form<'a>(state: &'a MavapayState) -> widget::Column<'a, BuySellMessage, theme::Theme> {
    let Some(MavapayFlowStep::BuyInputFrom {
        getting_invoice,
        ln_invoice,
        sending_quote,
    }) = state.steps.last()
    else {
        unreachable!()
    };

    let form = match state.btc_price {
        Some(price) => widget::container(match ln_invoice {
            Some((invoice, qr_code_data)) => widget::column![
                invoice_qr_code_display(
                    "Generated Lightning Invoice:",
                    invoice.as_str(),
                    qr_code_data
                ),
                match sending_quote {
                    true => button::primary(Some(clock_icon()), "Getting Quote..."),
                    false => button::primary(Some(card_icon()), "Get Quote")
                        .on_press(BuySellMessage::Mavapay(MavapayMessage::CreateQuote)),
                }
                .width(Length::Fill)
                .style(|th, st| {
                    let mut base = theme::button::secondary(th, st);
                    base.border = iced::Border::default()
                        .width(2)
                        .rounded(2)
                        .color(color::GREY_4);
                    base
                })
            ]
            .width(Length::Fill)
            .spacing(12),
            None => widget::column![
                widget::row![
                    widget::column![
                        widget::text(format!(
                            "{} ({})",
                            state.country.currency.name, state.country.currency.code
                        ))
                        .size(14)
                        .color(color::GREY_2),
                        widget::Space::new().height(5),
                        iced_aw::number_input(
                            &{ state.sat_amount as f64 * (price / 100_000_000.0) }.round(),
                            ..,
                            |a| { BuySellMessage::Mavapay(MavapayMessage::FiatAmountChanged(a,)) }
                        )
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
                        widget::text("Satoshis (BTCSAT)")
                            .size(14)
                            .color(color::GREY_2),
                        widget::Space::new().height(5),
                        iced_aw::number_input(&state.sat_amount, .., |a| {
                            BuySellMessage::Mavapay(MavapayMessage::SatAmountChanged(a as _))
                        })
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
                    true => button::secondary(Some(clock_icon()), "Getting Invoice..."),
                    false => button::primary(Some(card_icon()), "Generate Invoice").on_press(
                        BuySellMessage::Mavapay(MavapayMessage::GenerateLightningInvoice)
                    ),
                }
                .width(Length::Fill)
            ]
            .spacing(10)
            .align_x(iced::Alignment::Center)
            .width(Length::Fill),
        }),
        None => widget::container(
            text::p1_italic("Getting recent conversion rates, please wait")
                .width(Length::Fill)
                .center(),
        )
        .align_y(Alignment::Center)
        .align_x(Alignment::Center),
    }
    .padding(15)
    .style(theme::card::simple)
    .width(Length::Fixed(600.0));

    widget::column![
        // header text
        text::h4_bold("Buy Bitcoin using Fiat Money")
            .color(color::WHITE)
            .center(),
        widget::Space::new().height(Length::Fixed(20.0)),
        form,
        widget::Space::new().height(Length::Fixed(5.0)),
        text::p2_medium("Powered by Mavapay").color(color::GREY_3)
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
            text::caption(caption).color(color::BLUE),
            widget::text_input("...", value)
                .size(20)
                .style(|th: &theme::Theme, _: widget::text_input::Status| {
                    widget::text_input::Style {
                        background: iced::Color::WHITE.into(),
                        border: iced::Border::default()
                            .width(2)
                            .rounded(0)
                            .color(color::GREY_4),
                        icon: th.colors.text_inputs.primary.active.icon,
                        placeholder: th.colors.text_inputs.primary.active.placeholder,
                        value: iced::Color::BLACK,
                        selection: th.colors.text_inputs.primary.active.selection,
                    }
                })
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
                text::caption("Input Transfer Amount (in Satoshis)").color(color::BLUE),
                widget::row![
                    iced_aw::number_input(&sat_amount, .., |a| BuySellMessage::Mavapay(
                        MavapayMessage::SatAmountChanged(a as _)
                    ))
                    .on_submit(BuySellMessage::Mavapay(MavapayMessage::NormalizeAmounts))
                    .align_x(Alignment::Center)
                    .width(150)
                    .set_size(20)
                    .font(iced::Font::MONOSPACE)
                    .input_style(
                        |th: &theme::Theme, _: widget::text_input::Status| {
                            widget::text_input::Style {
                                background: iced::Color::WHITE.into(),
                                border: iced::Border::default()
                                    .width(2)
                                    .rounded(0)
                                    .color(color::GREY_4),
                                icon: th.colors.text_inputs.primary.active.icon,
                                placeholder: th.colors.text_inputs.primary.active.placeholder,
                                value: iced::Color::BLACK,
                                selection: th.colors.text_inputs.primary.active.selection,
                            }
                        }
                    ),
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
            if let Err(_) = bank_account_number.parse::<usize>() {
                validation_message = Some("Bank Account Number MUST be a number");
            } else {
                if bank_code.is_empty() {
                    validation_message = Some("Select a recipient bank");
                } else {
                    if bank_account_name.is_none() {
                        validation_message = Some("Verify your bank account details to continue");
                    }
                }
            };

            widget::column![
                text::h3("Setup Bank Details (Nigeria)"),
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
                            text::caption("Select Recipient Bank").color(color::BLUE),
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
                                .style(|_, _| {
                                    widget::pick_list::Style {
                                        text_color: iced::Color::BLACK,
                                        placeholder_color: iced::Color::BLACK,
                                        handle_color: iced::Color::BLACK,
                                        background: iced::Color::WHITE.into(),
                                        border: iced::Border::default()
                                            .width(3)
                                            .rounded(1)
                                            .color(color::GREY_4),
                                    }
                                })
                                .text_size(16),
                                widget::button("Verify Details")
                                    .style(|th, st| {
                                        let mut base = theme::button::secondary(th, st);
                                        base.border = iced::Border::default()
                                            .width(2)
                                            .rounded(2)
                                            .color(color::GREY_4);
                                        base
                                    })
                                    .on_press_maybe(
                                        (!(bank_account_number.is_empty() || bank_code.is_empty()))
                                            .then_some(BuySellMessage::Mavapay(
                                                MavapayMessage::VerifyNgnBankDetails
                                            ))
                                    )
                            ]
                            .spacing(5),
                        ]
                    }
                    Some(MavapayBanks::SouthAfrican(_)) => unreachable!(),
                    None => widget::column!["loading banks..."].align_x(iced::Alignment::Center),
                }
                .spacing(0),
                bank_account_name.as_ref().map(|s| {
                    widget::column![
                        text::caption("Is this your registered name?").color(color::BLUE),
                        widget::container(widget::text(s).color(color::BLACK).size(20))
                            .padding(8)
                            .style(|_| {
                                widget::container::Style::default()
                                    .background(color::WHITE)
                                    .border(
                                        iced::Border::default()
                                            .width(2)
                                            .rounded(2)
                                            .color(color::GREY_4),
                                    )
                                    .color(color::BLACK)
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
            if let Err(_) = bank_account_number.parse::<usize>() {
                validation_message = Some("Bank Account Number MUST be a number");
            } else {
                if bank_name.is_empty() {
                    validation_message = Some("Select the recipient's bank");
                } else {
                    if name.is_empty() {
                        validation_message = Some("Set the recipient's legal name");
                    }
                }
            };

            widget::column![
                text::h3("Setup Bank Details (South Africa)"),
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
                            text::caption("Select Recipient Bank").color(color::BLUE),
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
                            .style(|_, _| {
                                widget::pick_list::Style {
                                    text_color: iced::Color::BLACK,
                                    placeholder_color: iced::Color::BLACK,
                                    handle_color: iced::Color::BLACK,
                                    background: iced::Color::WHITE.into(),
                                    border: iced::Border::default()
                                        .width(3)
                                        .rounded(1)
                                        .color(color::GREY_4),
                                }
                            })
                            .text_size(16)
                        ]
                    }
                    Some(MavapayBanks::Nigerian(_)) => unreachable!(),
                    None => widget::column!["loading banks..."].align_x(iced::Alignment::Center),
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
            } else {
                if phone_number.is_empty() {
                    validation_message = Some("Set the recipient's phone number");
                }
            };

            widget::column![
                text::h3("Setup Mobile Money Details (Kenya-MPESA)"),
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
    .spacing(5)
    .width(iced::Length::Fill);

    widget::column![
        widget::Space::new().height(6),
        widget::container(form).padding(10).style(move |_| {
            widget::container::Style::default()
                .background(iced::Color::BLACK)
                .color(iced::Color::WHITE)
                .border(
                    iced::Border::default()
                        .color(color::GREY_5)
                        .width(1)
                        .rounded(5),
                )
        }),
        widget::Space::new().height(12),
        widget::row![
            widget::row![
                card::simple(widget::space().height(iced::Length::Fill).width(5)).padding(1),
                widget::space().width(10),
                text::p2_medium("Sell Bitcoin to Fiat Money")
                    .color(color::WHITE)
                    .center(),
            ]
            .align_y(iced::Alignment::Center),
            widget::space().width(iced::Length::Fill),
            match validation_message {
                None => match sending_quote {
                    true =>
                        button::secondary(Some(clock_icon()), "Fetching Quote..").style(|th, st| {
                            let mut base = theme::button::secondary(th, st);
                            base.border = iced::Border::default().rounded(3);
                            base
                        }),
                    false => button::primary(Some(enter_box_icon()), "Get Quote")
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
                    widget::button(widget::text(m).size(14))
                        .padding(12)
                        .style(|th, st| {
                            let mut base = theme::button::secondary(th, st);
                            base.border = iced::Border::default()
                                .rounded(2)
                                .width(1)
                                .color(color::RED);
                            base.text_color = color::GREY_2;
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

fn detail_row<'a>(
    label: &'a str,
    value: String,
    text_color: Option<iced::Color>,
) -> widget::Row<'a, BuySellMessage, theme::Theme> {
    widget::row![
        widget::column![
            text::p2_medium(label).color(color::GREY_2),
            text::p2_bold(value.clone()).color(text_color.unwrap_or(color::WHITE))
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
    details: &CheckoutDetails,
) -> widget::Container<'a, BuySellMessage, theme::Theme> {
    let CheckoutDetails {
        reference,
        reference_label,
        total_fiat,
        btc_amount,
        currency_symbol,
    } = details;

    card::simple(
        widget::column![
            widget::row![
                success_icon_badge(),
                widget::Space::new().width(10),
                widget::column![
                    text::p1_bold("Order Created Successfully"),
                    text::p2_medium(format!("{}: {}", reference_label, reference))
                        .color(color::GREY_2)
                ]
            ]
            .align_y(Alignment::Center),
            widget::Space::new().height(15),
            widget::row![
                widget::column![
                    text::p2_medium("Order Amount").color(color::GREY_2),
                    text::p1_bold(format!("{}{}", currency_symbol, total_fiat))
                ]
                .width(Length::Fill),
                widget::column![
                    text::p2_medium("Bitcoin Expected").color(color::GREY_2),
                    text::p1_bold(format!("{:.8} BTC", btc_amount))
                ]
                .width(Length::Fill),
            ],
            widget::Space::new().height(10),
            widget::row![widget::column![
                text::p2_medium("Order Status").color(color::GREY_2),
                widget::row![clock_icon(), text::p1_bold("PENDING")]
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
) -> widget::Container<'a, BuySellMessage, theme::Theme> {
    let CheckoutDetails {
        reference,
        total_fiat,
        currency_symbol,
        ..
    } = details;
    let account_number = quote.ngn_bank_account_number.clone();

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
                text::p1_bold("Payment Instructions"),
                text::p2_medium("Follow these steps to complete your order").color(color::GREY_2)
            ]
            ]
            .align_y(Alignment::Center),
            widget::Space::new().height(15),
            text::p2_medium("STEP 1: TRANSFER FUNDS TO OUR ACCOUNT")
            .color(color::GREY_2),
            widget::Space::new().height(10),
            quote.bank_name.clone().map(|bank_name|
                detail_row("Bank Name", bank_name, None)
            ),
            widget::Space::new().height(20),
            account_number.clone().map(|account_number|
                detail_row("Account Number", account_number, None)
            ),
            widget::Space::new().height(20),
            quote.ngn_account_name.clone().map(|account_name|
                detail_row("Account Name", account_name, None)
            ),
            widget::Space::new().height(20),
            detail_row(
                "Amount to Send",
                format!("{}{}", currency_symbol, total_fiat),
                Some(color::GREEN),
            ),
            widget::Space::new().height(20),
            text::p2_medium("STEP 2: INCLUDE THIS REFERENCE IN YOUR TRANSFER")
                .color(color::GREY_2),
            widget::Space::new().height(10),
            card::simple(
                widget::column![
                widget::row![
                    warning_icon().size(20).style(theme::text::warning),
                    widget::Space::new().width(10),
                    text::p2_medium("Critical: Include this reference number").style(theme::text::warning),
                ].align_y(Alignment::Center),
                widget::Space::new().height(20),
                widget::row![
                    text::h4_bold(reference.clone()),
                    widget::Button::new(clipboard_icon().style(theme::text::secondary))
                        .on_press(BuySellMessage::Clipboard(reference.clone()))
                        .style(theme::button::transparent),
                ].align_y(Alignment::Center),
                widget::Space::new().height(20),
                text::p2_medium("This helps us match your payment to your order. Without this reference, your order may be delayed.")
                .color(color::GREY_2)
                ].width(Length::Fill)
            ).style(theme::card::modal),
            widget::Space::new().height(20),
            text::p2_medium("STEP 3: WAIT FOR CONFIRMATION"),
            widget::Space::new().height(10),
            widget::row![
                reload_icon().size(16).style(theme::text::secondary),
                widget::Space::new().width(10),
                text::p2_medium("Waiting for payment confirmation...")
                    .color(color::GREY_2)
            ].align_y(Alignment::Center),
            widget::Space::new().height(10),
            button::primary(Some(reload_icon()), "Start Over")
                .on_press(BuySellMessage::ResetWidget)
        ].width(Length::Fill).padding(15)
    ).width(Length::Fill)
}

fn notes_card<'a>() -> widget::Container<'a, BuySellMessage, theme::Theme> {
    card::simple(
        widget::column![
            text::p1_bold("Important Notes"),
            widget::Space::new().height(10),
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

fn note_item<'a>(content: &'a str) -> widget::Row<'a, BuySellMessage, theme::Theme> {
    widget::row![
        dot_icon().size(4).color(color::ORANGE),
        widget::Space::new().width(8),
        text::p2_medium(content)
    ]
    .align_y(Alignment::Center)
}

fn order_success_view<'a>(
    buy_or_sell: &BuyOrSell,
    order: &GetOrderResponse,
    details: &CheckoutDetails,
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
        text::h4_bold("Order Confirmation"),
        widget::Space::new().height(10),
        widget::container(widget::column![
            card::simple(
                widget::column![widget::row![
                    success_icon_badge(),
                    widget::Space::new().width(15),
                    widget::column![
                        text::h4_bold(title),
                        text::p2_medium(subtitle).color(color::GREY_2)
                    ]
                ]
                .align_y(Alignment::Center)]
                .width(Length::Fill)
                .padding(20)
            )
            .width(Length::Fill),
            widget::Space::new().height(10),
            card::simple(
                widget::column![
                    text::p1_bold("Order Details"),
                    widget::Space::new().height(15),
                    detail_row(details.reference_label, order.order_id.clone(), None),
                    widget::Space::new().height(15),
                    widget::row![
                        widget::column![
                            text::p2_medium("Amount Paid").color(color::GREY_2),
                            text::p1_bold(format!(
                                "{}{}",
                                details.currency_symbol, details.total_fiat
                            ))
                        ]
                        .width(Length::Fill),
                        widget::column![
                            text::p2_medium("Bitcoin Received").color(color::GREY_2),
                            text::p1_bold(format!("{:.8} BTC", details.btc_amount))
                        ]
                        .width(Length::Fill)
                    ],
                    widget::Space::new().height(15),
                    widget::row![
                        widget::column![
                            text::p2_medium("Order Status").color(color::GREY_2),
                            text::p2_bold(order_status_text(&order.status))
                                .color(status_color(&order.status))
                        ]
                        .width(Length::Fill),
                        widget::column![
                            text::p2_medium("Payment Method").color(color::GREY_2),
                            text::p1_bold(format!("{}", order.payment_method))
                        ]
                        .width(Length::Fill)
                    ],
                    widget::column![
                        widget::Space::new().height(15),
                        text::p2_medium("Order Date").color(color::GREY_2),
                        text::p2_bold(pretty_timestamp(&order.created_at))
                    ]
                ]
                .width(Length::Fill)
                .padding(20)
            )
            .width(Length::Fill),
            widget::Space::new().height(10),
            card::simple(
                widget::column![
                    text::p2_medium("Thank you for using Mavapay!")
                        .color(color::GREY_2)
                        .center()
                        .width(Length::Fill),
                    widget::Space::new().height(15),
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

fn history_view<'a>(state: &'a MavapayState) -> widget::Column<'a, BuySellMessage, theme::Theme> {
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
                text::p2_medium("Loading transaction history...").color(color::GREY_2),
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
                text::p1_bold("No transactions found"),
                widget::Space::new().height(5),
                text::p2_medium("Your transactions will appear here once you buy or sell bitcoin.")
                    .color(color::GREY_2)
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
                col.push(transaction_row(idx, transaction))
            })
            .width(Length::Fill)
            .into(),
        (false, None) => button::primary(Some(reload_icon()), "Retry")
            .on_press(BuySellMessage::Mavapay(MavapayMessage::FetchTransactions))
            .into(),
    };

    widget::column![
        button::transparent(Some(previous_icon()), "Previous")
            .width(Length::Shrink)
            .on_press(BuySellMessage::ResetWidget),
        widget::Space::new().height(10),
        text::h4_bold("Order History"),
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

fn transaction_row<'a>(
    idx: usize,
    transaction: &'a OrderTransaction,
) -> widget::Container<'a, BuySellMessage, theme::Theme> {
    let (order_type, order_type_color) = order_type_from_payment(&transaction.payment_method);
    let (tx_status_text, tx_status_color) = transaction_status_info(transaction);

    card::simple(
        widget::column![
            widget::row![
                widget::column![
                    text::p2_medium("Order ID").color(color::GREY_2),
                    text::p2_bold(&transaction.order_id)
                ]
                .width(Length::Fill),
                badge(order_type, order_type_color),
                widget::Space::new().width(8),
                badge(tx_status_text, tx_status_color)
            ]
            .align_y(Alignment::Center),
            widget::Space::new().height(12),
            widget::row![
                widget::column![
                    text::p2_medium("Amount").color(color::GREY_2),
                    text::p2_bold(format_amount(transaction.amount, &transaction.currency))
                ]
                .width(Length::Fill),
                widget::column![
                    text::p2_medium("Payment").color(color::GREY_2),
                    text::p2_bold(transaction.payment_method.to_string())
                ]
                .width(Length::Fill),
                widget::column![
                    text::p2_medium("Date").color(color::GREY_2),
                    text::p2_bold(pretty_timestamp(&transaction.created_at))
                ]
                .width(Length::Fill),
                button::secondary(None, "View")
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

fn info_field<'a>(
    label: &'a str,
    value: impl ToString,
) -> widget::Column<'a, BuySellMessage, theme::Theme> {
    widget::column![
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

fn order_detail_view<'a>(
    state: &'a MavapayState,
) -> widget::Column<'a, BuySellMessage, theme::Theme> {
    let Some(MavapayFlowStep::OrderDetail {
        transaction,
        order,
        loading,
    }) = state.steps.last()
    else {
        unreachable!()
    };

    let (order_type, order_type_color) = order_type_from_payment(&transaction.payment_method);
    let (tx_status_text, tx_status_color) = transaction_status_info(transaction);

    let back_button = widget::button(
        widget::row![
            previous_icon().size(16).color(color::GREY_2),
            widget::Space::new().width(5),
            text::p2_medium("Back to History").color(color::GREY_2)
        ]
        .align_y(Alignment::Center),
    )
    .style(theme::button::transparent)
    .on_press(BuySellMessage::Mavapay(MavapayMessage::NavigateBack));

    let header = widget::row![
        text::h4_bold("Order Details"),
        widget::Space::new().width(Length::Fill),
        badge(order_type, order_type_color),
        widget::Space::new().width(8),
        badge(tx_status_text, tx_status_color)
    ]
    .align_y(Alignment::Center);

    // Get total fees from all quotes if available, otherwise use transaction fees
    let fees_display = order
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
        .unwrap_or_else(|| format_fees(transaction.fees, &transaction.currency));

    // Transaction summary card (always shown)
    let transaction_card = card::simple(
        widget::column![
            text::p1_bold("Transaction Summary"),
            widget::Space::new().height(15),
            detail_row("Transaction ID", transaction.transaction_id.clone(), None),
            widget::Space::new().height(8),
            detail_row("Order ID", transaction.order_id.clone(), None),
            widget::Space::new().height(8),
            widget::row![
                info_field(
                    "Amount",
                    format_amount(transaction.amount, &transaction.currency)
                ),
                info_field("Fees", fees_display),
            ],
            widget::Space::new().height(8),
            widget::row![
                info_field("Payment Method", &transaction.payment_method),
                info_field("Date", pretty_timestamp(&transaction.created_at)),
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
                text::p2_medium("Loading order details...").color(color::GREY_2)
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
                        text::p1_bold("Order Information"),
                        widget::Space::new().height(15),
                        detail_row("Order ID", order.order_id.clone(), None),
                        widget::Space::new().height(8),
                        widget::row![
                            info_field("Amount", format_amount(order.amount, &order.currency)),
                            info_field("Status", order_status_text(&order.status)),
                        ],
                        widget::Space::new().height(8),
                        widget::row![
                            info_field("Currency", &order.currency),
                            info_field("Payment Method", &order.payment_method),
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
                let (paid_amount, received_amount) = (
                    format_currency_amount(quote.total_amount, &quote.source_currency),
                    format_currency_amount(quote.equivalent_amount, &quote.target_currency),
                );

                let title = if quotes.len() > 1 {
                    format!("Quote #{}", idx + 1)
                } else {
                    "Quote Details".to_string()
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
                                format_currency_amount(
                                    quote.transaction_fees_in_source_currency,
                                    &quote.source_currency
                                )
                            ),
                            info_field(
                                "Target Fee",
                                format_currency_amount(
                                    quote.transaction_fees_in_target_currency,
                                    &quote.target_currency
                                )
                            ),
                            info_field(
                                "Fee (USD)",
                                format!(
                                    "${:.2}",
                                    quote.transaction_fees_in_usd_cent as f64 / 100.0
                                )
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
                    text::p1_bold("Failed to load order details")
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

// TODO: Use Breez SDK to satisfy lightning invoice, with user confirmation
fn checkout_form<'a>(state: &'a MavapayState) -> widget::Column<'a, BuySellMessage, theme::Theme> {
    let Some(MavapayFlowStep::Checkout {
        fulfilled_order,
        quote,
        invoice_qr_code_data,
        ..
    }) = state.steps.last()
    else {
        unreachable!()
    };

    let details = CheckoutDetails::from_quote(quote, state.sat_amount, state.country);

    match fulfilled_order {
        None => {
            widget::column![
                match state.buy_or_sell {
                    BuyOrSell::Buy => {
                        Some(widget::container(
                                widget::column![
                                text::p1_bold("Complete Your Order"),
                                text::p1_medium("Review your order details carefully before confirming your purchase. Once confirmed, your Bitcoin will be delivered to your wallet.")
                                    .color(color::GREY_2),
                                widget::Space::new().height(15),
                                widget::container(
                                    widget::column![
                                        summary_card(&details),
                                        instructions_card(quote, &details),
                                        notes_card()
                                    ].spacing(10)
                                        )
                                .padding(10)
                                .style(|t| widget::container::Style {
                                    border: iced::Border {
                                        radius: 25.0.into(),
                                        ..Default::default()
                                    },
                                    ..theme::container::background(t)
                                })
                            ])
                            .padding(20)
                            .style(theme::card::simple))
                    }
                    BuyOrSell::Sell =>
                        invoice_qr_code_data
                            .as_ref()
                            .map(|data| invoice_qr_code_display(
                                "Deposit into the following address to proceed",
                                quote.invoice.as_str(),
                                data
                            )),
                },
                widget::Space::new().height(10),
                (fulfilled_order.is_none() && cfg!(debug_assertions)).then(|| {
                    button::primary(Some(wrench_icon()), "Simulate Pay-In (Developer Option)")
                        .on_press(BuySellMessage::Mavapay(MavapayMessage::SimulatePayIn))
                }),
                widget::Space::new().height(10)
            ]
        }
        Some(order) => order_success_view(&state.buy_or_sell, order, &details),
    }
    .align_x(Alignment::Center)
    .width(600)
}

fn invoice_qr_code_display<'a>(
    caption: &'a str,
    invoice: &'a str,
    data: &'a iced::widget::qr_code::Data,
) -> widget::Container<'a, BuySellMessage, theme::Theme> {
    widget::container(
        widget::column![
            widget::column![
                text::caption(caption).color(color::BLUE),
                widget::row![
                    widget::container(
                        widget::text(format!("{}…", &invoice[..45]))
                            .font(iced::font::Font {
                                weight: iced::font::Weight::Medium,
                                ..iced::font::Font::MONOSPACE
                            })
                            .size(15)
                    )
                    .style(|_| {
                        widget::container::Style::default()
                            .background(color::WHITE)
                            .color(color::BLACK)
                            .border(iced::Border::default().width(0))
                    })
                    .align_x(iced::Alignment::Center)
                    .width(iced::Length::Fill)
                    .padding(7),
                    widget::button(clipboard_icon().color(color::WHITE).size(17).width(38))
                        .on_press(BuySellMessage::Mavapay(
                            MavapayMessage::WriteInvoiceToClipboard
                        ))
                        .style(|th, st| {
                            let mut base = theme::button::secondary(th, st);
                            base.border = iced::Border::default().rounded(0).width(0);
                            base.background = Some(color::GREY_6.into());
                            base
                        })
                        .padding(6)
                ]
                .spacing(1)
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
            .style(|_| {
                widget::container::Style::default()
                    .background(color::WHITE)
                    .color(iced::Color::BLACK)
            }),
        ]
        .spacing(7),
    )
    .height(iced::Length::Shrink)
    .width(800)
    .padding(10)
    .style(|_| {
        widget::container::Style::default()
            .background(color::BLACK)
            .color(iced::Color::WHITE)
            .border(
                iced::Border::default()
                    .color(color::GREY_4)
                    .width(1)
                    .rounded(5),
            )
    })
}

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

/// Determine order type based on payment method.
/// - BankTransfer/USDT = BUY (user paying fiat to receive BTC)
/// - Lightning/Onchain = SELL (user paying BTC to receive fiat)
fn order_type_from_payment(payment_method: &MavapayPaymentMethod) -> (&'static str, iced::Color) {
    match payment_method {
        MavapayPaymentMethod::BankTransfer | MavapayPaymentMethod::USDT => ("BUY", color::GREEN),
        MavapayPaymentMethod::Lightning | MavapayPaymentMethod::Onchain => ("SELL", color::ORANGE),
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
    match transaction.transaction_type {
        // DEPOSIT success just means payment received, order still processing
        TransactionType::Deposit => match transaction.status {
            TransactionStatus::Pending => ("Processing", color::ORANGE),
            TransactionStatus::Success | TransactionStatus::Paid => ("Processing", color::ORANGE),
            TransactionStatus::Expired => ("Expired", color::RED),
            TransactionStatus::Failed => ("Failed", color::RED),
        },
        // WITHDRAWAL success means order is actually complete
        TransactionType::Withdrawal => match transaction.status {
            TransactionStatus::Pending => ("Processing", color::ORANGE),
            TransactionStatus::Success | TransactionStatus::Paid => ("Complete", color::GREEN),
            TransactionStatus::Expired => ("Expired", color::RED),
            TransactionStatus::Failed => ("Failed", color::RED),
        },
    }
}

fn format_amount(amount: u64, currency: &MavapayCurrency) -> String {
    match currency {
        MavapayCurrency::Bitcoin => format!("{:.8} BTC", amount as f64 / 100_000_000.0),
        MavapayCurrency::KenyanShilling => format!("{:.2} KSh", amount as f64 / 100.0),
        MavapayCurrency::SouthAfricanRand => format!("{:.2} ZAR", amount as f64 / 100.0),
        MavapayCurrency::NigerianNaira => format!("{:.2} NGN", amount as f64 / 100.0),
    }
}

fn format_fees(fees: u64, currency: &MavapayCurrency) -> String {
    match currency {
        MavapayCurrency::Bitcoin => format!("{} sats", fees),
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
                ..Default::default()
            },
            ..Default::default()
        })
}
