//! View renderer for [`crate::app::state::spark::send::SparkSend`].
//!
//! Phase 4c ships the minimum viable Send UI: destination input, amount
//! input, Prepare / Confirm / Try again buttons, and tiny status cards
//! for each phase of the state machine. Intentionally plain —
//! polish (fee tier picker, fiat estimate, address book, QR scanner)
//! lands in a later phase once the bridge write path has soaked a bit.

use coincube_ui::{
    color,
    component::{
        amount::{BitcoinDisplayUnit, DisplayAmount},
        button,
        text::*,
    },
    image::{asset_logo, asset_network_logo},
    theme,
    widget::{Column, ColumnExt, Container, Element, Row},
};
use iced::{
    widget::{button as iced_button, container, text_input, Space},
    Alignment, Background, Length,
};

use coincube_core::miniscript::bitcoin::{Amount, Network};

use crate::app::breez_spark::assets::stable_token_as_sats;
use crate::app::state::spark::cross_chain::{self, supported_on};
use crate::app::state::spark::send::{CrossChainContext, SparkSendPhase, SparkSendTarget};
use crate::app::view::shared::picker::picker_row;
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
    /// Unified balance (sats: BTC + Stable Balance), shown on the YOU SEND card.
    pub balance_sats: u64,
    pub bitcoin_unit: BitcoinDisplayUnit,
    /// BTC/USD reference price for the cross-chain conversion-fee sats estimate.
    /// `None` when no price is known — the fee then shows in the asset only.
    pub reference_btc_usd_price: Option<f64>,
    pub show_direction_badges: bool,
    /// The "THEY RECEIVE" selection — drives the two-card selector and the
    /// destination placeholder.
    pub receive_target: SparkSendTarget,
    /// Bitcoin network the cube runs on — gates the stablecoin picker options
    /// (cross-chain is mainnet-only).
    pub network: Network,
    /// The cross-chain destination + routes for the current send, when there is
    /// one. Lives on the panel rather than in the phase because it must survive
    /// a failed send — a retry re-prepares from it.
    pub cross_chain_ctx: Option<&'a CrossChainContext>,
    /// Cross-chain slippage tolerance, in basis points, as typed. Empty means
    /// the SDK default. Only rendered behind the advanced disclosure.
    pub slippage_input: &'a str,
    /// Whether the advanced (slippage) disclosure is open.
    pub advanced_open: bool,
    /// Life left on the live cross-chain quote. `None` means there is no quote
    /// on screen — *not* "expired", which is its own variant. Anything other
    /// than `Valid` replaces Confirm with a re-quote.
    pub quote_countdown: Option<cross_chain::QuoteCountdown>,
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

        // ── Two-card selector: YOU SEND (Bitcoin) → THEY RECEIVE ───────
        // YOU SEND is fixed (the Spark wallet spends bitcoin); THEY RECEIVE is
        // the picker (bitcoin rails + USDt/USDC). Wired at the state's `view()`.
        content = content.push(spark_send_cards(
            self.receive_target,
            self.balance_sats,
            self.bitcoin_unit,
        ));

        // ── Input card ────────────────────────────────────────────────
        let destination = text_input(
            self.receive_target.destination_placeholder(),
            self.destination_input,
        )
        .on_input(|v| {
            Message::SparkSend(crate::app::view::SparkSendMessage::DestinationInputChanged(
                v,
            ))
        })
        .padding(10);

        // The amount field follows the wallet's display unit, like the Vault
        // send form: label, placeholder, and the parse in state all switch on
        // it, so a BTC-configured wallet enters BTC and a sats one enters sats.
        let is_btc = matches!(self.bitcoin_unit, BitcoinDisplayUnit::BTC);
        let amount_label = if is_btc {
            "Amount (BTC)"
        } else {
            "Amount (sats)"
        };
        let amount_placeholder = match (self.receive_target.is_stablecoin(), is_btc) {
            (true, true) => "Amount in BTC — funded from your Bitcoin balance",
            (true, false) => "Amount in sats — funded from your Bitcoin balance",
            (false, true) => "Amount in BTC (optional for invoices with amount)",
            (false, false) => "Amount in sats (optional for invoices with amount)",
        };
        let amount = text_input(amount_placeholder, self.amount_input)
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
                .push(h4_bold(amount_label))
                .push(amount),
        )
        .padding(16)
        .style(theme::card::simple);
        content = content.push(input_card);

        // ── Phase-specific body ───────────────────────────────────────
        content = content.push(phase_body(
            self.phase,
            self.cross_chain_ctx,
            self.slippage_input,
            &self.advanced_open,
            self.quote_countdown.clone(),
            self.reference_btc_usd_price,
            self.bitcoin_unit,
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
    cross_chain_ctx: Option<&CrossChainContext>,
    slippage_input: &str,
    advanced_open: &bool,
    quote_countdown: Option<cross_chain::QuoteCountdown>,
    reference_btc_usd_price: Option<f64>,
    bitcoin_unit: BitcoinDisplayUnit,
) -> Element<'a, Message> {
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
        SparkSendPhase::CrossChainRoutes => {
            let Some(ctx) = cross_chain_ctx else {
                // The phase can't be reached without a context — but render an
                // honest message rather than a blank card if it ever is.
                return Container::new(p1_regular("No cross-chain destination selected."))
                    .padding(16)
                    .style(theme::card::simple)
                    .into();
            };
            let (address, routes, selected) = (&ctx.address, &ctx.routes, &ctx.selected);

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
                            .on_input(|v| Message::SparkSend(SparkSendMessage::SlippageChanged(v)))
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
                // A quote whose expiry we couldn't parse counts as expired: an
                // unreadable expiry is not evidence the quote is *good*, and
                // re-quoting costs a second where sending against an
                // unknown-age rate costs money. `None` here means there is no
                // quote at all, which can't happen inside this branch — the
                // state sets the countdown the moment a quote lands.
                let expired = !matches!(
                    quote_countdown,
                    Some(cross_chain::QuoteCountdown::Valid { .. })
                );
                let mut col = Column::new()
                    .spacing(14)
                    .push(h4_bold("Preview"))
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
                    // The old single mixed-unit "Fee" line hid how much each part
                    // cost. Split it: a sats network fee (moving BTC to the
                    // provider) and a destination-asset conversion fee (provider
                    // spread + gas, already netted out of "They receive").
                    .push(kv_row(
                        "Network fee",
                        format!(
                            "{} sats",
                            format_u64_as_string(quote.source_transfer_fee_sats, ",")
                        ),
                    ))
                    .push(kv_row(
                        "Conversion fee",
                        conversion_fee_display(
                            quote.fee_amount,
                            quote.route.decimals,
                            &quote.route.asset,
                            reference_btc_usd_price,
                            bitcoin_unit,
                        ),
                    ))
                    // Headline: the full sats debit from the wallet, fees
                    // included. Set apart at the bottom so the user sees exactly
                    // what they'll pay — and it matches the amount the post-send
                    // celebration reports as sent.
                    .push(Space::new().height(Length::Fixed(4.0)))
                    .push(total_row(
                        "Total you send",
                        format!("{} sats", format_u64_as_string(ok.amount_sat, ",")),
                    ));

                // The countdown. Sending against a dead quote means sending
                // against a rate the provider no longer honours, so Confirm is
                // replaced — not merely disabled — once it runs out.
                col = col.push(match quote_countdown {
                    Some(cross_chain::QuoteCountdown::Valid { seconds_left }) => {
                        p2_regular(format!("Quote valid for {}s.", seconds_left))
                    }
                    _ => p1_regular("This quote has expired. Get a fresh one to continue."),
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
                    // Not `ConfirmRequested` (that only fires from `Prepared`).
                    // `CrossChainRetryRequested` re-sends the same retained quote
                    // — same swap id, so the BTC leg can't pay twice — or, once
                    // the quote has expired, downgrades to "check status".
                    button::primary(None, "Try again")
                        .on_press(Message::SparkSend(
                            SparkSendMessage::CrossChainRetryRequested,
                        ))
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

/// The cross-chain conversion fee for the preview: the fee in the destination
/// asset, with a sats (or BTC) approximation alongside — the fee is quoted in
/// USDT/USDC but the user spends bitcoin, so the sats figure is what compares
/// against "Total you send". Asset-only when no BTC/USD price is available.
fn conversion_fee_display(
    fee_amount: u128,
    decimals: u8,
    asset: &str,
    reference_btc_usd_price: Option<f64>,
    bitcoin_unit: BitcoinDisplayUnit,
) -> String {
    let asset_part = format!(
        "{} {}",
        cross_chain::format_asset_amount(fee_amount, decimals),
        asset,
    );
    // The sats hint needs a real BTC/USD price *and* a fee that fits the u64
    // conversion input; without either, show the asset amount alone rather than
    // a mispriced or clamped-and-understated estimate. (`sats == 0` alone can't
    // gate this — it also means a sub-1-sat fee, which is still worth showing.)
    let Some(price) = reference_btc_usd_price.filter(|p| *p > 0.0) else {
        return asset_part;
    };
    if fee_amount > u64::MAX as u128 {
        return asset_part;
    }
    // USDT/USDC are USD-pegged, so value the fee in sats the same way the wallet
    // values its Stable Balance holding.
    let sats = stable_token_as_sats(fee_amount as u64, u32::from(decimals), Some(price));
    let unit = if matches!(bitcoin_unit, BitcoinDisplayUnit::BTC) {
        "BTC"
    } else {
        "SATS"
    };
    format!(
        "{asset_part} (≈ {} {unit})",
        Amount::from_sat(sats).to_formatted_string_with_unit(bitcoin_unit),
    )
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

/// Like [`kv_row`] but with an emphasised (bold) value — for the headline
/// "total" line the eye should land on.
fn total_row<'a>(label: &'a str, value: String) -> Element<'a, Message> {
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
                .push(h4_bold(value)),
        )
        .into()
}

// ── Two-card "YOU SEND → THEY RECEIVE" selector + picker ─────────────────────

/// Logo for a send target: bitcoin rails get the BTC coin badged with the rail;
/// stablecoins get the plain coin (the destination chain isn't known until a
/// route is picked, so there's no network badge to show yet).
fn target_logo<'a>(target: SparkSendTarget, size: f32) -> Element<'a, Message> {
    match target {
        SparkSendTarget::Lightning => asset_network_logo("btc", "lightning", size),
        SparkSendTarget::OnChain => asset_network_logo("btc", "bitcoin", size),
        SparkSendTarget::Spark => asset_network_logo("btc", "spark", size),
        SparkSendTarget::Usdt => asset_logo("usdt")
            .width(Length::Fixed(size))
            .height(Length::Fixed(size))
            .into(),
        SparkSendTarget::Usdc => asset_logo("usdc")
            .width(Length::Fixed(size))
            .height(Length::Fixed(size))
            .into(),
    }
}

/// Small orange-outlined pill (asset network / rail label).
fn orange_badge<'a>(label: &str) -> Element<'a, Message> {
    Container::new(text(label.to_uppercase()).size(11).color(color::ORANGE))
        .padding([2, 8])
        .style(|_: &theme::Theme| container::Style {
            border: iced::Border {
                color: color::ORANGE,
                width: 1.0,
                radius: 8.0.into(),
            },
            ..Default::default()
        })
        .into()
}

/// Hover-highlight style for the tappable THEY RECEIVE card.
fn card_button_style(
    _: &theme::Theme,
    status: iced::widget::button::Status,
) -> iced::widget::button::Style {
    iced::widget::button::Style {
        background: Some(Background::Color(color::TRANSPARENT)),
        border: iced::Border {
            color: if matches!(status, iced::widget::button::Status::Hovered) {
                color::ORANGE
            } else {
                color::TRANSPARENT
            },
            width: 1.0,
            radius: 16.0.into(),
        },
        ..Default::default()
    }
}

/// The "YOU SEND (Bitcoin) → THEY RECEIVE (target)" pair. YOU SEND is fixed —
/// the Spark wallet always spends bitcoin — so only THEY RECEIVE is tappable.
fn spark_send_cards<'a>(
    target: SparkSendTarget,
    balance_sats: u64,
    bitcoin_unit: BitcoinDisplayUnit,
) -> Element<'a, Message> {
    let you_send = Container::new(
        Column::new()
            .spacing(6)
            .push(text("YOU SEND").size(P2_SIZE).style(theme::text::secondary))
            .push(
                Row::new()
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .push(asset_network_logo("btc", "spark", 40.0))
                    .push(
                        text("Bitcoin")
                            .size(H3_SIZE)
                            .bold()
                            .style(theme::text::primary),
                    ),
            )
            .push(
                text(format!(
                    "{} {}",
                    Amount::from_sat(balance_sats).to_formatted_string_with_unit(bitcoin_unit),
                    if matches!(bitcoin_unit, BitcoinDisplayUnit::BTC) {
                        "BTC"
                    } else {
                        "SATS"
                    }
                ))
                .size(P2_SIZE)
                .style(theme::text::secondary),
            )
            .push(orange_badge("SPARK")),
    )
    .padding(16)
    .width(Length::Fill)
    .height(Length::Fixed(160.0))
    .style(theme::card::simple);

    let they_receive = iced_button(
        Container::new(
            Column::new()
                .spacing(6)
                .push(
                    text("THEY RECEIVE")
                        .size(P2_SIZE)
                        .style(theme::text::secondary),
                )
                .push(
                    Row::new()
                        .spacing(8)
                        .align_y(Alignment::Center)
                        .push(target_logo(target, 40.0))
                        .push(
                            text(target.label())
                                .size(H3_SIZE)
                                .bold()
                                .style(theme::text::primary),
                        ),
                )
                .push(Space::new().height(Length::Fixed(18.0)))
                .push(orange_badge(target.badge())),
        )
        .padding(16)
        .width(Length::Fill)
        .height(Length::Fixed(160.0))
        .style(theme::card::simple),
    )
    .padding(0)
    .on_press(Message::SparkSend(SparkSendMessage::OpenReceivePicker))
    .style(card_button_style);

    Row::new()
        .spacing(12)
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .push(Container::new(you_send).width(Length::FillPortion(1)))
        .push(text("→").size(H3_SIZE).style(theme::text::secondary))
        .push(Container::new(they_receive).width(Length::FillPortion(1)))
        .into()
}

