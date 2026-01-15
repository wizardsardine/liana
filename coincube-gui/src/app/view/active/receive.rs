use coincube_core::miniscript::bitcoin::Amount;

use coincube_ui::{
    color,
    component::{button, form, text::*},
    icon, theme,
    widget::*,
};
use iced::{
    widget::{
        button as iced_button, container, qr_code, text::Wrapping, Column, Container, QRCode, Row,
        TextInput,
    },
    Alignment, Background, Length,
};

use crate::app::{
    settings::unit::BitcoinDisplayUnit,
    view::{ActiveReceiveMessage, ReceiveMethod},
};

pub fn active_receive_view<'a>(
    receive_method: &'a ReceiveMethod,
    address: Option<&'a String>,
    qr_data: Option<&'a qr_code::Data>,
    loading: bool,
    amount_input: &'a form::Value<String>,
    description_input: &'a str,
    bitcoin_unit: BitcoinDisplayUnit,
    error: Option<&'a String>,
) -> Element<'a, ActiveReceiveMessage> {
    let mut content = Column::new()
        .spacing(40)
        .width(Length::Fill)
        .align_x(Alignment::Center)
        .padding(40);

    content = content.push(method_toggle(receive_method));

    // Show input fields only for Lightning
    if *receive_method == ReceiveMethod::Lightning {
        content = content.push(input_fields(amount_input, description_input, bitcoin_unit));
    } else {
        // For on-chain, only show generate button if no address is displayed
        if address.is_none() && !loading {
            content = content.push(generate_button());
        }
    }

    if loading {
        content = content.push(
            Container::new(
                Column::new()
                    .spacing(20)
                    .align_x(Alignment::Center)
                    .push(text("Generating address...").size(18)),
            )
            .width(Length::Fill)
            .center_x(Length::Fill),
        );
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
                .push(action_buttons(receive_method)),
        );

        // Add generate new address button for on-chain
        if *receive_method == ReceiveMethod::OnChain {
            content = content.push(
                Container::new(
                    button::secondary(None, "Generate New Address")
                        .on_press(ActiveReceiveMessage::GenerateAddress)
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

fn method_toggle(current_method: &ReceiveMethod) -> Element<ActiveReceiveMessage> {
    let lightning_active = *current_method == ReceiveMethod::Lightning;
    let onchain_active = *current_method == ReceiveMethod::OnChain;

    let lightning_button = {
        let icon = icon::lightning_icon()
            .size(18)
            .style(move |_theme: &theme::Theme| iced::widget::text::Style {
                color: Some(if lightning_active {
                    color::WHITE
                } else {
                    color::GREY_2
                }),
            });

        let label = text("Lightning")
            .size(16)
            .style(move |_theme: &theme::Theme| iced::widget::text::Style {
                color: Some(if lightning_active {
                    color::WHITE
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
                    text_color: if lightning_active {
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
                .on_press(ActiveReceiveMessage::ToggleMethod(ReceiveMethod::Lightning)),
        )
        .style(move |_theme: &theme::Theme| container::Style {
            background: Some(Background::Color(if lightning_active {
                iced::color!(0x161716)
            } else {
                color::TRANSPARENT
            })),
            border: iced::Border {
                radius: 50.0.into(),
                color: if lightning_active {
                    color::ORANGE
                } else {
                    color::TRANSPARENT
                },
                width: if lightning_active { 0.7 } else { 0.0 },
            },
            ..Default::default()
        })
    };

    let onchain_button = {
        let icon = icon::bitcoin_icon()
            .size(18)
            .style(move |_theme: &theme::Theme| iced::widget::text::Style {
                color: Some(if onchain_active {
                    color::WHITE
                } else {
                    color::GREY_2
                }),
            });

        let label = text("On-chain")
            .size(16)
            .style(move |_theme: &theme::Theme| iced::widget::text::Style {
                color: Some(if onchain_active {
                    color::WHITE
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
                    text_color: if onchain_active {
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
                .on_press(ActiveReceiveMessage::ToggleMethod(ReceiveMethod::OnChain)),
        )
        .style(move |_theme: &theme::Theme| container::Style {
            background: Some(Background::Color(if onchain_active {
                iced::color!(0x161716)
            } else {
                color::TRANSPARENT
            })),
            border: iced::Border {
                radius: 50.0.into(),
                color: if onchain_active {
                    color::ORANGE
                } else {
                    color::TRANSPARENT
                },
                width: if onchain_active { 0.7 } else { 0.0 },
            },
            ..Default::default()
        })
    };

    Container::new(Row::new().push(lightning_button).push(onchain_button))
        .padding(4)
        .max_width(800.0)
        .style(|_theme: &theme::Theme| container::Style {
            background: Some(Background::Color(iced::color!(0x202020))),
            border: iced::Border {
                color: iced::color!(0x202020),
                radius: 50.0.into(),
                width: 50.0.into(),
            },
            ..Default::default()
        })
        .into()
}

fn input_fields<'a>(
    amount_input: &'a form::Value<String>,
    description_input: &'a str,
    bitcoin_unit: BitcoinDisplayUnit,
) -> Element<'a, ActiveReceiveMessage> {
    let amount_field = Column::new()
        .spacing(8)
        .push(
            text(format!("Amount ({})", bitcoin_unit.to_string()))
                .size(14)
                .style(theme::text::secondary),
        )
        .push(if matches!(bitcoin_unit, BitcoinDisplayUnit::BTC) {
            form::Form::new_amount_btc("Enter amount", amount_input, |v| {
                ActiveReceiveMessage::AmountInput(v)
            })
            .padding(10)
        } else {
            form::Form::new_amount_sats("Enter amount", amount_input, |v| {
                ActiveReceiveMessage::AmountInput(v)
            })
            .padding(10)
        });

    let description_field = Column::new()
        .spacing(8)
        .push(text("Description").size(14).style(theme::text::secondary))
        .push(
            TextInput::new("Optional", description_input)
                .on_input(ActiveReceiveMessage::DescriptionInput)
                .padding(12)
                .width(Length::Fill),
        );

    let is_amount_valid = match Amount::from_str_in(
        &amount_input.value,
        if matches!(bitcoin_unit, BitcoinDisplayUnit::BTC) {
            breez_sdk_liquid::bitcoin::Denomination::Bitcoin
        } else {
            breez_sdk_liquid::bitcoin::Denomination::Satoshi
        },
    ) {
        Ok(amount) => {
            if amount.eq(&Amount::ZERO) {
                false
            } else {
                true
            }
        }
        Err(_) => false,
    };

    let generate_btn = button::primary(None, "Generate Invoice")
        .on_press_maybe(is_amount_valid.then_some(ActiveReceiveMessage::GenerateAddress))
        .width(Length::Fill)
        .padding(5);

    Container::new(
        Column::new()
            .spacing(15)
            .max_width(500)
            .push(amount_field)
            .push(description_field)
            .push(generate_btn),
    )
    .width(Length::Fill)
    .center_x(Length::Fill)
    .into()
}

fn generate_button<'a>() -> Element<'a, ActiveReceiveMessage> {
    Container::new(
        button::primary(None, "Generate Address")
            .on_press(ActiveReceiveMessage::GenerateAddress)
            .width(Length::Fixed(200.0))
            .padding(15),
    )
    .width(Length::Fill)
    .center_x(Length::Fill)
    .into()
}

fn action_buttons(_receive_method: &ReceiveMethod) -> Element<ActiveReceiveMessage> {
    let copy_button = button::primary(Some(icon::clipboard_icon()), "Copy")
        .on_press(ActiveReceiveMessage::Copy)
        .width(Length::Fixed(150.0))
        .padding(15);

    Row::new().spacing(15).push(copy_button).into()
}
