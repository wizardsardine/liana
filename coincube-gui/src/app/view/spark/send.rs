//! View renderer for [`crate::app::state::spark::send::SparkSend`].
//!
//! Phase 4c ships the minimum viable Send UI: destination input, amount
//! input, Prepare / Confirm / Try again buttons, and tiny status cards
//! for each phase of the state machine. Intentionally plain —
//! polish (fee tier picker, fiat estimate, address book, QR scanner)
//! lands in a later phase once the bridge write path has soaked a bit.

use coincube_ui::{
    component::{
        amount::BitcoinDisplayUnit,
        button,
        text::{h4_bold, p1_regular, p2_regular},
    },
    theme,
    widget::{Column, Container, Element, Row},
};
use iced::{
    widget::{text_input, Space},
    Length,
};

use crate::app::state::spark::send::SparkSendPhase;
use crate::app::view::spark::{last_tx::last_transactions_section, SparkRecentTransaction};
use crate::app::view::{Message, SparkSendMessage};

pub struct SparkSendView<'a> {
    pub backend_available: bool,
    pub destination_input: &'a str,
    pub amount_input: &'a str,
    pub phase: &'a SparkSendPhase,
    pub sent_amount_display: &'a str,
    pub sent_celebration_context: &'a str,
    pub sent_quote: &'a coincube_ui::component::quote_display::Quote,
    pub sent_image_handle: &'a iced::widget::image::Handle,
    pub recent_transactions: &'a [SparkRecentTransaction],
    pub bitcoin_unit: BitcoinDisplayUnit,
    pub show_direction_badges: bool,
}

impl<'a> SparkSendView<'a> {
    pub fn render(self) -> Element<'a, Message> {
        if !self.backend_available {
            return Column::new()
                .spacing(20)
                .push(p1_regular(
                    "Spark is not available for this cube. Set up a Spark \
                     signer to send payments.",
                ))
                .into();
        }

        // ── Full-screen celebration for successful sends ─────────────
        if matches!(self.phase, SparkSendPhase::Sent(_)) {
            return coincube_ui::component::sent_celebration_page(
                self.sent_celebration_context,
                self.sent_amount_display,
                self.sent_quote,
                self.sent_image_handle,
                Message::SparkSend(crate::app::view::SparkSendMessage::Reset),
            );
        }

        let mut content = Column::new().spacing(20);

        // ── Input card ────────────────────────────────────────────────
        let destination = text_input(
            "BOLT11 invoice, Lightning address, BIP21 URI, or Bitcoin address",
            self.destination_input,
        )
        .on_input(|v| {
            Message::SparkSend(crate::app::view::SparkSendMessage::DestinationInputChanged(
                v,
            ))
        })
        .padding(10);

        let amount = text_input(
            "Amount in sats (optional for invoices with amount)",
            self.amount_input,
        )
        .on_input(|v| Message::SparkSend(crate::app::view::SparkSendMessage::AmountInputChanged(v)))
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

        // ── Last transactions ─────────────────────────────────────────
        content = content.push(last_transactions_section(
            self.recent_transactions,
            self.bitcoin_unit,
            self.show_direction_badges,
            |idx| Message::SparkSend(SparkSendMessage::SelectTransaction(idx)),
            Message::SparkSend(SparkSendMessage::History),
        ));

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

        SparkSendPhase::Preparing => Container::new(Column::new().spacing(10).push(p1_regular(
            "Preparing send… asking the Spark bridge for a fee quote.",
        )))
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
                                .on_press(Message::SparkSend(SparkSendMessage::ConfirmRequested))
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

        SparkSendPhase::Sending => Container::new(Column::new().spacing(10).push(p1_regular(
            "Sending… waiting for the Spark SDK to settle the payment.",
        )))
        .padding(16)
        .style(theme::card::simple)
        .into(),

        SparkSendPhase::Sent(_) => {
            // Handled by the full-screen celebration in render()
            Container::new(Column::new()).into()
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
