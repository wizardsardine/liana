//! View renderer for [`crate::app::state::spark::receive::SparkReceive`].
//!
//! Phase 4c ships the minimum viable Receive UI: method radio buttons,
//! BOLT11 amount/description inputs (hidden for on-chain), a Generate
//! button, and a result card that shows the invoice/address as a
//! selectable text field. QR codes, copy animations, Lightning Address
//! display, and the on-chain claim-deposit lifecycle land in Phase 4d.

use coincube_ui::{
    component::{
        button,
        text::{h2, h4_bold, p1_regular, p2_regular},
    },
    theme,
    widget::{Column, Container, Element, Row},
};
use iced::{
    widget::{qr_code, text_input, QRCode, Space},
    Alignment, Length,
};

use coincube_spark_protocol::DepositInfo;

use crate::app::state::spark::receive::{SparkReceiveMethod, SparkReceivePhase};
use crate::app::view::{Message, SparkReceiveMessage};

pub struct SparkReceiveView<'a> {
    pub backend_available: bool,
    pub method: SparkReceiveMethod,
    pub amount_input: &'a str,
    pub description_input: &'a str,
    pub phase: &'a SparkReceivePhase,
    pub qr_data: Option<&'a qr_code::Data>,
    /// Phase 4f: list of pending on-chain deposits surfaced from the
    /// SDK's `list_unclaimed_deposits` RPC. Rendered as a dedicated
    /// card below the main phase body.
    pub pending_deposits: &'a [DepositInfo],
    /// Phase 4f: which deposit is currently being claimed (in-flight
    /// RPC). Used to disable the row's button while waiting.
    pub claiming: Option<&'a (String, u32)>,
    /// Phase 4f: transient error from the most recent claim attempt.
    pub claim_error: Option<&'a str>,
    pub received_amount_display: &'a str,
    pub received_quote: &'a coincube_ui::component::quote_display::Quote,
    pub received_image_handle: &'a iced::widget::image::Handle,
}

impl<'a> SparkReceiveView<'a> {
    pub fn render(self) -> Element<'a, Message> {
        let heading = Container::new(h2("Spark — Receive"));

        if !self.backend_available {
            return Column::new()
                .spacing(20)
                .push(heading)
                .push(p1_regular(
                    "Spark is not available for this cube. Set up a Spark \
                     signer to receive payments.",
                ))
                .into();
        }

        // ── Full-screen celebration for received payments ─────────────
        if matches!(self.phase, SparkReceivePhase::Received { .. }) {
            return coincube_ui::component::received_celebration_page(
                "lightning-receive",
                self.received_amount_display,
                self.received_quote,
                self.received_image_handle,
                Message::SparkReceive(SparkReceiveMessage::Reset),
            );
        }

        let mut content = Column::new().spacing(20).push(heading);

        // ── Method picker ─────────────────────────────────────────────
        let method_picker = Container::new(
            Column::new().spacing(10).push(h4_bold("Method")).push(
                Row::new()
                    .spacing(10)
                    .push(method_chip(
                        "Lightning (BOLT11)",
                        self.method == SparkReceiveMethod::Bolt11,
                        SparkReceiveMethod::Bolt11,
                    ))
                    .push(method_chip(
                        "On-chain Bitcoin",
                        self.method == SparkReceiveMethod::OnchainBitcoin,
                        SparkReceiveMethod::OnchainBitcoin,
                    )),
            ),
        )
        .padding(16)
        .style(theme::card::simple);
        content = content.push(method_picker);

