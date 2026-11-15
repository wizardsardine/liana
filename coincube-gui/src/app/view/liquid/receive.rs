use coincube_core::miniscript::bitcoin::{Amount, Denomination};
use coincube_ui::locale::copy as localized_copy;
use coincube_ui::localized_format;

use crate::app::wallets::{DomainPaymentDetails, DomainPaymentStatus};

use coincube_ui::{
    color,
    component::{
        amount::DisplayAmount,
        button, card, form,
        tenshu::{segment, segmented},
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
    cube_name: &'a str,
    network: coincube_core::miniscript::bitcoin::Network,
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
    stablecoin_available: bool,
    recent_transaction: &[RecentTransaction],
    btc_balance: Amount,
    usdt_balance: u64,
    show_direction_badges: bool,
    mask_amounts: bool,
) -> Element<'a, LiquidReceiveMessage> {
    let mut content = Column::new()
        .spacing(16)
        .width(Length::Fill)
        .push(
            Container::new(
                Row::new()
                    .spacing(12)
                    .align_y(Alignment::Center)
                    .push(h4_bold(localized_copy("Payment details")).width(Length::Fill))
                    .push(receive_kind_selector(receive_asset, stablecoin_available)),
            )
            .padding([16, 24]),
        )
        .push(iced::widget::rule::horizontal(1));

    // Asset first, then only the receive rails implemented for that asset.
    // The sender-network picker is the validated SideShift entry for USDt;
    // it never turns prototype rail fixtures into wallet capabilities.
    content = content.push(
        Container::new(receive_method_selector(receive_asset, sender_network)).padding([0, 24]),
    );
    content = content.push(iced::widget::rule::horizontal(1));

    // ── Two-card "They Send → You Receive" layout ───────────────────────────
    content =
        content.push(Container::new(receive_cards(receive_asset, sender_network)).padding([0, 24]));

    // ── Input section (matches Send layout) ─────────────────────────────────
    match receive_method {
        ReceiveMethod::Lightning => {
            content = content.push(
                Container::new(input_fields(
                    amount_input,
                    description_input,
                    bitcoin_unit,
                    lightning_limits,
                    loading,
                ))
                .padding([0, 24]),
            );
        }
        ReceiveMethod::Usdt => {
            content = content.push(
                Container::new(usdt_input_fields(usdt_amount_input, loading)).padding([0, 24]),
            );
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
                    content = content.push(
                        Container::new(onchain_warning_box(onchain_limits, bitcoin_unit))
                            .padding([0, 24]),
                    );
                    content = content.push(crate::loading::loading_indicator(Some(
                        "Fetching swap limits",
                    )));
                } else {
                    content = content.push(Container::new(generate_button()).padding([0, 24]));
                }
            }
        }
    }

    if loading {
        content = content.push(crate::loading::loading_indicator(Some(
            "Generating Address",
        )));
    } else if let Some(addr) = address {
        // Clean on-chain addresses for display
        let display_addr = if *receive_method == ReceiveMethod::OnChain {
            let cleaned = addr.strip_prefix("bitcoin:").unwrap_or(addr);
            cleaned.split('?').next().unwrap_or(cleaned)
        } else {
            addr
        };

        // The generated request is already encoded in state. Keep the copy
        // action bound to that same request while showing its QR inline.
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
                button::primary(None, localized_copy("Copy"))
                    .on_press(LiquidReceiveMessage::Copy)
                    .width(Length::Fixed(80.0)),
            )
            .align_y(Alignment::Center);

        let mut button_row = Row::new();

        if *receive_method == ReceiveMethod::OnChain || *receive_method == ReceiveMethod::Usdt {
            button_row = button_row.push(
                button::secondary(None, localized_copy("Generate New Address"))
                    .on_press(LiquidReceiveMessage::GenerateAddress),
            );
            button_row = button_row.push(Space::new().width(Length::Fill));
        }

        if qr_data.is_some() {
            button_row = button_row.push(
                button::secondary(None, localized_copy("Enlarge QR"))
                    .on_press(LiquidReceiveMessage::ShowQrCode),
            );
        } else {
            button_row = button_row.push(p2_regular(localized_copy(
                "QR unavailable · copy the address instead",
            )));
        }

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

        let label = match receive_method {
            ReceiveMethod::Lightning => localized_copy("Lightning invoice"),
            ReceiveMethod::Liquid => localized_copy("Liquid address"),
            ReceiveMethod::OnChain => localized_copy("Swap address"),
            ReceiveMethod::Usdt => "Liquid USDt address",
        };
        let mut details = Column::new()
            .spacing(10)
            .width(Length::Fill)
            .push(p1_bold(label))
            .push(address_row);

        if let Some(desc) = description {
            details = details.push(p2_regular(desc).style(theme::text::secondary));
        }

        details = details.push(button_row);

        let ready_body: Element<'_, LiquidReceiveMessage> = if let Some(qr) = qr_data {
            Row::new()
                .spacing(24)
                .align_y(Alignment::Start)
                .width(Length::Fill)
                .push(QRCode::<coincube_ui::theme::Theme>::new(qr).total_size(160))
                .push(details)
                .into()
        } else {
            details.into()
        };

        content = content.push(Container::new(card::simple(ready_body)).padding([0, 24]));

        // OnChain warning box
        if *receive_method == ReceiveMethod::OnChain {
            content = content.push(
                Container::new(onchain_warning_box(onchain_limits, bitcoin_unit)).padding([0, 24]),
            );
        }
    }

    // ── Last transactions (matching Send screen) ────────────────────────────
    let mut recent = Column::new()
        .spacing(10)
        .push(h4_bold(localized_copy("Last transactions")));

    if recent_transaction.is_empty() {
        recent = recent.push(coincube_ui::component::empty_placeholder(
            icon::receipt_icon().size(80),
            coincube_ui::locale::copy("No transactions yet"),
            coincube_ui::locale::copy(
                "Your transaction history will appear here once you send or receive coins.",
            ),
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

            if mask_amounts {
                item = item.with_amount_override("••••••".into());
            } else if let Some(ref usdt_str) = tx.usdt_display {
                item = item.with_amount_override(usdt_str.clone());
            } else {
                let fiat_str = tx
                    .fiat_amount
                    .as_ref()
                    .map(|fiat| format!("{} {}", fiat.to_formatted_string(), fiat.currency()));
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
                            text(localized_copy("Pending"))
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
            recent = recent.push(tx_element);
        }

        // "View All Transactions" button
        let view_tx_button = {
            let the_icon = icon::history_icon()
                .size(18)
                .style(|_theme: &theme::Theme| iced::widget::text::Style {
                    color: Some(color::ORANGE),
                });

            let label = text(localized_copy("View All Transactions"))
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
                        radius: 8.0.into(),
                    },
                    ..Default::default()
                },
            ))
            .style(|_theme: &theme::Theme, _| iced_button::Style {
                background: Some(Background::Color(color::TRANSPARENT)),
                text_color: color::ORANGE,
                border: iced::Border {
                    radius: 8.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            })
            .on_press(LiquidReceiveMessage::History)
        };

        recent = recent
            .push(Space::new().height(Length::Fixed(20.0)))
            .push(
                Container::new(view_tx_button)
                    .width(Length::Fill)
                    .center_x(Length::Fill),
            )
            .push(Space::new().height(Length::Fixed(40.0)));
    }

    let network_name = match network {
        coincube_core::miniscript::bitcoin::Network::Bitcoin => "Liquid network",
        coincube_core::miniscript::bitcoin::Network::Testnet
        | coincube_core::miniscript::bitcoin::Network::Signet
        | coincube_core::miniscript::bitcoin::Network::Regtest => "Liquid test network",
        _ => "Liquid network",
    };
    let balance = if mask_amounts {
        "••••••".to_owned()
    } else if receive_asset == SendAsset::Usdt {
        format!("{} USDt", format_usdt_display(usdt_balance))
    } else {
        format!(
            "{} {}",
            btc_balance.to_formatted_string_with_unit(bitcoin_unit),
            if matches!(bitcoin_unit, BitcoinDisplayUnit::BTC) {
                "BTC"
            } else {
                "SATS"
            }
        )
    };
    let header = Column::new()
        .spacing(4)
        .push(
            iced_button(text(format!("‹ Liquid · {}", cube_name)).color(color::ORANGE))
                .padding([2, 0])
                .style(theme::button::transparent)
                .on_press(LiquidReceiveMessage::BackToOverview),
        )
        .push(h3(localized_copy("Receive")).bold())
        .push(
            p2_regular(format!("{} · {}", cube_name, network_name)).style(theme::text::secondary),
        );
    let aside = Column::new()
        .spacing(8)
        .push(p2_regular(localized_copy("Receiving into")).style(theme::text::secondary))
        .push(p1_bold(format!("Liquid · {}", cube_name)))
        .push(p2_bold(balance))
        .push(iced::widget::rule::horizontal(1))
        .push(
            button::transparent_border(None, localized_copy("See all in Activity"))
                .on_press(LiquidReceiveMessage::History),
        );
    let body = Column::new()
        .spacing(20)
        .width(Length::Fill)
        .push(header)
        .push(
            Row::new()
                .spacing(40)
                .align_y(Alignment::Start)
                .push(
                    Container::new(content)
                        .padding(iced::Padding::default().bottom(18.0))
                        .width(Length::Fixed(680.0))
                        .style(payment_card_style),
                )
                .push(Container::new(aside).width(Length::Fixed(240.0)))
                .wrap()
                .vertical_spacing(20),
        )
        .push(recent);
    if let Some(err) = error {
        Column::new()
            .push(
                Container::new(
                    Container::new(
                        text(coincube_ui::locale::notice(err.clone()))
                            .size(14)
                            .color(color::RED),
                    )
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
            .push(
                Container::new(Container::new(body).width(Length::Fill).max_width(960.0))
                    .width(Length::Fill),
            )
            .into()
    } else {
        Container::new(Container::new(body).width(Length::Fill).max_width(960.0))
            .width(Length::Fill)
            .into()
    }
}

fn receive_kind_selector(
    asset: SendAsset,
    stablecoin_available: bool,
) -> Element<'static, LiquidReceiveMessage> {
    let mut selector = Column::new().spacing(4).push(
        segmented(
            [
                segment("liquid-receive-kind-bitcoin", SendAsset::Lbtc, "Bitcoin"),
                segment(
                    "liquid-receive-kind-stablecoin",
                    SendAsset::Usdt,
                    localized_copy("Stablecoin"),
                )
                .enabled(stablecoin_available),
            ],
            Some(asset),
            LiquidReceiveMessage::FocusAction,
        )
        .on_select(LiquidReceiveMessage::SetReceiveAsset),
    );
    if !stablecoin_available {
        selector = selector.push(p2_regular(localized_copy(
            "USDt is unavailable on this network.",
        )));
    }
    selector.into()
}

