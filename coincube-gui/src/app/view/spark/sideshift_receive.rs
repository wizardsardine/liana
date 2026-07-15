//! View for the Spark "receive from another network" bridge.
//!
//! Mirrors [`crate::app::view::liquid::sideshift_receive`] — same card/QR/pill
//! components, same status-badge shape — with the differences the flow demands:
//! a refund-address field, a third-party disclosure, and copy that says
//! **bitcoin arrives**, never "receive USDT".

use coincube_ui::{
    color,
    component::{button, text::*},
    icon::{clipboard_icon, previous_icon},
    image::{network_logo, usdt_network_logo},
    theme,
    widget::Element,
};
use iced::{
    widget::{qr_code, Column, Container, Row, Space, TextInput},
    Alignment, Length,
};

use crate::app::state::spark::sideshift_receive::{SparkShiftPhase, SparkSideshiftReceiveFlow};
use crate::app::view::SparkSideshiftReceiveMessage as Msg;
use crate::services::sideshift::{deposit_options, ShiftResponse, ShiftStatusKind};

pub fn spark_sideshift_receive_view(flow: &SparkSideshiftReceiveFlow) -> Element<'_, Msg> {
    match flow.phase() {
        SparkShiftPhase::Setup => setup_view(flow),
        SparkShiftPhase::FetchingAffiliate | SparkShiftPhase::CreatingShift => {
            loading_view("Setting up your deposit address…")
        }
        SparkShiftPhase::Active => active_shift_view(flow),
        SparkShiftPhase::Failed => error_view(flow.error()),
    }
}

/// The logo for a deposit option, sized to `size`.
///
/// Only USDt has a coin SVG in the asset library, so it gets the composite
/// (USDt coin badged with its network). Every other asset — USDC, ETH, SOL,
/// TRX, BNB — would fall back to a *wrong* coin logo (BTC via `asset_logo`, or
/// USDt via `usdt_network_logo`), so those show the plain network logo instead.
/// The row's label carries the asset name, so the network logo is accurate, not
/// merely least-wrong. Showing a USDt or BTC coin next to "Ether" in a
/// funds-critical picker is the kind of thing that gets money sent to the wrong
/// place.
fn deposit_option_logo<'a>(
    option: &crate::services::sideshift::DepositOption,
    size: f32,
) -> Element<'a, Msg> {
    if option.coin == "usdt" {
        usdt_network_logo(option.network.network_slug(), size)
    } else {
        network_logo(option.network.network_slug())
            .width(Length::Fixed(size))
            .height(Length::Fixed(size))
            .into()
    }
}

// ── Setup: pick a deposit, give a refund address ────────────────────────────