        // ── Method-specific inputs ────────────────────────────────────
        if self.method == SparkReceiveMethod::Bolt11 {
            let amount = text_input("Amount in sats (optional)", self.amount_input)
                .on_input(|v| {
                    Message::SparkReceive(
                        crate::app::view::SparkReceiveMessage::AmountInputChanged(v),
                    )
                })
                .padding(10);

            let description = text_input(
                "Description shown to the payer (optional)",
                self.description_input,
            )
            .on_input(|v| {
                Message::SparkReceive(
                    crate::app::view::SparkReceiveMessage::DescriptionInputChanged(v),
                )
            })
            .padding(10);

            let form_card = Container::new(
                Column::new()
                    .spacing(10)
                    .push(h4_bold("Invoice details"))
                    .push(amount)
                    .push(Space::new().height(Length::Fixed(6.0)))
                    .push(description),
            )
            .padding(16)
            .style(theme::card::simple);
            content = content.push(form_card);
        } else {
            // On-chain: nothing to configure in Phase 4c. Explain the
            // deposit model so the user knows what to expect.
            let info_card = Container::new(
                Column::new()
                    .spacing(8)
                    .push(h4_bold("Spark on-chain receive"))
                    .push(p2_regular(
                        "Spark uses a deposit-address model — the user sends \
                         BTC to the address below, the bridge notices the \
                         incoming tx, and the funds become spendable after \
                         the SDK claims the deposit (automatic background \
                         process in a future phase; today the user may need \
                         to restart the cube to surface the claim). Phase \
                         4d wires the explicit claim lifecycle into the UI.",
                    )),
            )
            .padding(16)
            .style(theme::card::simple);
            content = content.push(info_card);
        }

        // ── Phase-specific body ───────────────────────────────────────
        content = content.push(phase_body(self.phase, self.qr_data));

        // ── Phase 4f: pending on-chain deposits card ──────────────────
        // Renders only when there's something to show. The list
        // refreshes automatically on `DepositsChanged` events from
        // the bridge, so the card appears the moment the SDK
        // observes an incoming deposit (no manual refresh needed).
        if !self.pending_deposits.is_empty() || self.claim_error.is_some() {
            content = content.push(pending_deposits_card(
                self.pending_deposits,
                self.claiming,
                self.claim_error,
            ));
        }

        content.into()
    }
}

fn pending_deposits_card<'a>(
    deposits: &'a [DepositInfo],
    claiming: Option<&'a (String, u32)>,
    claim_error: Option<&'a str>,
) -> Element<'a, Message> {
    let mut card = Column::new()
        .spacing(12)
        .push(h4_bold("Pending deposits"))
        .push(p2_regular(
            "Spark notices incoming on-chain transactions automatically. \
             Mature deposits can be claimed into your wallet below.",
        ));

    if let Some(err) = claim_error {
        card = card.push(p2_regular(format!("Claim failed: {}", err)));
    }

    for deposit in deposits {
        card = card.push(deposit_row(deposit, claiming));
    }

    Container::new(card)
        .padding(16)
        .style(theme::card::simple)
        .into()
}

fn deposit_row<'a>(
    deposit: &'a DepositInfo,
    claiming: Option<&'a (String, u32)>,
) -> Element<'a, Message> {
    let is_being_claimed = claiming
        .map(|(txid, vout)| txid == &deposit.txid && *vout == deposit.vout)
        .unwrap_or(false);

    let amount_label = format!("{} sats", deposit.amount_sat);
    let txid_short = if deposit.txid.len() > 16 {
        format!(
            "{}…{}",
            &deposit.txid[..8],
            &deposit.txid[deposit.txid.len() - 8..]
        )
    } else {
        deposit.txid.clone()
    };
    let txid_label = format!("{}:{}", txid_short, deposit.vout);

    // Right-side action: either a Claim button (mature, idle), a
    // disabled "Claiming…" button (in-flight), or a status text
    // (immature / errored).
    let action: Element<'_, Message> = if is_being_claimed {
        // Disabled button — `on_press_maybe(None)` keeps the visual
        // weight without firing on click.
        button::primary(None, "Claiming…")
            .on_press_maybe(None)
            .width(Length::Fixed(140.0))
            .into()
    } else if !deposit.is_mature {
        // Immature — show waiting status, no button.
        p2_regular("Waiting for confirmation").into()
    } else if let Some(err) = &deposit.claim_error {
        // Previous claim attempt failed for a reason the SDK
        // surfaces. Show a short hint + Retry button.
        Row::new()
            .spacing(8)
            .align_y(Alignment::Center)
            .push(p2_regular(format!("Error: {}", short_error(err))))
            .push(
                button::secondary(None, "Retry")
                    .on_press(Message::SparkReceive(
                        crate::app::view::SparkReceiveMessage::ClaimDepositRequested {
                            txid: deposit.txid.clone(),
                            vout: deposit.vout,
                        },
                    ))
                    .width(Length::Fixed(100.0)),
            )
            .into()
    } else {
        button::primary(None, "Claim")
            .on_press(Message::SparkReceive(
                crate::app::view::SparkReceiveMessage::ClaimDepositRequested {
                    txid: deposit.txid.clone(),
                    vout: deposit.vout,
                },
            ))
            .width(Length::Fixed(140.0))
            .into()
    };

    Container::new(
        Row::new()
            .spacing(12)
            .align_y(Alignment::Center)
            .push(
                Column::new()
                    .spacing(4)
                    .width(Length::Fill)
                    .push(p1_regular(amount_label))
                    .push(p2_regular(txid_label)),
            )
            .push(action),
    )
    .padding(10)
    .style(theme::card::simple)
    .into()
}

