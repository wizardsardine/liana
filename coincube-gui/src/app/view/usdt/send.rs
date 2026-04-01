use coincube_ui::{
    color,
    component::{button, text::*},
    icon::{arrow_right, clipboard_icon, previous_icon},
    image::usdt_network_logo,
    theme,
    widget::*,
};
use iced::{
    widget::{Column, Container, Row, Space, TextInput},
    Alignment, Length,
};

use crate::app::breez::assets::format_usdt_display;
use crate::app::state::usdt::send::SendPhase;
use crate::app::view::liquid::RecentTransaction;
use crate::app::view::{SideshiftSendMessage, SideshiftShiftType};
use crate::services::sideshift::{ShiftResponse, ShiftStatusKind, SideshiftNetwork};
use breez_sdk_liquid::model::PaymentDetails;

// ---------------------------------------------------------------------------
// Top-level entry point
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub fn usdt_send_view<'a>(
    phase: &SendPhase,
    selected_network: Option<&SideshiftNetwork>,
    detected_networks: &[SideshiftNetwork],
    shift_type: &SideshiftShiftType,
    recipient_address: &'a str,
    amount_input: &'a str,
    usdt_balance: u64,
    recent_transactions: &'a [RecentTransaction],
    shift: Option<&'a ShiftResponse>,
    shift_status: Option<&'a ShiftStatusKind>,
    loading: bool,
    error: Option<&'a str>,
    usdt_asset_id: &str,
) -> Element<'a, SideshiftSendMessage> {
    match phase {
        SendPhase::AddressInput => address_input_view(
            recipient_address,
            selected_network,
            detected_networks,
            usdt_balance,
            recent_transactions,
            error,
            usdt_asset_id,
        ),
        SendPhase::NetworkDisambiguation => disambiguation_view(
            recipient_address,
            selected_network,
            detected_networks,
            error,
        ),
        SendPhase::AmountInput => amount_input_view(
            selected_network,
            shift_type,
            amount_input,
            usdt_balance,
            loading,
            error,
        ),
        SendPhase::FetchingAffiliate
        | SendPhase::FetchingQuote
        | SendPhase::CreatingShift
        | SendPhase::Sending => loading_view(phase),
        SendPhase::Review => review_view(selected_network, shift, amount_input, error),
        SendPhase::Sent => sent_view(selected_network, shift, shift_status),
        SendPhase::Failed => error_view(error),
        SendPhase::LiquidNative => Column::new().into(),
    }
}

// ---------------------------------------------------------------------------
// Address input with balance (initial screen)
// ---------------------------------------------------------------------------

