//! View renderer for [`crate::app::state::spark::send::SparkSend`].
//!
//! Phase 4c ships the minimum viable Send UI: destination input, amount
//! input, Prepare / Confirm / Try again buttons, and tiny status cards
//! for each phase of the state machine. Intentionally plain —
//! polish (fee tier picker, fiat estimate, address book, QR scanner)
//! lands in a later phase once the bridge write path has soaked a bit.

use coincube_ui::{
    component::{
        button,
        text::{h2, h4_bold, p1_regular, p2_regular},
    },
    theme,
    widget::{Column, Container, Element, Row},
};
use iced::{
    widget::{text_input, Space},
    Length,
};

use crate::app::state::spark::send::SparkSendPhase;
use crate::app::view::Message;

pub struct SparkSendView<'a> {
    pub backend_available: bool,
    pub destination_input: &'a str,
    pub amount_input: &'a str,
    pub phase: &'a SparkSendPhase,
}

impl<'a> SparkSendView<'a> {
    pub fn render(self) -> Element<'a, Message> {
        let heading = Container::new(h2("Spark — Send"));

        if !self.backend_available {
            return Column::new()
                .spacing(20)
                .push(heading)
                .push(p1_regular(
                    "Spark is not available for this cube. Set up a Spark \
                     signer to send payments.",
                ))
                .into();
        }

        let mut content = Column::new().spacing(20).push(heading);

        // ── Input card ────────────────────────────────────────────────
        let destination = text_input(
            "BOLT11 invoice, Lightning address, BIP21 URI, or Bitcoin address",
            self.destination_input,
        )
        .on_input(|v| {
            Message::SparkSend(
                crate::app::view::SparkSendMessage::DestinationInputChanged(v),
            )
        })
        .padding(10);

        let amount = text_input(
            "Amount in sats (optional for invoices with amount)",
            self.amount_input,
        )
        .on_input(|v| {
            Message::SparkSend(crate::app::view::SparkSendMessage::AmountInputChanged(v))
        })
        .padding(10);

        let input_card = Container::new(
            Column::new()
                .spacing(10)
                .push(h4_bold("Destination"))
                .push(destination)
                .push(Space::new().height(Length::Fixed(8.0)))
                .push(h4_bold("Amount"))
                .push(amount),
        )
        .padding(16)
        .style(theme::card::simple);
        content = content.push(input_card);

        // ── Phase-specific body ───────────────────────────────────────
        content = content.push(phase_body(self.phase));

        content.into()
    }
}

fn phase_body<'a>(phase: &SparkSendPhase) -> Element<'a, Message> {
    use crate::app::view::SparkSendMessage;

    match phase {
        SparkSendPhase::Idle => Container::new(
            Column::new()
                .spacing(10)
                .push(p2_regular(
                    "Enter a destination and amount above, then press Prepare \
                     to see the fee quote.",
                ))
                .push(Space::new().height(Length::Fixed(8.0)))
                .push(
                    button::primary(None, "Prepare")
                        .on_press(Message::SparkSend(SparkSendMessage::PrepareRequested))
                        .width(Length::Fixed(160.0)),
                ),
        )
        .padding(16)
        .style(theme::card::simple)
        .into(),

        SparkSendPhase::Preparing => Container::new(
            Column::new()
                .spacing(10)
                .push(p1_regular("Preparing send… asking the Spark bridge for a fee quote.")),
        )
        .padding(16)
        .style(theme::card::simple)
        .into(),

        SparkSendPhase::Prepared(ok) => Container::new(
            Column::new()
                .spacing(14)
                .push(h4_bold("Preview"))
                .push(kv_row("Method", ok.method.clone()))
                .push(kv_row("Amount", format!("{} sats", ok.amount_sat)))
                .push(kv_row("Fee", format!("{} sats", ok.fee_sat)))
                .push(kv_row(
                    "Total",
                    format!("{} sats", ok.amount_sat.saturating_add(ok.fee_sat)),
                ))
                .push(Space::new().height(Length::Fixed(8.0)))
                .push(
                    Row::new()
                        .spacing(10)
                        .push(
                            button::primary(None, "Confirm and send")
                                .on_press(Message::SparkSend(
                                    SparkSendMessage::ConfirmRequested,
                                ))
                                .width(Length::Fixed(200.0)),
                        )
                        .push(
                            button::transparent_border(None, "Cancel")
                                .on_press(Message::SparkSend(SparkSendMessage::Reset))
                                .width(Length::Fixed(120.0)),
                        ),
                ),
        )
        .padding(16)
        .style(theme::card::simple)
        .into(),

        SparkSendPhase::Sending => Container::new(
            Column::new()
                .spacing(10)
                .push(p1_regular("Sending… waiting for the Spark SDK to settle the payment.")),
        )
        .padding(16)
        .style(theme::card::simple)
        .into(),

        SparkSendPhase::Sent(ok) => {
            // Clone the payment id so we can hand it to both the kv
            // row (display) and the Copy button (Clipboard message).
            let payment_id = ok.payment_id.clone();
            Container::new(
                Column::new()
                    .spacing(14)
                    .push(h4_bold("Sent"))
                    .push(kv_row("Payment id", ok.payment_id.clone()))
                    .push(kv_row("Amount", format!("{} sats", ok.amount_sat)))
                    .push(kv_row("Fee", format!("{} sats", ok.fee_sat)))
                    .push(Space::new().height(Length::Fixed(8.0)))
                    .push(
                        Row::new()
                            .spacing(10)
                            .push(
                                button::secondary(None, "Copy id")
                                    .on_press(Message::Clipboard(payment_id))
                                    .width(Length::Fixed(140.0)),
                            )
                            .push(
                                button::primary(None, "Send another")
                                    .on_press(Message::SparkSend(SparkSendMessage::Reset))
                                    .width(Length::Fixed(160.0)),
                            ),
                    ),
            )
            .padding(16)
            .style(theme::card::simple)
            .into()
        }

        SparkSendPhase::Error(err) => Container::new(
            Column::new()
                .spacing(10)
                .push(h4_bold("Error"))
                .push(p1_regular(err.clone()))
                .push(Space::new().height(Length::Fixed(8.0)))
                .push(
                    button::primary(None, "Try again")
                        .on_press(Message::SparkSend(SparkSendMessage::Reset))
                        .width(Length::Fixed(140.0)),
                ),
        )
        .padding(16)
        .style(theme::card::simple)
        .into(),
    }
}

fn kv_row<'a>(label: &'a str, value: String) -> Element<'a, Message> {
    Row::new()
        .spacing(20)
        .push(
            Column::new()
                .width(Length::FillPortion(1))
                .push(h4_bold(label)),
        )
        .push(
            Column::new()
                .width(Length::FillPortion(3))
                .push(p1_regular(value)),
        )
        .into()
}
