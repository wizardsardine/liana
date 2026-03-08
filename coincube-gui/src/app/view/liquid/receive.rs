use coincube_core::miniscript::bitcoin::{Amount, Denomination};

use coincube_ui::{
    color,
    component::{amount::DisplayAmount, button, form, text::*},
    icon, theme,
    widget::*,
};
use iced::{
    widget::{
        button as iced_button, container, qr_code, text::Wrapping, Column, Container, QRCode, Row,
        Space, TextInput,
    },
    Alignment, Background, Length,
};

use crate::app::{
    settings::unit::BitcoinDisplayUnit,
    view::{LiquidReceiveMessage, ReceiveMethod},
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
) -> Element<'a, LiquidReceiveMessage> {
    let mut content = Column::new()
        .spacing(40)
        .width(Length::Fill)
        .align_x(Alignment::Center)
        .padding(40);

    content = content.push(method_toggle(receive_method));

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
                content = content.push(generate_button());
            }
        }
    }

    if loading {
        content = content.push(crate::loading::loading_indicator(Some(
            "Generating Address",
        )));
    } else if let (Some(addr), Some(qr)) = (address, qr_data) {
        // Lightning invoices contain more data, so use smaller cell size
        let cell_size = if *receive_method == ReceiveMethod::Lightning {
            4
        } else {
            8
        };

        // Clean on-chain addresses for display (but keep original for QR code)
        let display_addr = if *receive_method == ReceiveMethod::OnChain {
            let cleaned = addr.strip_prefix("bitcoin:").unwrap_or(addr);
            cleaned.split('?').next().unwrap_or(cleaned)
        } else {
            addr
        };

        content = content.push(
            Column::new()
                .spacing(30)
                .align_x(Alignment::Center)
                .push(
                    Container::new(QRCode::<theme::Theme>::new(qr).cell_size(cell_size))
                        .padding(30)
                        .style(theme::card::simple),
                )
                .push(
                    Container::new(
                        text(display_addr)
                            .size(12)
                            .style(theme::text::secondary)
                            .wrapping(Wrapping::Glyph),
                    )
                    .width(Length::Fill)
                    .max_width(600)
                    .padding(10)
                    .center_x(Length::Fill),
                )
                .push(action_buttons(receive_method, onchain_limits, bitcoin_unit)),
        );

        // Add generate new address button for on-chain
        if *receive_method == ReceiveMethod::OnChain || *receive_method == ReceiveMethod::Usdt {
            content = content.push(
                Container::new(
                    button::secondary(None, "Generate New Address")
                        .on_press(LiquidReceiveMessage::GenerateAddress)
                        .width(Length::Fixed(200.0))
                        .padding(10),
                )
                .width(Length::Fill)
                .center_x(Length::Fill)
                .padding(10),
            );
        }
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

        let label = text("On-chain")
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
        let label = text("USDt")
            .size(16)
            .style(move |_theme: &theme::Theme| iced::widget::text::Style {
                color: Some(if usdt_active { color::ORANGE } else { color::GREY_2 }),
            });

        let button_content = Container::new(
            Row::new()
                .spacing(8)
                .align_y(iced::alignment::Vertical::Center)
                .push(label),
        )
        .width(Length::Fill)
        .align_x(iced::alignment::Horizontal::Center);

        Container::new(
            iced_button(Container::new(button_content).padding([10, 30]))
                .style(move |_theme: &theme::Theme, _status| iced_button::Style {
                    background: Some(Background::Color(color::TRANSPARENT)),
                    text_color: if usdt_active { color::WHITE } else { color::GREY_2 },
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
                color: if usdt_active { color::ORANGE } else { color::TRANSPARENT },
                width: if usdt_active { 0.7 } else { 0.0 },
            },
            ..Default::default()
        })
    };

    Container::new(
        Row::new()
            .push(lightning_button)
            .push(liquid_button)
            .push(usdt_button)
            .push(onchain_button),
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
    let mut amount_field = Column::new()
        .spacing(5)
        .push(
            text(format!("Amount ({})", bitcoin_unit))
                .size(14)
                .style(theme::text::secondary),
        )
        .push(if matches!(bitcoin_unit, BitcoinDisplayUnit::BTC) {
            form::Form::new_amount_btc("Enter amount", amount_input, |v| {
                LiquidReceiveMessage::AmountInput(v)
            })
            .padding(10)
        } else {
            form::Form::new_amount_sats("Enter amount", amount_input, |v| {
                LiquidReceiveMessage::AmountInput(v)
            })
            .padding(10)
        });

    if let Some((min_sat, max_sat)) = lightning_limits {
        let min_btc = Amount::from_sat(min_sat);
        let max_btc = Amount::from_sat(max_sat);
        amount_field = amount_field.push(
            text(format!(
                "Enter an amount between {} and {}",
                min_btc.to_formatted_string_with_unit(bitcoin_unit),
                max_btc.to_formatted_string_with_unit(bitcoin_unit),
            ))
            .size(12),
        );
    }

    let description_field = Column::new()
        .spacing(8)
        .push(text("Description").size(14).style(theme::text::secondary))
        .push(
            TextInput::new("Optional", description_input)
                .on_input(LiquidReceiveMessage::DescriptionInput)
                .padding(12)
                .width(Length::Fill),
        );

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

    let generate_btn = button::primary(None, "Generate Invoice")
        .on_press_maybe(
            (is_amount_valid && !loading).then_some(LiquidReceiveMessage::GenerateAddress),
        )
        .width(Length::Fill)
        .padding(5);

    Container::new(
        Column::new()
            .spacing(15)
            .max_width(500)
            .push(amount_field)
            .push(Space::new().width(3))
            .push(description_field)
            .push(generate_btn),
    )
    .width(Length::Fill)
    .center_x(Length::Fill)
    .into()
}

