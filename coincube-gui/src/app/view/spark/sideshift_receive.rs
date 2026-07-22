//! Inline view for the Spark "receive from another network" (SideShift) flow.
//!
//! Rendered **below the Spark Receive two-card selector**, in place of the
//! Bitcoin rail form — the THEY SEND card shows which asset was picked, so this
//! body carries only what the swap adds: a refund-address field, a third-party
//! disclosure, and the deposit address once a shift is live. Copy always says
//! **bitcoin arrives**, never "receive USDT". No standalone title / asset
//! picker / Back button — navigation happens via the cards + THEY SEND modal.

use coincube_core::miniscript::bitcoin::Amount;
use coincube_ui::{
    color,
    component::{
        amount::{BitcoinDisplayUnit, DisplayAmount},
        button,
        text::*,
    },
    icon::clipboard_icon,
    theme,
    widget::Element,
};
use iced::{
    widget::{qr_code, scrollable, Column, Container, Row, Space, TextInput},
    Alignment, Length,
};

use crate::app::state::spark::esplora::DEPOSIT_MATURITY_CONFIRMATIONS;
use crate::app::state::spark::sideshift_receive::{SparkShiftPhase, SparkSideshiftReceiveFlow};
use crate::app::view::SparkSideshiftReceiveMessage as Msg;
use crate::services::sideshift::{ShiftResponse, ShiftStatusKind};

pub fn spark_sideshift_receive_view(
    flow: &SparkSideshiftReceiveFlow,
    bitcoin_unit: BitcoinDisplayUnit,
) -> Element<'_, Msg> {
    match flow.phase() {
        SparkShiftPhase::Setup => setup_body(flow),
        SparkShiftPhase::FetchingAffiliate | SparkShiftPhase::CreatingShift => {
            loading_body("Setting up your deposit address…")
        }
        SparkShiftPhase::Active => active_body(flow, bitcoin_unit),
        SparkShiftPhase::Arrived => arrived_body(flow, bitcoin_unit),
        SparkShiftPhase::Failed => error_body(flow.error()),
    }
}

// ── Setup: give a refund address, generate the deposit ──────────────────────

fn setup_body(flow: &SparkSideshiftReceiveFlow) -> Element<'_, Msg> {
    let selected = flow.selected();

    // The refund address. Collected *before* any deposit address is shown,
    // because a shift that fails or gets held has to have somewhere to send the
    // money back to — and asking for it afterwards is useless.
    let mut refund = Column::new()
        .spacing(10)
        .push(h4_bold("Refund address"))
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
    let refund_card = Container::new(refund)
        .padding(16)
        .width(Length::Fill)
        .style(theme::card::simple);

    // Master invariant 9: name the third party and the jurisdictional caveat.
    // The user is handing funds to SideShift, not to us, and they should know
    // that before they do it — not after something goes wrong.
    let disclosure = Container::new(
        text(format!(
            "Conversion is performed by SideShift, a third-party service. It isn't available \
             in all jurisdictions, and swaps may be held for review. Bitcoin arrives on-chain \
             and is added to your wallet after about {DEPOSIT_MATURITY_CONFIRMATIONS} \
             confirmations.",
        ))
        .size(P2_SIZE)
        .style(theme::text::secondary),
    )
    .padding([8, 12])
    .width(Length::Fill)
    .style(theme::card::simple);

    let generate_btn = if flow.is_loading() {
        button::primary(None, "Generating…").width(Length::Fill)
    } else {
        button::primary(None, "Generate deposit address")
            .on_press(Msg::Generate)
            .width(Length::Fill)
    };

    let mut col = Column::new().spacing(20).push(refund_card).push(disclosure);
    if let Some(err) = flow.error() {
        col = col.push(
            Container::new(text(err).size(P2_SIZE).color(color::RED))
                .padding([8, 12])
                .width(Length::Fill)
                .style(theme::card::error),
        );
    }
    col.push(generate_btn).into()
}

// ── Active shift: deposit address + status ──────────────────────────────────

