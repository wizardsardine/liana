use coincube_ui::{
    color,
    component::{button, text::*},
    icon::{clipboard_icon, previous_icon},
    image::usdt_network_logo,
    theme,
    widget::{ColumnExt, Element},
};
use iced::{
    widget::{qr_code, Column, Container, Row, Space, TextInput},
    Alignment, Length,
};

use crate::app::state::liquid::sideshift_receive::ReceivePhase;
use crate::app::view::{SideshiftReceiveMessage, SideshiftShiftType};
use crate::services::sideshift::{ShiftResponse, ShiftStatusKind, SideshiftNetwork};

// ---------------------------------------------------------------------------
// Top-level entry point
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub fn sideshift_receive_view<'a>(
    phase: &ReceivePhase,
    selected_network: &SideshiftNetwork,
    shift_type: &SideshiftShiftType,
    amount_input: &'a str,
    shift: Option<&'a ShiftResponse>,
    qr_data: Option<&'a qr_code::Data>,
    shift_status: Option<&'a ShiftStatusKind>,
    loading: bool,
    error: Option<&'a str>,
) -> Element<'a, SideshiftReceiveMessage> {
    match phase {
        ReceivePhase::NetworkSelection => network_picker_view(),
        ReceivePhase::ExternalSetup => {
            external_setup_view(selected_network, shift_type, amount_input, loading, error)
        }
        ReceivePhase::FetchingAffiliate
        | ReceivePhase::FetchingQuote
        | ReceivePhase::CreatingShift => loading_view(phase),
        ReceivePhase::Active => {
            active_shift_view(selected_network, shift, qr_data, shift_status, amount_input)
        }
        ReceivePhase::Failed => error_view(error),
    }
}

// ---------------------------------------------------------------------------
// Network picker
// ---------------------------------------------------------------------------

fn network_picker_view() -> Element<'static, SideshiftReceiveMessage> {
    let title = Column::new().spacing(4).push(h3("Receive USDt")).push(
        text("Choose the network you are receiving from.")
            .size(P1_SIZE)
            .style(theme::text::secondary),
    );

    let mut network_list = Column::new().spacing(8);

    for network in SideshiftNetwork::all() {
        let row_content = Row::new()
            .spacing(12)
            .align_y(Alignment::Center)
            .push(usdt_network_logo(network.network_slug(), 36.0))
            .push(
                Column::new()
                    .spacing(2)
                    .push(text(network.display_name()).size(P1_SIZE).bold())
                    .push_maybe(
                        network
                            .swap_subtitle()
                            .map(|s| text(s).size(P2_SIZE).style(theme::text::secondary)),
                    ),
            );

        let card = Container::new(row_content)
            .padding([12, 16])
            .width(Length::Fill)
            .style(theme::card::simple);

        let btn = iced::widget::button(card)
            .on_press(SideshiftReceiveMessage::SelectNetwork(*network))
            .style(theme::button::transparent_border)
            .width(Length::Fill);

        network_list = network_list.push(btn);
    }

    Container::new(
        Column::new()
            .spacing(20)
            .push(title)
            .push(network_list)
            .max_width(600)
            .width(Length::Fill),
    )
    .center_x(Length::Fill)
    .into()
}

// ---------------------------------------------------------------------------
// External setup (amount + shift type choice)
// ---------------------------------------------------------------------------