fn usdt_input_fields<'a>(
    usdt_amount_input: &'a form::Value<String>,
    loading: bool,
) -> Element<'a, LiquidReceiveMessage> {
    let amount_field = Column::new()
        .spacing(5)
        .push(
            text("Amount (USDt) — Optional")
                .size(14)
                .style(theme::text::secondary),
        )
        .push(
            TextInput::new("e.g. 1.50", &usdt_amount_input.value)
                .on_input(LiquidReceiveMessage::UsdtAmountInput)
                .padding(12)
                .width(Length::Fill),
        )
        .push(
            text("Leave empty to generate an amountless address")
                .size(12)
                .style(theme::text::secondary),
        );

    let is_valid = usdt_amount_input.value.trim().is_empty()
        || usdt_amount_input.value.trim().parse::<f64>().ok().map_or(false, |v| v > 0.0);

    let generate_btn = button::primary(None, "Generate USDt Address")
        .on_press_maybe((is_valid && !loading).then_some(LiquidReceiveMessage::GenerateAddress))
        .width(Length::Fill)
        .padding(5);

    Container::new(
        Column::new()
            .spacing(15)
            .max_width(500)
            .push(amount_field)
            .push(generate_btn),
    )
    .width(Length::Fill)
    .center_x(Length::Fill)
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

fn action_buttons<'a>(
    receive_method: &ReceiveMethod,
    onchain_limits: Option<(u64, u64)>,
    bitcoin_unit: BitcoinDisplayUnit,
) -> Element<'a, LiquidReceiveMessage> {
    let copy_button = button::primary(Some(icon::clipboard_icon()), "Copy")
        .on_press(LiquidReceiveMessage::Copy)
        .width(Length::Fixed(150.0))
        .padding(15);

    let mut column = Column::new()
        .spacing(15)
        .align_x(Alignment::Center)
        .push(Row::new().spacing(15).push(copy_button));

    if *receive_method == ReceiveMethod::Liquid {
        column = column.push(
            text("Share this address to receive L-BTC from any Liquid wallet")
                .size(13)
                .style(theme::text::secondary),
        );
    }

    if *receive_method == ReceiveMethod::Usdt {
        column = column.push(
            text("Share this address to receive USDt (Liquid Tether) from any Liquid wallet")
                .size(13)
                .style(theme::text::secondary),
        );
    }

    if *receive_method == ReceiveMethod::OnChain {
        let mut warning_content = Column::new().spacing(8).push(
            Row::new()
                .spacing(8)
                .push(icon::warning_icon().size(16).color(color::ORANGE))
                .push(
                    text("Important")
                        .size(14)
                        .bold()
                        .style(|_theme: &theme::Theme| iced::widget::text::Style {
                            color: Some(color::ORANGE),
                        }),
                ),
        );

        if let Some((min_sat, max_sat)) = onchain_limits {
            let min_btc = Amount::from_sat(min_sat);
            let max_btc = Amount::from_sat(max_sat);
            warning_content = warning_content.push(
                text(format!(
                    "- Receive amount must be between {} and {}",
                    min_btc.to_formatted_string_with_unit(bitcoin_unit),
                    max_btc.to_formatted_string_with_unit(bitcoin_unit),
                ))
                .size(14)
                .style(theme::text::secondary),
            );
        } else {
            warning_content = warning_content.push(
                text("- Receive amount must be within the specified limits")
                    .size(14)
                    .style(theme::text::secondary),
            );
        }

        warning_content = warning_content
            .push(
                text("- Use this address for ONE transaction only")
                    .size(14)
                    .style(theme::text::secondary),
            )
            .push(
                text("- For multiple transactions, generate new addresses")
                    .size(14)
                    .style(theme::text::secondary),
            );

        let warning_box = Container::new(warning_content)
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
            });

        column = column.push(warning_box);
    }

    column.into()
}