fn active_body(
    flow: &SparkSideshiftReceiveFlow,
    bitcoin_unit: BitcoinDisplayUnit,
) -> Element<'_, Msg> {
    let Some(shift) = flow.shift() else {
        return error_body(Some("Shift data missing."));
    };
    let selected = flow.selected();
    let status = flow.shift_status();

    // Sending the wrong asset or chain to this address loses it, and sending an
    // amount outside SideShift's min/max gets it refunded rather than converted.
    // Neither can be caught at input — the deposit is an external send — so both
    // constraints are restated here, next to the address, where the quiet
    // "Limits" row in the details card below is easy to miss.
    let mut warning_lines = Column::new().spacing(2).align_x(Alignment::Center).push(
        text(format!(
            "Only send {} on {} ({})",
            selected.coin.to_uppercase(),
            selected.network.network_name(),
            selected.network.standard_label(),
        ))
        .size(P2_SIZE),
    );
    if let (Some(min), Some(max)) = (&shift.deposit_min, &shift.deposit_max) {
        warning_lines = warning_lines.push(
            text(format!(
                "Send between {min} and {max} {} — amounts outside this range are refunded.",
                selected.coin.to_uppercase(),
            ))
            .size(P2_SIZE),
        );
    }
    let warning_badge = Container::new(
        Container::new(warning_lines)
            .padding([6, 12])
            .style(theme::pill::warning),
    )
    .center_x(Length::Fill);

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

    // The address is a ~100-char monospace string; bound the row to `Fill` and
    // let it scroll horizontally so it stays inside the card and the copy button
    // stays visible (see `liquid::receive` / `vault::receive`).
    let address_row = Row::new()
        .spacing(8)
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .push(
            Container::new(
                scrollable(
                    Column::new()
                        .push(Space::new().height(Length::Fixed(4.0)))
                        .push(
                            text(&shift.deposit_address)
                                .size(P2_SIZE)
                                .font(iced::Font::MONOSPACE),
                        )
                        .push(Space::new().height(Length::Fixed(4.0))),
                )
                .direction(scrollable::Direction::Horizontal(
                    scrollable::Scrollbar::new().width(2).scroller_width(2),
                )),
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
        let mut col = Column::new()
            .spacing(8)
            .align_x(Alignment::Center)
            .width(Length::Fill)
            .push(
                Container::new(text(label).size(P2_SIZE).color(style_color))
                    .padding([4, 10])
                    .style(theme::pill::simple),
            );
        // Once SideShift reports the settle output, show how much bitcoin is on
        // the way — in whichever unit the user has chosen. Shown from the moment
        // a deposit is detected through settle; before settle it's SideShift's
        // moving estimate, so it's labelled "≈", and it firms up as "arriving"
        // under the "Bitcoin arriving" badge.
        if matches!(
            status,
            ShiftStatusKind::Pending
                | ShiftStatusKind::Processing
                | ShiftStatusKind::Settling
                | ShiftStatusKind::Settled
        ) {
            if let Some(sats) = flow.settle_amount_sat() {
                col = col.push(
                    text(settle_amount_display(sats, status, bitcoin_unit))
                        .size(P1_SIZE)
                        .bold(),
                );
            }
        }
        col.push(
            text(spark_shift_guidance(status))
                .size(P2_SIZE)
                .style(theme::text::secondary),
        )
        .into()
    } else {
        Space::new().height(Length::Fixed(0.0)).into()
    };

    Container::new(
        Column::new()
            .spacing(18)
            .width(Length::Fill)
            .push(warning_badge)
            .push(qr_section)
            .push(address_row)
            .push(info_card(shift, &selected))
            .push(status_section),
    )
    .padding(16)
    .width(Length::Fill)
    .style(theme::card::simple)
    .into()
}

// ── Arrived: the swap's bitcoin has landed and been claimed ──────────────────