fn address_input_view<'a>(
    recipient_address: &'a str,
    selected_network: Option<&SideshiftNetwork>,
    detected_networks: &[SideshiftNetwork],
    usdt_balance: u64,
    recent_transactions: &'a [RecentTransaction],
    error: Option<&'a str>,
    usdt_asset_id: &str,
) -> Element<'a, SideshiftSendMessage> {
    let mut content = Column::new().spacing(20).align_x(Alignment::Center);

    // Balance card
    let balance_col = Column::new()
        .spacing(4)
        .push(
            Row::new()
                .spacing(10)
                .align_y(Alignment::Center)
                .push(text(format_usdt_display(usdt_balance)).size(H2_SIZE).bold())
                .push(text("USDt").size(H2_SIZE).color(color::GREY_3)),
        )
        .push(
            text("Liquid Network")
                .size(P1_SIZE)
                .style(theme::text::secondary),
        );
    let balance_inner = Column::new()
        .spacing(8)
        .push(h4_bold("Balance"))
        .push(balance_col);
    content = content.push(crate::app::view::balance_header_card(balance_inner));

    // Address input with inline detection chip and arrow button
    let can_proceed = !recipient_address.trim().is_empty()
        && !detected_networks.is_empty()
        && (detected_networks.len() == 1 || selected_network.is_some());

    // Build the detection chip that sits inside the input row
    let detection_chip: Option<Element<'a, SideshiftSendMessage>> =
        if recipient_address.trim().is_empty() {
            None
        } else if detected_networks.is_empty() {
            Some(
                Container::new(text("Unknown").size(P2_SIZE).color(color::RED))
                    .padding([4, 8])
                    .style(theme::pill::error)
                    .into(),
            )
        } else if detected_networks.len() == 1 {
            let net = &detected_networks[0];
            Some(
                Container::new(
                    Row::new()
                        .spacing(6)
                        .align_y(Alignment::Center)
                        .push(usdt_network_logo(net.network_slug(), 24.0))
                        .push(text(net.network_name()).size(P2_SIZE).bold()),
                )
                .padding([0, 12])
                .height(Length::Fixed(50.0))
                .center_y(Length::Fixed(50.0))
                .style(theme::pill::simple)
                .into(),
            )
        } else {
            // Multiple networks — inline disambiguator pills
            let mut row = Row::new().spacing(4).align_y(Alignment::Center);
            for network in detected_networks {
                let is_selected = selected_network == Some(network);
                let pill =
                    Container::new(
                        Row::new()
                            .spacing(4)
                            .align_y(Alignment::Center)
                            .push(usdt_network_logo(network.network_slug(), 22.0))
                            .push(text(network.network_name()).size(P2_SIZE).color(
                                if is_selected {
                                    color::ORANGE
                                } else {
                                    color::GREY_3
                                },
                            )),
                    )
                    .padding([0, 8])
                    .height(Length::Fixed(50.0))
                    .center_y(Length::Fixed(50.0))
                    .style(if is_selected {
                        theme::container::border_orange
                    } else {
                        theme::pill::simple
                    });

                let btn = iced::widget::button(pill)
                    .on_press(SideshiftSendMessage::DisambiguateNetwork(*network))
                    .style(theme::button::transparent);
                row = row.push(btn);
            }
            Some(row.into())
        };

    let mut input_row = Row::new().spacing(10).align_y(Alignment::Center).push(
        TextInput::new("Paste USDt address (any network)…", recipient_address)
            .on_input(SideshiftSendMessage::RecipientAddressEdited)
            .padding(15)
            .size(16),
    );

    if let Some(chip) = detection_chip {
        input_row = input_row.push(chip);
    }

    input_row = input_row.push(
        Container::new(
            iced::widget::button(
                Container::new(arrow_right())
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_x(Alignment::Center)
                    .align_y(Alignment::Center),
            )
            .on_press_maybe(if can_proceed {
                Some(SideshiftSendMessage::Next)
            } else {
                None
            })
            .width(Length::Fixed(50.0))
            .height(Length::Fixed(50.0))
            .style(theme::button::primary),
        )
        .width(Length::Fixed(50.0))
        .height(Length::Fixed(50.0)),
    );

    let input_section = Column::new()
        .spacing(20)
        .width(Length::Fill)
        .push(
            Container::new(h4_bold(
                "Enter USDt Address (Liquid, Ethereum, Tron, Binance, or Solana)",
            ))
            .padding(iced::Padding::new(0.0).top(5))
            .width(Length::Fill),
        )
        .push(input_row);

    content = content.push(input_section);

    // Error
    if let Some(err) = error {
        content = content.push(
            Container::new(text(err).size(P2_SIZE).color(color::RED))
                .padding([8, 12])
                .style(theme::card::error),
        );
    }

    // Recent USDt transactions
    if !recent_transactions.is_empty() {
        let mut tx_list = Column::new().spacing(8);
        tx_list = tx_list.push(h4_bold("Last transactions"));

        for tx in recent_transactions.iter().take(5) {
            let usdt_amount = if let PaymentDetails::Liquid { asset_id, .. } = &tx.details {
                if !usdt_asset_id.is_empty() && asset_id == usdt_asset_id {
                    Some(format_usdt_display(tx.amount.to_sat()))
                } else {
                    None
                }
            } else {
                None
            };

            let amount_text = if let Some(ref usdt) = usdt_amount {
                format!("{}{} USDt", if tx.is_incoming { "+ " } else { "- " }, usdt)
            } else {
                continue;
            };

            let amount_color = if tx.is_incoming {
                color::GREEN
            } else {
                color::RED
            };

            let row = Row::new()
                .spacing(8)
                .align_y(Alignment::Center)
                .push(
                    Column::new()
                        .spacing(2)
                        .width(Length::Fill)
                        .push(text(&tx.description).size(P1_SIZE))
                        .push(
                            text(&tx.time_ago)
                                .size(P2_SIZE)
                                .style(theme::text::secondary),
                        ),
                )
                .push(text(amount_text).size(P1_SIZE).color(amount_color));

            tx_list = tx_list.push(
                Container::new(row)
                    .padding([10, 12])
                    .width(Length::Fill)
                    .style(theme::card::simple),
            );
        }

        content = content.push(tx_list);
    }

    content.into()
}