fn receive_method_selector(
    asset: SendAsset,
    sender: SenderNetwork,
) -> Element<'static, LiquidReceiveMessage> {
    let methods = if asset == SendAsset::Lbtc {
        vec![
            segment(
                "liquid-receive-method-lightning",
                SenderNetwork::Lightning,
                "Lightning invoice",
            ),
            segment(
                "liquid-receive-method-liquid",
                SenderNetwork::Liquid,
                "Liquid address",
            ),
            segment(
                "liquid-receive-method-bitcoin",
                SenderNetwork::Bitcoin,
                "Bitcoin address · on-chain swap",
            ),
        ]
    } else {
        vec![segment(
            "liquid-receive-method-usdt",
            SenderNetwork::Liquid,
            "Liquid USDt address",
        )]
    };
    let row = segmented(methods, Some(sender), LiquidReceiveMessage::FocusAction)
        .on_select(LiquidReceiveMessage::SetSenderNetwork);
    let mut content = Column::new()
        .spacing(8)
        .push(p1_bold(localized_copy("Method")))
        .push(row);
    if asset == SendAsset::Usdt {
        content = content.push(
            button::secondary(None, localized_copy("From another network"))
                .on_press(LiquidReceiveMessage::OpenSenderPicker),
        );
    }
    content.into()
}