fn arrived_body(
    flow: &SparkSideshiftReceiveFlow,
    bitcoin_unit: BitcoinDisplayUnit,
) -> Element<'_, Msg> {
    let mut col = Column::new()
        .spacing(12)
        .align_x(Alignment::Center)
        .width(Length::Fill)
        .push(
            Container::new(text("Bitcoin arrived").size(P2_SIZE).color(color::GREEN))
                .padding([4, 10])
                .style(theme::pill::simple),
        );

    if let Some(sats) = flow.arrived_amount_sat() {
        col = col.push(
            text(arrived_amount_display(sats, bitcoin_unit))
                .size(H4_SIZE)
                .bold(),
        );
    }

    Container::new(
        col.push(
            text("Your bitcoin has been added to your Spark wallet.")
                .size(P2_SIZE)
                .style(theme::text::secondary),
        )
        .push(
            button::primary(None, "Done")
                .on_press(Msg::Reset)
                .width(Length::Fixed(160.0)),
        ),
    )
    .padding(24)
    .width(Length::Fill)
    .center_x(Length::Fill)
    .style(theme::card::simple)
    .into()
}

/// Spark-specific status guidance. Diverges from the shared
/// [`ShiftStatusKind::guidance`] at `Settled`: a Spark settle lands as on-chain
/// BTC in the wallet's static deposit address, so it *hasn't* arrived in the
/// spendable balance yet — it's confirming, and gets added automatically once it
/// matures. The shared copy says "has arrived", which is what sent users looking
/// for a transaction that wasn't there yet.
fn spark_shift_guidance(status: &ShiftStatusKind) -> &'static str {
    match status {
        ShiftStatusKind::Settled => {
            "Converting complete. Your bitcoin is arriving on-chain and will be added to your \
             wallet automatically after about 3 confirmations — ~30 minutes."
        }
        other => other.guidance(),
    }
}

fn settle_amount_display(
    sats: u64,
    status: &ShiftStatusKind,
    bitcoin_unit: BitcoinDisplayUnit,
) -> String {
    let settled = matches!(status, ShiftStatusKind::Settled);
    format!(
        "{}{} {}",
        if settled { "" } else { "≈ " },
        Amount::from_sat(sats).to_formatted_string_with_unit(bitcoin_unit),
        bitcoin_unit_label(bitcoin_unit),
    )
}

fn arrived_amount_display(sats: u64, bitcoin_unit: BitcoinDisplayUnit) -> String {
    format!(
        "{} {}",
        Amount::from_sat(sats).to_formatted_string_with_unit(bitcoin_unit),
        bitcoin_unit_label(bitcoin_unit),
    )
}

fn bitcoin_unit_label(bitcoin_unit: BitcoinDisplayUnit) -> &'static str {
    if matches!(bitcoin_unit, BitcoinDisplayUnit::BTC) {
        "BTC"
    } else {
        "SATS"
    }
}

fn status_badge(status: &ShiftStatusKind) -> (&'static str, iced::Color) {
    match status {
        ShiftStatusKind::Waiting => ("Waiting for deposit", color::GREY_3),
        ShiftStatusKind::Pending | ShiftStatusKind::Processing => {
            ("Deposit detected", color::ORANGE)
        }
        ShiftStatusKind::Settling => ("Settling…", color::ORANGE),
        // Not "received": a Spark settle is on-chain BTC into the wallet's
        // static deposit address, so at this point it's confirming, not yet
        // spendable. Orange (in-progress), not green (done) — the green
        // "arrived" moment is the celebration that fires once it's claimed.
        ShiftStatusKind::Settled => ("Bitcoin arriving", color::ORANGE),
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
        .style(theme::card::border)
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

fn loading_body<'a>(label: &'a str) -> Element<'a, Msg> {
    Container::new(text(label).size(P1_SIZE).style(theme::text::secondary))
        .padding(24)
        .width(Length::Fill)
        .center_x(Length::Fill)
        .style(theme::card::simple)
        .into()
}