fn external_setup_view<'a>(
    network: &SideshiftNetwork,
    shift_type: &SideshiftShiftType,
    amount_input: &'a str,
    loading: bool,
    error: Option<&'a str>,
) -> Element<'a, SideshiftReceiveMessage> {
    let back_btn = button::secondary(Some(previous_icon()), "Back")
        .width(Length::Fixed(150.0))
        .on_press(SideshiftReceiveMessage::Back);

    let title = Column::new()
        .spacing(4)
        .push(h3(format!("Receive {}", network.display_name())))
        .push(
            text(format!(
                "{} USDt will be swapped and delivered as Liquid USDt.",
                network.standard_label()
            ))
            .size(P1_SIZE)
            .style(theme::text::secondary),
        );

    let amount_section = Column::new()
        .spacing(6)
        .push(text("Amount (optional)").size(P2_SIZE).bold())
        .push(
            text("Enter an amount for a fixed rate, or leave blank for variable rate.")
                .size(P2_SIZE)
                .style(theme::text::secondary),
        )
        .push(
            TextInput::new("e.g. 100.00", amount_input)
                .on_input(SideshiftReceiveMessage::AmountInput)
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
                SideshiftShiftType::Fixed => "Rate locked until expiry",
                SideshiftShiftType::Variable => "Rate determined at settlement",
            })
            .size(P2_SIZE)
            .style(theme::text::secondary),
        );

    let generate_btn = if loading {
        button::primary(None, "Generating…")
            .width(Length::Fill)
            .style(theme::button::primary)
    } else {
        button::primary(None, "Generate Deposit Address")
            .on_press(SideshiftReceiveMessage::Generate)
            .width(Length::Fill)
    };

    let mut inner = Column::new()
        .spacing(20)
        .push(title)
        .push(amount_section)
        .push(rate_indicator);

    if let Some(err) = error {
        inner = inner.push(
            Container::new(text(err).size(P2_SIZE).color(color::RED))
                .padding([8, 12])
                .style(theme::card::error),
        );
    }

    Column::new()
        .spacing(16)
        .push(back_btn)
        .push(
            Container::new(inner.push(generate_btn).max_width(520).width(Length::Fill))
                .center_x(Length::Fill),
        )
        .width(Length::Fill)
        .into()
}

// ---------------------------------------------------------------------------
// Loading / progress view
// ---------------------------------------------------------------------------

fn loading_view(phase: &ReceivePhase) -> Element<'static, SideshiftReceiveMessage> {
    let label = match phase {
        ReceivePhase::FetchingAffiliate => "Connecting to SideShift…",
        ReceivePhase::FetchingQuote => "Fetching quote…",
        ReceivePhase::CreatingShift => "Creating deposit address…",
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
// Active shift — deposit address display
// ---------------------------------------------------------------------------

fn active_shift_view<'a>(
    network: &SideshiftNetwork,
    shift: Option<&'a ShiftResponse>,
    qr_data: Option<&'a qr_code::Data>,
    shift_status: Option<&'a ShiftStatusKind>,
    amount_input: &'a str,
) -> Element<'a, SideshiftReceiveMessage> {
    let Some(shift) = shift else {
        return error_view(Some("Shift data missing."));
    };

    let title = Column::new()
        .spacing(8)
        .align_x(Alignment::Center)
        .push(usdt_network_logo(network.network_slug(), 48.0))
        .push(h3(network.display_name()))
        .push(
            text("USDt sent here is swapped to Liquid USDt.")
                .size(P1_SIZE)
                .style(theme::text::secondary),
        );

    let warning_badge = Container::new(
        text(format!(
            "Only send {} USDt ({})",
            network.network_name(),
            network.standard_label()
        ))
        .size(P2_SIZE),
    )
    .padding([6, 12])
    .style(theme::pill::warning);

    let qr_section: Element<SideshiftReceiveMessage> = if let Some(data) = qr_data {
        Container::new(
            Container::new(qr_code(data).cell_size(10))
                .padding(20)
                .style(theme::card::simple),
        )
        .center_x(Length::Fill)
        .into()
    } else {
        Space::new().height(Length::Fixed(0.0)).into()
    };

    let address_row = Row::new()
        .spacing(8)
        .align_y(Alignment::Center)
        .push(
            Container::new(
                text(&shift.deposit_address)
                    .size(P2_SIZE)
                    .font(iced::Font::MONOSPACE),
            )
            .padding([8, 12])
            .style(theme::card::simple)
            .width(Length::Fill),
        )
        .push(
            iced::widget::button(clipboard_icon().size(16))
                .on_press(SideshiftReceiveMessage::Copy)
                .style(theme::button::transparent_border),
        );

    let info_rows = info_card(shift, amount_input);

    let status_badge: Element<SideshiftReceiveMessage> = if let Some(status) = shift_status {
        let (label, style_color) = match status {
            ShiftStatusKind::Waiting => ("Waiting for deposit", color::GREY_3),
            ShiftStatusKind::Pending | ShiftStatusKind::Processing => {
                ("Deposit detected", color::ORANGE)
            }
            ShiftStatusKind::Settling => ("Settling…", color::ORANGE),
            ShiftStatusKind::Settled => ("Settled ✓", color::GREEN),
            ShiftStatusKind::Expired => ("Expired", color::RED),
            ShiftStatusKind::Refunded => ("Refunded", color::GREY_3),
            ShiftStatusKind::Error => ("Error", color::RED),
            ShiftStatusKind::Unknown(_) => ("Unknown", color::GREY_3),
        };
        Container::new(text(label).size(P2_SIZE).color(style_color))
            .padding([4, 10])
            .style(theme::pill::simple)
            .into()
    } else {
        Space::new().height(Length::Fixed(0.0)).into()
    };

    let is_terminal = shift_status.map(|s| s.is_terminal()).unwrap_or(false);

    let back_btn: Element<SideshiftReceiveMessage> = if is_terminal {
        iced::widget::button(
            Row::new()
                .spacing(5)
                .align_y(Alignment::Center)
                .push(previous_icon().style(theme::text::secondary))
                .push(text("Done").size(P1_SIZE).style(theme::text::secondary)),
        )
        .on_press(SideshiftReceiveMessage::Reset)
        .style(theme::button::transparent)
        .into()
    } else {
        Space::new().height(Length::Fixed(0.0)).into()
    };

    Column::new()
        .spacing(16)
        .push(back_btn)
        .push(
            Container::new(
                Column::new()
                    .spacing(16)
                    .align_x(Alignment::Center)
                    .push(title)
                    .push(warning_badge)
                    .push(qr_section)
                    .push(address_row)
                    .push(info_rows)
                    .push(status_badge)
                    .max_width(520)
                    .width(Length::Fill),
            )
            .center_x(Length::Fill),
        )
        .width(Length::Fill)
        .into()
}

