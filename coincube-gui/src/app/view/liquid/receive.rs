use coincube_core::miniscript::bitcoin::{Amount, Denomination};

use crate::app::wallets::{DomainPaymentDetails, DomainPaymentStatus};

use coincube_ui::{
    color,
    component::{
        amount::DisplayAmount,
        button, card, form,
        text::*,
        transaction::{TransactionDirection, TransactionListItem},
    },
    icon, theme,
    widget::Element,
};
use iced::{
    widget::{
        button as iced_button, container, qr_code, scrollable, Column, Container, QRCode, Row,
        Space, TextInput,
    },
    Alignment, Background, Length,
};

use coincube_ui::image::asset_network_logo;

use crate::app::{
    breez_liquid::assets::{format_usdt_display, parse_asset_to_minor_units, USDT_PRECISION},
    settings::unit::BitcoinDisplayUnit,
    state::liquid::send::SendAsset,
    view::{liquid::RecentTransaction, LiquidReceiveMessage, ReceiveMethod, SenderNetwork},
};

#[allow(clippy::too_many_arguments)]
pub fn liquid_receive_view<'a>(
    receive_method: &'a ReceiveMethod,
    address: Option<&'a String>,
    qr_data: Option<&'a qr_code::Data>,
    loading: bool,
    amount_input: &'a form::Value<String>,
    usdt_amount_input: &'a form::Value<String>,
    description_input: &'a str,
    bitcoin_unit: BitcoinDisplayUnit,
    error: Option<&'a String>,
    lightning_limits: Option<(u64, u64)>,
    onchain_limits: Option<(u64, u64)>,
    receive_asset: SendAsset,
    sender_network: SenderNetwork,
    recent_transaction: &[RecentTransaction],
    btc_balance: Amount,
    usdt_balance: u64,
    show_direction_badges: bool,
) -> Element<'a, LiquidReceiveMessage> {
    let mut content = Column::new().spacing(20).width(Length::Fill);

    // ── Two-card "You Receive ← They Send" layout ───────────────────────────
    content = content.push(receive_cards(
        receive_asset,
        sender_network,
        btc_balance,
        usdt_balance,
        bitcoin_unit,
    ));

    // ── Input section (matches Send layout) ─────────────────────────────────
    match receive_method {
        ReceiveMethod::Lightning => {
            content = content.push(input_fields(
                amount_input,
                description_input,
                bitcoin_unit,
                lightning_limits,
                loading,
            ));
        }
        ReceiveMethod::Usdt => {
            content = content.push(usdt_input_fields(usdt_amount_input, loading));
        }
        _ => {
            // Liquid / OnChain: only show generate button if no address is displayed
            if address.is_none() && !loading {
                // For the BTC onchain swap-receive tab, wait until the dynamic
                // min/max swap limits have been fetched from the SDK before
                // letting the user generate an address — the warning box below
                // depends on them and we don't want to show a receive address
                // without the user first seeing the constraints.
                if *receive_method == ReceiveMethod::OnChain && onchain_limits.is_none() {
                    content = content.push(onchain_warning_box(onchain_limits, bitcoin_unit));
                    content = content.push(crate::loading::loading_indicator(Some(
                        "Fetching swap limits",
                    )));
                } else {
                    content = content.push(generate_button());
                }
            }
        }
    }

    if loading {
        content = content.push(crate::loading::loading_indicator(Some(
            "Generating Address",
        )));
    } else if let (Some(addr), Some(_qr)) = (address, qr_data) {
        // Clean on-chain addresses for display
        let display_addr = if *receive_method == ReceiveMethod::OnChain {
            let cleaned = addr.strip_prefix("bitcoin:").unwrap_or(addr);
            cleaned.split('?').next().unwrap_or(cleaned)
        } else {
            addr
        };

        // Address card (Vault-style): scrollable address + copy icon + action buttons
        let address_row = Row::new()
            .push(
                Container::new(
                    scrollable(
                        Column::new()
                            .push(Space::new().height(Length::Fixed(10.0)))
                            .push(
                                p2_regular(display_addr)
                                    .small()
                                    .style(theme::text::secondary),
                            )
                            .push(Space::new().height(Length::Fixed(10.0))),
                    )
                    .direction(scrollable::Direction::Horizontal(
                        scrollable::Scrollbar::new().width(2).scroller_width(2),
                    )),
                )
                .width(Length::Fill),
            )
            .push(
                iced::widget::Button::new(icon::clipboard_icon().style(theme::text::secondary))
                    .on_press(LiquidReceiveMessage::Copy)
                    .style(theme::button::transparent_border),
            )
            .align_y(Alignment::Center);

        let mut button_row = Row::new();

        if *receive_method == ReceiveMethod::OnChain || *receive_method == ReceiveMethod::Usdt {
            button_row = button_row.push(
                button::secondary(None, "Generate New Address")
                    .on_press(LiquidReceiveMessage::GenerateAddress),
            );
            button_row = button_row.push(Space::new().width(Length::Fill));
        }

        button_row = button_row.push(
            button::secondary(None, "Show QR Code").on_press(LiquidReceiveMessage::ShowQrCode),
        );

        // Descriptive text inside the card
        let description = match receive_method {
            ReceiveMethod::Lightning => Some("Share this invoice to receive sats via Lightning"),
            ReceiveMethod::Liquid => {
                Some("Share this address to receive L-BTC from any Liquid wallet")
            }
            ReceiveMethod::OnChain => None, // OnChain has its own warning box below
            ReceiveMethod::Usdt => {
                Some("Share this address to receive USDt (Liquid Tether) from any Liquid wallet")
            }
        };

        let mut card_col = Column::new().spacing(10).push(address_row);

        if let Some(desc) = description {
            card_col = card_col.push(p2_regular(desc).style(theme::text::secondary));
        }

        card_col = card_col.push(button_row);

        content = content.push(card::simple(card_col));

        // OnChain warning box
        if *receive_method == ReceiveMethod::OnChain {
            content = content.push(onchain_warning_box(onchain_limits, bitcoin_unit));
        }
    }

    // ── Last transactions (matching Send screen) ────────────────────────────
    content = content.push(Column::new().spacing(10).push(h4_bold("Last transactions")));

    if recent_transaction.is_empty() {
        content = content.push(coincube_ui::component::empty_placeholder(
            icon::receipt_icon().size(80),
            "No transactions yet",
            "Your transaction history will appear here once you send or receive coins.",
        ));
    } else {
        for (idx, tx) in recent_transaction.iter().enumerate() {
            let direction = if tx.is_incoming {
                TransactionDirection::Incoming
            } else {
                TransactionDirection::Outgoing
            };

            let is_usdt = tx.usdt_display.is_some();

            let tx_icon = if is_usdt {
                coincube_ui::image::asset_network_logo("usdt", "liquid", 40.0)
            } else {
                match &tx.details {
                    DomainPaymentDetails::Lightning { .. } => {
                        coincube_ui::image::asset_network_logo("btc", "lightning", 40.0)
                    }
                    DomainPaymentDetails::LiquidAsset { .. } => {
                        coincube_ui::image::asset_network_logo("lbtc", "liquid", 40.0)
                    }
                    DomainPaymentDetails::OnChainBitcoin { .. } => {
                        coincube_ui::image::asset_network_logo("btc", "bitcoin", 40.0)
                    }
                }
            };

            let display_amount = if is_usdt {
                Amount::ZERO
            } else if tx.is_incoming {
                tx.amount
            } else {
                tx.amount + tx.fees_sat
            };

            let mut item = TransactionListItem::new(direction, &display_amount, bitcoin_unit)
                .with_custom_icon(tx_icon)
                .with_show_direction_badge(show_direction_badges)
                .with_label(tx.description.clone())
                .with_time_ago(tx.time_ago.clone());

            if let Some(ref usdt_str) = tx.usdt_display {
                item = item.with_amount_override(usdt_str.clone());
            } else {
                let fiat_str = tx
                    .fiat_amount
                    .as_ref()
                    .map(|fiat| format!("~{} {}", fiat.to_rounded_string(), fiat.currency()));
                if let Some(fiat) = fiat_str {
                    item = item.with_fiat_amount(fiat);
                }
            }

            if matches!(tx.status, DomainPaymentStatus::Pending) {
                let (bg, fg) = (color::GREY_3, color::BLACK);
                let pending_badge = Container::new(
                    Row::new()
                        .push(
                            icon::warning_icon()
                                .size(14)
                                .style(move |_| iced::widget::text::Style { color: Some(fg) }),
                        )
                        .push(
                            text("Pending")
                                .bold()
                                .size(14)
                                .style(move |_| iced::widget::text::Style { color: Some(fg) }),
                        )
                        .spacing(4),
                )
                .padding([2, 8])
                .style(move |_| iced::widget::container::Style {
                    background: Some(iced::Background::Color(bg)),
                    border: iced::Border {
                        radius: 12.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                });
                item = item.with_custom_status(pending_badge.into());
            }

            let tx_element: Element<'_, LiquidReceiveMessage> = item
                .view(LiquidReceiveMessage::SelectTransaction(idx))
                .into();
            content = content.push(tx_element);
        }

        // "View All Transactions" button
        let view_tx_button = {
            let the_icon = icon::history_icon()
                .size(18)
                .style(|_theme: &theme::Theme| iced::widget::text::Style {
                    color: Some(color::ORANGE),
                });

            let label = text("View All Transactions")
                .size(15)
                .style(|_theme: &theme::Theme| iced::widget::text::Style {
                    color: Some(color::ORANGE),
                });

            let button_content = Row::new()
                .spacing(8)
                .align_y(iced::alignment::Vertical::Center)
                .push(the_icon)
                .push(label);

            iced_button(Container::new(button_content).padding([10, 20]).style(
                |_theme: &theme::Theme| container::Style {
                    background: Some(Background::Color(color::TRANSPARENT)),
                    border: iced::Border {
                        color: color::ORANGE,
                        width: 1.5,
                        radius: 20.0.into(),
                    },
                    ..Default::default()
                },
            ))
            .style(|_theme: &theme::Theme, _| iced_button::Style {
                background: Some(Background::Color(color::TRANSPARENT)),
                text_color: color::ORANGE,
                border: iced::Border {
                    radius: 20.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            })
            .on_press(LiquidReceiveMessage::History)
        };

        content = content
            .push(Space::new().height(Length::Fixed(20.0)))
            .push(
                Container::new(view_tx_button)
                    .width(Length::Fill)
                    .center_x(Length::Fill),
            )
            .push(Space::new().height(Length::Fixed(40.0)));
    }

    if let Some(err) = error {
        Column::new()
            .push(
                Container::new(
                    Container::new(text(err).size(14).color(color::RED))
                        .padding(10)
                        .center_x(Length::Fill)
                        .style(theme::card::error)
                        .width(Length::Fill)
                        .max_width(800),
                )
                .width(Length::Fill)
                .padding([20, 0])
                .align_x(Alignment::Center),
            )
            .push(content)
            .into()
    } else {
        content.into()
    }
}

#[allow(dead_code)]
fn method_toggle(current_method: &ReceiveMethod) -> Element<LiquidReceiveMessage> {
    let lightning_liquid = *current_method == ReceiveMethod::Lightning;
    let liquid_active = *current_method == ReceiveMethod::Liquid;
    let onchain_liquid = *current_method == ReceiveMethod::OnChain;
    let usdt_active = *current_method == ReceiveMethod::Usdt;

    let lightning_button = {
        let icon = icon::lightning_icon()
            .size(18)
            .style(move |_theme: &theme::Theme| iced::widget::text::Style {
                color: Some(if lightning_liquid {
                    color::ORANGE
                } else {
                    color::GREY_2
                }),
            });

        let label = text("Lightning")
            .size(16)
            .style(move |_theme: &theme::Theme| iced::widget::text::Style {
                color: Some(if lightning_liquid {
                    color::ORANGE
                } else {
                    color::GREY_2
                }),
            });

        let button_content = Container::new(
            Row::new()
                .spacing(8)
                .align_y(iced::alignment::Vertical::Center)
                .push(icon)
                .push(label),
        )
        .width(Length::Fill)
        .align_x(iced::alignment::Horizontal::Center);

        Container::new(
            iced_button(Container::new(button_content).padding([10, 30]))
                .style(move |_theme: &theme::Theme, _status| iced_button::Style {
                    background: Some(Background::Color(color::TRANSPARENT)),
                    text_color: if lightning_liquid {
                        color::WHITE
                    } else {
                        color::GREY_2
                    },
                    border: iced::Border {
                        radius: 50.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .on_press(LiquidReceiveMessage::ToggleMethod(ReceiveMethod::Lightning)),
        )
        .style(move |_theme: &theme::Theme| container::Style {
            background: Some(Background::Color(if lightning_liquid {
                iced::color!(0x161716)
            } else {
                color::TRANSPARENT
            })),
            border: iced::Border {
                radius: 50.0.into(),
                color: if lightning_liquid {
                    color::ORANGE
                } else {
                    color::TRANSPARENT
                },
                width: if lightning_liquid { 0.7 } else { 0.0 },
            },
            ..Default::default()
        })
    };

    let onchain_button = {
        let icon = icon::bitcoin_icon()
            .size(18)
            .style(move |_theme: &theme::Theme| iced::widget::text::Style {
                color: Some(if onchain_liquid {
                    color::ORANGE
                } else {
                    color::GREY_2
                }),
            });

        let label = text("Bitcoin")
            .size(16)
            .style(move |_theme: &theme::Theme| iced::widget::text::Style {
                color: Some(if onchain_liquid {
                    color::ORANGE
                } else {
                    color::GREY_2
                }),
            });

        let button_content = Container::new(
            Row::new()
                .spacing(8)
                .align_y(iced::alignment::Vertical::Center)
                .push(icon)
                .push(label),
        )
        .width(Length::Fill)
        .align_x(iced::alignment::Horizontal::Center);

        Container::new(
            iced_button(Container::new(button_content).padding([10, 30]))
                .style(move |_theme: &theme::Theme, _status| iced_button::Style {
                    background: Some(Background::Color(color::TRANSPARENT)),
                    text_color: if onchain_liquid {
                        color::WHITE
                    } else {
                        color::GREY_2
                    },
                    border: iced::Border {
                        radius: 50.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .on_press(LiquidReceiveMessage::ToggleMethod(ReceiveMethod::OnChain)),
        )
        .style(move |_theme: &theme::Theme| container::Style {
            background: Some(Background::Color(if onchain_liquid {
                iced::color!(0x161716)
            } else {
                color::TRANSPARENT
            })),
            border: iced::Border {
                radius: 50.0.into(),
                color: if onchain_liquid {
                    color::ORANGE
                } else {
                    color::TRANSPARENT
                },
                width: if onchain_liquid { 0.7 } else { 0.0 },
            },
            ..Default::default()
        })
    };

    let liquid_button = {
        let icon = icon::droplet_icon()
            .size(18)
            .style(move |_theme: &theme::Theme| iced::widget::text::Style {
                color: Some(if liquid_active {
                    color::ORANGE
                } else {
                    color::GREY_2
                }),
            });

        let label =
            text("Liquid")
                .size(16)
                .style(move |_theme: &theme::Theme| iced::widget::text::Style {
                    color: Some(if liquid_active {
                        color::ORANGE
                    } else {
                        color::GREY_2
                    }),
                });

        let button_content = Container::new(
            Row::new()
                .spacing(8)
                .align_y(iced::alignment::Vertical::Center)
                .push(icon)
                .push(label),
        )
        .width(Length::Fill)
        .align_x(iced::alignment::Horizontal::Center);

        Container::new(
            iced_button(Container::new(button_content).padding([10, 30]))
                .style(move |_theme: &theme::Theme, _status| iced_button::Style {
                    background: Some(Background::Color(color::TRANSPARENT)),
                    text_color: if liquid_active {
                        color::WHITE
                    } else {
                        color::GREY_2
                    },
                    border: iced::Border {
                        radius: 50.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .on_press(LiquidReceiveMessage::ToggleMethod(ReceiveMethod::Liquid)),
        )
        .style(move |_theme: &theme::Theme| container::Style {
            background: Some(Background::Color(if liquid_active {
                iced::color!(0x161716)
            } else {
                color::TRANSPARENT
            })),
            border: iced::Border {
                radius: 50.0.into(),
                color: if liquid_active {
                    color::ORANGE
                } else {
                    color::TRANSPARENT
                },
                width: if liquid_active { 0.7 } else { 0.0 },
            },
            ..Default::default()
        })
    };

    let usdt_button = {
        let ico = icon::usd_icon()
            .size(18)
            .style(move |_theme: &theme::Theme| iced::widget::text::Style {
                color: Some(if usdt_active {
                    color::ORANGE
                } else {
                    color::GREY_2
                }),
            });

        let label =
            text("USDt")
                .size(16)
                .style(move |_theme: &theme::Theme| iced::widget::text::Style {
                    color: Some(if usdt_active {
                        color::ORANGE
                    } else {
                        color::GREY_2
                    }),
                });

        let button_content = Container::new(
            Row::new()
                .spacing(8)
                .align_y(iced::alignment::Vertical::Center)
                .push(ico)
                .push(label),
        )
        .width(Length::Fill)
        .align_x(iced::alignment::Horizontal::Center);

        Container::new(
            iced_button(Container::new(button_content).padding([10, 30]))
                .style(move |_theme: &theme::Theme, _status| iced_button::Style {
                    background: Some(Background::Color(color::TRANSPARENT)),
                    text_color: if usdt_active {
                        color::WHITE
                    } else {
                        color::GREY_2
                    },
                    border: iced::Border {
                        radius: 50.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .on_press(LiquidReceiveMessage::ToggleMethod(ReceiveMethod::Usdt)),
        )
        .style(move |_theme: &theme::Theme| container::Style {
            background: Some(Background::Color(if usdt_active {
                iced::color!(0x161716)
            } else {
                color::TRANSPARENT
            })),
            border: iced::Border {
                radius: 50.0.into(),
                color: if usdt_active {
                    color::ORANGE
                } else {
                    color::TRANSPARENT
                },
                width: if usdt_active { 0.7 } else { 0.0 },
            },
            ..Default::default()
        })
    };

    Container::new(
        Row::new()
            .push(lightning_button)
            .push(liquid_button)
            .push(onchain_button)
            .push(usdt_button),
    )
    .padding(4)
    .max_width(800.0)
    .style(|_theme: &theme::Theme| container::Style {
        background: Some(Background::Color(iced::color!(0x202020))),
        border: iced::Border {
            color: iced::color!(0x202020),
            radius: 50.0.into(),
            width: 2.0,
        },
        ..Default::default()
    })
    .into()
}

fn input_fields<'a>(
    amount_input: &'a form::Value<String>,
    description_input: &'a str,
    bitcoin_unit: BitcoinDisplayUnit,
    lightning_limits: Option<(u64, u64)>,
    loading: bool,
) -> Element<'a, LiquidReceiveMessage> {
    let is_amount_valid = match Amount::from_str_in(
        &amount_input.value,
        if matches!(bitcoin_unit, BitcoinDisplayUnit::BTC) {
            Denomination::Bitcoin
        } else {
            Denomination::Satoshi
        },
    ) {
        Ok(amount) => {
            if let Some((min_sat, max_sat)) = lightning_limits {
                let min_sat = Amount::from_sat(min_sat);
                let max_sat = Amount::from_sat(max_sat);
                amount >= min_sat && amount <= max_sat
            } else {
                false
            }
        }
        Err(_) => false,
    };

    let mut hints = Column::new().spacing(4);
    if let Some((min_sat, max_sat)) = lightning_limits {
        let min_btc = Amount::from_sat(min_sat);
        let max_btc = Amount::from_sat(max_sat);
        hints = hints.push(
            text(format!(
                "Enter an amount between {} and {}",
                min_btc.to_formatted_string_with_unit(bitcoin_unit),
                max_btc.to_formatted_string_with_unit(bitcoin_unit),
            ))
            .size(12)
            .style(theme::text::secondary),
        );
    }

    let amount_form = if matches!(bitcoin_unit, BitcoinDisplayUnit::BTC) {
        form::Form::new_amount_btc("Enter amount", amount_input, |v| {
            LiquidReceiveMessage::AmountInput(v)
        })
        .size(16)
        .padding(15)
    } else {
        form::Form::new_amount_sats("Enter amount", amount_input, |v| {
            LiquidReceiveMessage::AmountInput(v)
        })
        .size(16)
        .padding(15)
    };

    let description_input_field = TextInput::new("Description (optional)", description_input)
        .on_input(LiquidReceiveMessage::DescriptionInput)
        .padding(15)
        .size(16)
        .width(Length::Fill);

    let next_btn = Container::new(
        iced::widget::button(
            Container::new(icon::arrow_right())
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(Alignment::Center)
                .align_y(Alignment::Center),
        )
        .on_press_maybe(
            (is_amount_valid && !loading).then_some(LiquidReceiveMessage::GenerateAddress),
        )
        .width(Length::Fixed(50.0))
        .height(Length::Fixed(50.0))
        .style(theme::button::primary),
    )
    .width(Length::Fixed(50.0))
    .height(Length::Fixed(50.0));

    Container::new(
        Column::new()
            .spacing(12)
            .width(Length::Fill)
            .push(h4_bold("Invoice details"))
            .push(hints)
            .push(
                Row::new()
                    .spacing(10)
                    .align_y(Alignment::Center)
                    .push(amount_form)
                    .push(description_input_field)
                    .push(next_btn),
            ),
    )
    .padding(16)
    .width(Length::Fill)
    .style(theme::card::simple)
    .into()
}

fn usdt_input_fields<'a>(
    usdt_amount_input: &'a form::Value<String>,
    loading: bool,
) -> Element<'a, LiquidReceiveMessage> {
    let is_valid = usdt_amount_input.value.trim().is_empty()
        || parse_asset_to_minor_units(usdt_amount_input.value.trim(), USDT_PRECISION)
            .is_some_and(|units| units > 0);

    let amount_input = TextInput::new(
        "USDt amount (optional, e.g. 1.50)",
        &usdt_amount_input.value,
    )
    .on_input(LiquidReceiveMessage::UsdtAmountInput)
    .padding(15)
    .size(16)
    .width(Length::Fill);

    let next_btn = Container::new(
        iced::widget::button(
            Container::new(icon::arrow_right())
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(Alignment::Center)
                .align_y(Alignment::Center),
        )
        .on_press_maybe((is_valid && !loading).then_some(LiquidReceiveMessage::GenerateAddress))
        .width(Length::Fixed(50.0))
        .height(Length::Fixed(50.0))
        .style(theme::button::primary),
    )
    .width(Length::Fixed(50.0))
    .height(Length::Fixed(50.0));

    Container::new(
        Column::new()
            .spacing(12)
            .width(Length::Fill)
            .push(h4_bold("USDt amount"))
            .push(
                Row::new()
                    .spacing(10)
                    .align_y(Alignment::Center)
                    .push(amount_input)
                    .push(next_btn),
            ),
    )
    .padding(16)
    .width(Length::Fill)
    .style(theme::card::simple)
    .into()
}

fn generate_button<'a>() -> Element<'a, LiquidReceiveMessage> {
    Container::new(
        button::primary(None, "Generate Address")
            .on_press(LiquidReceiveMessage::GenerateAddress)
            .width(Length::Fixed(200.0))
            .padding(15),
    )
    .width(Length::Fill)
    .center_x(Length::Fill)
    .into()
}

fn onchain_warning_box<'a>(
    onchain_limits: Option<(u64, u64)>,
    bitcoin_unit: BitcoinDisplayUnit,
) -> Element<'a, LiquidReceiveMessage> {
    let mut warning_content = Column::new().spacing(8).push(
        Row::new()
            .spacing(8)
            .push(icon::warning_icon().size(16).color(color::ORANGE))
            .push(text("Bitcoin onchain → L-BTC swap").size(14).bold().style(
                |_theme: &theme::Theme| iced::widget::text::Style {
                    color: Some(color::ORANGE),
                },
            )),
    );

    if let Some((min_sat, max_sat)) = onchain_limits {
        let min_btc = Amount::from_sat(min_sat);
        let max_btc = Amount::from_sat(max_sat);
        warning_content = warning_content.push(
            text(format!(
                "- Send between {} and {} — outside this range the swap cannot settle.",
                min_btc.to_formatted_string_with_unit(bitcoin_unit),
                max_btc.to_formatted_string_with_unit(bitcoin_unit),
            ))
            .size(14)
            .style(theme::text::secondary),
        );
    } else {
        warning_content = warning_content.push(
            text("- Fetching current swap limits…")
                .size(14)
                .style(theme::text::secondary),
        );
    }

    warning_content = warning_content
        .push(
            text("- Use this address for ONE deposit only.")
                .size(14)
                .style(theme::text::secondary),
        )
        .push(
            text(
                "- If your deposit is outside the range or the swap fails, \
                 funds are recoverable via Refund in the Transactions tab.",
            )
            .size(14)
            .style(theme::text::secondary),
        );

    Container::new(warning_content)
        .padding(15)
        .width(Length::Fill)
        .max_width(600)
        .style(|_theme: &theme::Theme| container::Style {
            background: Some(Background::Color(iced::color!(0x2A2520))),
            border: iced::Border {
                color: color::ORANGE,
                width: 1.0,
                radius: 8.0.into(),
            },
            ..Default::default()
        })
        .into()
}

/// Standalone USDt receive view — no tab bar, only USDt address generation.
pub fn usdt_only_receive_view<'a>(
    address: Option<&'a String>,
    qr_data: Option<&'a qr_code::Data>,
    loading: bool,
    usdt_amount_input: &'a form::Value<String>,
    error: Option<&'a String>,
) -> Element<'a, LiquidReceiveMessage> {
    let mut content = Column::new()
        .spacing(40)
        .width(Length::Fill)
        .align_x(Alignment::Center)
        .padding(40);

    if address.is_none() || loading {
        content = content.push(usdt_input_fields(usdt_amount_input, loading));
    }

    if loading {
        content = content.push(crate::loading::loading_indicator(Some(
            "Generating Address",
        )));
    } else if let (Some(addr), Some(qr)) = (address, qr_data) {
        let address_row = Row::new()
            .spacing(8)
            .align_y(Alignment::Center)
            .push(
                Container::new(
                    text(addr)
                        .size(12)
                        .font(iced::Font::MONOSPACE)
                        .style(theme::text::secondary)
                        .wrapping(iced::widget::text::Wrapping::Glyph),
                )
                .padding([8, 12])
                .style(theme::card::simple)
                .width(Length::Fill),
            )
            .push(
                iced::widget::button(icon::clipboard_icon().size(16))
                    .on_press(LiquidReceiveMessage::Copy)
                    .style(theme::button::transparent_border),
            );

        content = content.push(
            Column::new()
                .spacing(20)
                .align_x(Alignment::Center)
                .push(
                    Container::new(QRCode::<theme::Theme>::new(qr).cell_size(6))
                        .padding(20)
                        .style(theme::card::simple),
                )
                .push(address_row)
                .push(
                    text(
                        "Share this address to receive USDt (Liquid Tether) from any Liquid wallet",
                    )
                    .size(13)
                    .style(theme::text::secondary),
                )
                .push(
                    Container::new(
                        button::secondary(None, "Generate New Address")
                            .on_press(LiquidReceiveMessage::GenerateAddress)
                            .width(Length::Fixed(200.0))
                            .padding(10),
                    )
                    .width(Length::Fill)
                    .center_x(Length::Fill),
                ),
        );
    }

    if let Some(err) = error {
        Column::new()
            .push(
                Container::new(
                    Container::new(text(err).size(14).color(color::RED))
                        .padding(10)
                        .center_x(Length::Fill)
                        .style(theme::card::error)
                        .width(Length::Fill)
                        .max_width(800),
                )
                .width(Length::Fill)
                .padding([20, 40])
                .align_x(Alignment::Center),
            )
            .push(content)
            .into()
    } else {
        content.into()
    }
}

// ── Two-card layout ─────────────────────────────────────────────────────────

fn receive_cards(
    receive_asset: SendAsset,
    sender_network: SenderNetwork,
    btc_balance: Amount,
    usdt_balance: u64,
    bitcoin_unit: BitcoinDisplayUnit,
) -> Element<'static, LiquidReceiveMessage> {
    let they_send_card = {
        let asset_label = match (receive_asset, sender_network) {
            (SendAsset::Lbtc, SenderNetwork::Lightning) => "BTC",
            (SendAsset::Lbtc, SenderNetwork::Bitcoin) => "BTC",
            (SendAsset::Lbtc, _) => "L-BTC",
            (SendAsset::Usdt, _) => "USDt",
        };
        let (asset_slug, network_slug) = match (receive_asset, sender_network) {
            (SendAsset::Lbtc, SenderNetwork::Lightning) => ("btc", "lightning"),
            (SendAsset::Lbtc, SenderNetwork::Bitcoin) => ("btc", "bitcoin"),
            (SendAsset::Lbtc, _) => ("lbtc", "liquid"),
            (SendAsset::Usdt, SenderNetwork::Ethereum) => ("usdt", "ethereum"),
            (SendAsset::Usdt, SenderNetwork::Tron) => ("usdt", "tron"),
            (SendAsset::Usdt, SenderNetwork::Binance) => ("usdt", "bsc"),
            (SendAsset::Usdt, SenderNetwork::Solana) => ("usdt", "solana"),
            (SendAsset::Usdt, _) => ("usdt", "liquid"),
        };
        let ico: Element<'static, LiquidReceiveMessage> =
            asset_network_logo(asset_slug, network_slug, 40.0);

        let network_label = sender_network.display_name();

        let card_content = Column::new()
            .spacing(6)
            .push(
                text("THEY SEND")
                    .size(P2_SIZE)
                    .style(theme::text::secondary),
            )
            .push(
                Row::new()
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .push(ico)
                    .push(
                        text(asset_label)
                            .size(H3_SIZE)
                            .bold()
                            .style(theme::text::primary),
                    ),
            )
            .push(
                Container::new(
                    text(network_label.to_uppercase())
                        .size(11)
                        .color(color::ORANGE),
                )
                .padding([2, 8])
                .style(|_: &theme::Theme| container::Style {
                    border: iced::Border {
                        color: color::ORANGE,
                        width: 1.0,
                        radius: 8.0.into(),
                    },
                    ..Default::default()
                }),
            );

        iced_button(
            Container::new(card_content)
                .padding(16)
                .width(Length::Fill)
                .height(Length::Fixed(160.0))
                .style(theme::card::simple),
        )
        .padding(0)
        .on_press(LiquidReceiveMessage::OpenSenderPicker)
        .style(|_: &theme::Theme, status| iced::widget::button::Style {
            background: Some(Background::Color(color::TRANSPARENT)),
            border: iced::Border {
                color: if matches!(status, iced::widget::button::Status::Hovered) {
                    color::ORANGE
                } else {
                    color::TRANSPARENT
                },
                width: 1.0,
                radius: 16.0.into(),
            },
            ..Default::default()
        })
    };

    let you_receive_card = {
        let asset_label = match receive_asset {
            SendAsset::Lbtc => "L-BTC",
            SendAsset::Usdt => "USDt",
        };
        let asset_slug = match receive_asset {
            SendAsset::Lbtc => "lbtc",
            SendAsset::Usdt => "usdt",
        };
        let ico: Element<'static, LiquidReceiveMessage> =
            asset_network_logo(asset_slug, "liquid", 40.0);

        let balance_text = match receive_asset {
            SendAsset::Lbtc => format!(
                "Balance: {}",
                btc_balance.to_formatted_string_with_unit(bitcoin_unit)
            ),
            SendAsset::Usdt => format!("Balance: {} USDt", format_usdt_display(usdt_balance)),
        };

        let card_content = Column::new()
            .spacing(6)
            .push(
                text("YOU RECEIVE")
                    .size(P2_SIZE)
                    .style(theme::text::secondary),
            )
            .push(
                Row::new()
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .push(ico)
                    .push(
                        text(asset_label)
                            .size(H3_SIZE)
                            .bold()
                            .style(theme::text::primary),
                    ),
            )
            .push(
                text(balance_text)
                    .size(P2_SIZE)
                    .style(theme::text::secondary),
            )
            .push(
                Container::new(text("LIQUID").size(11).color(color::ORANGE))
                    .padding([2, 8])
                    .style(|_: &theme::Theme| container::Style {
                        border: iced::Border {
                            color: color::ORANGE,
                            width: 1.0,
                            radius: 8.0.into(),
                        },
                        ..Default::default()
                    }),
            );

        iced_button(
            Container::new(card_content)
                .padding(16)
                .width(Length::Fill)
                .height(Length::Fixed(160.0))
                .style(theme::card::simple),
        )
        .padding(0)
        .on_press(LiquidReceiveMessage::OpenReceivePicker)
        .style(|_: &theme::Theme, status| iced::widget::button::Style {
            background: Some(Background::Color(color::TRANSPARENT)),
            border: iced::Border {
                color: if matches!(status, iced::widget::button::Status::Hovered) {
                    color::ORANGE
                } else {
                    color::TRANSPARENT
                },
                width: 1.0,
                radius: 16.0.into(),
            },
            ..Default::default()
        })
    };

    let arrow = text("←").size(H3_SIZE).style(theme::text::secondary);

    Row::new()
        .spacing(12)
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .push(Container::new(you_receive_card).width(Length::FillPortion(1)))
        .push(arrow)
        .push(Container::new(they_send_card).width(Length::FillPortion(1)))
        .into()
}

// ── Picker modals ───────────────────────────────────────────────────────────

/// Full-screen celebration view when a payment is received.
pub fn received_celebration_page<'a>(
    context: &str,
    amount_display: &'a str,
    quote: &'a coincube_ui::component::quote_display::Quote,
    image_handle: &'a iced::widget::image::Handle,
) -> Element<'a, LiquidReceiveMessage> {
    coincube_ui::component::received_celebration_page(
        context,
        amount_display,
        quote,
        image_handle,
        LiquidReceiveMessage::DismissCelebration,
    )
}

/// QR code modal overlay (matches Vault receive pattern).
pub fn qr_modal<'a>(
    qr: &'a qr_code::Data,
    _address: &'a str,
    receive_method: &ReceiveMethod,
) -> Element<'a, LiquidReceiveMessage> {
    let cell_size = if *receive_method == ReceiveMethod::Lightning {
        5
    } else {
        8
    };

    Column::new()
        .push(
            Row::new()
                .push(Space::new().width(Length::Fill))
                .push(
                    Container::new(
                        QRCode::<coincube_ui::theme::Theme>::new(qr).cell_size(cell_size),
                    )
                    .padding(10),
                )
                .push(Space::new().width(Length::Fill)),
        )
        .width(Length::Fill)
        .max_width(400)
        .into()
}