fn payment_card_style(theme: &theme::Theme) -> container::Style {
    let shell = &theme.colors.shell;
    container::Style {
        background: Some(Background::Color(shell.panel)),
        border: iced::Border {
            color: shell.line,
            width: 1.0,
            radius: 16.0.into(),
        },
        ..Default::default()
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
                        radius: 6.0.into(),
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
                radius: 6.0.into(),
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
                        radius: 6.0.into(),
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
                radius: 6.0.into(),
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
                        radius: 6.0.into(),
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
                radius: 6.0.into(),
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
                        radius: 6.0.into(),
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
                radius: 6.0.into(),
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
            radius: 10.0.into(),
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
            text(localized_format!(
                "Enter an amount between {} and {} {}",
                "Introduce un importe entre {} y {} {}",
                min_btc.to_formatted_string_with_unit(bitcoin_unit),
                max_btc.to_formatted_string_with_unit(bitcoin_unit),
                bitcoin_unit,
            ))
            .size(12)
            .style(theme::text::secondary),
        );
    }

    let amount_form = if matches!(bitcoin_unit, BitcoinDisplayUnit::BTC) {
        form::Form::new_amount_btc(
            coincube_ui::locale::copy("Enter amount"),
            amount_input,
            LiquidReceiveMessage::AmountInput,
        )
        .maybe_warning(amount_input.warning.map(coincube_ui::locale::copy))
        .size(16)
        .padding(15)
    } else {
        form::Form::new_amount_sats(
            coincube_ui::locale::copy("Enter amount"),
            amount_input,
            LiquidReceiveMessage::AmountInput,
        )
        .maybe_warning(amount_input.warning.map(coincube_ui::locale::copy))
        .size(16)
        .padding(15)
    };

    let description_input_field =
        TextInput::new(localized_copy("Description (optional)"), description_input)
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
            .push(h4_bold(localized_copy("Invoice details")))
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
        localized_copy("USDt amount (optional, e.g. 1.50)"),
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
            .push(h4_bold(localized_copy("USDt amount")))
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
        button::primary(None, localized_copy("Generate Address"))
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
            .push(
                text(localized_copy("Bitcoin onchain → L-BTC swap"))
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
            text(localized_format!(
                "- Send between {} and {} — outside this range the swap cannot settle.",
                "- Enviar entre {} y {} - fuera de esta gama el swap no puede establecerse.",
                min_btc.to_formatted_string_with_unit(bitcoin_unit),
                max_btc.to_formatted_string_with_unit(bitcoin_unit),
            ))
            .size(14)
            .style(theme::text::secondary),
        );
    } else {
        warning_content = warning_content.push(
            text(localized_copy("- Fetching current swap limits…"))
                .size(14)
                .style(theme::text::secondary),
        );
    }

    warning_content = warning_content
        .push(
            text(localized_copy("- Use this address for ONE deposit only."))
                .size(14)
                .style(theme::text::secondary),
        )
        .push(
            text(localized_copy(
                "- If your deposit is outside the range or the swap fails, \
                 funds are recoverable via Refund in the Transactions tab.",
            ))
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
                    text(localized_copy(
                        "Share this address to receive USDt (Liquid Tether) from any Liquid wallet",
                    ))
                    .size(13)
                    .style(theme::text::secondary),
                )
                .push(
                    Container::new(
                        button::secondary(None, localized_copy("Generate New Address"))
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
                    Container::new(
                        text(coincube_ui::locale::notice(err.clone()))
                            .size(14)
                            .color(color::RED),
                    )
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
                text(localized_copy("THEY SEND"))
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
                .style(route_card_style),
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
                radius: 8.0.into(),
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

        let card_content = Column::new()
            .spacing(6)
            .push(
                text(localized_copy("YOU RECEIVE"))
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
                text(localized_copy("Any amount"))
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
                .style(route_card_style),
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
                radius: 8.0.into(),
            },
            ..Default::default()
        })
    };

    let arrow = text("→").size(H3_SIZE).style(theme::text::secondary);

    Row::new()
        .spacing(12)
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .push(Container::new(they_send_card).width(Length::FillPortion(1)))
        .push(arrow)
        .push(Container::new(you_receive_card).width(Length::FillPortion(1)))
        .into()
}