// ---------------------------------------------------------------------------
// Error view
// ---------------------------------------------------------------------------

fn error_view(error: Option<&str>) -> Element<SideshiftReceiveMessage> {
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
                    .on_press(SideshiftReceiveMessage::Reset)
                    .width(Length::Fill),
            )
            .max_width(520)
            .width(Length::Fill),
    )
    .center_x(Length::Fill)
    .into()
}

fn info_card(
    shift: &ShiftResponse,
    amount_input: &str,
) -> Element<'static, SideshiftReceiveMessage> {
    let mut rows = Column::new().spacing(8);

    if let Some(min) = &shift.deposit_min {
        rows = rows.push(info_row("Min deposit", min));
    }
    if let Some(max) = &shift.deposit_max {
        rows = rows.push(info_row("Max deposit", max));
    }
    if !amount_input.is_empty() {
        if let Some(dep) = &shift.deposit_amount {
            rows = rows.push(info_row("You send", dep));
        }
        if let Some(settle) = &shift.settle_amount {
            rows = rows.push(info_row("You receive", settle));
        }
    }
    if let Some(rate) = &shift.rate {
        rows = rows.push(info_row("Rate", rate));
    }
    if let Some(fee) = &shift.network_fee_usd {
        rows = rows.push(info_row("Network fee", &format!("${}", fee)));
    }
    if let Some(aff) = &shift.affiliate_fee_percent {
        rows = rows.push(info_row("Service fee", &format!("{}%", aff)));
    }
    if let Some(exp) = &shift.expires_at {
        rows = rows.push(info_row("Expires", exp));
    }
    rows = rows.push(info_row("Swap ID", &shift.id));

    Container::new(rows)
        .padding([12, 16])
        .width(Length::Fill)
        .style(theme::card::simple)
        .into()
}

fn info_row(label: &str, value: &str) -> Element<'static, SideshiftReceiveMessage> {
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