/// Truncate a long SDK error string for inline display. The full
/// error stays in the panel state; this just keeps the row from
/// blowing up vertically.
fn short_error(err: &str) -> String {
    const MAX: usize = 60;
    if err.len() <= MAX {
        err.to_string()
    } else {
        format!("{}…", &err[..MAX])
    }
}

fn method_chip<'a>(
    label: &'static str,
    active: bool,
    target: SparkReceiveMethod,
) -> Element<'a, Message> {
    let btn = if active {
        button::primary(None, label)
    } else {
        button::transparent_border(None, label)
    };
    btn.on_press(Message::SparkReceive(
        crate::app::view::SparkReceiveMessage::MethodSelected(target),
    ))
    .width(Length::Fixed(200.0))
    .into()
}

fn phase_body<'a>(
    phase: &'a SparkReceivePhase,
    qr_data: Option<&'a qr_code::Data>,
) -> Element<'a, Message> {
    use crate::app::view::SparkReceiveMessage;

    match phase {
        SparkReceivePhase::Idle => Container::new(
            Column::new().spacing(10).push(
                button::primary(None, "Generate")
                    .on_press(Message::SparkReceive(
                        SparkReceiveMessage::GenerateRequested,
                    ))
                    .width(Length::Fixed(160.0)),
            ),
        )
        .padding(16)
        .style(theme::card::simple)
        .into(),

        SparkReceivePhase::Generating => Container::new(Column::new().spacing(10).push(
            p1_regular("Generating… asking the Spark bridge for a payment request."),
        ))
        .padding(16)
        .style(theme::card::simple)
        .into(),

        SparkReceivePhase::Generated(ok) => {
            // QR code block — rendered only if we successfully
            // encoded the payment request in state. Falls back to a
            // plain "QR unavailable" note if encoding failed; for
            // typical BOLT11 / BTC address payloads this branch
            // should never fire in practice.
            let qr_block: Element<'_, Message> = if let Some(qr) = qr_data {
                Container::new(QRCode::<coincube_ui::theme::Theme>::new(qr).cell_size(6))
                    .align_x(iced::alignment::Horizontal::Center)
                    .width(Length::Fill)
                    .into()
            } else {
                p2_regular("QR code unavailable for this payload.").into()
            };

            let payment_request = ok.payment_request.clone();
            Container::new(
                Column::new()
                    .spacing(14)
                    .align_x(Alignment::Center)
                    .push(h4_bold("Payment request"))
                    .push(qr_block)
                    .push(Space::new().height(Length::Fixed(6.0)))
                    // Click-to-select text_input as a manual-copy
                    // fallback when the button isn't enough.
                    .push(text_input("", &ok.payment_request).padding(10))
                    .push(
                        Row::new()
                            .spacing(10)
                            .push(
                                button::primary(None, "Copy")
                                    .on_press(Message::Clipboard(payment_request))
                                    .width(Length::Fixed(120.0)),
                            )
                            .push(
                                button::transparent_border(None, "Generate another")
                                    .on_press(Message::SparkReceive(SparkReceiveMessage::Reset))
                                    .width(Length::Fixed(180.0)),
                            ),
                    )
                    .push(kv_row("Fee", format!("{} sats", ok.fee_sat))),
            )
            .padding(16)
            .style(theme::card::simple)
            .into()
        }

        SparkReceivePhase::Received { .. } => {
            // Handled by the full-screen celebration in render()
            Container::new(Column::new()).into()
        }

        SparkReceivePhase::Error(err) => Container::new(
            Column::new()
                .spacing(10)
                .push(h4_bold("Error"))
                .push(p1_regular(err.clone()))
                .push(Space::new().height(Length::Fixed(8.0)))
                .push(
                    button::primary(None, "Try again")
                        .on_press(Message::SparkReceive(SparkReceiveMessage::Reset))
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