// ---------------------------------------------------------------------------
// Network disambiguation (full-screen fallback for ambiguous addresses)
// ---------------------------------------------------------------------------

fn disambiguation_view<'a>(
    recipient_address: &'a str,
    selected_network: Option<&SideshiftNetwork>,
    detected_networks: &[SideshiftNetwork],
    error: Option<&'a str>,
) -> Element<'a, SideshiftSendMessage> {
    let back_btn = iced::widget::button(
        Row::new()
            .spacing(5)
            .align_y(Alignment::Center)
            .push(previous_icon().style(theme::text::secondary))
            .push(text("Previous").size(P1_SIZE).style(theme::text::secondary)),
    )
    .on_press(SideshiftSendMessage::Back)
    .style(theme::button::transparent);

    let title = Column::new().spacing(4).push(h3("Select Network")).push(
        text("This address is compatible with multiple networks. Please select one.")
            .size(P1_SIZE)
            .style(theme::text::secondary),
    );

    let addr_preview = Container::new(
        text(recipient_address)
            .size(P2_SIZE)
            .font(iced::Font::MONOSPACE),
    )
    .padding([8, 12])
    .style(theme::card::simple)
    .width(Length::Fill);

    let mut network_list = Column::new().spacing(8);
    for network in detected_networks {
        let is_selected = selected_network == Some(network);

        let row_content = Row::new()
            .spacing(12)
            .align_y(Alignment::Center)
            .push(usdt_network_logo(network.network_slug(), 36.0))
            .push(
                Column::new()
                    .spacing(2)
                    .push(text(network.display_name()).size(P1_SIZE).bold())
                    .push(
                        text(network.standard_label())
                            .size(P2_SIZE)
                            .style(theme::text::secondary),
                    ),
            )
            .push(Space::new().width(Length::Fill))
            .push_maybe(if is_selected {
                Some(
                    Container::new(text("Selected").size(P2_SIZE).color(color::ORANGE))
                        .padding([2, 8])
                        .style(theme::pill::simple),
                )
            } else {
                None
            });

        let card = Container::new(row_content)
            .padding([12, 16])
            .width(Length::Fill)
            .style(if is_selected {
                theme::container::border_orange
            } else {
                theme::card::simple
            });

        let btn = iced::widget::button(card)
            .on_press(SideshiftSendMessage::DisambiguateNetwork(*network))
            .style(theme::button::transparent_border)
            .width(Length::Fill);

        network_list = network_list.push(btn);
    }

    let next_btn = if selected_network.is_some() {
        button::primary(None, "Continue")
            .on_press(SideshiftSendMessage::Next)
            .width(Length::Fill)
    } else {
        button::primary(None, "Continue")
            .width(Length::Fill)
            .style(theme::button::primary)
    };

    let mut col = Column::new()
        .spacing(16)
        .push(back_btn)
        .push(title)
        .push(addr_preview)
        .push(network_list);

    if let Some(err) = error {
        col = col.push(
            Container::new(text(err).size(P2_SIZE).color(color::RED))
                .padding([8, 12])
                .style(theme::card::error),
        );
    }

    Container::new(col.push(next_btn).max_width(520).width(Length::Fill))
        .center_x(Length::Fill)
        .into()
}

// ---------------------------------------------------------------------------
// Amount input
// ---------------------------------------------------------------------------