fn error_body(error: Option<&str>) -> Element<'_, Msg> {
    Column::new()
        .spacing(16)
        .push(
            Container::new(
                Column::new()
                    .spacing(8)
                    .push(h4_bold("Something went wrong"))
                    .push(
                        text(error.unwrap_or("The swap couldn't be set up."))
                            .size(P2_SIZE)
                            .style(theme::text::secondary),
                    ),
            )
            .padding(16)
            .width(Length::Fill)
            .style(theme::card::error),
        )
        .push(
            button::primary(None, "Start over")
                .on_press(Msg::Reset)
                .width(Length::Fixed(160.0)),
        )
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settled_guidance_uses_spark_confirming_copy() {
        let spark_copy = spark_shift_guidance(&ShiftStatusKind::Settled);
        let shared_copy = ShiftStatusKind::Settled.guidance();

        assert_ne!(spark_copy, shared_copy);
        assert!(spark_copy.contains("arriving on-chain"));
        assert!(spark_copy.contains("automatically"));
        assert!(spark_copy.contains("3 confirmations"));
        assert!(!spark_copy.contains("has arrived"));
    }

    #[test]
    fn non_settled_guidance_delegates_to_shared_status_copy() {
        for status in [
            ShiftStatusKind::Waiting,
            ShiftStatusKind::Pending,
            ShiftStatusKind::Processing,
            ShiftStatusKind::Settling,
            ShiftStatusKind::Expired,
            ShiftStatusKind::Review,
            ShiftStatusKind::Refunding,
            ShiftStatusKind::Refunded,
            ShiftStatusKind::Error,
        ] {
            assert_eq!(spark_shift_guidance(&status), status.guidance());
        }
    }

    #[test]
    fn status_badges_distinguish_arriving_review_and_done_states() {
        let (settled_label, settled_color) = status_badge(&ShiftStatusKind::Settled);
        assert_eq!(settled_label, "Bitcoin arriving");
        assert_eq!(settled_color, color::ORANGE);

        let (review_label, review_color) = status_badge(&ShiftStatusKind::Review);
        assert_eq!(review_label, "On hold for review");
        assert_eq!(review_color, color::RED);

        let (refunded_label, refunded_color) = status_badge(&ShiftStatusKind::Refunded);
        assert_eq!(refunded_label, "Refunded");
        assert_eq!(refunded_color, color::GREY_3);

        let (unknown_label, unknown_color) =
            status_badge(&ShiftStatusKind::Unknown("future".to_string()));
        assert_eq!(unknown_label, "Checking…");
        assert_eq!(unknown_color, color::GREY_3);
    }

    #[test]
    fn status_badges_cover_waiting_in_flight_and_failed_states() {
        for status in [ShiftStatusKind::Pending, ShiftStatusKind::Processing] {
            let (label, color) = status_badge(&status);
            assert_eq!(label, "Deposit detected");
            assert_eq!(color, color::ORANGE);
        }

        assert_eq!(
            status_badge(&ShiftStatusKind::Waiting),
            ("Waiting for deposit", color::GREY_3)
        );
        assert_eq!(
            status_badge(&ShiftStatusKind::Settling),
            ("Settling…", color::ORANGE)
        );
        assert_eq!(
            status_badge(&ShiftStatusKind::Expired),
            ("Expired", color::RED)
        );
        assert_eq!(
            status_badge(&ShiftStatusKind::Refunding),
            ("Refunding…", color::ORANGE)
        );
        assert_eq!(
            status_badge(&ShiftStatusKind::Error),
            ("Failed", color::RED)
        );
    }

    #[test]
    fn settle_amount_display_is_approx_until_settled_and_uses_selected_unit() {
        assert_eq!(
            settle_amount_display(
                123_456_789,
                &ShiftStatusKind::Processing,
                BitcoinDisplayUnit::BTC
            ),
            "≈ 1.23 456 789 BTC"
        );
        assert_eq!(
            settle_amount_display(
                123_456_789,
                &ShiftStatusKind::Settled,
                BitcoinDisplayUnit::BTC
            ),
            "1.23 456 789 BTC"
        );
        assert_eq!(
            settle_amount_display(
                123_456_789,
                &ShiftStatusKind::Pending,
                BitcoinDisplayUnit::Sats
            ),
            "≈ 123,456,789 SATS"
        );
    }

    #[test]
    fn arrived_amount_display_is_never_approximate() {
        assert_eq!(
            arrived_amount_display(42_000, BitcoinDisplayUnit::Sats),
            "42,000 SATS"
        );
        assert_eq!(
            arrived_amount_display(42_000, BitcoinDisplayUnit::BTC),
            "0.00 042 000 BTC"
        );
    }
}