/// "You Receive" asset picker modal.
pub fn receive_asset_picker_modal(current: SendAsset) -> Element<'static, LiquidReceiveMessage> {
    let title = text("YOU RECEIVE").size(16).bold();

    let lbtc_row = receive_picker_row(
        asset_network_logo("lbtc", "liquid", 36.0),
        "L-BTC",
        "Liquid",
        current == SendAsset::Lbtc,
        LiquidReceiveMessage::SetReceiveAsset(SendAsset::Lbtc),
    );

    let usdt_row = receive_picker_row(
        asset_network_logo("usdt", "liquid", 36.0),
        "USDt",
        "Liquid",
        current == SendAsset::Usdt,
        LiquidReceiveMessage::SetReceiveAsset(SendAsset::Usdt),
    );

    Column::new()
        .spacing(16)
        .padding(24)
        .max_width(420)
        .push(title)
        .push(lbtc_row)
        .push(usdt_row)
        .into()
}

/// "They Send" network picker modal.
pub fn sender_network_picker_modal(
    receive_asset: SendAsset,
    current_network: SenderNetwork,
) -> Element<'static, LiquidReceiveMessage> {
    let title = text("THEY SEND").size(16).bold();

    let options = SenderNetwork::options_for_receive_asset(receive_asset);

    let mut col = Column::new()
        .spacing(8)
        .padding(24)
        .max_width(420)
        .push(title);

    for network in options {
        let is_selected = network == current_network;
        let (asset_slug, label, net_slug, net_label) = match network {
            SenderNetwork::Lightning => ("btc", "BTC", "lightning", "Lightning"),
            SenderNetwork::Liquid if receive_asset == SendAsset::Usdt => {
                ("usdt", "USDt", "liquid", "Liquid")
            }
            SenderNetwork::Liquid => ("lbtc", "L-BTC", "liquid", "Liquid"),
            SenderNetwork::Bitcoin => ("btc", "BTC", "bitcoin", "Bitcoin"),
            SenderNetwork::Ethereum => ("usdt", "USDt", "ethereum", "Ethereum"),
            SenderNetwork::Tron => ("usdt", "USDt", "tron", "Tron"),
            SenderNetwork::Binance => ("usdt", "USDt", "bsc", "Binance"),
            SenderNetwork::Solana => ("usdt", "USDt", "solana", "Solana"),
        };
        let ico: Element<'static, LiquidReceiveMessage> =
            asset_network_logo(asset_slug, net_slug, 36.0);

        col = col.push(receive_picker_row(
            ico,
            label,
            net_label,
            is_selected,
            LiquidReceiveMessage::SetSenderNetwork(network),
        ));
    }

    col.into()
}