fn amount_input_view<'a>(
    selected_network: Option<&SideshiftNetwork>,
    shift_type: &SideshiftShiftType,
    amount_input: &'a str,
    usdt_balance: u64,
    loading: bool,
    error: Option<&'a str>,
) -> Element<'a, SideshiftSendMessage> {
    let network = selected_network
        .copied()
        .unwrap_or(SideshiftNetwork::Ethereum);

    let back_btn = iced::widget::button(
        Row::new()
            .spacing(5)
            .align_y(Alignment::Center)
            .push(previous_icon().style(theme::text::secondary))
            .push(text("Previous").size(P1_SIZE).style(theme::text::secondary)),
    )
    .on_press(SideshiftSendMessage::Back)
    .style(theme::button::transparent);

    let title = Column::new()
        .spacing(4)
        .push(h3(format!("Send to {}", network.display_name())))
        .push(
            text(format!(
                "Your Liquid USDt will be swapped and delivered as {} USDt.",
                network.standard_label()
            ))
            .size(P1_SIZE)
            .style(theme::text::secondary),
        );

    // Detected network badge
    let network_badge = Container::new(
        Row::new()
            .spacing(8)
            .align_y(Alignment::Center)
            .push(usdt_network_logo(network.network_slug(), 28.0))
            .push(text("Network:").size(P2_SIZE).style(theme::text::secondary))
            .push(text(network.display_name()).size(P2_SIZE).bold()),
    )
    .padding([4, 10])
    .style(theme::pill::simple);

    // Balance display
    let balance_row = Row::new()
        .spacing(6)
        .align_y(Alignment::Center)
        .push(
            text("Available:")
                .size(P2_SIZE)
                .style(theme::text::secondary),
        )
        .push(
            text(format!("{} USDt", format_usdt_display(usdt_balance)))
                .size(P2_SIZE)
                .bold(),
        );

    let amount_section = Column::new()
        .spacing(6)
        .push(text("Amount to send (USDt)").size(P2_SIZE).bold())
        .push(
            text("Enter the amount of USDt to send. A fixed rate is locked at confirmation.")
                .size(P2_SIZE)
                .style(theme::text::secondary),
        )
        .push(
            TextInput::new("e.g. 50.00", amount_input)
                .on_input(SideshiftSendMessage::AmountInput)
                .padding([10, 14])
                .size(P1_SIZE),
        );

    let rate_indicator = Row::new()
        .spacing(8)
        .align_y(Alignment::Center)
        .push(
            Container::new(
                text(match shift_type {
                    SideshiftShiftType::Fixed => "Fixed Rate",
                    SideshiftShiftType::Variable => "Variable Rate",
                })
                .size(P2_SIZE)
                .color(match shift_type {
                    SideshiftShiftType::Fixed => color::ORANGE,
                    SideshiftShiftType::Variable => color::GREY_3,
                }),
            )
            .padding([4, 10])
            .style(theme::pill::simple),
        )
        .push(
            text(match shift_type {
                SideshiftShiftType::Fixed => "Rate locked at confirmation",
                SideshiftShiftType::Variable => "Rate determined at settlement",
            })
            .size(P2_SIZE)
            .style(theme::text::secondary),
        );

    let review_btn = if loading {
        button::primary(None, "Preparing…")
            .width(Length::Fill)
            .style(theme::button::primary)
    } else {
        button::primary(None, "Review Swap")
            .on_press(SideshiftSendMessage::Generate)
            .width(Length::Fill)
    };

    let mut col = Column::new()
        .spacing(16)
        .push(back_btn)
        .push(title)
        .push(network_badge)
        .push(balance_row)
        .push(amount_section)
        .push(rate_indicator);

    if let Some(err) = error {
        col = col.push(
            Container::new(text(err).size(P2_SIZE).color(color::RED))
                .padding([8, 12])
                .style(theme::card::error),
        );
    }

    Container::new(col.push(review_btn).max_width(520).width(Length::Fill))
        .center_x(Length::Fill)
        .into()
}

// ---------------------------------------------------------------------------
// Loading view
// ---------------------------------------------------------------------------