fn route_card_style(theme: &theme::Theme) -> container::Style {
    let shell = &theme.colors.shell;
    container::Style {
        background: Some(Background::Color(shell.background)),
        border: iced::Border {
            color: shell.line,
            width: 1.0,
            radius: 8.0.into(),
        },
        ..Default::default()
    }
}

// ── Picker modals ───────────────────────────────────────────────────────────

/// Full-screen celebration view when a payment is received.
pub fn received_celebration_page<'a>(
    context: &str,
    amount_display: &'a str,
    quote: &'a coincube_ui::component::quote_display::Quote,
    image_handle: &'a iced::widget::image::Handle,
) -> Element<'a, LiquidReceiveMessage> {
    use coincube_ui::component::payment_result::{payment_result, Outcome};
    payment_result(
        Outcome::Received,
        amount_display,
        context,
        quote,
        image_handle,
        LiquidReceiveMessage::DismissCelebration,
    )
    .detail(localized_copy("Into Liquid"))
    .voice(coincube_ui::worldview_copy::VoiceLine::PaymentReceived.get())
    .into()
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
    let title = text(localized_copy("YOU RECEIVE")).size(16).bold();

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
    let title = text(localized_copy("THEY SEND")).size(16).bold();

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
                crate::app::view::shared::picker::picker_row_idle
            }),
    )
    .on_press(on_press)
    .style(|_: &theme::Theme, _| iced::widget::button::Style {
        background: Some(Background::Color(color::TRANSPARENT)),
        border: iced::Border {
            radius: 8.0.into(),
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
            radius: 8.0.into(),
        },
        ..Default::default()
    }
}