fn receive_picker_row<'a>(
    ico: impl Into<Element<'a, LiquidReceiveMessage>>,
    label: &str,
    network: &str,
    is_selected: bool,
    on_press: LiquidReceiveMessage,
) -> Element<'a, LiquidReceiveMessage> {
    let mut row = Row::new()
        .spacing(12)
        .align_y(Alignment::Center)
        .push(ico)
        .push(
            Column::new().spacing(2).push(
                text(label.to_string())
                    .size(14)
                    .bold()
                    .style(theme::text::primary),
            ),
        )
        .push(
            Container::new(text(network.to_uppercase()).size(10).color(color::ORANGE))
                .padding([2, 6])
                .style(|_: &theme::Theme| container::Style {
                    border: iced::Border {
                        color: color::ORANGE,
                        width: 1.0,
                        radius: 6.0.into(),
                    },
                    ..Default::default()
                }),
        )
        .push(Space::new().width(Length::Fill));

    if is_selected {
        row = row.push(icon::check2_icon().size(18).color(color::ORANGE));
    }

    iced_button(
        Container::new(row)
            .padding([12, 16])
            .width(Length::Fill)
            .style(if is_selected {
                picker_row_selected
            } else {
                theme::card::simple
            }),
    )
    .on_press(on_press)
    .style(|_: &theme::Theme, _| iced::widget::button::Style {
        background: Some(Background::Color(color::TRANSPARENT)),
        border: iced::Border {
            radius: 12.0.into(),
            ..Default::default()
        },
        ..Default::default()
    })
    .width(Length::Fill)
    .into()
}

/// Selected row in picker modals — orange border with subtle tinted background.
fn picker_row_selected(theme: &theme::Theme) -> iced::widget::container::Style {
    let bg = match theme.mode {
        coincube_ui::theme::palette::ThemeMode::Dark => iced::color!(0x1a1a10),
        coincube_ui::theme::palette::ThemeMode::Light => iced::color!(0xFFF5E6),
    };
    iced::widget::container::Style {
        background: Some(Background::Color(bg)),
        border: iced::Border {
            color: color::ORANGE,
            width: 1.0,
            radius: 12.0.into(),
        },
        ..Default::default()
    }
}