fn loading_view(phase: &SendPhase) -> Element<'static, SideshiftSendMessage> {
    let label = match phase {
        SendPhase::FetchingAffiliate => "Connecting to SideShift…",
        SendPhase::FetchingQuote => "Fetching quote…",
        SendPhase::CreatingShift => "Creating swap…",
        SendPhase::Sending => "Sending Liquid USDt…",
        _ => "Loading…",
    };

    Container::new(
        Column::new()
            .spacing(20)
            .align_x(Alignment::Center)
            .push(Space::new().height(Length::Fixed(60.0)))
            .push(text(label).size(P1_SIZE).style(theme::text::secondary))
            .max_width(520)
            .width(Length::Fill),
    )
    .center_x(Length::Fill)
    .into()
}

// ---------------------------------------------------------------------------
// Review — show fees before confirming
// ---------------------------------------------------------------------------

fn review_view<'a>(
    selected_network: Option<&SideshiftNetwork>,
    shift: Option<&'a ShiftResponse>,
    amount_input: &'a str,
    error: Option<&'a str>,
) -> Element<'a, SideshiftSendMessage> {
    let Some(shift) = shift else {
        return error_view(Some("Swap data missing."));
    };

    let network = selected_network
        .copied()
        .unwrap_or(SideshiftNetwork::Ethereum);

    let title = Column::new().spacing(4).push(h3("Review Swap")).push(
        text(format!(
            "You will send Liquid USDt. SideShift delivers {} USDt to the recipient.",
            network.standard_label()
        ))
        .size(P1_SIZE)
        .style(theme::text::secondary),
    );

    let info = info_card(shift, amount_input);

    let warning = Container::new(
        text(format!(
            "SideShift will deliver {} ({}) to the recipient address you provided.",
            network.display_name(),
            network.standard_label()
        ))
        .size(P2_SIZE)
        .style(theme::text::secondary),
    )
    .padding([10, 14])
    .style(theme::card::simple)
    .width(Length::Fill);

    let back_btn = iced::widget::button(
        Row::new()
            .spacing(5)
            .align_y(Alignment::Center)
            .push(previous_icon().style(theme::text::secondary))
            .push(text("Previous").size(P1_SIZE).style(theme::text::secondary)),
    )
    .on_press(SideshiftSendMessage::Back)
    .style(theme::button::transparent)
    .width(Length::FillPortion(1));

    let confirm_btn = button::primary(None, "Confirm & Send")
        .on_press(SideshiftSendMessage::ConfirmSend)
        .width(Length::FillPortion(1));

    let action_row = Row::new()
        .spacing(10)
        .push(back_btn)
        .push(confirm_btn)
        .width(Length::Fill);

    let mut col = Column::new()
        .spacing(16)
        .push(title)
        .push(info)
        .push(warning);

    if let Some(err) = error {
        col = col.push(
            Container::new(text(err).size(P2_SIZE).color(color::RED))
                .padding([8, 12])
                .style(theme::card::error),
        );
    }

    Container::new(col.push(action_row).max_width(520).width(Length::Fill))
        .center_x(Length::Fill)
        .into()
}

// ---------------------------------------------------------------------------
// Sent / polling view
// ---------------------------------------------------------------------------