#[cfg(test)]
mod privacy_tests {
    use super::*;
    use crate::app::view::shared::landing::capture;

    #[test]
    fn receive_payment_hierarchy_and_focus_hold_at_compact_and_wide_sizes() {
        let input = form::Value::<String>::default();
        for (appearance, theme) in [
            ("dark", theme::Theme::dark()),
            ("light", theme::Theme::light()),
        ] {
            for outer in [(1024, 768), (1440, 900)] {
                let mounted = capture::inspect_wide_content(
                    "liquid-receive-payment-hierarchy",
                    liquid_receive_view(
                        "Everyday",
                        coincube_core::miniscript::bitcoin::Network::Bitcoin,
                        &ReceiveMethod::Liquid,
                        None,
                        None,
                        false,
                        &input,
                        &input,
                        "",
                        BitcoinDisplayUnit::Sats,
                        None,
                        None,
                        None,
                        SendAsset::Lbtc,
                        SenderNetwork::Liquid,
                        true,
                        &[],
                        Amount::from_sat(42_000),
                        0,
                        false,
                        false,
                    ),
                    outer,
                    appearance,
                    theme,
                    outer.0 - 248 - 48,
                );
                for label in [
                    "‹ Liquid",
                    "Receive",
                    "Everyday · Liquid network",
                    "Payment details",
                    "Receiving into",
                    "Method",
                    "Liquid address",
                ] {
                    assert!(
                        mounted.contains(label),
                        "{} missing at {} {:?}",
                        label,
                        appearance,
                        outer
                    );
                }
                for id in [
                    "liquid-receive-kind-bitcoin",
                    "liquid-receive-kind-stablecoin",
                    "liquid-receive-method-lightning",
                    "liquid-receive-method-liquid",
                    "liquid-receive-method-bitcoin",
                ] {
                    assert!(
                        mounted.has_focus(iced::advanced::widget::Id::new(id)),
                        "{} not focusable",
                        id
                    );
                }
                assert!(!mounted.contains("✓"));
                let x = |label| {
                    mounted
                        .text
                        .iter()
                        .find(|(value, _)| value == label)
                        .unwrap()
                        .1
                        .x
                };
                assert!(x("THEY SEND") < x("YOU RECEIVE"));
            }
        }
        let messages = capture::activate(
            receive_kind_selector(SendAsset::Lbtc, true),
            iced::advanced::widget::Id::new("liquid-receive-kind-bitcoin"),
            (1024, 768),
            false,
        );
        assert!(messages.iter().any(|message| matches!(
            message,
            LiquidReceiveMessage::SetReceiveAsset(SendAsset::Lbtc)
        )));
    }