/// The "THEY RECEIVE" picker modal: bitcoin rails, then (mainnet only) the
/// cross-chain stablecoins. Overlaid by the state's `view()`.
pub fn send_target_picker_modal<'a>(
    current: SparkSendTarget,
    network: Network,
) -> Element<'a, Message> {
    let row = |target: SparkSendTarget| {
        picker_row(
            target_logo(target, 36.0),
            target.label(),
            "",
            target.badge(),
            current == target,
            Message::SparkSend(SparkSendMessage::SetReceiveTarget(target)),
        )
    };

    let mut list = Column::new()
        .spacing(8)
        .push(text("Bitcoin").size(P2_SIZE).style(theme::text::secondary));
    for target in [
        SparkSendTarget::Lightning,
        SparkSendTarget::OnChain,
        SparkSendTarget::Spark,
    ] {
        list = list.push(row(target));
    }

    // Cross-chain stablecoins are mainnet-only (providers/chains have no test
    // deployment), so they only appear there.
    if supported_on(network) {
        list = list.push(Space::new().height(Length::Fixed(6.0))).push(
            text("Stablecoin (cross-chain)")
                .size(P2_SIZE)
                .style(theme::text::secondary),
        );
        for target in [SparkSendTarget::Usdt, SparkSendTarget::Usdc] {
            list = list.push(row(target));
        }
    }

    Column::new()
        .spacing(12)
        .padding(24)
        .max_width(460)
        .push(text("THEY RECEIVE").size(16).bold())
        .push(list)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversion_fee_shows_a_sats_estimate_when_a_price_is_known() {
        // 0.097921 USDT at $65,000/BTC ≈ 150 sats.
        let s = conversion_fee_display(97_921, 6, "USDT", Some(65_000.0), BitcoinDisplayUnit::Sats);
        assert_eq!(s, "0.097921 USDT (≈ 150 SATS)");
    }

    #[test]
    fn conversion_fee_is_asset_only_without_a_price() {
        let s = conversion_fee_display(97_921, 6, "USDT", None, BitcoinDisplayUnit::Sats);
        assert_eq!(s, "0.097921 USDT");
    }

    #[test]
    fn conversion_fee_shows_the_hint_for_a_sub_one_sat_fee_when_priced() {
        // A tiny fee rounds to 0 sats, but with a price known the hint should
        // still show — the old `sats == 0` gate wrongly suppressed it.
        let s = conversion_fee_display(1, 6, "USDT", Some(65_000.0), BitcoinDisplayUnit::Sats);
        assert_eq!(s, "0.000001 USDT (≈ 0 SATS)");
    }

    #[test]
    fn conversion_fee_skips_the_hint_when_the_fee_overflows_u64() {
        // A fee that doesn't fit u64 must not be clamped-and-understated — drop
        // the sats hint rather than show a wrong figure.
        let s = conversion_fee_display(
            u128::MAX,
            6,
            "USDT",
            Some(65_000.0),
            BitcoinDisplayUnit::Sats,
        );
        assert!(
            !s.contains('≈'),
            "an overflowing fee must not show a clamped estimate: {}",
            s
        );
    }
}