fn sent_view<'a>(
    selected_network: Option<&SideshiftNetwork>,
    shift: Option<&'a ShiftResponse>,
    shift_status: Option<&'a ShiftStatusKind>,
) -> Element<'a, SideshiftSendMessage> {
    let network = selected_network
        .copied()
        .unwrap_or(SideshiftNetwork::Ethereum);

    let status_label = shift_status.map(|s| s.display()).unwrap_or("Processing…");

    let (status_color, status_icon) = match shift_status {
        Some(ShiftStatusKind::Settled) => (color::GREEN, "✓"),
        Some(ShiftStatusKind::Expired | ShiftStatusKind::Error) => (color::RED, "✗"),
        _ => (color::ORANGE, "⟳"),
    };

    let title = Column::new().spacing(4).push(h3("Swap Submitted")).push(
        text(format!(
            "Your Liquid USDt is being swapped to {} USDt.",
            network.standard_label()
        ))
        .size(P1_SIZE)
        .style(theme::text::secondary),
    );

    let status_badge = Container::new(
        Row::new()
            .spacing(6)
            .align_y(Alignment::Center)
            .push(text(status_icon).size(P1_SIZE).color(status_color))
            .push(text(status_label).size(P1_SIZE).color(status_color)),
    )
    .padding([8, 16])
    .style(theme::card::simple);

    let mut col = Column::new().spacing(20).push(title).push(status_badge);

    if let Some(shift) = shift {
        col = col.push(
            Column::new()
                .spacing(6)
                .push(
                    Row::new()
                        .spacing(8)
                        .align_y(Alignment::Center)
                        .push(
                            text("Swap ID")
                                .size(P2_SIZE)
                                .style(theme::text::secondary)
                                .width(Length::Fixed(80.0)),
                        )
                        .push(text(&shift.id).size(P2_SIZE).font(iced::Font::MONOSPACE))
                        .push(
                            iced::widget::button(clipboard_icon().size(16))
                                .on_press(SideshiftSendMessage::Copy)
                                .style(theme::button::transparent_border),
                        ),
                )
                .push_maybe(shift.settle_address.as_ref().map(|addr| {
                    Row::new()
                        .spacing(8)
                        .align_y(Alignment::Center)
                        .push(
                            text("Recipient")
                                .size(P2_SIZE)
                                .style(theme::text::secondary)
                                .width(Length::Fixed(80.0)),
                        )
                        .push(text(addr).size(P2_SIZE).font(iced::Font::MONOSPACE))
                })),
        );
    }

    let done_btn = button::primary(None, "Done")
        .on_press(SideshiftSendMessage::Reset)
        .width(Length::Fill);

    Container::new(col.push(done_btn).max_width(520).width(Length::Fill))
        .center_x(Length::Fill)
        .into()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn info_card(shift: &ShiftResponse, amount_input: &str) -> Element<'static, SideshiftSendMessage> {
    let mut rows = Column::new().spacing(8);

    if let Some(dep) = &shift.deposit_amount {
        rows = rows.push(info_row("You send", &format!("{} Liquid USDt", dep)));
    } else if !amount_input.is_empty() {
        rows = rows.push(info_row(
            "You send",
            &format!("{} Liquid USDt", amount_input),
        ));
    }
    if let Some(settle) = &shift.settle_amount {
        rows = rows.push(info_row("Recipient receives", settle));
    }
    if let Some(rate) = &shift.rate {
        rows = rows.push(info_row("Rate", rate));
    }
    if let Some(min) = &shift.deposit_min {
        rows = rows.push(info_row("Min", min));
    }
    if let Some(max) = &shift.deposit_max {
        rows = rows.push(info_row("Max", max));
    }
    if let Some(fee) = &shift.network_fee_usd {
        rows = rows.push(info_row("Network fee", &format!("${}", fee)));
    }
    if let Some(aff) = &shift.affiliate_fee_percent {
        rows = rows.push(info_row("Service fee", &format!("{}%", aff)));
    }
    if let Some(addr) = &shift.settle_address {
        rows = rows.push(info_row("To", addr));
    }
    rows = rows.push(info_row("Swap ID", &shift.id));

    Container::new(rows)
        .padding([12, 16])
        .width(Length::Fill)
        .style(theme::card::simple)
        .into()
}

fn info_row(label: &str, value: &str) -> Element<'static, SideshiftSendMessage> {
    Row::new()
        .spacing(8)
        .align_y(Alignment::Center)
        .push(
            text(label.to_string())
                .size(P2_SIZE)
                .style(theme::text::secondary)
                .width(Length::FillPortion(2)),
        )
        .push(
            text(value.to_string())
                .size(P2_SIZE)
                .font(iced::Font::MONOSPACE)
                .width(Length::FillPortion(3)),
        )
        .into()
}

fn error_view(error: Option<&str>) -> Element<SideshiftSendMessage> {
    let msg = error.unwrap_or("An unknown error occurred.");

    Container::new(
        Column::new()
            .spacing(16)
            .push(
                Container::new(text(msg).size(P1_SIZE).color(color::RED))
                    .padding([12, 16])
                    .style(theme::card::error)
                    .width(Length::Fill),
            )
            .push(
                button::primary(None, "Try Again")
                    .on_press(SideshiftSendMessage::Reset)
                    .width(Length::Fill),
            )
            .max_width(520)
            .width(Length::Fill),
    )
    .center_x(Length::Fill)
    .into()
}
