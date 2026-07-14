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
    widget::{Column, ColumnExt, Container, Element, Row},
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
    /// Cross-chain slippage tolerance, in basis points, as typed. Empty means
    /// the SDK default. Only rendered behind the advanced disclosure.
    pub slippage_input: &'a str,
    /// Whether the advanced (slippage) disclosure is open.
    pub advanced_open: bool,
    /// Seconds left on the live cross-chain quote. `None` when there's no quote
    /// on screen; `<= 0` means expired, which replaces Confirm with a re-quote.
    pub quote_seconds_left: Option<i64>,
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
                "has been sent successfully.",
                Message::SparkSend(crate::app::view::SparkSendMessage::Reset),
            );
        }

        let mut content = Column::new().spacing(20);

        // ── Input card ────────────────────────────────────────────────
        let destination = text_input(
            "Spark address/invoice, BOLT11 invoice, Lightning address, BIP21 URI, or Bitcoin address",
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
        content = content.push(phase_body(
            self.phase,
            self.slippage_input,
            &self.advanced_open,
            self.quote_seconds_left,
        ));

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

fn phase_body<'a>(
    phase: &SparkSendPhase,
    slippage_input: &str,
    advanced_open: &bool,
    quote_seconds_left: Option<i64>,
) -> Element<'a, Message> {
    use crate::app::state::spark::cross_chain;
    use crate::app::view::SparkSendMessage;
    use coincube_ui::component::amount::format_u64_as_string;

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

        // The chain/asset confirmation stop. A cross-chain address does not
        // announce its network, and USDT exists on all of them — so the
        // detected chain and asset are spelled out in words, and the user has
        // to choose a route before anything is quoted.
        SparkSendPhase::CrossChainRoutes {
            address,
            routes,
            selected,
        } => {
            let mut col = Column::new()
                .spacing(12)
                .push(h4_bold("Confirm the network"))
                .push(kv_row("Address", address.address.clone()));

            if let Some(route) = routes.get(*selected) {
                col = col.push(p1_regular(cross_chain::chain_confirmation(address, route)));
            }

            // One row per route. Kept as explicit rows rather than a dropdown:
            // the provider and destination chain are the whole decision here,
            // and a collapsed picker is the kind of thing users click past.
            // The shared `button::*` helpers take `&'static str`, and a route
            // label is built at render time — so these are hand-rolled with the
            // same theme styles rather than leaking the strings.
            let mut route_rows = Column::new().spacing(6);
            for (idx, route) in routes.iter().enumerate() {
                let label = iced::widget::text(format!(
                    "{} on {} — via {}",
                    route.asset, route.chain, route.provider
                ))
                .align_y(iced::Alignment::Center);
                let style = if idx == *selected {
                    theme::button::primary
                } else {
                    theme::button::container_border
                };
                route_rows = route_rows.push(
                    iced::widget::button(label)
                        .style(style)
                        .padding(10)
                        .width(Length::Fill)
                        .on_press(Message::SparkSend(
                            SparkSendMessage::CrossChainRouteSelected(idx),
                        )),
                );
            }
            col = col.push(route_rows);

            // Slippage lives behind a disclosure. A normal user should never
            // have to reason in basis points; the SDK default (1%) is right for
            // them, and surfacing the control by default just invites someone
            // to type a number they don't understand into a money path.
            col = col.push(
                button::transparent_border(
                    None,
                    if *advanced_open {
                        "Hide advanced"
                    } else {
                        "Advanced"
                    },
                )
                .on_press(Message::SparkSend(SparkSendMessage::ToggleAdvanced))
                .width(Length::Fixed(160.0)),
            );
            if *advanced_open {
                col = col
                    .push(p2_regular(format!(
                        "Max slippage, in basis points ({}–{}). Leave blank for the default of \
                         {} ({}%).",
                        cross_chain::MIN_SLIPPAGE_BPS,
                        cross_chain::MAX_SLIPPAGE_BPS,
                        cross_chain::DEFAULT_SLIPPAGE_BPS,
                        cross_chain::DEFAULT_SLIPPAGE_BPS as f64 / 100.0,
                    )))
                    .push(
                        text_input("100", slippage_input)
                            .on_input(|v| {
                                Message::SparkSend(SparkSendMessage::SlippageChanged(v))
                            })
                            .padding(10),
                    );
            }

            col = col.push(Space::new().height(Length::Fixed(8.0))).push(
                Row::new()
                    .spacing(10)
                    .push(
                        button::primary(None, "Get quote")
                            .on_press(Message::SparkSend(
                                SparkSendMessage::CrossChainQuoteRequested,
                            ))
                            .width(Length::Fixed(160.0)),
                    )
                    .push(
                        button::transparent_border(None, "Cancel")
                            .on_press(Message::SparkSend(SparkSendMessage::Reset))
                            .width(Length::Fixed(120.0)),
                    ),
            );

            Container::new(col)
                .padding(16)
                .style(theme::card::simple)
                .into()
        }

        SparkSendPhase::Prepared(ok) => {
            // Cross-chain quotes are denominated in the destination asset and
            // they expire, so they get their own preview rather than being
            // squeezed into the sats-shaped one.
            if let Some(quote) = &ok.cross_chain {
                let expired = quote_seconds_left.is_none_or(|s| s <= 0);
                let mut col = Column::new()
                    .spacing(14)
                    .push(h4_bold("Preview"))
                    .push(kv_row(
                        "You send",
                        format!("{} sats", format_u64_as_string(ok.amount_sat, ",")),
                    ))
                    .push(kv_row(
                        "They receive",
                        format!(
                            "≈ {} {}",
                            cross_chain::format_asset_amount(
                                quote.estimated_out,
                                quote.route.decimals
                            ),
                            quote.route.asset,
                        ),
                    ))
                    .push(kv_row(
                        "Network",
                        format!("{} — via {}", quote.route.chain, quote.route.provider),
                    ))
                    .push(kv_row(
                        "Fee",
                        format!(
                            "{} {} + {} sats network",
                            cross_chain::format_asset_amount(
                                quote.fee_amount,
                                quote.route.decimals
                            ),
                            quote.route.asset,
                            quote.source_transfer_fee_sats,
                        ),
                    ));

                // The countdown. Sending against a dead quote means sending
                // against a rate the provider no longer honours, so Confirm is
                // replaced — not merely disabled — once it runs out.
                col = col.push(if expired {
                    p1_regular("This quote has expired. Get a fresh one to continue.")
                } else {
                    p2_regular(format!(
                        "Quote valid for {}s.",
                        quote_seconds_left.unwrap_or(0)
                    ))
                });

                let action = if expired {
                    button::primary(None, "Get a fresh quote")
                        .on_press(Message::SparkSend(SparkSendMessage::ReQuoteRequested))
                        .width(Length::Fixed(200.0))
                } else {
                    button::primary(None, "Confirm and send")
                        .on_press(Message::SparkSend(SparkSendMessage::ConfirmRequested))
                        .width(Length::Fixed(200.0))
                };

                col = col.push(Space::new().height(Length::Fixed(8.0))).push(
                    Row::new().spacing(10).push(action).push(
                        button::transparent_border(None, "Cancel")
                            .on_press(Message::SparkSend(SparkSendMessage::Reset))
                            .width(Length::Fixed(120.0)),
                    ),
                );

                return Container::new(col)
                    .padding(16)
                    .style(theme::card::simple)
                    .into();
            }

            Container::new(
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
                    .push_maybe(ok.method.starts_with("Spark").then(|| {
                        p2_regular(
                            "Spark transfer — instant, lower fee than Lightning or on-chain.",
                        )
                    }))
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
            .into()
        }

        // A failed cross-chain send. What we offer here depends entirely on
        // whether a retry can double-pay — see `RetryPolicy`. When it can, the
        // user gets "check status", never "try again".
        SparkSendPhase::CrossChainFailed { message, policy } => {
            let mut actions = Row::new().spacing(10);
            if policy.may_retry() {
                actions = actions.push(
                    button::primary(None, "Try again")
                        .on_press(Message::SparkSend(SparkSendMessage::ConfirmRequested))
                        .width(Length::Fixed(160.0)),
                );
            } else {
                actions = actions.push(
                    button::primary(None, "Check status")
                        .on_press(Message::SparkSend(SparkSendMessage::History))
                        .width(Length::Fixed(160.0)),
                );
            }
            actions = actions.push(
                button::transparent_border(None, "Start over")
                    .on_press(Message::SparkSend(SparkSendMessage::Reset))
                    .width(Length::Fixed(140.0)),
            );

            Container::new(
                Column::new()
                    .spacing(12)
                    .push(h4_bold("Send failed"))
                    .push(p1_regular(message.clone()))
                    .push(p2_regular(policy.guidance()))
                    .push(Space::new().height(Length::Fixed(8.0)))
                    .push(actions),
            )
            .padding(16)
            .style(theme::card::simple)
            .into()
        }

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