    #[test]
    fn address_remains_copyable_when_qr_encoding_is_unavailable() {
        let input = form::Value::<String>::default();
        let address = "lq1q-test-address".to_string();
        let mounted = capture::inspect_wide_content(
            "liquid-receive-no-qr",
            liquid_receive_view(
                "Everyday",
                coincube_core::miniscript::bitcoin::Network::Bitcoin,
                &ReceiveMethod::Liquid,
                Some(&address),
                None,
                false,
                &input,
                &input,
                "",
                BitcoinDisplayUnit::Sats,
                None,
                None,
                None,
                SendAsset::Lbtc,
                SenderNetwork::Liquid,
                true,
                &[],
                Amount::ZERO,
                0,
                false,
                false,
            ),
            (1024, 768),
            "light",
            theme::Theme::light(),
            1024 - 248 - 48,
        );
        assert!(mounted.contains(&address));
        assert!(mounted.contains("QR unavailable"));
    }

    #[test]
    fn generated_liquid_address_shows_inline_qr_and_copy_action() {
        let input = form::Value::<String>::default();
        let address = "lq1qsyntheticreceive7q".to_owned();
        let qr = qr_code::Data::new(&address).expect("synthetic address fits a QR code");
        for (appearance, theme) in [
            ("dark", theme::Theme::dark()),
            ("light", theme::Theme::light()),
        ] {
            for outer in [(1024, 768), (1440, 900)] {
                let mounted = capture::inspect_wide_content(
                    "liquid-receive-ready",
                    liquid_receive_view(
                        "Everyday",
                        coincube_core::miniscript::bitcoin::Network::Bitcoin,
                        &ReceiveMethod::Liquid,
                        Some(&address),
                        Some(&qr),
                        false,
                        &input,
                        &input,
                        "",
                        BitcoinDisplayUnit::Sats,
                        None,
                        None,
                        None,
                        SendAsset::Lbtc,
                        SenderNetwork::Liquid,
                        true,
                        &[],
                        Amount::ZERO,
                        0,
                        false,
                        false,
                    ),
                    outer,
                    appearance,
                    theme,
                    outer.0 - 248 - 48,
                );
                assert!(mounted.contains(&address));
                assert!(mounted.contains("Copy"));
                assert!(mounted.contains("Enlarge QR"));
                assert!(!mounted.contains("QR unavailable"));
                let ready_label = mounted
                    .text
                    .iter()
                    .filter(|(label, _)| label == "Liquid address")
                    .max_by(|(_, a), (_, b)| a.y.total_cmp(&b.y))
                    .map(|(_, bounds)| *bounds)
                    .expect("ready card label");
                assert!(
                    ready_label.x > 180.0,
                    "ready details must sit beside the QR at {} {:?}: {:?}",
                    appearance,
                    outer,
                    ready_label,
                );
            }
        }
    }