fn setup_view(flow: &SparkSideshiftReceiveFlow) -> Element<'_, Msg> {
    let selected = flow.selected();

    let back_btn = button::secondary(Some(previous_icon()), "Back")
        .width(Length::Fixed(150.0))
        .on_press(Msg::Back);

    // Master invariant 8: the user is receiving *bitcoin*. Whatever they send
    // is converted at the shift rate — saying "receive USDt" would be false.
    let title = Column::new()
        .spacing(4)
        .push(h3("Receive from another network"))
        .push(
            text(
                "Send a supported asset from another chain and it arrives as bitcoin in your \
                 Spark wallet, converted at the rate at the time of the swap.",
            )
            .size(P1_SIZE)
            .style(theme::text::secondary),
        );

    // What you're depositing. Asset and chain together, because an asset alone
    // is ambiguous — USDt on Tron and USDt on Ethereum are not interchangeable.
    let mut picker = Column::new()
        .spacing(8)
        .push(text("You send").size(P2_SIZE).bold());
    for option in deposit_options() {
        let row = Row::new()
            .spacing(12)
            .align_y(Alignment::Center)
            .push(deposit_option_logo(option, 32.0))
            .push(text(option.label).size(P1_SIZE));

        let card = Container::new(row)
            .padding([12, 16])
            .width(Length::Fill)
            .style(if *option == selected {
                theme::card::border
            } else {
                theme::card::simple
            });

        picker = picker.push(
            iced::widget::button(card)
                .on_press(Msg::SelectDeposit(option.key()))
                .style(theme::button::transparent_border)
                .width(Length::Fill),
        );
    }

    // The refund address. Collected *before* any deposit address is shown,
    // because a shift that fails or gets held has to have somewhere to send the
    // money back to — and asking for it afterwards is useless.
    let mut refund = Column::new()
        .spacing(6)
        .push(text("Refund address").size(P2_SIZE).bold())
        .push(
            text(format!(
                "Your {} address. If the swap can't complete, your deposit is returned here — \
                 on {}, not as bitcoin.",
                selected.network.network_name(),
                selected.network.network_name(),
            ))
            .size(P2_SIZE)
            .style(theme::text::secondary),
        )
        .push(
            TextInput::new(
                &format!("Your {} address", selected.network.network_name()),
                flow.refund_address(),
            )
            .on_input(Msg::RefundAddressEdited)
            .padding([10, 14])
            .size(P1_SIZE),
        );
    if let Some(err) = flow.refund_error() {
        refund = refund.push(text(err).size(P2_SIZE).color(color::RED));
    }

    // Master invariant 9: name the third party and the jurisdictional caveat.
    // The user is handing funds to SideShift, not to us, and they should know
    // that before they do it — not after something goes wrong.
    let disclosure = Container::new(
        text(
            "Conversion is performed by SideShift, a third-party service. It isn't available \
             in all jurisdictions, and swaps may be held for review. Bitcoin arrives on-chain, \
             after about one confirmation.",
        )
        .size(P2_SIZE)
        .style(theme::text::secondary),
    )
    .padding([8, 12])
    .style(theme::card::simple);

    let generate_btn = if flow.is_loading() {
        button::primary(None, "Generating…").width(Length::Fill)
    } else {
        button::primary(None, "Generate deposit address")
            .on_press(Msg::Generate)
            .width(Length::Fill)
    };

    let mut inner = Column::new()
        .spacing(20)
        .push(title)
        .push(picker)
        .push(refund)
        .push(disclosure);

    if let Some(err) = flow.error() {
        inner = inner.push(
            Container::new(text(err).size(P2_SIZE).color(color::RED))
                .padding([8, 12])
                .style(theme::card::error),
        );
    }
    inner = inner.push(generate_btn);

    Column::new()
        .spacing(16)
        .push(back_btn)
        .push(Container::new(inner.max_width(600).width(Length::Fill)).center_x(Length::Fill))
        .into()
}

// ── Active shift ────────────────────────────────────────────────────────────

fn active_shift_view(flow: &SparkSideshiftReceiveFlow) -> Element<'_, Msg> {
    let Some(shift) = flow.shift() else {
        return error_view(Some("Shift data missing."));
    };
    let selected = flow.selected();
    let status = flow.shift_status();

    let title = Column::new()
        .spacing(8)
        .align_x(Alignment::Center)
        .push(deposit_option_logo(&selected, 48.0))
        .push(h3(format!("Send {}", selected.label)))
        .push(
            text("It arrives as bitcoin in your Spark wallet.")
                .size(P1_SIZE)
                .style(theme::text::secondary),
        );

    // Sending the wrong asset or the wrong chain to this address loses it, so
    // the exact pair is restated next to the address itself.
    let warning_badge = Container::new(
        text(format!(
            "Only send {} on {} ({})",
            selected.coin.to_uppercase(),
            selected.network.network_name(),
            selected.network.standard_label(),
        ))
        .size(P2_SIZE),
    )
    .padding([6, 12])
    .style(theme::pill::warning);

    let qr_section: Element<Msg> = if let Some(data) = flow.qr_data() {
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
                .on_press(Msg::Copy)
                .style(theme::button::transparent_border),
        );

    let status_section: Element<Msg> = if let Some(status) = status {
        let (label, style_color) = status_badge(status);
        Column::new()
            .spacing(8)
            .push(
                Container::new(text(label).size(P2_SIZE).color(style_color))
                    .padding([4, 10])
                    .style(theme::pill::simple),
            )
            // The guidance is the whole point of the held/failed states: it
            // tells the user whether they have to *do* something, instead of
            // showing a spinner over a shift that will never move.
            .push(
                text(status.guidance())
                    .size(P2_SIZE)
                    .style(theme::text::secondary),
            )
            .into()
    } else {
        Space::new().height(Length::Fixed(0.0)).into()
    };

    let is_terminal = status.map(ShiftStatusKind::is_terminal).unwrap_or(false);

    let footer: Element<Msg> = if is_terminal {
        button::primary(None, "Done")
            .on_press(Msg::Reset)
            .width(Length::Fixed(150.0))
            .into()
    } else {
        button::secondary(Some(previous_icon()), "Back")
            .on_press(Msg::Back)
            .width(Length::Fixed(150.0))
            .into()
    };

    Container::new(
        Column::new()
            .spacing(18)
            .align_x(Alignment::Center)
            .push(title)
            .push(warning_badge)
            .push(qr_section)
            .push(address_row)
            .push(info_card(shift, &selected))
            .push(status_section)
            .push(footer)
            .max_width(600)
            .width(Length::Fill),
    )
    .center_x(Length::Fill)
    .into()
}