    #[test]
    fn unavailable_stablecoin_has_network_explanation() {
        let mounted = capture::inspect(
            "liquid-receive-regtest",
            receive_kind_selector(SendAsset::Lbtc, false),
            (1024, 768),
            "dark",
            theme::Theme::dark(),
        );
        assert!(mounted.contains("Stablecoin"));
        assert!(mounted.contains("USDt is unavailable on this network"));
    }

    #[test]
    fn receive_passive_balances_and_recent_rows_follow_global_mask() {
        let input = form::Value::<String>::default();
        let rows = vec![
            RecentTransaction {
                description: "Bitcoin payment".into(),
                time_ago: "just now".into(),
                amount: Amount::from_sat(98_765_432),
                fees_sat: Amount::ZERO,
                fiat_amount: Some(
                    crate::app::view::vault::fiat::FiatAmount::new(
                        3456.78,
                        crate::services::fiat::Currency::USD,
                    )
                    .unwrap(),
                ),
                is_incoming: true,
                status: DomainPaymentStatus::Complete,
                details: DomainPaymentDetails::Lightning {
                    swap_id: None,
                    description: "Bitcoin payment".into(),
                    payer_note: None,
                },
                usdt_display: None,
            },
            RecentTransaction {
                description: "Token payment".into(),
                time_ago: "just now".into(),
                amount: Amount::ZERO,
                fees_sat: Amount::ZERO,
                fiat_amount: None,
                is_incoming: false,
                status: DomainPaymentStatus::Complete,
                details: DomainPaymentDetails::LiquidAsset {
                    asset_id: "usdt".into(),
                    asset_info: None,
                    description: "Token payment".into(),
                    payer_note: None,
                },
                usdt_display: Some("2468.13 USDt".into()),
            },
        ];
        let method = ReceiveMethod::Liquid;
        let render = |mask_amounts, receive_asset| {
            liquid_receive_view(
                "Everyday",
                coincube_core::miniscript::bitcoin::Network::Bitcoin,
                &method,
                None,
                None,
                false,
                &input,
                &input,
                "",
                BitcoinDisplayUnit::Sats,
                None,
                None,
                None,
                receive_asset,
                SenderNetwork::Liquid,
                true,
                &rows,
                Amount::from_sat(12_345_678),
                135_791_113,
                true,
                mask_amounts,
            )
        };
        let hidden = capture::inspect(
            "liquid-receive-hidden",
            render(true, SendAsset::Lbtc),
            (1440, 900),
            "dark",
            theme::Theme::dark(),
        );
        assert!(hidden.contains("••••••"));
        for sentinel in ["12,345,678", "98,765,432", "2468.13", "3,456.78"] {
            assert!(
                !hidden.contains(sentinel),
                "hidden Liquid receive amount {}",
                sentinel
            );
        }
        let shown = capture::inspect(
            "liquid-receive-shown",
            render(false, SendAsset::Lbtc),
            (1440, 900),
            "dark",
            theme::Theme::dark(),
        );
        assert!(shown.contains("12,345,678"));
        assert!(shown.contains("98,765,432"));
        assert!(shown.contains("2468.13"));
        assert!(shown.contains("3,456.78"));
        let hidden_usdt = capture::inspect(
            "liquid-receive-usdt-hidden",
            render(true, SendAsset::Usdt),
            (1440, 900),
            "dark",
            theme::Theme::dark(),
        );
        let usdt_balance = format!("{} USDt", format_usdt_display(135_791_113));
        assert!(!hidden_usdt.contains(&usdt_balance));
        let shown_usdt = capture::inspect(
            "liquid-receive-usdt-shown",
            render(false, SendAsset::Usdt),
            (1440, 900),
            "dark",
            theme::Theme::dark(),
        );
        assert!(shown_usdt.contains(&usdt_balance));
    }
}