fn status_badge(status: &ShiftStatusKind) -> (&'static str, iced::Color) {
    match status {
        ShiftStatusKind::Waiting => ("Waiting for deposit", color::GREY_3),
        ShiftStatusKind::Pending | ShiftStatusKind::Processing => {
            ("Deposit detected", color::ORANGE)
        }
        ShiftStatusKind::Settling => ("Settling…", color::ORANGE),
        ShiftStatusKind::Settled => ("Bitcoin received ✓", color::GREEN),
        ShiftStatusKind::Expired => ("Expired", color::RED),
        // Not a spinner: a held shift stays here until the user acts.
        ShiftStatusKind::Review => ("On hold for review", color::RED),
        ShiftStatusKind::Refunding => ("Refunding…", color::ORANGE),
        ShiftStatusKind::Refunded => ("Refunded", color::GREY_3),
        ShiftStatusKind::Error => ("Failed", color::RED),
        ShiftStatusKind::Unknown(_) => ("Checking…", color::GREY_3),
    }
}

fn info_card<'a>(
    shift: &'a ShiftResponse,
    selected: &crate::services::sideshift::DepositOption,
) -> Element<'a, Msg> {
    let mut rows = Column::new()
        .spacing(8)
        .push(kv("Shift ID", shift.id.clone()))
        .push(kv("You send", selected.label.to_string()))
        .push(kv("You receive", "Bitcoin (BTC)".to_string()));

    if let (Some(min), Some(max)) = (&shift.deposit_min, &shift.deposit_max) {
        rows = rows.push(kv("Limits", format!("{min} – {max}")));
    }

    Container::new(rows)
        .padding([12, 16])
        .width(Length::Fill)
        .style(theme::card::simple)
        .into()
}

fn kv<'a>(label: &'a str, value: String) -> Element<'a, Msg> {
    Row::new()
        .push(text(label).size(P2_SIZE).style(theme::text::secondary))
        .push(Space::new().width(Length::Fill))
        .push(text(value).size(P2_SIZE))
        .into()
}

// ── Loading / error ─────────────────────────────────────────────────────────

fn loading_view<'a>(label: &'a str) -> Element<'a, Msg> {
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

fn error_view(error: Option<&str>) -> Element<'_, Msg> {
    Container::new(
        Column::new()
            .spacing(20)
            .align_x(Alignment::Center)
            .push(Space::new().height(Length::Fixed(40.0)))
            .push(h3("Something went wrong"))
            .push(
                text(error.unwrap_or("The swap couldn't be set up."))
                    .size(P1_SIZE)
                    .style(theme::text::secondary),
            )
            .push(
                button::primary(None, "Start over")
                    .on_press(Msg::Reset)
                    .width(Length::Fixed(160.0)),
            )
            .max_width(520)
            .width(Length::Fill),
    )
    .center_x(Length::Fill)
    .into()
}
