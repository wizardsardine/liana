//! View renderer for [`crate::app::state::spark::send::SparkSend`].
//!
//! Phase 4c ships the minimum viable Send UI: destination input, amount
//! input, Prepare / Confirm / Try again buttons, and tiny status cards
//! for each phase of the state machine. Intentionally plain —
//! polish (fee tier picker, fiat estimate, address book, QR scanner)
//! lands in a later phase once the bridge write path has soaked a bit.

use coincube_ui::locale::copy as localized_copy;
use coincube_ui::locale::Text as CopyText;
use coincube_ui::localized_format;
use coincube_ui::{
    color,
    component::{
        amount::{BitcoinDisplayUnit, DisplayAmount},
        button,
        tenshu::{ActionButton, ActionKind},
        text::*,
    },
    icon,
    image::{asset_logo, asset_network_logo},
    theme,
    widget::{action::Action, Column, ColumnExt, Container, Element, Row, RowExt},
};
use iced::{
    widget::{container, text_input, Space},
    Alignment, Background, Length,
};

use coincube_core::miniscript::bitcoin::{Amount, Network};

use crate::app::breez_spark::assets::stable_token_as_sats;
use crate::app::state::spark::cross_chain::{self, supported_on};
use crate::app::state::spark::send::{CrossChainContext, SparkSendPhase, SparkSendTarget};
use crate::app::view::shared::payment_columns::{payment_columns, rail_card};
use crate::app::view::spark::{last_tx::recent_rail_section, SparkRecentTransaction};
use crate::app::view::{Message, SparkSendMessage};

pub struct SparkSendView<'a> {
    pub backend_available: bool,
    pub cube_name: &'a str,
    pub destination_input: &'a str,
    pub amount_input: &'a str,
    /// Whether `amount_input` was filled from the destination invoice rather
    /// than typed. The field then shows the amount but doesn't accept edits —
    /// the invoice fixes what gets paid, so an editable field would promise a
    /// choice the payer doesn't have.
    pub amount_set_by_invoice: bool,
    pub phase: &'a SparkSendPhase,
    pub recipient_identities: &'a [crate::services::branta::RecipientIdentity],
    pub recipient_mismatch: bool,
    pub recipient_checks_enabled: bool,
    /// Whether this exact destination has a Branta lookup path during Prepare.
    pub recipient_lookup_eligible: bool,
    pub theme_mode: theme::palette::ThemeMode,
    pub sent_amount_display: &'a str,
    pub sent_celebration_context: &'a str,
    /// Who the sent payment went to, as typed (receipt's To row).
    pub sent_recipient: &'a str,
    /// Fee and amount plus fee, in the unit the payment was made in
    /// (receipt's Fee and Total rows).
    pub sent_fee_display: &'a str,
    pub sent_total_display: &'a str,
    pub sent_quote: &'a coincube_ui::component::quote_display::Quote,
    pub sent_image_handle: &'a iced::widget::image::Handle,
    pub recent_transactions: &'a [SparkRecentTransaction],
    /// Unified balance (sats: BTC + Stable Balance), shown on the YOU SEND card.
    pub balance_sats: u64,
    /// The side panel's balances, each in its own unit.
    pub balances: crate::app::view::spark::SparkBalances<'a>,
    /// The balance this send pays from (S4).
    pub pay_from: coincube_spark_protocol::WalletAsset,
    /// Why US Dollar can't pay this send, or `None` when it can.
    pub usdb_unavailable: Option<&'static str>,
    /// The route menu open under its tile, if any.
    pub open_menu: Option<SendMenu>,
    /// What the prepared preview pays from, and the conversion funding it.
    pub paid_from: coincube_spark_protocol::WalletAsset,
    pub conversion: Option<&'a coincube_spark_protocol::ConversionQuote>,
    /// The USDB a prepared USDB invoice payment moves. Review's amounts are
    /// then in USDB: its preview's sats figures are zero.
    pub token_payment: Option<&'a coincube_spark_protocol::TokenPayment>,
    /// Line under the Amount field when the entered amount exceeds the
    /// bitcoin balance and USDB will cover the rest — see
    /// `state::spark::send::usdb_conversion_hint`.
    pub usdb_conversion_hint: Option<String>,
    pub bitcoin_unit: BitcoinDisplayUnit,
    /// BTC/USD reference price for the cross-chain conversion-fee sats estimate.
    /// `None` when no price is known — the fee then shows in the asset only.
    pub reference_btc_usd_price: Option<f64>,
    pub show_direction_badges: bool,
    pub mask_amounts: bool,
    /// The "THEY RECEIVE" selection — drives the two-card selector and the
    /// destination placeholder.
    pub receive_target: SparkSendTarget,
    /// Bitcoin network the cube runs on — gates the stablecoin picker options
    /// (cross-chain is mainnet-only).
    pub network: Network,
    pub chain: crate::chain::ChainId,
    /// The cross-chain destination + routes for the current send, when there is
    /// one. Lives on the panel rather than in the phase because it must survive
    /// a failed send — a retry re-prepares from it.
    pub cross_chain_ctx: Option<&'a CrossChainContext>,
    /// What the typed stablecoin destination is, once looked up: its network
    /// (named on the THEY RECEIVE tile and under Destination), and the one
    /// chosen when the address works on several.
    pub detected: Option<&'a cross_chain::DetectedDestination>,
    /// Why the typed amount can't go on the chosen network, if it can't.
    pub amount_issue: Option<cross_chain::AmountIssue>,
    /// Review waits for "they use this network" (a network the user chose).
    pub awaiting_network_ack: bool,
    pub network_acknowledged: bool,
    /// Cross-chain slippage tolerance, in basis points, as typed. Empty means
    /// the SDK default. Only rendered behind the advanced disclosure.
    pub slippage_input: &'a str,
    /// Life left on the live cross-chain quote. `None` means there is no quote
    /// on screen — *not* "expired", which is its own variant. Anything other
    /// than `Valid` replaces Confirm with a re-quote.
    pub quote_countdown: Option<cross_chain::QuoteCountdown>,
}

/// Under the amount when US Dollar pays (H-Send, pay from USDB).
pub const PAID_FROM_USDB_NOTE: &str = "Paid from USDB: the amount converts to bitcoin at send. \
     Review shows the rate and conversion fee before anything moves.";

/// Under the Amount field for US Dollar over Spark: nothing converts.
pub const US_DOLLAR_SEND_NOTE: &str = "Sent from your US Dollar balance and received as US \
     Dollar, with no conversion.";

/// The pay-from picker's explanation of paying from bitcoin (S2's
/// "Always on" shortfall conversion).
pub const PAY_FROM_BITCOIN_NOTE: &str = "Bitcoin is used first. If it can't cover a payment, \
     just enough USDB converts to cover the rest, and Review shows it.";

impl<'a> SparkSendView<'a> {
    fn pays_from_usdb(&self) -> bool {
        self.pay_from == coincube_spark_protocol::WalletAsset::Usdb
    }

    /// "Send at least 1,500 sats on Solana", beside the Amount label, once
    /// the network is settled and its route publishes a floor.
    fn route_minimum_line(&self) -> Option<String> {
        let detected = self.detected?;
        let network = detected.chosen.as_deref()?;
        let min = cross_chain::effective_min_sats(detected.route()?, self.reference_btc_usd_price)?;
        Some(localized_format!(
            "Send at least {} sats on {}",
            "Envía al menos {} sats en {}",
            coincube_ui::component::amount::format_u64_as_string(min, ","),
            network
        ))
    }

    /// The amount's problem on the chosen network, with "Use the minimum"
    /// when it is too small.
    fn amount_issue_note(&self) -> Option<Element<'a, Message>> {
        let issue = self.amount_issue?;
        let network = self
            .detected
            .and_then(|d| d.chosen.clone())
            .unwrap_or_default();
        Some(
            Row::new()
                .spacing(12)
                .align_y(Alignment::Center)
                .push(
                    p2_regular(issue.message(&network))
                        .style(theme::text::warning)
                        .width(Length::Fill),
                )
                .push_maybe(
                    matches!(issue, cross_chain::AmountIssue::BelowMinimum { .. }).then(|| {
                        iced::widget::button(
                            text(localized_copy("Use the minimum"))
                                .size(P2_SIZE)
                                .style(coincube_ui::theme::tenshu::text::accent),
                        )
                        .padding([2, 0])
                        .style(theme::button::transparent)
                        .on_press(Message::SparkSend(SparkSendMessage::UseMinimum))
                    }),
                )
                .into(),
        )
    }

    /// Why Prepare can't go yet on a stablecoin send, said in its footer.
    /// `None` when it can.
    fn stablecoin_blocker(&self) -> Option<&'static str> {
        if !self.receive_target.is_stablecoin() {
            return None;
        }
        let detected = self.detected?;
        if detected.network.is_problem() {
            Some("Paste an address on a supported network to continue.")
        } else if detected.chosen.is_none() {
            Some("Choose the network to continue.")
        } else if self.amount_issue.is_some() {
            Some("Change the amount to continue.")
        } else {
            None
        }
    }

    pub fn render(self) -> Element<'a, Message> {
        if !self.backend_available {
            return Column::new()
                .spacing(20)
                .push(p1_regular(localized_copy(
                    "Spark is not available for this cube. Set up a Spark \
                     signer to send payments.",
                )))
                .into();
        }

        // ── Payment result: the receipt first, the worldview art beside it ──
        if let SparkSendPhase::Sent(ok) = self.phase {
            use coincube_ui::component::payment_result::{
                payment_result, short_id, Outcome, ReceiptRow,
            };
            // An on-chain exit is broadcast, not settled: Activity carries
            // the confirmation.
            let outcome = if self.sent_celebration_context == "bitcoin-send" {
                Outcome::Confirming
            } else {
                Outcome::Sent
            };
            let recipient = self.sent_recipient;
            // A Lightning address reads in full; an invoice or address is cut
            // to its ends, with Copy for the whole value.
            let to = if recipient.contains('@') {
                ReceiptRow::new(localized_copy("To"), recipient)
            } else {
                ReceiptRow::new(localized_copy("To"), short_id(recipient))
                    .mono()
                    .on_copy(Message::Clipboard(recipient.to_string()))
            };
            let mut rows = Vec::new();
            if !recipient.is_empty() {
                rows.push(to);
            }
            rows.push(ReceiptRow::new(localized_copy("Paid from"), "Spark"));
            if !self.sent_fee_display.is_empty() {
                rows.push(ReceiptRow::new(
                    localized_copy("Fee"),
                    self.sent_fee_display,
                ));
                rows.push(ReceiptRow::new(
                    localized_copy("Total"),
                    self.sent_total_display,
                ));
            }
            if !ok.payment_id.is_empty() {
                rows.push(
                    ReceiptRow::new(localized_copy("Payment ID"), short_id(&ok.payment_id))
                        .mono()
                        .on_copy(Message::Clipboard(ok.payment_id.clone())),
                );
            }
            return payment_result(
                outcome,
                self.sent_amount_display,
                self.sent_celebration_context,
                self.sent_quote,
                self.sent_image_handle,
                Message::SparkSend(crate::app::view::SparkSendMessage::Reset),
            )
            .rows(rows)
            .voice(coincube_ui::worldview_copy::VoiceLine::PaymentSent.get())
            .on_activity(Message::SparkSend(
                crate::app::view::SparkSendMessage::ViewActivity,
            ))
            .into();
        }

        // ── Payment details card ──────────────────────────────────────
        // Route tiles, Destination, Amount and — while composing — the
        // Prepare footer, on one surface (P-Send). Each slot below stays in
        // place whatever the phase, so the inputs keep their widget state
        // when a note, a hint or the loading line comes and goes.
        let destination = text_input(
            localized_copy(self.receive_target.destination_placeholder()),
            self.destination_input,
        )
        .id(DESTINATION_INPUT_ID)
        .on_input(|v| {
            Message::SparkSend(crate::app::view::SparkSendMessage::DestinationInputChanged(
                v,
            ))
        })
        .padding(12);

        // The amount field follows the wallet's display unit, like the Vault
        // send form: label, placeholder, and the parse in state all switch on
        // it, so a BTC-configured wallet enters BTC and a sats one enters sats.
        // A US Dollar send is typed in US Dollars whatever the unit.
        let usd_send = self.receive_target == SparkSendTarget::SparkUsd;
        let is_btc = matches!(self.bitcoin_unit, BitcoinDisplayUnit::BTC);
        let amount_label = if usd_send {
            localized_copy("Amount (US Dollar)")
        } else if is_btc {
            localized_copy("Amount (BTC)")
        } else {
            localized_copy("Amount (sats)")
        };
        let amount_placeholder = match (self.receive_target.is_stablecoin(), is_btc) {
            _ if usd_send => "Amount in US Dollars (optional for invoices with amount)",
            (true, true) => "Amount in BTC — funded from your Bitcoin balance",
            (true, false) => "Amount in sats — funded from your Bitcoin balance",
            (false, true) => "Amount in BTC (optional for invoices with amount)",
            (false, false) => "Amount in sats (optional for invoices with amount)",
        };
        // An invoice-set amount leaves the field without `on_input`, which is
        // what makes it read-only — it still shows and can be selected, it just
        // can't be edited into something the invoice won't honour.
        let mut amount = text_input(amount_placeholder, self.amount_input)
            .id(AMOUNT_INPUT_ID)
            .padding(12);
        if !self.amount_set_by_invoice {
            amount = amount.on_input(|v| {
                Message::SparkSend(crate::app::view::SparkSendMessage::AmountInputChanged(v))
            });
        }

        // Both notes share one slot below the field, never above it.
        let amount_notes = Column::new()
            .spacing(6)
            .push_maybe(self.amount_set_by_invoice.then(|| {
                p2_regular(localized_copy("This invoice sets its own amount."))
                    .style(theme::text::secondary)
            }))
            .push_maybe(
                self.usdb_conversion_hint
                    .clone()
                    .filter(|_| !self.mask_amounts && !self.pays_from_usdb())
                    .map(|hint| p2_regular(hint).style(theme::text::secondary)),
            )
            .push_maybe(
                (self.pays_from_usdb() && !usd_send)
                    .then(|| p2_regular(PAID_FROM_USDB_NOTE).style(theme::text::secondary)),
            )
            .push_maybe(
                usd_send.then(|| p2_regular(US_DOLLAR_SEND_NOTE).style(theme::text::secondary)),
            )
            .push_maybe(self.amount_issue_note());

        // Unified balance (BTC + Stable Balance), as the YOU SEND card showed
        // it; a US Dollar send can spend only the US Dollar balance.
        let us_dollar_available = || {
            let (rows, _) = self.balances.rows();
            rows.into_iter()
                .find(|(name, _, _)| *name == "US Dollar")
                .map_or_else(|| "0".to_owned(), |(_, figure, _)| figure)
        };
        let available = Row::new()
            .spacing(6)
            .push(Space::new().width(Length::Fill))
            .push(p2_regular(localized_copy("Available")).style(theme::text::secondary))
            .push(
                text(if self.mask_amounts {
                    "••••••".to_owned()
                } else if usd_send {
                    format!("{} {}", us_dollar_available(), localized_copy("US Dollar"))
                } else {
                    format!(
                        "{} {}",
                        Amount::from_sat(self.balance_sats)
                            .to_formatted_string_with_unit(self.bitcoin_unit),
                        if matches!(self.bitcoin_unit, BitcoinDisplayUnit::BTC) {
                            "BTC"
                        } else {
                            "SATS"
                        }
                    )
                })
                .size(P2_SIZE)
                .style(theme::text::secondary),
            );

        // Idle and Preparing render as this card's footer. Every other phase
        // — and the recipient mismatch, whatever the phase — keeps its own
        // card below, exactly as before.
        let footer_in_card = !self.recipient_mismatch
            && matches!(self.phase, SparkSendPhase::Idle | SparkSendPhase::Preparing);
        let body = phase_body(&self);
        // A prepared quote replaces Payment details on the same surface
        // (P-Send Review): nothing under the quote can be edited, and Back
        // returns to the form with what was typed.
        let (footer, below) = if footer_in_card || is_review(self.phase) {
            (Some(body), None)
        } else {
            (None, Some(body))
        };

        let card = if is_review(self.phase) {
            Container::new(footer.unwrap_or_else(|| Space::new().into()))
        } else {
            Container::new(
                Column::new()
                    .push(
                        Container::new(
                            Row::new()
                                .spacing(12)
                                .align_y(Alignment::Center)
                                // Bitcoin rail, US Dollar or stablecoin is all
                                // picked from the THEY RECEIVE tile.
                                .push(
                                    h4_bold(localized_copy("Payment details")).width(Length::Fill),
                                ),
                        )
                        .padding([16, 24])
                        .width(Length::Fill),
                    )
                    .push(iced::widget::rule::horizontal(1))
                    .push(card_section(spark_send_cards(
                        self.receive_target,
                        self.detected,
                        self.pay_from,
                        SendMenus {
                            open: self.open_menu,
                            usdb_unavailable: self.usdb_unavailable,
                            balances: self.balances,
                            network: self.network,
                        },
                    )))
                    .push(iced::widget::rule::horizontal(1))
                    .push(card_section(
                        Column::new()
                            .spacing(10)
                            .push(p1_bold(localized_copy("Destination")))
                            .push(destination)
                            .push(Column::new().push_maybe(self.detected.map(|detected| {
                                detected_destination(detected, self.receive_target.label())
                            }))),
                    ))
                    .push(iced::widget::rule::horizontal(1))
                    .push(card_section(
                        Column::new()
                            .spacing(10)
                            .push(
                                Row::new()
                                    .spacing(12)
                                    .align_y(Alignment::Center)
                                    .push(p1_bold(amount_label).width(Length::Fill))
                                    .push_maybe(self.route_minimum_line().map(|line| {
                                        p2_regular(line).style(theme::text::secondary)
                                    })),
                            )
                            .push(amount)
                            .push(amount_notes)
                            .push(available),
                    ))
                    .push(Column::new().push_maybe(footer.map(|footer| {
                        Column::new()
                            .push(iced::widget::rule::horizontal(1))
                            .push(card_section(footer))
                    }))),
            )
        }
        .width(Length::Fill)
        .style(payment_card_style);

        let content = Column::new()
            .push(card)
            // ── Phase-specific body (every phase but Idle/Preparing) ──
            .push(Column::new().push_maybe(below.map(|body| {
                Column::new()
                    .push(Space::new().height(Length::Fixed(20.0)))
                    .push(body)
            })));

        // The steps are on the head's rule. The rail lists what the payment
        // can come from, what it leaves on Review, then Spark's latest
        // payments (they sat under the form before, which left the right of
        // the page empty).
        let recent = recent_rail_section(
            self.recent_transactions,
            self.bitcoin_unit,
            self.mask_amounts,
            |idx| Message::SparkSend(SparkSendMessage::SelectTransaction(idx)),
            Message::SparkSend(SparkSendMessage::History),
        );
        let rail = Column::new().spacing(20).push(self.balances.view());
        let rail = match self.phase {
            // A mismatch blocks the payment: no balances after a send that
            // must not happen.
            SparkSendPhase::Prepared(ok) if is_review(self.phase) && !self.recipient_mismatch => {
                rail.push(iced::widget::rule::horizontal(1))
                    .push(after_payment(&self, ok))
            }
            _ => rail,
        }
        .push(iced::widget::rule::horizontal(1))
        .push(recent);
        // The title, the Cube line, Back to Spark and the steps are in the
        // page head (`head`).
        payment_columns(content, rail_card(rail))
    }
}

/// Where `phase` stands on [`SEND_STEPS`](crate::app::view::spark::SEND_STEPS):
/// Review from a quote until the payment is known to have landed, Sent once
/// it has, and Details otherwise.
pub fn send_step(phase: &SparkSendPhase) -> usize {
    match phase {
        SparkSendPhase::Prepared(_)
        | SparkSendPhase::Sending
        | SparkSendPhase::CrossChainFailed { .. }
        | SparkSendPhase::OutcomeUnknown { .. } => 1,
        SparkSendPhase::Sent(_) => 2,
        SparkSendPhase::Idle | SparkSendPhase::Preparing | SparkSendPhase::Error(_) => 0,
    }
}

fn phase_body<'a>(view: &SparkSendView<'a>) -> Element<'a, Message> {
    let phase = view.phase;
    let recipient_mismatch = view.recipient_mismatch;
    use crate::app::view::SparkSendMessage;

    // Review shows the mismatch in its own card, in place of the quote.
    if recipient_mismatch && !is_review(phase) {
        return Container::new(
            Column::new()
                .spacing(12)
                .push(p1_regular(crate::services::branta::MISMATCH_MESSAGE))
                .push(
                    button::primary(None, localized_copy("Replace payment request"))
                        .on_press(Message::SparkSend(SparkSendMessage::Reset)),
                ),
        )
        .padding(16)
        .style(theme::card::simple)
        .into();
    }

    match phase {
        // Idle and Preparing are the Payment details card's footer (see
        // `render`). Prepare stays available whatever is typed: the state
        // parses the input and reports what is wrong, so the view does not
        // second-guess it with a validator of its own.
        SparkSendPhase::Idle => match view.stablecoin_blocker() {
            Some(blocker) => compose_footer(blocker, None),
            None => compose_footer(
                "Prepare gets a fee quote. Nothing is sent yet.",
                Some(Message::SparkSend(SparkSendMessage::PrepareRequested)),
            ),
        },

        // Same slot, same Prepare — without a press, so a held key or a
        // pointer release cannot reach it while the quote is being fetched.
        SparkSendPhase::Preparing => compose_footer(
            "Preparing send… asking the Spark bridge for a fee quote.",
            None,
        ),

        SparkSendPhase::Prepared(ok) => review_card(view, ok),

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
                    button::primary(None, localized_copy("Try again"))
                        .on_press(Message::SparkSend(
                            SparkSendMessage::CrossChainRetryRequested,
                        ))
                        .width(Length::Fixed(160.0)),
                );
            } else {
                actions = actions.push(
                    button::primary(None, localized_copy("Check status"))
                        .on_press(Message::SparkSend(SparkSendMessage::History))
                        .width(Length::Fixed(160.0)),
                );
            }
            actions = actions.push(
                button::transparent_border(None, localized_copy("Start over"))
                    .on_press(Message::SparkSend(SparkSendMessage::Reset))
                    .width(Length::Fixed(140.0)),
            );

            Container::new(
                Column::new()
                    .spacing(12)
                    .push(h4_bold(localized_copy("Send failed")))
                    .push(p1_regular(coincube_ui::locale::notice(message.clone())))
                    .push(p2_regular(localized_copy(policy.guidance())))
                    .push(Space::new().height(Length::Fixed(8.0)))
                    .push(actions),
            )
            .padding(16)
            .style(theme::card::simple)
            .into()
        }

        SparkSendPhase::Sending => Container::new(Column::new().spacing(10).push(p1_regular(
            localized_copy("Sending… waiting for the Spark SDK to settle the payment."),
        )))
        .padding(16)
        .style(theme::card::simple)
        .into(),

        // A dispatched payment with no answer. The heading is the whole point:
        // this must never read as "failed", because the payment may well have
        // gone through — and until the check finishes the only thing offered is
        // waiting, plus a read-only look at the transaction list. Nothing that
        // could send it a second time.
        SparkSendPhase::OutcomeUnknown {
            message,
            outcome,
            checking,
            guard,
        } => {
            let guard = *guard;
            let mut content = Column::new()
                .spacing(12)
                .push(h4_bold(localized_copy("Payment status unconfirmed")))
                .push(p1_regular(coincube_ui::locale::notice(message.clone())));

            let mut actions = Row::new().spacing(10);
            match (checking, outcome) {
                (true, _) | (_, None) => {
                    content = content.push(p2_regular(localized_copy(
                        "Checking your payment history to find out whether it went through. \
                         Don't send it again until this finishes.",
                    )));
                }
                (false, Some(outcome)) => {
                    content = content.push(p2_regular(coincube_ui::locale::notice(
                        outcome.guidance(guard),
                    )));
                    if outcome.may_resend(guard) {
                        actions = actions.push(
                            // Keeps the original idempotency key — see
                            // `ResendAfterUnknownRequested`. Never `Reset`,
                            // which would mint a new one and could pay twice.
                            button::primary(None, localized_copy("Send again"))
                                .on_press(Message::SparkSend(
                                    SparkSendMessage::ResendAfterUnknownRequested,
                                ))
                                .width(Length::Fixed(160.0)),
                        );
                    }
                    actions = actions.push(
                        button::secondary(None, CopyText::CheckAgain.get())
                            .on_press(Message::SparkSend(SparkSendMessage::ReconcileRequested))
                            .width(Length::Fixed(140.0)),
                    );
                }
            }

            // Offered in both arms, and last in the row either way. It is
            // read-only — a redirect to the transaction list, no resend and no
            // new idempotency key — so it does not undercut "wait while the
            // check runs". Building it inside the resolved arm alone left the
            // checking arm with an empty action row: the one screen where the
            // user most wants to go look for their payment offered nothing to
            // press.
            actions = actions.push(
                button::transparent_border(None, localized_copy("View transactions"))
                    .on_press(Message::SparkSend(SparkSendMessage::History))
                    .width(Length::Fixed(180.0)),
            );

            Container::new(
                content
                    .push(Space::new().height(Length::Fixed(8.0)))
                    .push(actions),
            )
            .padding(16)
            .style(theme::card::simple)
            .into()
        }

        SparkSendPhase::Sent(_) => {
            // Handled by the full-screen celebration in render()
            Container::new(Column::new()).into()
        }

        SparkSendPhase::Error(err) => Container::new(
            Column::new()
                .spacing(10)
                .push(h4_bold(localized_copy("Error")))
                .push(p1_regular(coincube_ui::locale::notice(err.clone())))
                .push(Space::new().height(Length::Fixed(8.0)))
                .push(
                    button::primary(None, localized_copy("Try again"))
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

// ── Review (P-Send prepared) ─────────────────────────────────────────────────

const REVIEW_BACK_ID: &str = "spark-send-review-back";
const REVIEW_CONFIRM_ID: &str = "spark-send-review-confirm";
const REVIEW_REPLACE_ID: &str = "spark-send-review-replace";
const RECIPIENT_IDENTITY_ID: &str = "spark-send-recipient-identity";

/// Whether `phase` is Review: any prepared quote, bitcoin or cross-chain.
fn is_review(phase: &SparkSendPhase) -> bool {
    matches!(phase, SparkSendPhase::Prepared(_))
}

/// What a prepare method is, in words. Unknown tags show as they come.
fn method_label(method: &str) -> &str {
    match method {
        "Bolt11Invoice" => "Lightning invoice",
        "LnurlPay" => "Lightning address",
        "SparkAddress" => "Spark address",
        "SparkInvoice" => "Spark invoice",
        "BitcoinAddress" => "Bitcoin address · on-chain",
        other => other,
    }
}

/// An address or invoice too long for the row, kept recognisable at both
/// ends: "lnbc15u1pn…x7q2g9".
fn shorten_middle(value: &str, head: usize, tail: usize) -> String {
    let chars: Vec<char> = value.trim().chars().collect();
    if chars.len() <= head + tail + 1 {
        return chars.into_iter().collect();
    }
    let start: String = chars[..head].iter().collect();
    let end: String = chars[chars.len() - tail..].iter().collect();
    format!("{start}…{end}")
}

/// A bitcoin figure and its unit in the wallet's display unit: ("1,000", "SATS").
fn btc_figure(sats: u64, unit: BitcoinDisplayUnit) -> (String, &'static str) {
    (
        Amount::from_sat(sats).to_formatted_string_with_unit(unit),
        if matches!(unit, BitcoinDisplayUnit::BTC) {
            "BTC"
        } else {
            "SATS"
        },
    )
}

/// A protocol sats amount as the `u64` display takes, clamped.
fn clamp_sats(sats: u128) -> u64 {
    sats.min(u128::from(u64::MAX)) as u64
}

fn btc_line(sats: u64, unit: BitcoinDisplayUnit) -> String {
    let (figure, unit) = btc_figure(sats, unit);
    format!("{figure} {unit}")
}

fn usdb_figure(amount: u128, usdb_decimals: u32) -> String {
    crate::app::breez_spark::assets::format_token_display_u128(amount, usdb_decimals)
}

/// "≈ $0.74" for `sats` at the BTC/USD price, when there is one.
fn usd_estimate(sats: u64, price: Option<f64>) -> Option<String> {
    price.filter(|price| *price > 0.0).map(|price| {
        format!(
            "≈ ${}",
            crate::app::view::spark::swap::dollars(sats as f64 / 100_000_000.0 * price)
        )
    })
}

/// One side of the route on Review: the asset, the amount and its rail.
struct ReviewTile<'a> {
    caption: &'static str,
    logo: Element<'a, Message>,
    asset: &'static str,
    figure: String,
    unit: &'static str,
    note: Option<String>,
    rail: String,
}

fn review_tile(tile: ReviewTile<'_>) -> Element<'_, Message> {
    use coincube_ui::component::tenshu::{caption, heading};
    Container::new(
        Column::new()
            .spacing(8)
            .push(
                text(tile.caption)
                    .size(P2_SIZE)
                    .style(theme::text::secondary),
            )
            .push(
                Row::new()
                    .spacing(10)
                    .align_y(Alignment::Center)
                    .push(tile.logo)
                    .push(
                        text(tile.asset)
                            .size(H5_SIZE)
                            .bold()
                            .style(theme::text::primary),
                    ),
            )
            .push(
                Column::new()
                    .spacing(2)
                    .push(
                        Row::new()
                            .spacing(4)
                            .align_y(Alignment::End)
                            .push(heading(tile.figure))
                            .push(caption(tile.unit).size(13)),
                    )
                    // Always a line, so both tiles keep one height.
                    .push(caption(tile.note.unwrap_or_default()).size(13)),
            )
            .push(orange_badge(&tile.rail)),
    )
    .padding([14, 16])
    .width(Length::Fill)
    .style(fixed_tile_style)
    .into()
}

/// You send → They receive, with the amounts the quote settled on.
fn review_route<'a>(
    view: &SparkSendView<'a>,
    ok: &'a coincube_spark_protocol::PrepareSendOk,
    usdb_decimals: u32,
) -> Element<'a, Message> {
    use coincube_spark_protocol::WalletAsset;
    let unit = view.bitcoin_unit;
    let price = view.reference_btc_usd_price;
    let usdb_logo = || -> Element<'a, Message> {
        asset_logo("usdb")
            .width(Length::Fixed(40.0))
            .height(Length::Fixed(40.0))
            .into()
    };
    let total_sats = ok.amount_sat.saturating_add(ok.fee_sat);

    let you_send = match (view.token_payment, view.paid_from, view.conversion) {
        (Some(payment), _, _) => ReviewTile {
            caption: "You send",
            logo: usdb_logo(),
            asset: "US Dollar",
            figure: usdb_figure(
                payment.amount.amount.saturating_add(payment.fee.amount),
                usdb_decimals,
            ),
            unit: "USDB",
            note: None,
            rail: "USDB".into(),
        },
        (None, WalletAsset::Usdb, Some(quote)) => ReviewTile {
            caption: "You send",
            logo: usdb_logo(),
            asset: "US Dollar",
            figure: usdb_figure(quote.amount_in.amount, usdb_decimals),
            unit: "USDB",
            note: Some(localized_format!(
                "Converts to {}",
                "Se convierte en {}",
                btc_line(clamp_sats(quote.amount_out.amount), unit)
            )),
            rail: "USDB".into(),
        },
        _ => {
            let (figure, unit) = btc_figure(total_sats, unit);
            ReviewTile {
                caption: "You send",
                logo: asset_network_logo("btc", "spark", 40.0),
                asset: "Bitcoin",
                figure,
                unit,
                note: usd_estimate(total_sats, price),
                rail: "SPARK".into(),
            }
        }
    };
    let they_receive = match view.token_payment {
        Some(payment) => ReviewTile {
            caption: "They receive",
            logo: usdb_logo(),
            asset: "US Dollar",
            figure: usdb_figure(payment.amount.amount, usdb_decimals),
            unit: "USDB",
            note: None,
            rail: view.receive_target.badge().into(),
        },
        None => {
            let (figure, unit) = btc_figure(ok.amount_sat, unit);
            ReviewTile {
                caption: "They receive",
                logo: target_logo(view.receive_target, 40.0),
                asset: view.receive_target.label(),
                figure,
                unit,
                note: usd_estimate(ok.amount_sat, price),
                rail: view.receive_target.badge().into(),
            }
        }
    };

    Row::new()
        .spacing(16)
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .push(Container::new(review_tile(you_send)).width(Length::FillPortion(1)))
        .push(text("→").size(H5_SIZE).style(theme::text::secondary))
        .push(Container::new(review_tile(they_receive)).width(Length::FillPortion(1)))
        .into()
}

/// A quote row: muted label left, the value right with an optional line
/// under it. `strong` is the total the eye should land on.
fn quote_row<'a>(
    label: &'a str,
    value: Element<'a, Message>,
    note: Option<String>,
    strong: bool,
) -> Element<'a, Message> {
    use coincube_ui::component::tenshu::{body, caption};
    let label = if strong {
        coincube_ui::component::tenshu::label(label)
    } else {
        body(label).style(coincube_ui::theme::tenshu::text::muted)
    };
    Row::new()
        .spacing(16)
        .align_y(Alignment::Start)
        .push(Container::new(label).width(Length::Fixed(160.0)))
        .push(
            Column::new()
                .spacing(2)
                .width(Length::Fill)
                .align_x(Alignment::End)
                .push(value)
                .push_maybe(note.map(|note| caption(note).align_x(Alignment::End))),
        )
        .padding([10, 0])
        .into()
}

/// Rows separated by hairlines, as the design's `.kv` list.
fn ruled<'a>(rows: Vec<Element<'a, Message>>) -> Element<'a, Message> {
    let mut column = Column::new();
    for (index, row) in rows.into_iter().enumerate() {
        if index > 0 {
            column = column.push(iced::widget::rule::horizontal(1));
        }
        column = column.push(row);
    }
    column.into()
}

/// Review's figures: where it goes, what it costs, what pays, the total.
fn review_quote<'a>(
    view: &SparkSendView<'a>,
    ok: &'a coincube_spark_protocol::PrepareSendOk,
    usdb_decimals: u32,
) -> Element<'a, Message> {
    use coincube_spark_protocol::WalletAsset;
    use coincube_ui::component::tenshu::{body, label, mono};
    let unit = view.bitcoin_unit;
    let price = view.reference_btc_usd_price;
    let total_sats = ok.amount_sat.saturating_add(ok.fee_sat);
    let value = |value: String| -> Element<'a, Message> { body(value).into() };

    let to_note = if ok.method.starts_with("Spark") {
        localized_copy("Spark transfer — instant, lower fee than Lightning or on-chain.").to_owned()
    } else {
        localized_copy(method_label(&ok.method)).to_owned()
    };
    let mut rows = vec![quote_row(
        localized_copy("To"),
        mono(shorten_middle(view.destination_input, 14, 8)).into(),
        Some(to_note),
        false,
    )];

    match view.token_payment {
        Some(payment) => {
            rows.push(quote_row(
                localized_copy("Amount"),
                value(usdb_amount_line(payment.amount.amount, usdb_decimals)),
                None,
                false,
            ));
            rows.push(quote_row(
                localized_copy("Fee"),
                value(usdb_amount_line(payment.fee.amount, usdb_decimals)),
                None,
                false,
            ));
        }
        None => {
            rows.push(quote_row(
                localized_copy("Amount"),
                value(btc_line(ok.amount_sat, unit)),
                usd_estimate(ok.amount_sat, price),
                false,
            ));
            rows.push(quote_row(
                localized_copy("Fee"),
                value(btc_line(ok.fee_sat, unit)),
                None,
                false,
            ));
        }
    }

    let (paid_from, paid_note) = match (view.paid_from, view.conversion) {
        (WalletAsset::Usdb, Some(_)) => (
            localized_copy("US Dollar (USDB)"),
            Some(localized_copy("Converts to bitcoin at send").to_owned()),
        ),
        (WalletAsset::Usdb, None) => (localized_copy("US Dollar (USDB)"), None),
        (WalletAsset::Bitcoin, Some(_)) => (
            localized_copy("Bitcoin, then US Dollar (USDB)"),
            Some(localized_copy("USDB converts to cover what bitcoin can't").to_owned()),
        ),
        (WalletAsset::Bitcoin, None) => ("Bitcoin", None),
    };
    rows.push(quote_row(
        localized_copy("Paid from"),
        value(paid_from.to_owned()),
        paid_note,
        false,
    ));

    if let Some(quote) = view.conversion {
        let mut terms = Vec::new();
        if let Some(rate) = crate::app::view::spark::swap::quote_rate(quote, usdb_decimals) {
            terms.push(format!(
                "1 BTC ≈ ${}",
                crate::app::view::spark::swap::dollars(rate)
            ));
        }
        terms.push(localized_format!(
            "fee {}",
            "comisión {}",
            crate::app::view::spark::swap::quote_fee_line(quote, usdb_decimals)
        ));
        terms.push(localized_format!(
            "max price change {}",
            "cambio máximo de precio {}",
            crate::app::view::spark::swap::price_change(quote.max_slippage_bps)
        ));
        rows.push(quote_row(
            localized_copy("Conversion"),
            value(conversion_line(quote, usdb_decimals, unit)),
            Some(terms.join(" · ")),
            false,
        ));
    }

    let (total, total_note) = match (view.token_payment, view.paid_from, view.conversion) {
        (Some(payment), _, _) => (
            usdb_amount_line(
                payment.amount.amount.saturating_add(payment.fee.amount),
                usdb_decimals,
            ),
            None,
        ),
        (None, WalletAsset::Usdb, Some(quote)) => (
            usdb_amount_line(quote.amount_in.amount, usdb_decimals),
            Some(localized_format!(
                "Converts to {} · {} pays this send",
                "Se convierte en {} · {} paga este envío",
                btc_line(clamp_sats(quote.amount_out.amount), unit),
                btc_line(total_sats, unit)
            )),
        ),
        (None, _, Some(quote)) => (
            btc_line(total_sats, unit),
            Some(localized_format!(
                "Includes {} converted from {}",
                "Incluye {} convertidos de {}",
                btc_line(clamp_sats(quote.amount_out.amount), unit),
                usdb_amount_line(quote.amount_in.amount, usdb_decimals)
            )),
        ),
        (None, _, None) => (btc_line(total_sats, unit), usd_estimate(total_sats, price)),
    };
    rows.push(quote_row(
        localized_copy("Total you send"),
        label(total).size(16).into(),
        total_note,
        true,
    ));

    Column::new()
        .push(ruled(rows))
        .push_maybe(view.conversion.map(|quote| {
            Container::new(
                Row::new()
                    .spacing(10)
                    .align_y(Alignment::Start)
                    .push(icon::warning_icon().size(14).style(|theme: &theme::Theme| {
                        iced::widget::text::Style {
                            color: Some(theme.colors.shell.warn),
                        }
                    }))
                    .push(
                        coincube_ui::component::tenshu::caption(localized_format!(
                            "If the price moves more than {} before the conversion settles, \
                             the conversion is refunded and nothing is sent.",
                            "Si el precio se mueve más de {} antes de que se complete la \
                             conversión, se reembolsa y no se envía nada.",
                            crate::app::view::spark::swap::price_change(quote.max_slippage_bps)
                        ))
                        .size(13)
                        .width(Length::Fill),
                    ),
            )
            .padding([10, 12])
            .width(Length::Fill)
            .style(notice_style)
        }))
        .spacing(16)
        .into()
}

/// The quiet inset for the price-move notice and the identity card.
fn notice_style(theme: &theme::Theme) -> container::Style {
    let shell = &theme.colors.shell;
    container::Style {
        background: Some(Background::Color(shell.background)),
        border: iced::Border {
            color: shell.line,
            width: 1.0,
            radius: 10.0.into(),
        },
        ..Default::default()
    }
}

/// A check's status mark: passed, or not known.
fn check_mark<'a>(passed: bool) -> Element<'a, Message> {
    let (glyph, tone) = if passed {
        (
            icon::check_circle_icon(),
            coincube_ui::theme::tenshu::Tone::Good,
        )
    } else {
        (
            icon::dash_circle_icon(),
            coincube_ui::theme::tenshu::Tone::Muted,
        )
    };
    glyph
        .size(16)
        .style(move |theme: &theme::Theme| coincube_ui::theme::tenshu::text::tone(theme, tone))
        .into()
}

fn check_row<'a>(
    passed: bool,
    label: &'a str,
    status: Element<'a, Message>,
) -> Element<'a, Message> {
    check_row_noted(passed, label, None, status)
}

/// A check row whose label carries a muted note beneath it.
fn check_row_noted<'a>(
    passed: bool,
    label: &'a str,
    note: Option<&'a str>,
    status: Element<'a, Message>,
) -> Element<'a, Message> {
    Row::new()
        .spacing(10)
        .align_y(Alignment::Center)
        .push(check_mark(passed))
        .push(
            Column::new()
                .spacing(2)
                .width(Length::Fill)
                .push(coincube_ui::component::tenshu::body(label))
                .push_maybe(note.map(coincube_ui::component::tenshu::caption)),
        )
        .push(status)
        .padding([10, 0])
        .into()
}

/// Lookups always run in Branta's strict privacy mode (`services::branta`
/// hard-codes it), so Review can say so without consulting a setting.
const STRICT_MODE_NOTE: &str = "Strict mode: Branta never gets the plaintext address or amount.";

fn check_status<'a>(value: String, passed: bool) -> Element<'a, Message> {
    let tone = if passed {
        coincube_ui::theme::tenshu::Tone::Good
    } else {
        coincube_ui::theme::tenshu::Tone::Muted
    };
    coincube_ui::component::tenshu::label(value)
        .style(move |theme: &theme::Theme| coincube_ui::theme::tenshu::text::tone(theme, tone))
        .into()
}

/// The Branta row. Identified, the recipient's card takes the status's
/// place; otherwise the status says why there is none.
fn recipient_status_row<'a>(
    enabled: bool,
    identities: &'a [crate::services::branta::RecipientIdentity],
    mode: theme::palette::ThemeMode,
) -> Element<'a, Message> {
    let label = localized_copy("Recipient identity (Branta)");
    if !enabled {
        return check_row(
            false,
            label,
            Row::new()
                .spacing(12)
                .align_y(Alignment::Center)
                .push(check_status(localized_copy("Checks off").to_owned(), false))
                .push(
                    iced::widget::button(
                        text(localized_copy("Turn on in Settings"))
                            .size(P2_SIZE)
                            .style(coincube_ui::theme::tenshu::text::accent),
                    )
                    .padding([2, 0])
                    .style(theme::button::transparent)
                    .on_press(Message::Menu(crate::app::menu::Menu::Cube(
                        crate::app::menu::CubeSubMenu::Settings(
                            crate::app::menu::CubeSettingsOption::PaymentChecks,
                        ),
                    ))),
                )
                .into(),
        );
    }
    match identities
        .iter()
        .enumerate()
        .find(|(_, identity)| identity.is_current())
    {
        Some((index, identity)) => check_row_noted(
            true,
            label,
            Some(localized_copy(STRICT_MODE_NOTE)),
            identity_card(index, identity, mode),
        ),
        None => check_row_noted(
            false,
            label,
            Some(localized_copy(STRICT_MODE_NOTE)),
            check_status(localized_copy("Not identified").to_owned(), false),
        ),
    }
}

/// The identified recipient, compact enough to sit in the check row.
fn identity_card<'a>(
    index: usize,
    identity: &'a crate::services::branta::RecipientIdentity,
    mode: theme::palette::ThemeMode,
) -> Element<'a, Message> {
    use coincube_ui::component::tenshu::{caption, label};
    let logo = match mode {
        theme::palette::ThemeMode::Light => identity.logo_light.as_ref().or(identity.logo.as_ref()),
        theme::palette::ThemeMode::Dark => identity.logo.as_ref(),
    };
    let mark: Element<'a, Message> = match logo {
        Some(logo) => iced::widget::image(logo.clone())
            .width(36)
            .height(36)
            .into(),
        None => Container::new(
            label(
                identity
                    .platform
                    .chars()
                    .next()
                    .map(|c| c.to_uppercase().to_string())
                    .unwrap_or_default(),
            )
            .style(coincube_ui::theme::tenshu::text::accent),
        )
        .center(Length::Fixed(36.0))
        .style(|theme: &theme::Theme| container::Style {
            background: Some(Background::Color(theme.colors.shell.selected)),
            border: iced::Border {
                radius: 8.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into(),
    };
    let who = Column::new()
        .spacing(1)
        .width(Length::Fill)
        .push(label(&identity.platform).wrapping(iced::widget::text::Wrapping::WordOrGlyph))
        .push(
            caption(localized_copy("Identified by Branta")).style(|theme: &theme::Theme| {
                coincube_ui::theme::tenshu::text::tone(
                    theme,
                    coincube_ui::theme::tenshu::Tone::Good,
                )
            }),
        )
        .push_maybe(identity.description.as_deref().map(|description| {
            caption(description).wrapping(iced::widget::text::Wrapping::WordOrGlyph)
        }));
    Container::new(
        Row::new()
            .spacing(12)
            .align_y(Alignment::Center)
            .push(mark)
            .push(who)
            .push(
                ActionButton::new(
                    iced::advanced::widget::Id::new(RECIPIENT_IDENTITY_ID),
                    localized_copy("View"),
                    ActionKind::Secondary,
                    Message::FocusAction,
                )
                .on_press(Message::SparkSend(
                    SparkSendMessage::OpenRecipientIdentity(index),
                )),
            ),
    )
    .padding([10, 12])
    .width(Length::Fixed(340.0))
    .style(notice_style)
    .into()
}

/// Review's checks on where the payment goes.
fn review_checks<'a>(
    view: &SparkSendView<'a>,
    ok: &'a coincube_spark_protocol::PrepareSendOk,
) -> Element<'a, Message> {
    let mut rows = vec![
        check_row(
            true,
            localized_copy("Destination format"),
            check_status(localized_copy(method_label(&ok.method)).to_owned(), true),
        ),
        check_row(
            true,
            CopyText::Network.get(),
            check_status(
                crate::chain::canonical_network_name(view.chain).to_owned(),
                true,
            ),
        ),
    ];
    if view.recipient_lookup_eligible {
        rows.push(recipient_status_row(
            view.recipient_checks_enabled,
            view.recipient_identities,
            view.theme_mode,
        ));
    }
    Column::new()
        .spacing(6)
        .push(coincube_ui::component::tenshu::label(localized_copy(
            "Recipient checks",
        )))
        .push(ruled(rows))
        .into()
}

/// Review's side panel: each balance now and after this payment, estimated
/// (as Swap's Review shows it).
fn after_payment<'a>(
    view: &SparkSendView<'a>,
    ok: &'a coincube_spark_protocol::PrepareSendOk,
) -> Element<'a, Message> {
    use coincube_ui::component::tenshu::{body, caption, MASK};
    let usdb_decimals = view.balances.stable.map_or(6, |usdb| usdb.decimals);
    let converted_in = view.conversion.map_or(0, |quote| quote.amount_in.amount);
    let converted_out = view
        .conversion
        .map_or(0, |quote| clamp_sats(quote.amount_out.amount));
    let btc_now = view.balances.btc_sats;
    let btc_after = btc_now
        .saturating_add(converted_out)
        .saturating_sub(ok.amount_sat.saturating_add(ok.fee_sat));
    let row = |name: &'static str, now: String, after: String| -> Element<'a, Message> {
        Column::new()
            .spacing(2)
            .push(body(name))
            .push(caption(if view.mask_amounts {
                format!("{MASK} → {MASK}")
            } else {
                format!("{now} → {after}")
            }))
            .into()
    };
    Column::new()
        .spacing(10)
        .push(caption(localized_copy("After this payment, estimated")).size(13))
        .push(row(
            "Bitcoin",
            btc_line(btc_now, view.bitcoin_unit),
            btc_line(btc_after, view.bitcoin_unit),
        ))
        .push_maybe(view.balances.stable.map(|usdb| {
            let spent = view.token_payment.map_or(converted_in, |payment| {
                payment.amount.amount.saturating_add(payment.fee.amount)
            });
            row(
                "US Dollar",
                usdb_amount_line(usdb.balance, usdb_decimals),
                usdb_amount_line(usdb.balance.saturating_sub(spent), usdb_decimals),
            )
        }))
        .into()
}

/// Review's footer: a status line, then its actions on the right.
fn review_footer<'a>(
    status: &'static str,
    actions: Vec<Element<'a, Message>>,
) -> Element<'a, Message> {
    let mut row = Row::new().spacing(12).align_y(Alignment::Center).push(
        p2_regular(status)
            .style(theme::text::secondary)
            .width(Length::Fill),
    );
    for action in actions {
        row = row.push(action);
    }
    row.into()
}

/// The Review card (P-Send prepared): the route, the quote, the recipient
/// checks and Confirm. A Branta mismatch replaces the quote and offers only
/// a new payment request.
fn review_card<'a>(
    view: &SparkSendView<'a>,
    ok: &'a coincube_spark_protocol::PrepareSendOk,
) -> Element<'a, Message> {
    let usdb_decimals = view.balances.stable.map_or(6, |usdb| usdb.decimals);
    let action = |id: &'static str, label: &'static str, kind, message| -> Element<'a, Message> {
        ActionButton::new(
            iced::advanced::widget::Id::new(id),
            label,
            kind,
            Message::FocusAction,
        )
        .on_press(message)
        .into()
    };
    let head = Container::new(
        Row::new()
            .spacing(12)
            .align_y(Alignment::Center)
            .push(h4_bold(localized_copy("Review and confirm")).width(Length::Fill))
            .push(
                p2_regular(localized_copy(method_label(&ok.method))).style(theme::text::secondary),
            ),
    )
    .padding([16, 24])
    .width(Length::Fill);

    if let Some(quote) = ok.cross_chain.as_deref() {
        return cross_chain_review_card(view, ok, quote, head);
    }

    let mut card = Column::new()
        .push(head)
        .push(iced::widget::rule::horizontal(1))
        .push(card_section(review_route(view, ok, usdb_decimals)))
        .push(iced::widget::rule::horizontal(1));

    if view.recipient_mismatch {
        return card
            .push(card_section(
                Container::new(
                    Row::new()
                        .spacing(12)
                        .align_y(Alignment::Start)
                        .push(icon::warning_icon().size(18).style(|theme: &theme::Theme| {
                            coincube_ui::theme::tenshu::text::tone(
                                theme,
                                coincube_ui::theme::tenshu::Tone::Bad,
                            )
                        }))
                        .push(
                            p1_bold(crate::services::branta::MISMATCH_MESSAGE)
                                .style(|theme: &theme::Theme| {
                                    coincube_ui::theme::tenshu::text::tone(
                                        theme,
                                        coincube_ui::theme::tenshu::Tone::Bad,
                                    )
                                })
                                .width(Length::Fill),
                        ),
                )
                .padding([14, 16])
                .width(Length::Fill)
                .style(|theme: &theme::Theme| container::Style {
                    border: iced::Border {
                        color: theme.colors.shell.bad,
                        width: 1.0,
                        radius: 10.0.into(),
                    },
                    ..Default::default()
                }),
            ))
            .push(iced::widget::rule::horizontal(1))
            .push(card_section(review_footer(
                "Nothing has been sent.",
                vec![action(
                    REVIEW_REPLACE_ID,
                    localized_copy("Replace payment request"),
                    ActionKind::Primary,
                    Message::SparkSend(SparkSendMessage::Reset),
                )],
            )))
            .into();
    }

    card = card
        .push(card_section(review_quote(view, ok, usdb_decimals)))
        .push(iced::widget::rule::horizontal(1))
        .push(card_section(review_checks(view, ok)))
        .push(iced::widget::rule::horizontal(1))
        .push(card_section(review_footer(
            "Nothing moves until you confirm.",
            vec![
                action(
                    REVIEW_BACK_ID,
                    CopyText::Back.get(),
                    ActionKind::Secondary,
                    Message::SparkSend(SparkSendMessage::EditDetails),
                ),
                action(
                    REVIEW_CONFIRM_ID,
                    localized_copy("Confirm and send"),
                    ActionKind::Primary,
                    Message::SparkSend(SparkSendMessage::ConfirmRequested),
                ),
            ],
        )));
    card.into()
}

// ── Review, cross-chain ──────────────────────────────────────────────────────

const REVIEW_REQUOTE_ID: &str = "spark-send-review-requote";

/// What "price may move up to" offers, in basis points. The SDK's default
/// (1%) is the middle one, so leaving it alone is the usual answer.
const SLIPPAGE_PRESETS_BPS: [u32; 3] = [50, 100, 300];

/// Review for a stablecoin send: the same card as a bitcoin send's, with the
/// quote a cross-chain route needs — what they receive in the destination
/// asset and on which network, both fees, how far the price may move, and
/// how long the quote lasts.
///
/// The network is settled on Details. Review only asks the user to confirm it
/// when they chose it among several (an EVM address valid on many chains):
/// an address that belongs to one network already says where it goes.
fn cross_chain_review_card<'a>(
    view: &SparkSendView<'a>,
    ok: &'a coincube_spark_protocol::PrepareSendOk,
    quote: &'a coincube_spark_protocol::CrossChainQuote,
    head: Container<'a, Message>,
) -> Element<'a, Message> {
    use coincube_ui::component::tenshu::{body, label, mono};
    let unit = view.bitcoin_unit;
    let price = view.reference_btc_usd_price;
    let network = cross_chain::chain_name(&quote.route.chain);
    let coin = view.receive_target.label();
    let total_sats = ok.amount_sat.saturating_add(ok.fee_sat);
    let value = |value: String| -> Element<'a, Message> { body(value).into() };

    let (figure, figure_unit) = btc_figure(total_sats, unit);
    let you_send = ReviewTile {
        caption: "You send",
        logo: asset_network_logo("btc", "spark", 40.0),
        asset: "Bitcoin",
        figure,
        unit: figure_unit,
        note: usd_estimate(total_sats, price),
        rail: "SPARK".into(),
    };
    let they_receive = ReviewTile {
        caption: "They receive",
        logo: target_logo(view.receive_target, 40.0),
        asset: coin,
        figure: cross_chain::format_asset_amount(quote.estimated_out, quote.route.decimals),
        unit: if view.receive_target == SparkSendTarget::Usdc {
            "USDC"
        } else {
            "USDT"
        },
        note: Some(localized_format!("on {}", "en {}", network)),
        rail: network.to_uppercase(),
    };
    let route = Row::new()
        .spacing(16)
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .push(Container::new(review_tile(you_send)).width(Length::FillPortion(1)))
        .push(text("→").size(H5_SIZE).style(theme::text::secondary))
        .push(Container::new(review_tile(they_receive)).width(Length::FillPortion(1)));

    let countdown = view.quote_countdown.clone();
    let live = matches!(countdown, Some(cross_chain::QuoteCountdown::Valid { .. }));
    let quote_value: Element<'a, Message> = match countdown {
        Some(cross_chain::QuoteCountdown::Valid { seconds_left }) => value(localized_format!(
            "Valid for {}:{:02}",
            "Válida durante {}:{:02}",
            seconds_left / 60,
            seconds_left % 60
        )),
        _ => text(localized_copy("Expired. Get a new quote to send."))
            .size(P1_SIZE)
            .style(theme::text::warning)
            .into(),
    };

    let current_bps = view
        .slippage_input
        .trim()
        .parse::<u32>()
        .unwrap_or(cross_chain::DEFAULT_SLIPPAGE_BPS);
    let mut presets = Row::new().spacing(6);
    for bps in SLIPPAGE_PRESETS_BPS {
        let selected = bps == current_bps;
        presets = presets.push(
            iced::widget::button(text(format!("{}%", bps as f64 / 100.0)).size(P2_SIZE))
                .padding([4, 10])
                .style(if selected {
                    theme::button::primary
                } else {
                    theme::button::container_border
                })
                .on_press_maybe(
                    (!selected)
                        .then_some(Message::SparkSend(SparkSendMessage::SlippagePreset(bps))),
                ),
        );
    }

    let rows = vec![
        quote_row(
            localized_copy("To"),
            mono(shorten_middle(view.destination_input, 14, 8)).into(),
            Some(localized_format!("{} on {}", "{} en {}", coin, network)),
            false,
        ),
        quote_row(
            localized_copy("Amount"),
            value(btc_line(ok.amount_sat, unit)),
            usd_estimate(ok.amount_sat, price),
            false,
        ),
        quote_row(
            localized_copy("Network fee"),
            value(btc_line(quote.source_transfer_fee_sats, unit)),
            None,
            false,
        ),
        quote_row(
            localized_copy("Conversion fee"),
            value(conversion_fee_display(
                quote.fee_amount,
                quote.route.decimals,
                &quote.route.asset,
                price,
                unit,
            )),
            Some(localized_copy("Already taken from what they receive").to_owned()),
            false,
        ),
        quote_row(
            localized_copy("Price may move up to"),
            presets.into(),
            None,
            false,
        ),
        quote_row(localized_copy("Quote"), quote_value, None, false),
        quote_row(
            localized_copy("Total you send"),
            label(btc_line(total_sats, unit)).size(16).into(),
            usd_estimate(total_sats, price),
            true,
        ),
    ];

    // Asked only when the user picked the network themselves.
    let chose_network = view
        .detected
        .is_some_and(cross_chain::DetectedDestination::network_was_chosen);
    let acknowledgement = chose_network.then(|| {
        Container::new(
            Column::new()
                .spacing(10)
                .push(
                    Row::new()
                        .spacing(10)
                        .align_y(Alignment::Start)
                        .push(icon::warning_icon().size(14).style(|theme: &theme::Theme| {
                            iced::widget::text::Style {
                                color: Some(theme.colors.shell.warn),
                            }
                        }))
                        .push(
                            coincube_ui::component::tenshu::caption(localized_format!(
                                "This address works on several networks. {} sent on the wrong \
                                 one can't be recovered.",
                                "Esta dirección funciona en varias redes. Los {} enviados a la \
                                 red equivocada no se pueden recuperar.",
                                coin
                            ))
                            .size(13)
                            .width(Length::Fill),
                        ),
                )
                .push(
                    coincube_ui::widget::CheckBox::new(view.network_acknowledged)
                        .label(localized_format!(
                            "The recipient uses {} for {}",
                            "El destinatario usa {} para {}",
                            network,
                            coin
                        ))
                        .style(theme::checkbox::primary)
                        .size(18)
                        .on_toggle(|checked| {
                            Message::SparkSend(SparkSendMessage::AcknowledgeNetwork(checked))
                        }),
                ),
        )
        .padding([10, 12])
        .width(Length::Fill)
        .style(notice_style)
    });

    let action = |id: &'static str, label: &'static str, kind, message: Option<Message>| {
        Element::from(
            ActionButton::new(
                iced::advanced::widget::Id::new(id),
                label,
                kind,
                Message::FocusAction,
            )
            .on_press_maybe(message),
        )
    };
    let primary = if live {
        action(
            REVIEW_CONFIRM_ID,
            localized_copy("Confirm and send"),
            ActionKind::Primary,
            (!view.awaiting_network_ack)
                .then_some(Message::SparkSend(SparkSendMessage::ConfirmRequested)),
        )
    } else {
        action(
            REVIEW_REQUOTE_ID,
            localized_copy("Get a new quote"),
            ActionKind::Primary,
            Some(Message::SparkSend(SparkSendMessage::ReQuoteRequested)),
        )
    };
    let status = if !live {
        "This quote has expired. Nothing has been sent."
    } else if view.awaiting_network_ack {
        "Confirm the network to send."
    } else {
        "Nothing moves until you confirm."
    };

    Column::new()
        .push(head)
        .push(iced::widget::rule::horizontal(1))
        .push(card_section(route))
        .push(iced::widget::rule::horizontal(1))
        .push(card_section(
            Column::new()
                .spacing(16)
                .push(ruled(rows))
                .push_maybe(acknowledgement),
        ))
        .push(iced::widget::rule::horizontal(1))
        .push(card_section(review_footer(
            status,
            vec![
                action(
                    REVIEW_BACK_ID,
                    CopyText::Back.get(),
                    ActionKind::Secondary,
                    Some(Message::SparkSend(SparkSendMessage::EditDetails)),
                ),
                primary,
            ],
        )))
        .into()
}

/// Under Destination, once the address is looked up: what it is, and — for
/// an address several networks reach — the choice between them. Nothing is
/// pre-chosen there: guessing the chain is how funds are lost.
fn detected_destination<'a>(
    detected: &'a cross_chain::DetectedDestination,
    coin: &str,
) -> Element<'a, Message> {
    let mut column = Column::new()
        .spacing(10)
        .push(
            p2_regular(detected.network.note(coin)).style(if detected.network.is_problem() {
                theme::text::warning
            } else {
                theme::text::secondary
            }),
        );
    let choices = detected.choices();
    if !choices.is_empty() {
        let mut row = Row::new().spacing(8).width(Length::Fill);
        for chain in choices {
            let selected = detected.chosen.as_deref() == Some(chain.as_str());
            row = row.push(
                iced::widget::button(
                    text(chain.clone())
                        .size(P1_SIZE)
                        .align_x(Alignment::Center)
                        .width(Length::Fill),
                )
                .padding([10, 12])
                .width(Length::FillPortion(1))
                .style(if selected {
                    theme::button::primary
                } else {
                    theme::button::container_border
                })
                .on_press(Message::SparkSend(SparkSendMessage::ChooseNetwork(
                    chain.clone(),
                ))),
            );
        }
        column = column.push(row);
    }
    column.into()
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
        SparkSendTarget::SparkUsd => asset_network_logo("usdb", "spark", size),
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

// ── Payment details card: layout, actions and their stable IDs ─────────────

// Stable, purpose-specific focus targets. Targets are named by rail, never by
// list position, so a reordered or filtered picker cannot move focus (or a
// held key) onto a different target.
const DESTINATION_INPUT_ID: &str = "spark-send-destination";
const AMOUNT_INPUT_ID: &str = "spark-send-amount";
const RECEIVE_TARGET_ACTION_ID: &str = "spark-send-receive-target";
pub const PAY_FROM_ACTION_ID: &str = "spark-send-pay-from";
pub const PAY_FROM_BITCOIN_ID: &str = "spark-send-pay-from-bitcoin";
pub const PAY_FROM_USDB_ID: &str = "spark-send-pay-from-usdb";

fn pay_from_label(asset: coincube_spark_protocol::WalletAsset) -> &'static str {
    match asset {
        coincube_spark_protocol::WalletAsset::Bitcoin => "Bitcoin",
        coincube_spark_protocol::WalletAsset::Usdb => localized_copy("US Dollar"),
    }
}

/// A USDB amount in token base units, for display: "2.50 USDB".
pub fn usdb_amount_line(amount: u128, usdb_decimals: u32) -> String {
    format!(
        "{} USDB",
        crate::app::breez_spark::assets::format_token_display_u128(amount, usdb_decimals)
    )
}

/// Review's Conversion row: "12.40 USDB → 15,003 SATS", bitcoin in the
/// wallet's display unit.
pub fn conversion_line(
    quote: &coincube_spark_protocol::ConversionQuote,
    usdb_decimals: u32,
    unit: BitcoinDisplayUnit,
) -> String {
    let side = |amount: coincube_spark_protocol::AssetAmount| match amount.asset {
        coincube_spark_protocol::WalletAsset::Usdb => {
            usdb_amount_line(amount.amount, usdb_decimals)
        }
        coincube_spark_protocol::WalletAsset::Bitcoin => btc_line(clamp_sats(amount.amount), unit),
    };
    format!("{} → {}", side(quote.amount_in), side(quote.amount_out))
}

/// The "YOU SEND FROM" menu: Bitcoin or US Dollar, each with its balance.
/// US Dollar can't be picked when `usdb_unavailable` says why; that reason
/// shows in its place.
pub fn pay_from_picker_menu<'a>(
    current: coincube_spark_protocol::WalletAsset,
    usdb_unavailable: Option<&'static str>,
    balances: crate::app::view::spark::SparkBalances<'a>,
) -> Element<'a, Message> {
    use crate::app::view::shared::picker::{
        dropdown_heading, dropdown_menu, dropdown_note, dropdown_option, DROPDOWN_LOGO,
    };
    use coincube_spark_protocol::WalletAsset;
    let (rows, _) = balances.rows();
    let balance_of = |name: &str| {
        rows.iter()
            .find(|(row, _, _)| *row == name)
            .map(|(_, figure, unit)| format!("{} {}", figure, unit))
            .unwrap_or_else(|| "0".to_owned())
    };
    let option = |asset: WalletAsset, id: &'static str, coin: &str, chip: &str| {
        let enabled = asset == WalletAsset::Bitcoin || usdb_unavailable.is_none();
        let line = match (asset, usdb_unavailable) {
            (WalletAsset::Usdb, Some(reason)) => localized_copy(reason).to_owned(),
            _ => balance_of(pay_from_label(asset)),
        };
        dropdown_option(
            id,
            asset_network_logo(coin, "spark", DROPDOWN_LOGO),
            pay_from_label(asset),
            chip,
            Some(line),
            asset == current,
            enabled.then_some(Message::SparkSend(SparkSendMessage::SelectPayFrom(asset))),
            Message::FocusAction,
        )
    };
    dropdown_menu(
        Column::new()
            .push(dropdown_heading(localized_copy("Your Spark balances")))
            .push(option(
                WalletAsset::Bitcoin,
                PAY_FROM_BITCOIN_ID,
                "btc",
                "SPARK",
            ))
            .push(option(WalletAsset::Usdb, PAY_FROM_USDB_ID, "usdb", "USDB"))
            // How a Bitcoin payment falls back to USDB; paying from US Dollar
            // has its own note under the tiles.
            .push_maybe(
                (current == WalletAsset::Bitcoin).then(|| dropdown_note(PAY_FROM_BITCOIN_NOTE)),
            ),
    )
}
const PREPARE_ACTION_ID: &str = "spark-send-prepare";

fn target_action_id(target: SparkSendTarget) -> &'static str {
    match target {
        SparkSendTarget::Lightning => "spark-send-target-lightning",
        SparkSendTarget::OnChain => "spark-send-target-on-chain",
        SparkSendTarget::Spark => "spark-send-target-spark",
        SparkSendTarget::SparkUsd => "spark-send-target-spark-us-dollar",
        SparkSendTarget::Usdt => "spark-send-target-usdt",
        SparkSendTarget::Usdc => "spark-send-target-usdc",
    }
}

/// One full-width section of the Payment details card. The 24px inset also
/// leaves room for the four-pixel focus ring around an Action at its edge.
fn card_section<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    Container::new(content)
        .padding([20, 24])
        .width(Length::Fill)
        .into()
}

/// The Payment details surface: the shell panel with a hairline border.
fn payment_card_style(theme: &theme::Theme) -> container::Style {
    let shell = &theme.colors.shell;
    container::Style {
        background: Some(Background::Color(shell.panel)),
        text_color: Some(shell.ink),
        border: iced::Border {
            color: shell.line,
            width: 1.0,
            radius: 16.0.into(),
        },
        ..Default::default()
    }
}

/// A route tile. Hover only changes the outline; selection never applies here.
fn route_tile_style(theme: &theme::Theme, hovered: bool) -> (Background, iced::Border) {
    let shell = &theme.colors.shell;
    (
        Background::Color(shell.background),
        iced::Border {
            color: if hovered {
                shell.accent_ink
            } else {
                shell.line
            },
            width: 1.0,
            radius: 8.0.into(),
        },
    )
}

/// Status-independent fill for the fixed YOU SEND tile, and Swap's tiles.
pub(crate) fn fixed_tile_style(theme: &theme::Theme) -> container::Style {
    let (background, border) = route_tile_style(theme, false);
    container::Style {
        background: Some(background),
        border,
        ..Default::default()
    }
}

/// The tappable THEY RECEIVE tile: the button draws the tile itself, so the
/// hover outline is not hidden under an inner card.
pub(crate) fn receive_tile_button_style(
    theme: &theme::Theme,
    status: iced::widget::button::Status,
) -> iced::widget::button::Style {
    let hovered = matches!(
        status,
        iced::widget::button::Status::Hovered | iced::widget::button::Status::Pressed
    );
    let (background, border) = route_tile_style(theme, hovered);
    iced::widget::button::Style {
        background: Some(background),
        text_color: theme.colors.shell.ink,
        border,
        ..Default::default()
    }
}

/// The card's footer while composing: one status line and Prepare. `None`
/// renders Prepare without a press — Tab skips it and no key or pointer can
/// activate it — rather than hiding it, so the footer keeps its shape.
fn compose_footer<'a>(status: &'static str, prepare: Option<Message>) -> Element<'a, Message> {
    Row::new()
        .spacing(16)
        .align_y(Alignment::Center)
        .push(
            p2_regular(status)
                .style(theme::text::secondary)
                .width(Length::Fill),
        )
        .push(
            Action::new(
                PREPARE_ACTION_ID,
                text(localized_copy("Prepare")).size(P1_SIZE).bold(),
                Message::FocusAction,
            )
            .style(theme::button::primary)
            .padding([10, 24])
            .on_press_maybe(prepare),
        )
        .into()
}

/// Where a send target's asset travels, as it reads after the asset on the
/// THEY RECEIVE tile ("Bitcoin on Lightning", "USDt cross-chain").
fn target_place(target: SparkSendTarget) -> &'static str {
    match target {
        SparkSendTarget::Lightning => "on Lightning",
        SparkSendTarget::OnChain => "on-chain",
        SparkSendTarget::Spark | SparkSendTarget::SparkUsd => "on Spark",
        SparkSendTarget::Usdt | SparkSendTarget::Usdc => "cross-chain",
    }
}

/// The "YOU SEND → THEY RECEIVE" pair, as slim tiles like Receive's. THEY
/// RECEIVE always opens the rail menu; YOU SEND opens the balance menu only
/// where the target leaves a choice (bitcoin rails). Each menu is anchored
/// under its tile; a chevron and the hover outline mark each tappable tile.
fn spark_send_cards<'a>(
    target: SparkSendTarget,
    detected: Option<&cross_chain::DetectedDestination>,
    pay_from: coincube_spark_protocol::WalletAsset,
    menus: SendMenus<'a>,
) -> Element<'a, Message> {
    use crate::app::view::shared::picker::anchored_dropdown;
    // A stablecoin send is BTC-funded cross-chain and US Dollar over Spark
    // moves US Dollar, so only bitcoin rails offer a choice of balance (S4).
    let choose = target.fixed_pay_from().is_none();
    let pay_from = target.fixed_pay_from().unwrap_or(pay_from);
    let pay_from_open = choose && menus.open == Some(SendMenu::PayFrom);
    let (logo, label): (Element<'a, Message>, &'static str) = match pay_from {
        coincube_spark_protocol::WalletAsset::Usdb => (
            asset_network_logo("usdb", "spark", 36.0),
            localized_copy("US Dollar"),
        ),
        _ => (asset_network_logo("btc", "spark", 36.0), "Bitcoin"),
    };
    let you_send_body = super::route_tile_body(
        logo,
        "You send",
        None,
        label.to_string(),
        localized_copy("in Spark").to_string(),
        choose.then_some(pay_from_open),
    );
    let you_send: Element<'a, Message> = if choose {
        anchored_dropdown(
            Action::new(PAY_FROM_ACTION_ID, you_send_body, Message::FocusAction)
                .style(if pay_from_open {
                    open_tile_button_style
                } else {
                    receive_tile_button_style
                })
                .padding(0)
                .width(Length::Fill)
                .focus_radius(8.0)
                .on_press(Message::SparkSend(SparkSendMessage::OpenPayFromPicker)),
            pay_from_open
                .then(|| pay_from_picker_menu(pay_from, menus.usdb_unavailable, menus.balances)),
            Message::SparkSend(SparkSendMessage::ClosePayFromPicker),
        )
    } else {
        Container::new(you_send_body)
            .width(Length::Fill)
            .style(fixed_tile_style)
            .into()
    };

    let receive_open = menus.open == Some(SendMenu::TheyReceive);
    let they_receive = anchored_dropdown(
        Action::new(
            RECEIVE_TARGET_ACTION_ID,
            super::route_tile_body(
                target_logo(target, 36.0),
                "They receive",
                None,
                localized_copy(target.label()).to_string(),
                detected
                    .filter(|_| target.is_stablecoin())
                    .and_then(cross_chain::DetectedDestination::tile_place)
                    .unwrap_or_else(|| localized_copy(target_place(target)).to_string()),
                Some(receive_open),
            ),
            Message::FocusAction,
        )
        .style(if receive_open {
            open_tile_button_style
        } else {
            receive_tile_button_style
        })
        .padding(0)
        .width(Length::Fill)
        .focus_radius(8.0)
        .on_press(Message::SparkSend(SparkSendMessage::OpenReceivePicker)),
        receive_open.then(|| send_target_picker_menu(target, menus.network)),
        Message::SparkSend(SparkSendMessage::CloseReceivePicker),
    );

    // An open menu floats under its tile and takes no room, so the pair keeps
    // its height and the arrow stays centred either way.
    Row::new()
        .spacing(12)
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .push(Container::new(you_send).width(Length::FillPortion(1)))
        .push(text("→").size(P1_SIZE + 2).style(theme::text::secondary))
        .push(Container::new(they_receive).width(Length::FillPortion(1)))
        .into()
}

/// Which route menu is open on Send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendMenu {
    PayFrom,
    TheyReceive,
}

/// What the route tiles need to draw their menus.
#[derive(Clone, Copy)]
pub struct SendMenus<'a> {
    pub open: Option<SendMenu>,
    pub usdb_unavailable: Option<&'static str>,
    pub balances: crate::app::view::spark::SparkBalances<'a>,
    pub network: Network,
}

/// A route tile while its menu is open: the hovered tile's surface with the
/// accent line, so the tile reads as the menu's owner.
pub(crate) fn open_tile_button_style(
    theme: &theme::Theme,
    _: iced::widget::button::Status,
) -> iced::widget::button::Style {
    let (background, mut border) = route_tile_style(theme, true);
    border.color = theme.colors.shell.accent;
    iced::widget::button::Style {
        background: Some(background),
        text_color: theme.colors.shell.ink,
        border,
        ..Default::default()
    }
}

/// The "THEY RECEIVE" menu: bitcoin rails, then (mainnet only) US Dollar over
/// Spark and the cross-chain stablecoins. Selection is styling only; every
/// offered rail stays pressable.
pub fn send_target_picker_menu<'a>(
    current: SparkSendTarget,
    network: Network,
) -> Element<'a, Message> {
    use crate::app::view::shared::picker::{
        dropdown_heading, dropdown_menu, dropdown_option, DROPDOWN_LOGO,
    };
    let row = |target: SparkSendTarget| {
        dropdown_option(
            target_action_id(target),
            target_logo(target, DROPDOWN_LOGO),
            localized_copy(target.label()),
            localized_copy(target.badge()),
            None,
            current == target,
            Some(Message::SparkSend(SparkSendMessage::SetReceiveTarget(
                target,
            ))),
            Message::FocusAction,
        )
    };

    let mut list = Column::new().push(dropdown_heading("Bitcoin"));
    for target in [
        SparkSendTarget::Lightning,
        SparkSendTarget::OnChain,
        SparkSendTarget::Spark,
    ] {
        list = list.push(row(target));
    }

    // USDB is a mainnet token, so US Dollar over Spark is offered there only.
    if network == Network::Bitcoin {
        list = list
            .push(dropdown_heading(localized_copy("US Dollar")))
            .push(row(SparkSendTarget::SparkUsd));
    }

    // Cross-chain stablecoins are mainnet-only (providers/chains have no test
    // deployment), so they only appear there.
    if supported_on(network) {
        list = list.push(dropdown_heading(localized_copy("Stablecoin (cross-chain)")));
        for target in [SparkSendTarget::Usdt, SparkSendTarget::Usdc] {
            list = list.push(row(target));
        }
    }

    dropdown_menu(list)
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
    fn conversion_fee_is_asset_only_with_non_positive_prices() {
        for price in [Some(0.0), Some(-1.0)] {
            let s = conversion_fee_display(97_921, 6, "USDT", price, BitcoinDisplayUnit::Sats);
            assert_eq!(s, "0.097921 USDT");
        }
    }

    #[test]
    fn conversion_fee_uses_the_selected_bitcoin_display_unit() {
        // 0.097921 USDT at $65,000/BTC = 150 sats, rendered in BTC mode.
        let s = conversion_fee_display(97_921, 6, "USDT", Some(65_000.0), BitcoinDisplayUnit::BTC);
        assert_eq!(s, "0.097921 USDT (≈ 0.00 000 150 BTC)");
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

    // ── R07b: the real compose presenter inside the real dashboard ──────────
    //
    // Synthetic facts only. Nothing here starts a wallet, the Spark bridge, an
    // SDK call, a session or an OS window: the production `SparkSendView` and
    // target picker are composed exactly as the state's `view()` composes them,
    // then laid out, operated and (optionally) drawn by the software renderer.

    use crate::app::cache::Cache;
    use crate::app::menu::{Menu, SparkSubMenu};
    use crate::app::state::spark::send::{ReconcileOutcome, RetryGuard};
    use crate::app::view::shared::landing::capture;
    use coincube_spark_protocol::{CrossChainQuote, CrossChainRoute, PrepareSendOk};
    use coincube_ui::component::quote_display::Quote;
    use coincube_ui::theme::palette::ThemeMode;
    use iced::advanced::widget::operation::{self, focusable, Focusable, TextInput};
    use iced::advanced::widget::{Id, Operation, Tree};
    use iced::advanced::{clipboard, layout, overlay, renderer::Headless, Layout, Shell};
    use iced::{keyboard, mouse, Event, Point, Rectangle, Size, Vector};

    const COMPACT: (u32, u32) = (1024, 768);
    const WIDE: (u32, u32) = (1440, 900);
    const FULL_HEIGHT: (u32, u32) = (1440, 2400);
    // Unified spendable balance includes the BTC component and synthetic USDB.
    const SYNTHETIC_BALANCE_SATS: u64 = 35_000;
    const SYNTHETIC_BITCOIN_SATS: u64 = 25_053;
    /// Shaped like the real kinds, valid as none of them. `15u` is 15 microbitcoin,
    /// or 1,500 sats, matching the ordinary prepared quote below.
    const SYNTHETIC_INVOICE: &str =
        "lnbc15u1synthetic0r07b0fixture0not0a0payable0invoice0long0enough0to0test0wrapping";
    const SYNTHETIC_CROSS_CHAIN_ADDRESS: &str =
        "0xsynthetic-r07b-usdc-cross-chain-address-not-payable";
    const SYNTHETIC_HINT: &str = "Your bitcoin balance is 25,053 sats; the rest is converted \
                                  from your synthetic USDB at the time of sending.";

    #[test]
    fn recipient_status_uses_only_current_identity_and_device_setting() {
        let ticket = crate::services::branta::test_ticket(true);
        let identity = crate::services::branta::test_identity(&ticket, "Example Coffee Co.");
        let inspect = |enabled, identities: &[crate::services::branta::RecipientIdentity]| {
            capture::inspect(
                "spark-send-recipient-status",
                recipient_status_row(enabled, identities, ThemeMode::Dark),
                (1024, 768),
                "dark",
                theme::Theme::dark(),
            )
        };
        // Identified, the recipient's card takes the status's place.
        let identified = inspect(true, std::slice::from_ref(&identity));
        assert!(identified.contains("Example Coffee Co."));
        assert!(identified.contains("Identified by Branta"));
        assert!(identified.contains("View"));
        assert!(!identified.contains("Verified"));
        // Lookups always run in strict mode; the note rides with every lookup.
        assert!(identified.contains(STRICT_MODE_NOTE));
        let off = inspect(false, std::slice::from_ref(&identity));
        assert!(off.contains("Checks off"));
        assert!(off.contains("Turn on in Settings"));
        assert!(!off.contains("Identified"));
        assert!(!off.contains("Strict mode"));
        ticket.set_test_enabled(false);
        let silent = inspect(true, &[identity]);
        assert!(silent.contains("Not identified"));
        assert!(silent.contains(STRICT_MODE_NOTE));
    }

    #[test]
    fn prepared_recipient_status_is_hidden_without_a_lookup_eligible_request() {
        let mut eligible = Fixture::ordinary_prepared();
        eligible.recipient_checks_enabled = false;
        let render = |fixture: &Fixture| {
            capture::inspect(
                "spark-send-recipient-eligibility",
                fixture.view().render(),
                (1024, 768),
                "dark",
                theme::Theme::dark(),
            )
        };
        assert!(render(&eligible).contains("Checks off"));

        let mut unsupported = Fixture::ordinary_prepared();
        unsupported.target = SparkSendTarget::Spark;
        unsupported.destination = "spark1synthetic-destination".to_owned();
        unsupported.recipient_lookup_eligible = false;
        unsupported.recipient_checks_enabled = false;
        if let SparkSendPhase::Prepared(ok) = &mut unsupported.phase {
            ok.method = "SparkAddress".to_owned();
        }
        let mounted = render(&unsupported);
        assert!(mounted.contains("Spark transfer"));
        assert!(!mounted.contains("Recipient identity"));
        assert!(!mounted.contains("Checks off"));
    }

    struct Fixture {
        backend_available: bool,
        destination: String,
        amount: String,
        invoice_amount: bool,
        phase: SparkSendPhase,
        mismatch: bool,
        recipient_checks_enabled: bool,
        recipient_lookup_eligible: bool,
        mode: ThemeMode,
        unit: BitcoinDisplayUnit,
        hint: Option<String>,
        mask_amounts: bool,
        target: SparkSendTarget,
        network: Network,
        countdown: Option<cross_chain::QuoteCountdown>,
        quote: Quote,
        image: iced::widget::image::Handle,
        stable: Option<coincube_spark_protocol::UsdbBalanceSnapshot>,
        pay_from: coincube_spark_protocol::WalletAsset,
        usdb_unavailable: Option<&'static str>,
        conversion: Option<coincube_spark_protocol::ConversionQuote>,
        token_payment: Option<coincube_spark_protocol::TokenPayment>,
        identities: Vec<crate::services::branta::RecipientIdentity>,
    }

    impl Fixture {
        fn new(phase: SparkSendPhase) -> Self {
            Self {
                backend_available: true,
                destination: String::new(),
                amount: String::new(),
                invoice_amount: false,
                phase,
                mismatch: false,
                recipient_checks_enabled: true,
                recipient_lookup_eligible: false,
                mode: ThemeMode::Dark,
                unit: BitcoinDisplayUnit::Sats,
                hint: None,
                mask_amounts: false,
                target: SparkSendTarget::Lightning,
                network: Network::Bitcoin,
                countdown: None,
                quote: Quote {
                    id: String::new(),
                    text: String::new(),
                    author: String::new(),
                    source: String::new(),
                    category: String::new(),
                    length: String::new(),
                    contexts: Vec::new(),
                },
                image: iced::widget::image::Handle::from_rgba(1, 1, vec![0u8; 4]),
                stable: None,
                pay_from: coincube_spark_protocol::WalletAsset::Bitcoin,
                usdb_unavailable: Some("You have no USDB."),
                conversion: None,
                token_payment: None,
                identities: Vec::new(),
            }
        }

        fn filled(phase: SparkSendPhase) -> Self {
            let mut fixture = Self::new(phase);
            fixture.destination = SYNTHETIC_INVOICE.to_string();
            fixture.amount = "30000".to_string();
            fixture.hint = Some(SYNTHETIC_HINT.to_string());
            fixture.recipient_lookup_eligible = true;
            fixture
        }

        fn invoice(phase: SparkSendPhase) -> Self {
            let mut fixture = Self::new(phase);
            fixture.destination = SYNTHETIC_INVOICE.to_string();
            fixture.amount = "1500".to_string();
            fixture.invoice_amount = true;
            fixture.recipient_lookup_eligible = true;
            fixture
        }

        fn ordinary_prepared() -> Self {
            Self::invoice(ordinary_prepared())
        }

        fn cross_chain_prepared() -> Self {
            let mut fixture = Self::filled(cross_chain_prepared());
            fixture.destination = SYNTHETIC_CROSS_CHAIN_ADDRESS.to_string();
            fixture.target = SparkSendTarget::Usdc;
            fixture.recipient_lookup_eligible = false;
            fixture
        }

        fn themed(mut self, mode: ThemeMode) -> Self {
            self.mode = mode;
            self
        }

        fn view(&self) -> SparkSendView<'_> {
            SparkSendView {
                backend_available: self.backend_available,
                cube_name: "Everyday",
                destination_input: &self.destination,
                amount_input: &self.amount,
                amount_set_by_invoice: self.invoice_amount,
                phase: &self.phase,
                recipient_identities: &self.identities,
                recipient_mismatch: self.mismatch,
                recipient_checks_enabled: self.recipient_checks_enabled,
                recipient_lookup_eligible: self.recipient_lookup_eligible,
                theme_mode: self.mode,
                sent_amount_display: "",
                sent_celebration_context: "",
                sent_recipient: "",
                sent_fee_display: "",
                sent_total_display: "",
                sent_quote: &self.quote,
                sent_image_handle: &self.image,
                recent_transactions: &[],
                balance_sats: SYNTHETIC_BALANCE_SATS,
                balances: crate::app::view::spark::SparkBalances {
                    btc_sats: SYNTHETIC_BALANCE_SATS,
                    stable: self.stable.as_ref(),
                    fiat_converter: None,
                    btc_usd_price: None,
                    bitcoin_unit: self.unit,
                    masked: self.mask_amounts,
                },
                usdb_conversion_hint: self.hint.clone(),
                pay_from: self.pay_from,
                usdb_unavailable: self.usdb_unavailable,
                open_menu: None,
                paid_from: self.pay_from,
                conversion: self.conversion.as_ref(),
                token_payment: self.token_payment.as_ref(),
                bitcoin_unit: self.unit,
                reference_btc_usd_price: None,
                show_direction_badges: false,
                mask_amounts: self.mask_amounts,
                receive_target: self.target,
                network: self.network,
                chain: self.network.into(),
                cross_chain_ctx: None,
                detected: None,
                amount_issue: None,
                awaiting_network_ack: false,
                network_acknowledged: false,
                slippage_input: "",
                quote_countdown: self.countdown.clone(),
            }
        }

        fn cache(&self) -> Cache {
            Cache {
                network: self.network,
                theme_mode: self.mode,
                bitcoin_unit: self.unit,
                ..Default::default()
            }
        }

        fn theme(&self) -> theme::Theme {
            match self.mode {
                ThemeMode::Dark => theme::Theme::dark(),
                ThemeMode::Light => theme::Theme::light(),
            }
        }
    }

    /// The panel exactly as `SparkSend::view` builds it: the real dashboard
    /// around the real presenter, with the THEY RECEIVE menu open under its
    /// tile when `picker_open`.
    fn screen<'a>(
        fixture: &'a Fixture,
        menu: &'a Menu,
        cache: &'a Cache,
        picker_open: bool,
    ) -> Element<'a, Message> {
        crate::app::view::dashboard_payment(
            menu,
            cache,
            Some(crate::app::view::spark::sub_page_head(cache, "Send").rule(
                coincube_ui::component::tenshu::steps(
                    &crate::app::view::spark::SEND_STEPS,
                    send_step(&fixture.phase),
                ),
            )),
            {
                let mut view = fixture.view();
                if picker_open {
                    view.open_menu = Some(SendMenu::TheyReceive);
                }
                view.render()
            },
        )
    }

    fn spark_send_menu() -> Menu {
        Menu::Spark(SparkSubMenu::Send)
    }

    fn headless_renderer() -> iced::Renderer {
        for bytes in coincube_ui::font::load() {
            iced::advanced::graphics::text::font_system()
                .write()
                .unwrap()
                .load_font(bytes);
        }
        // 14px is the app's `default_text_size` (main.rs).
        iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
            coincube_ui::font::REGULAR,
            iced::Pixels(14.0),
            Some("tiny-skia"),
        ))
        .expect("software renderer for the synthetic R07b presenter")
    }

    #[derive(Default)]
    struct Probe {
        text: Vec<(String, Rectangle)>,
        focusable: Vec<(Option<Id>, Rectangle, bool)>,
        inputs: Vec<(Option<Id>, Rectangle, String)>,
    }

    impl Operation for Probe {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
            operate(self);
        }

        fn text(&mut self, _id: Option<&Id>, bounds: Rectangle, value: &str) {
            self.text.push((value.to_owned(), bounds));
        }

        fn focusable(&mut self, id: Option<&Id>, bounds: Rectangle, state: &mut dyn Focusable) {
            self.focusable
                .push((id.cloned(), bounds, state.is_focused()));
        }

        fn text_input(&mut self, id: Option<&Id>, bounds: Rectangle, state: &mut dyn TextInput) {
            self.inputs
                .push((id.cloned(), bounds, state.text().to_owned()));
        }
    }

    impl Probe {
        fn contains(&self, needle: &str) -> bool {
            self.text.iter().any(|(value, _)| value.contains(needle))
        }

        /// The first text that is exactly `needle`, in draw order.
        fn text_bounds(&self, needle: &str) -> Option<Rectangle> {
            self.text
                .iter()
                .find_map(|(value, bounds)| (value == needle).then_some(*bounds))
        }

        fn action_ids(&self) -> Vec<Id> {
            self.focusable
                .iter()
                .filter_map(|(id, _, _)| id.clone())
                .collect()
        }

        fn bounds(&self, id: &'static str) -> Option<Rectangle> {
            let id = Id::new(id);
            self.focusable
                .iter()
                .find_map(|(actual, bounds, _)| (actual.as_ref() == Some(&id)).then_some(*bounds))
        }

        fn focused(&self) -> Vec<Id> {
            self.focusable
                .iter()
                .filter(|(_, _, focused)| *focused)
                .filter_map(|(id, _, _)| id.clone())
                .collect()
        }

        fn input_text(&self, id: &'static str) -> Option<String> {
            let id = Id::new(id);
            self.inputs
                .iter()
                .find_map(|(actual, _, text)| (actual.as_ref() == Some(&id)).then(|| text.clone()))
        }
    }

    /// Owns one laid-out screen and its retained widget tree, so a re-render
    /// diffs the same tree the way the runtime does between frames.
    struct Harness<'a> {
        element: Element<'a, Message>,
        tree: Tree,
        node: layout::Node,
        renderer: iced::Renderer,
        size: Size,
    }

    impl<'a> Harness<'a> {
        fn new(element: Element<'a, Message>, outer: (u32, u32)) -> Self {
            let renderer = headless_renderer();
            let size = Size::new(outer.0 as f32, outer.1 as f32);
            let mut element = element;
            let mut tree = Tree::new(element.as_widget());
            let node = element.as_widget_mut().layout(
                &mut tree,
                &renderer,
                &layout::Limits::new(Size::ZERO, size),
            );
            Self {
                element,
                tree,
                node,
                renderer,
                size,
            }
        }

        fn rerender(&mut self, element: Element<'a, Message>) {
            self.element = element;
            self.tree.diff(self.element.as_widget());
            self.node = self.element.as_widget_mut().layout(
                &mut self.tree,
                &self.renderer,
                &layout::Limits::new(Size::ZERO, self.size),
            );
        }

        /// The runtime's whole-tree operation: the base first, then the
        /// overlay, which is where the open menu's rows live.
        fn operate(&mut self, operation: &mut dyn Operation) {
            self.element.as_widget_mut().operate(
                &mut self.tree,
                Layout::new(&self.node),
                &self.renderer,
                operation,
            );
            let viewport = Rectangle::with_size(self.size);
            if let Some(mut overlay) = self.element.as_widget_mut().overlay(
                &mut self.tree,
                Layout::new(&self.node),
                &self.renderer,
                &viewport,
                Vector::ZERO,
            ) {
                let node = overlay.as_overlay_mut().layout(&self.renderer, self.size);
                overlay
                    .as_overlay_mut()
                    .operate(Layout::new(&node), &self.renderer, operation);
            }
        }

        fn probe(&mut self) -> Probe {
            let mut probe = Probe::default();
            self.operate(&mut probe);
            probe
        }

        /// What `App` does with `Message::FocusAction(id)`: Iced's whole-tree
        /// focus, which also blurs every other focusable (inputs included).
        fn focus(&mut self, id: &'static str) {
            let mut current: Box<dyn Operation<()>> = Box::new(focusable::focus::<()>(Id::new(id)));
            loop {
                self.operate(&mut operation::black_box(current.as_mut()));
                match current.finish() {
                    operation::Outcome::Chain(next) => current = next,
                    _ => break,
                }
            }
        }

        fn send(&mut self, event: Event, cursor: mouse::Cursor) -> Vec<Message> {
            let mut messages = Vec::new();
            let viewport = Rectangle::with_size(self.size);
            let mut captured = false;
            if let Some(mut overlay) = self.element.as_widget_mut().overlay(
                &mut self.tree,
                Layout::new(&self.node),
                &self.renderer,
                &viewport,
                Vector::ZERO,
            ) {
                let node = overlay.as_overlay_mut().layout(&self.renderer, self.size);
                let mut shell = Shell::new(&mut messages);
                overlay.as_overlay_mut().update(
                    &event,
                    Layout::new(&node),
                    cursor,
                    &self.renderer,
                    &mut clipboard::Null,
                    &mut shell,
                );
                captured = shell.is_event_captured();
            }
            if !captured {
                self.element.as_widget_mut().update(
                    &mut self.tree,
                    &event,
                    Layout::new(&self.node),
                    cursor,
                    &self.renderer,
                    &mut clipboard::Null,
                    &mut Shell::new(&mut messages),
                    &viewport,
                );
            }
            messages
        }

        fn press_at(&mut self, point: iced::Point) -> Vec<Message> {
            self.send(
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
                mouse::Cursor::Available(point),
            )
        }

        fn release_at(&mut self, point: iced::Point) -> Vec<Message> {
            self.send(
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
                mouse::Cursor::Available(point),
            )
        }

        fn click(&mut self, id: &'static str) -> Vec<Message> {
            let center = self
                .probe()
                .bounds(id)
                .unwrap_or_else(|| panic!("{} is not a focus target here", id))
                .center();
            let mut messages = self.press_at(center);
            messages.extend(self.release_at(center));
            messages
        }

        fn key(
            &mut self,
            named: keyboard::key::Named,
            pressed: bool,
            repeat: bool,
        ) -> Vec<Message> {
            let key = keyboard::Key::Named(named);
            let physical_key =
                keyboard::key::Physical::Unidentified(keyboard::key::NativeCode::Unidentified);
            let event = if pressed {
                keyboard::Event::KeyPressed {
                    key: key.clone(),
                    modified_key: key,
                    physical_key,
                    location: keyboard::Location::Standard,
                    modifiers: keyboard::Modifiers::empty(),
                    text: None,
                    repeat,
                }
            } else {
                keyboard::Event::KeyReleased {
                    key: key.clone(),
                    modified_key: key,
                    physical_key,
                    location: keyboard::Location::Standard,
                    modifiers: keyboard::Modifiers::empty(),
                }
            };
            self.send(Event::Keyboard(event), mouse::Cursor::Unavailable)
        }

        fn type_text(&mut self, value: &'static str) -> Vec<Message> {
            let key = keyboard::Key::Character(value.into());
            self.send(
                Event::Keyboard(keyboard::Event::KeyPressed {
                    key: key.clone(),
                    modified_key: key,
                    physical_key: keyboard::key::Physical::Unidentified(
                        keyboard::key::NativeCode::Unidentified,
                    ),
                    location: keyboard::Location::Standard,
                    modifiers: keyboard::Modifiers::empty(),
                    text: Some(value.into()),
                    repeat: false,
                }),
                mouse::Cursor::Unavailable,
            )
        }

        /// Nothing in the dashboard may run past the window's right edge.
        fn assert_no_horizontal_overflow(&mut self, scenario: &str) {
            let width = self.size.width;
            for (value, bounds) in &self.probe().text {
                assert!(
                    bounds.x >= -1.0 && bounds.x + bounds.width <= width + 1.0,
                    "{}: text {:?} at {:?} runs outside the {}px window",
                    scenario,
                    value,
                    bounds,
                    width
                );
            }
        }

        fn render_pixels(&mut self, scenario: &str, theme: &theme::Theme, redraw: bool) -> Vec<u8> {
            let viewport = Rectangle::with_size(self.size);
            if redraw {
                // The open menu is drawn as an overlay. Use the
                // same overlay-first event route as actual pointer/key input.
                let messages = self.send(
                    Event::Window(iced::window::Event::RedrawRequested(
                        std::time::Instant::now(),
                    )),
                    mouse::Cursor::Unavailable,
                );
                // The dashboard scroll widget may publish its current zero
                // offset on the first redraw. No other message is passive.
                assert!(
                    messages.is_empty()
                        || matches!(messages.as_slice(), [Message::Scroll(offset)] if *offset == 0.0),
                    "{}: initial redraw emitted an effectful or nonzero message {:?}",
                    scenario,
                    messages
                );
            }
            // A regression paints this same tree both before and after its
            // first redraw. Never composite a later frame over earlier pixels.
            iced::advanced::Renderer::reset(&mut self.renderer, viewport);
            let style = iced::advanced::renderer::Style {
                text_color: theme.colors.text.primary,
            };
            self.element.as_widget().draw(
                &self.tree,
                &mut self.renderer,
                theme,
                &style,
                Layout::new(&self.node),
                mouse::Cursor::Unavailable,
                &viewport,
            );
            if let Some(mut overlay) = self
                .element
                .as_widget_mut()
                .overlay(
                    &mut self.tree,
                    Layout::new(&self.node),
                    &self.renderer,
                    &viewport,
                    Vector::ZERO,
                )
                .map(overlay::Nested::new)
            {
                // Match UserInterface::draw: Nested supplies the renderer
                // layer that keeps the menu above the dashboard's batches.
                let node = overlay.layout(&self.renderer, self.size);
                overlay.draw(
                    &mut self.renderer,
                    theme,
                    &style,
                    Layout::new(&node),
                    mouse::Cursor::Unavailable,
                );
            }
            let (width, height) = (self.size.width as u32, self.size.height as u32);
            self.renderer.screenshot(
                Size::new(width, height),
                1.0,
                theme.colors.general.background,
            )
        }

        /// Optional evidence: set `R07B_CAPTURE_DIR` (and `R07B_CAPTURE_HEAD`
        /// to the exact commit) to save the real rendered pixels. Without it
        /// the test only asserts. Headless pixels are not native-shell proof.
        fn capture(&mut self, scenario: &str, theme: &theme::Theme, appearance: &str) {
            let Ok(directory) = std::env::var("R07B_CAPTURE_DIR") else {
                return;
            };
            let head = std::env::var("R07B_CAPTURE_HEAD").unwrap_or_else(|_| "uncommitted".into());
            std::fs::create_dir_all(&directory).expect("create the disposable capture directory");
            let pixels = self.render_pixels(scenario, theme, true);
            let (width, height) = (self.size.width as u32, self.size.height as u32);
            let filename = format!(
                "r07b-{}-{}-{}x{}-{}-dashboard-synthetic.png",
                scenario, appearance, width, height, head
            );
            image::save_buffer(
                std::path::Path::new(&directory).join(filename),
                &pixels,
                width,
                height,
                image::ColorType::Rgba8,
            )
            .expect("save the synthetic R07b dashboard capture");
        }
    }

    /// A comparable reading of the messages a gesture produced.
    #[derive(Debug, Clone, PartialEq)]
    enum Seen {
        Focus(Id),
        Prepare,
        OpenPicker,
        ClosePicker,
        Target(SparkSendTarget),
        Destination(String),
        Amount(String),
        Other(String),
    }

    fn seen(messages: Vec<Message>) -> Vec<Seen> {
        messages
            .into_iter()
            .map(|message| match message {
                Message::FocusAction(id) => Seen::Focus(id),
                Message::SparkSend(SparkSendMessage::PrepareRequested) => Seen::Prepare,
                Message::SparkSend(SparkSendMessage::OpenReceivePicker) => Seen::OpenPicker,
                Message::SparkSend(SparkSendMessage::CloseReceivePicker) => Seen::ClosePicker,
                Message::SparkSend(SparkSendMessage::SetReceiveTarget(target)) => {
                    Seen::Target(target)
                }
                Message::SparkSend(SparkSendMessage::DestinationInputChanged(value)) => {
                    Seen::Destination(value)
                }
                Message::SparkSend(SparkSendMessage::AmountInputChanged(value)) => {
                    Seen::Amount(value)
                }
                other => Seen::Other(format!("{:?}", other)),
            })
            .collect()
    }

    fn themes() -> [ThemeMode; 2] {
        [ThemeMode::Dark, ThemeMode::Light]
    }

    fn appearance(mode: ThemeMode) -> &'static str {
        match mode {
            ThemeMode::Dark => "dark",
            ThemeMode::Light => "light",
        }
    }

    fn rendered_luma_range(pixels: &[u8], size: Size, bounds: Rectangle) -> u16 {
        let width = size.width as u32;
        let height = size.height as u32;
        let x0 = bounds.x.max(0.0).floor() as u32;
        let y0 = bounds.y.max(0.0).floor() as u32;
        let x1 = (bounds.x + bounds.width).ceil().min(size.width) as u32;
        let y1 = (bounds.y + bounds.height).ceil().min(size.height) as u32;
        assert!(
            x1 > x0 && y1 > y0 && x1 <= width && y1 <= height,
            "text bounds must be visible: {:?} in {:?}",
            bounds,
            size
        );
        let mut min = u16::MAX;
        let mut max = 0;
        for y in y0..y1 {
            for x in x0..x1 {
                let at = ((y * width + x) * 4) as usize;
                let luma = ((u32::from(pixels[at]) * 299
                    + u32::from(pixels[at + 1]) * 587
                    + u32::from(pixels[at + 2]) * 114)
                    / 1000) as u16;
                min = min.min(luma);
                max = max.max(luma);
            }
        }
        max - min
    }

    /// The YOU SEND card's former balance expression, kept verbatim so the
    /// relocated "Available" figure is checked against the original wording.
    fn original_balance_text(balance_sats: u64, bitcoin_unit: BitcoinDisplayUnit) -> String {
        format!(
            "{} {}",
            Amount::from_sat(balance_sats).to_formatted_string_with_unit(bitcoin_unit),
            if matches!(bitcoin_unit, BitcoinDisplayUnit::BTC) {
                "BTC"
            } else {
                "SATS"
            }
        )
    }

    /// Send under its pinned head, at the top and scrolled (Tenshu v175 L3).
    /// Opt-in: `TOP_BAR_CAPTURE_DIR=<dir> cargo test -p coincube-gui --lib
    /// page_head_capture -- --ignored`.
    #[test]
    #[ignore = "writes PNG frames; needs a GPU adapter"]
    fn page_head_capture_send() {
        use crate::app::view::nav::shell::capture_tests;
        let Some(directory) = capture_tests::directory() else {
            return;
        };
        let mut renderer = capture_tests::renderer();
        let menu = spark_send_menu();
        for (appearance, palette) in capture_tests::themes() {
            let mut fixture = Fixture::filled(SparkSendPhase::Idle);
            if appearance == "light" {
                fixture.mode = ThemeMode::Light;
            }
            let cache = Cache {
                cube_name: "Everyday".into(),
                ..fixture.cache()
            };
            capture_tests::save_page_head_frames(
                &mut renderer,
                &directory,
                "send",
                appearance,
                &palette,
                || screen(&fixture, &menu, &cache, false),
            );
        }
    }

    #[test]
    fn the_steps_sit_on_the_head_and_follow_the_phase() {
        assert_eq!(super::send_step(&SparkSendPhase::Idle), 0);
        assert_eq!(super::send_step(&SparkSendPhase::Preparing), 0);
        assert_eq!(super::send_step(&SparkSendPhase::Error("no".into())), 0);
        assert_eq!(super::send_step(&ordinary_prepared()), 1);
        assert_eq!(super::send_step(&SparkSendPhase::Sending), 1);

        let menu = spark_send_menu();
        for fixture in [
            Fixture::filled(SparkSendPhase::Idle),
            Fixture::ordinary_prepared(),
        ] {
            let cache = fixture.cache();
            let mut harness = Harness::new(screen(&fixture, &menu, &cache, false), COMPACT);
            let probe = harness.probe();
            let head = probe.text_bounds("Back to Spark").expect("the head");
            let title = probe.text_bounds("Send").expect("the title");
            for label in crate::app::view::spark::SEND_STEPS {
                // The side panel has its own "Review" link; the step is the
                // one under the head's title row, on the page column.
                assert!(
                    probe.text.iter().any(|(value, bounds)| value == label
                        && bounds.y > head.y
                        && bounds.y < head.y + 120.0
                        && bounds.x >= title.x - 1.0),
                    "{} is on the head's rule: head {:?}",
                    label,
                    head
                );
            }
        }
    }

    /// S4: You send opens the pay-from picker on a bitcoin rail; a
    /// stablecoin send has no choice.
    #[test]
    fn you_send_opens_the_pay_from_picker_on_bitcoin_rails() {
        let menu = spark_send_menu();
        let fixture = Fixture::filled(SparkSendPhase::Idle);
        let cache = fixture.cache();
        let mut harness = Harness::new(screen(&fixture, &menu, &cache, false), COMPACT);
        assert!(harness.probe().bounds(PAY_FROM_ACTION_ID).is_some());
        let mut stablecoin = Fixture::filled(SparkSendPhase::Idle);
        stablecoin.target = SparkSendTarget::Usdt;
        let cache = stablecoin.cache();
        let mut harness = Harness::new(screen(&stablecoin, &menu, &cache, false), COMPACT);
        assert!(harness.probe().bounds(PAY_FROM_ACTION_ID).is_none());
    }

    #[test]
    fn the_pay_from_picker_offers_us_dollar_only_when_it_covers() {
        use crate::app::view::shared::landing::capture;
        use coincube_spark_protocol::WalletAsset;
        let balances = |stable| crate::app::view::spark::SparkBalances {
            btc_sats: 35_000,
            stable,
            fiat_converter: None,
            btc_usd_price: None,
            bitcoin_unit: BitcoinDisplayUnit::Sats,
            masked: false,
        };
        let usdb = coincube_spark_protocol::UsdbBalanceSnapshot {
            token_identifier: "btkn1synthetic".into(),
            balance: 12_400_000,
            decimals: 6,
            ticker: "USDB".into(),
        };
        let short = capture::inspect(
            "spark-send-pay-from-short",
            pay_from_picker_menu(
                WalletAsset::Bitcoin,
                Some("Your USDB doesn't cover this amount."),
                balances(Some(&usdb)),
            ),
            (1440, 900),
            "dark",
            theme::Theme::dark(),
        );
        assert!(short.contains("YOUR SPARK BALANCES") && short.contains("Bitcoin is used first."));
        assert!(short.contains("Your USDB doesn't cover this amount."));
        assert!(!short.has_focus(iced::advanced::widget::Id::new(PAY_FROM_USDB_ID)));
        let picked = capture::activate(
            pay_from_picker_menu(WalletAsset::Bitcoin, None, balances(Some(&usdb))),
            iced::advanced::widget::Id::new(PAY_FROM_USDB_ID),
            (1440, 900),
            true,
        );
        assert!(picked.iter().any(|m| matches!(
            m,
            Message::SparkSend(SparkSendMessage::SelectPayFrom(WalletAsset::Usdb))
        )));
        let covered = capture::inspect(
            "spark-send-pay-from-covered",
            pay_from_picker_menu(WalletAsset::Bitcoin, None, balances(Some(&usdb))),
            (1440, 900),
            "dark",
            theme::Theme::dark(),
        );
        assert!(covered.contains("12.40 USDB") && covered.contains("35,000 SATS"));
        let from_usd = capture::inspect(
            "spark-send-pay-from-usd",
            pay_from_picker_menu(WalletAsset::Usdb, None, balances(Some(&usdb))),
            (1440, 900),
            "dark",
            theme::Theme::dark(),
        );
        assert!(
            from_usd.contains("YOUR SPARK BALANCES")
                && !from_usd.contains("Bitcoin is used first."),
            "the Bitcoin-first note shows only while Bitcoin is selected"
        );
    }

    #[test]
    fn paying_from_usdb_says_so_and_review_shows_the_conversion() {
        use coincube_spark_protocol::{AssetAmount, ConversionQuote, WalletAsset};
        let menu = spark_send_menu();
        let mut fixture = Fixture::filled(SparkSendPhase::Idle);
        fixture.pay_from = WalletAsset::Usdb;
        fixture.usdb_unavailable = None;
        let cache = fixture.cache();
        let mut harness = Harness::new(screen(&fixture, &menu, &cache, false), COMPACT);
        let probe = harness.probe();
        assert!(probe.contains("US Dollar") && probe.contains("Paid from USDB:"));

        let mut review = Fixture::ordinary_prepared();
        review.pay_from = WalletAsset::Usdb;
        review.conversion = Some(ConversionQuote {
            amount_in: AssetAmount {
                asset: WalletAsset::Usdb,
                amount: 12_400_000,
            },
            amount_out: AssetAmount {
                asset: WalletAsset::Bitcoin,
                amount: 15_003,
            },
            fee: AssetAmount {
                asset: WalletAsset::Usdb,
                amount: 12_400,
            },
            amount_adjustment: None,
            max_slippage_bps: 50,
        });
        let cache = review.cache();
        let mut harness = Harness::new(screen(&review, &menu, &cache, false), FULL_HEIGHT);
        let probe = harness.probe();
        assert!(probe.contains("Paid from") && probe.contains("US Dollar (USDB)"));
        assert!(probe.contains("12.40 USDB → 15,003 SATS"));
        // The conversion's own rate, fee and cap, not just the send fee.
        assert!(probe.contains("1 BTC ≈ $82,650.14"));
        assert!(probe.contains("fee 0.01 USDB (0.10%)"));
        assert!(probe.contains("max price change 0.5%"));
        assert!(probe.contains("the conversion is refunded and nothing is sent."));
        // The total is what leaves the wallet: USDB.
        assert!(probe.contains("12.40 USDB") && probe.contains("Total you send"));

        // Paying from bitcoin with a USDB shortfall shows the same rows.
        drop(probe);
        drop(harness);
        review.pay_from = WalletAsset::Bitcoin;
        let cache = review.cache();
        let mut harness = Harness::new(screen(&review, &menu, &cache, false), FULL_HEIGHT);
        let probe = harness.probe();
        assert!(probe.contains("12.40 USDB → 15,003 SATS"));
        assert!(probe.contains("Bitcoin, then US Dollar (USDB)"));
        assert!(probe.contains("Includes 15,003 SATS converted from 12.40 USDB"));
    }

    /// A USDB invoice paid from USDB: Review's figures are in USDB, it is
    /// paid from US Dollar, and there is no conversion to show.
    #[test]
    fn review_of_a_usdb_invoice_shows_usdb_and_no_conversion() {
        use coincube_spark_protocol::{AssetAmount, TokenPayment, WalletAsset};
        let menu = spark_send_menu();
        let mut review = Fixture::ordinary_prepared();
        review.target = SparkSendTarget::Spark;
        review.recipient_lookup_eligible = false;
        review.pay_from = WalletAsset::Usdb;
        if let SparkSendPhase::Prepared(ok) = &mut review.phase {
            ok.method = "SparkInvoice".to_owned();
            ok.amount_sat = 0;
            ok.fee_sat = 0;
            ok.has_token_leg = Some(true);
        }
        let usdb = |amount| AssetAmount {
            asset: WalletAsset::Usdb,
            amount,
        };
        review.token_payment = Some(TokenPayment {
            amount: usdb(2_500_000),
            fee: usdb(10_000),
        });
        let cache = review.cache();
        let mut harness = Harness::new(screen(&review, &menu, &cache, false), FULL_HEIGHT);
        let probe = harness.probe();
        assert!(probe.contains("Paid from") && probe.contains("US Dollar"));
        assert!(probe.contains("2.50 USDB"), "amount in USDB");
        assert!(probe.contains("0.01 USDB"), "fee in USDB");
        assert!(probe.contains("2.51 USDB"), "total in USDB");
        // The preview's sats are zero here; none of them may show as a figure.
        // (The side panel's bitcoin balance does read in sats.)
        assert!(
            !probe.contains("Total you send0 SATS") && !probe.contains(" 0 SATS"),
            "no figure reads as sats"
        );
        assert!(!probe.contains("Conversion") && !probe.contains("max price change"));
    }

    #[test]
    fn the_side_panel_lists_bitcoin_and_us_dollar() {
        let mut fixture = Fixture::filled(SparkSendPhase::Idle);
        fixture.stable = Some(coincube_spark_protocol::UsdbBalanceSnapshot {
            token_identifier: "btkn1synthetic".into(),
            balance: 12_400_000,
            decimals: 6,
            ticker: "USDB".into(),
        });
        let menu = spark_send_menu();
        let cache = fixture.cache();
        let mut harness = Harness::new(screen(&fixture, &menu, &cache, false), COMPACT);
        let probe = harness.probe();
        for expected in [
            "Available in Spark",
            "Bitcoin",
            "US Dollar",
            "12.40",
            "USDB",
        ] {
            assert!(probe.contains(expected), "missing {:?}", expected);
        }
    }

    #[test]
    fn masked_payment_card_hides_available_balance_and_conversion_hint() {
        let mut fixture = Fixture::filled(SparkSendPhase::Idle);
        fixture.mask_amounts = true;
        let menu = spark_send_menu();
        let cache = fixture.cache();
        let mut harness = Harness::new(screen(&fixture, &menu, &cache, false), COMPACT);
        let probe = harness.probe();
        assert!(probe.contains("••••••"));
        assert!(!probe.contains(&original_balance_text(
            SYNTHETIC_BALANCE_SATS,
            BitcoinDisplayUnit::Sats,
        )));
        assert!(!probe.contains(SYNTHETIC_HINT));
        assert!(probe.bounds(RECEIVE_TARGET_ACTION_ID).is_some());
    }

    #[test]
    fn compose_states_render_inside_the_dashboard_at_compact_wide_and_full_height() {
        let menu = spark_send_menu();
        for mode in themes() {
            let cases = [
                ("empty", Fixture::new(SparkSendPhase::Idle).themed(mode)),
                ("filled", Fixture::filled(SparkSendPhase::Idle).themed(mode)),
                (
                    "invoice",
                    Fixture::invoice(SparkSendPhase::Idle).themed(mode),
                ),
                (
                    "preparing",
                    Fixture::filled(SparkSendPhase::Preparing).themed(mode),
                ),
            ];
            for (scenario, fixture) in &cases {
                let cache = fixture.cache();
                let theme = fixture.theme();
                let mut sizes = vec![COMPACT, WIDE];
                if *scenario != "empty" {
                    sizes.push(FULL_HEIGHT);
                }
                for outer in sizes {
                    let mut harness = Harness::new(screen(fixture, &menu, &cache, false), outer);
                    let label = format!("{} {} {:?}", scenario, appearance(mode), outer);
                    harness.assert_no_horizontal_overflow(&label);
                    let probe = harness.probe();
                    for expected in [
                        "Back to Spark",
                        "Send",
                        "Payment details",
                        "Available in Spark",
                        "You send",
                        "They receive",
                        "Destination",
                        "Amount (sats)",
                        "Available",
                    ] {
                        assert!(
                            probe.contains(expected),
                            "{}: missing {:?}",
                            label,
                            expected
                        );
                    }
                    assert!(
                        probe.contains(&original_balance_text(
                            SYNTHETIC_BALANCE_SATS,
                            BitcoinDisplayUnit::Sats
                        )),
                        "{}: the available balance must keep the original formatting",
                        label
                    );
                    assert!(
                        !probe.contains("Add a destination"),
                        "{}: the Destination field below needs no prompt on the tile",
                        label
                    );
                    assert!(
                        probe.input_text(DESTINATION_INPUT_ID).is_some(),
                        "{}: native destination input",
                        label
                    );
                    // No Bitcoin / Stablecoin toggle: THEY RECEIVE's picker
                    // holds every target.
                    for id in ["spark-send-kind-bitcoin", "spark-send-kind-stablecoin"] {
                        assert!(
                            probe.bounds(id).is_none(),
                            "{}: toggle back at {}",
                            label,
                            id
                        );
                    }
                    assert!(
                        probe.input_text(AMOUNT_INPUT_ID).is_some(),
                        "{}: native amount input",
                        label
                    );
                    let preparing = matches!(fixture.phase, SparkSendPhase::Preparing);
                    assert_eq!(
                        probe.bounds(PREPARE_ACTION_ID).is_some(),
                        !preparing,
                        "{}: Prepare is a focus target exactly when it is pressable",
                        label
                    );
                    assert_eq!(
                        probe.contains("Preparing send… asking the Spark bridge for a fee quote."),
                        preparing,
                        "{}: the existing preparation message",
                        label
                    );
                    assert!(
                        probe.contains("Prepare"),
                        "{}: the Prepare control is always drawn while composing",
                        label
                    );
                    for absent in ["Confirm and send", "Send again", "Try again", "Get quote"] {
                        assert!(
                            !probe.contains(absent),
                            "{}: compose must not offer {:?}",
                            label,
                            absent
                        );
                    }
                    assert_eq!(
                        probe.contains("This invoice sets its own amount."),
                        fixture.invoice_amount,
                        "{}: invoice note",
                        label
                    );
                    assert_eq!(
                        probe.contains("the rest is converted"),
                        fixture.hint.is_some(),
                        "{}: conversion hint",
                        label
                    );
                    harness.capture(scenario, &theme, appearance(mode));
                }
            }
        }
    }

    #[test]
    fn owned_actions_send_one_exact_intent_by_pointer_enter_and_space() {
        let menu = spark_send_menu();
        let fixture = Fixture::filled(SparkSendPhase::Idle);
        let cache = fixture.cache();
        for (id, intent) in [
            (PREPARE_ACTION_ID, Seen::Prepare),
            (RECEIVE_TARGET_ACTION_ID, Seen::OpenPicker),
        ] {
            // A pointer needs the control on screen without scrolling; the
            // keyboard path is checked at the compact minimum as well.
            for outer in [WIDE, FULL_HEIGHT] {
                let mut harness = Harness::new(screen(&fixture, &menu, &cache, false), outer);
                assert_eq!(
                    seen(harness.click(id)),
                    vec![Seen::Focus(Id::new(id)), intent.clone()],
                    "{} pointer at {:?}",
                    id,
                    outer
                );
            }
            for outer in [COMPACT, WIDE] {
                let mut harness = Harness::new(screen(&fixture, &menu, &cache, false), outer);
                harness.focus(id);
                assert_eq!(harness.probe().focused(), vec![Id::new(id)]);
                for named in [keyboard::key::Named::Enter, keyboard::key::Named::Space] {
                    assert_eq!(
                        seen(harness.key(named, true, false)),
                        vec![intent.clone()],
                        "{} {:?} press",
                        id,
                        named
                    );
                    assert!(
                        harness.key(named, true, true).is_empty(),
                        "{} {:?} auto-repeat must not repeat the intent",
                        id,
                        named
                    );
                    assert!(
                        harness.key(named, false, false).is_empty(),
                        "{} {:?} release",
                        id,
                        named
                    );
                }
            }
        }
    }

    fn with(mut fixture: Fixture, edit: impl FnOnce(&mut Fixture)) -> Fixture {
        edit(&mut fixture);
        fixture
    }

    #[test]
    fn prepare_held_across_preparing_never_fires_on_release() {
        let menu = spark_send_menu();
        let idle = Fixture::filled(SparkSendPhase::Idle);
        let preparing = Fixture::filled(SparkSendPhase::Preparing);
        let idle_cache = idle.cache();
        let preparing_cache = preparing.cache();

        // Keyboard: Enter fires once on press; while held, the phase moves to
        // Preparing (no press, no focus target) and back to Idle.
        let mut harness = Harness::new(screen(&idle, &menu, &idle_cache, false), WIDE);
        harness.focus(PREPARE_ACTION_ID);
        assert_eq!(
            seen(harness.key(keyboard::key::Named::Enter, true, false)),
            vec![Seen::Prepare]
        );
        harness.rerender(screen(&preparing, &menu, &preparing_cache, false));
        let probe = harness.probe();
        assert!(probe.bounds(PREPARE_ACTION_ID).is_none());
        assert!(
            probe.focused().is_empty(),
            "nothing keeps focus on a loading Prepare"
        );
        assert!(harness
            .key(keyboard::key::Named::Enter, true, false)
            .is_empty());
        harness.rerender(screen(&idle, &menu, &idle_cache, false));
        assert!(harness
            .key(keyboard::key::Named::Enter, false, false)
            .is_empty());
        assert!(harness.probe().focused().is_empty());

        // Pointer: a press that began on Idle and is released after a
        // Preparing pass must not complete as a click.
        let mut harness = Harness::new(screen(&idle, &menu, &idle_cache, false), WIDE);
        let center = harness.probe().bounds(PREPARE_ACTION_ID).unwrap().center();
        assert_eq!(
            seen(harness.press_at(center)),
            vec![Seen::Focus(Id::new(PREPARE_ACTION_ID))]
        );
        harness.rerender(screen(&preparing, &menu, &preparing_cache, false));
        harness.rerender(screen(&idle, &menu, &idle_cache, false));
        assert!(harness.release_at(center).is_empty());
    }

    #[test]
    fn native_inputs_edit_through_their_callbacks_and_keep_state_across_slots() {
        let menu = spark_send_menu();
        let empty = Fixture::new(SparkSendPhase::Idle);
        let cache = empty.cache();
        // The replacement view must outlive the harness that borrows it.
        let mut changed = Fixture::invoice(SparkSendPhase::Idle);
        changed.target = SparkSendTarget::Spark;
        let changed_cache = changed.cache();
        let mut harness = Harness::new(screen(&empty, &menu, &cache, false), COMPACT);

        // A pointer press on each field reaches the field itself: no parent
        // hit target covers it.
        let probe = harness.probe();
        for id in [DESTINATION_INPUT_ID, AMOUNT_INPUT_ID] {
            let center = probe
                .inputs
                .iter()
                .find_map(|(actual, bounds, _)| {
                    (actual.as_ref() == Some(&Id::new(id))).then_some(bounds.center())
                })
                .unwrap();
            assert!(harness.press_at(center).is_empty());
            harness.release_at(center);
            assert_eq!(harness.probe().focused(), vec![Id::new(id)], "{}", id);
        }

        harness.focus(DESTINATION_INPUT_ID);
        assert_eq!(
            seen(harness.type_text("l")),
            vec![Seen::Destination("l".to_string())]
        );
        harness.focus(AMOUNT_INPUT_ID);
        assert_eq!(
            seen(harness.type_text("5")),
            vec![Seen::Amount("5".to_string())]
        );

        // Pointer focus on an Action blurs the field through the same
        // whole-tree focus App applies for `FocusAction`.
        harness.focus(DESTINATION_INPUT_ID);
        let messages = seen(harness.click(RECEIVE_TARGET_ACTION_ID));
        assert_eq!(
            messages,
            vec![
                Seen::Focus(Id::new(RECEIVE_TARGET_ACTION_ID)),
                Seen::OpenPicker
            ]
        );
        harness.focus(RECEIVE_TARGET_ACTION_ID);
        assert_eq!(
            harness.probe().focused(),
            vec![Id::new(RECEIVE_TARGET_ACTION_ID)]
        );

        // Notes, hints and a new target appear without moving the inputs'
        // slots, so a focused field stays focused and keeps its value.
        harness.focus(DESTINATION_INPUT_ID);
        harness.rerender(screen(&changed, &menu, &changed_cache, false));
        let probe = harness.probe();
        assert_eq!(probe.focused(), vec![Id::new(DESTINATION_INPUT_ID)]);
        assert_eq!(
            probe.input_text(DESTINATION_INPUT_ID).as_deref(),
            Some(SYNTHETIC_INVOICE)
        );
        assert_eq!(probe.input_text(AMOUNT_INPUT_ID).as_deref(), Some("1500"));
        drop(harness);

        // An invoice-set amount shows and selects but does not edit.
        let invoice = Fixture::invoice(SparkSendPhase::Idle);
        let invoice_cache = invoice.cache();
        let mut harness = Harness::new(screen(&invoice, &menu, &invoice_cache, false), WIDE);
        harness.focus(AMOUNT_INPUT_ID);
        assert!(harness.type_text("9").is_empty());
        let probe = harness.probe();
        assert_eq!(probe.input_text(AMOUNT_INPUT_ID).as_deref(), Some("1500"));
        assert!(probe.contains("This invoice sets its own amount."));
    }

    #[test]
    fn amount_label_and_placeholder_follow_the_display_unit_and_target() {
        let menu = spark_send_menu();
        for (unit, target, label, placeholder) in [
            (
                BitcoinDisplayUnit::Sats,
                SparkSendTarget::Lightning,
                "Amount (sats)",
                "Amount in sats (optional for invoices with amount)",
            ),
            (
                BitcoinDisplayUnit::BTC,
                SparkSendTarget::OnChain,
                "Amount (BTC)",
                "Amount in BTC (optional for invoices with amount)",
            ),
            (
                BitcoinDisplayUnit::Sats,
                SparkSendTarget::Usdt,
                "Amount (sats)",
                "Amount in sats — funded from your Bitcoin balance",
            ),
            (
                BitcoinDisplayUnit::BTC,
                SparkSendTarget::Usdc,
                "Amount (BTC)",
                "Amount in BTC — funded from your Bitcoin balance",
            ),
        ] {
            let mut fixture = Fixture::new(SparkSendPhase::Idle);
            fixture.unit = unit;
            fixture.target = target;
            let cache = fixture.cache();
            let mut harness = Harness::new(screen(&fixture, &menu, &cache, false), COMPACT);
            let probe = harness.probe();
            assert!(probe.contains(label), "{:?} {:?}", unit, target);
            assert_eq!(
                probe.input_text(AMOUNT_INPUT_ID).as_deref(),
                Some(placeholder),
                "an empty field shows its placeholder"
            );
            assert_eq!(
                probe.input_text(DESTINATION_INPUT_ID).as_deref(),
                Some(target.destination_placeholder())
            );
            assert!(probe.contains(&original_balance_text(SYNTHETIC_BALANCE_SATS, unit)));
        }
    }

    #[test]
    fn picker_offers_the_real_rails_per_network_with_semantic_targets() {
        let menu = spark_send_menu();
        for (network, offered) in [
            (Network::Bitcoin, SparkSendTarget::all().to_vec()),
            (
                Network::Testnet,
                vec![
                    SparkSendTarget::Lightning,
                    SparkSendTarget::OnChain,
                    SparkSendTarget::Spark,
                ],
            ),
            (
                Network::Signet,
                vec![
                    SparkSendTarget::Lightning,
                    SparkSendTarget::OnChain,
                    SparkSendTarget::Spark,
                ],
            ),
            (
                Network::Regtest,
                vec![
                    SparkSendTarget::Lightning,
                    SparkSendTarget::OnChain,
                    SparkSendTarget::Spark,
                ],
            ),
        ] {
            let mut fixture = Fixture::new(SparkSendPhase::Idle);
            fixture.network = network;
            let cache = fixture.cache();
            let mut harness = Harness::new(screen(&fixture, &menu, &cache, true), COMPACT);
            let probe = harness.probe();
            let picker: Vec<Id> = probe
                .action_ids()
                .into_iter()
                .filter(|id| {
                    SparkSendTarget::all()
                        .iter()
                        .any(|target| *id == Id::new(target_action_id(*target)))
                })
                .collect();
            let expected: Vec<Id> = offered
                .iter()
                .map(|target| Id::new(target_action_id(*target)))
                .collect();
            assert_eq!(picker, expected, "{:?}: offered rails, in order", network);

            // Adjacent rows leave the four-pixel ring outset clear.
            let rows: Vec<Rectangle> = offered
                .iter()
                .map(|target| probe.bounds(target_action_id(*target)).unwrap())
                .collect();
            for pair in rows.windows(2) {
                assert!(
                    pair[1].y - (pair[0].y + pair[0].height) >= 4.0,
                    "{:?}: rows {:?} and {:?} would paint over a focus ring",
                    network,
                    pair[0],
                    pair[1]
                );
            }

            for target in &offered {
                let id = target_action_id(*target);
                let mut harness = Harness::new(screen(&fixture, &menu, &cache, true), COMPACT);
                assert_eq!(
                    seen(harness.click(id)),
                    vec![Seen::Focus(Id::new(id)), Seen::Target(*target)],
                    "{:?} pointer on {}",
                    network,
                    id
                );
                harness.focus(id);
                assert_eq!(
                    seen(harness.key(keyboard::key::Named::Enter, true, false)),
                    vec![Seen::Target(*target)],
                    "{:?} Enter on {}",
                    network,
                    id
                );
                harness.key(keyboard::key::Named::Enter, false, false);
            }

            // Escape closes the menu from its rows; there is no Close button.
            let mut harness = Harness::new(screen(&fixture, &menu, &cache, true), COMPACT);
            harness.focus(target_action_id(offered[0]));
            assert_eq!(
                seen(harness.key(keyboard::key::Named::Escape, true, false)),
                vec![Seen::ClosePicker]
            );
        }
    }

    /// On a window too short for the whole THEY RECEIVE menu, the menu stops
    /// above the window's bottom edge and scrolls instead of running off it.
    #[test]
    fn the_receive_menu_scrolls_in_a_short_window() {
        const SHORT: (u32, u32) = (1024, 600);
        let menu = spark_send_menu();
        for mode in themes() {
            let fixture = Fixture::new(SparkSendPhase::Idle).themed(mode);
            let cache = fixture.cache();
            let theme = fixture.theme();
            let mut harness = Harness::new(screen(&fixture, &menu, &cache, true), SHORT);
            // Laid out in full, the last rail would sit below the window.
            let usdc = harness
                .probe()
                .bounds(target_action_id(SparkSendTarget::Usdc))
                .expect("USDC row bounds");
            assert!(usdc.y + usdc.height > SHORT.1 as f32);
            // The menu's frame is drawn inside the window: its bottom line
            // sits above the bottom margin, under the menu's own column.
            let pixels = harness.render_pixels("picker-short", &theme, true);
            let column = Rectangle::new(
                Point::new(usdc.x, SHORT.1 as f32 - 4.0),
                Size::new(usdc.width, 4.0),
            );
            assert!(
                rendered_luma_range(&pixels, harness.size, column) < 12,
                "{}: the menu must stop short of the window's bottom edge",
                appearance(mode)
            );
            harness.capture("picker-short", &theme, appearance(mode));
        }
    }

    #[test]
    fn a_changed_or_filtered_picker_cannot_activate_another_rail() {
        let menu = spark_send_menu();
        let mainnet = Fixture::new(SparkSendPhase::Idle);
        let mainnet_cache = mainnet.cache();

        // The focused rail disappears (network without cross-chain): the held
        // focus does not slide onto whichever row now sits in its place.
        let mut regtest = Fixture::new(SparkSendPhase::Idle);
        regtest.network = Network::Regtest;
        let regtest_cache = regtest.cache();
        let usdt = target_action_id(SparkSendTarget::Usdt);
        let mut harness = Harness::new(screen(&mainnet, &menu, &mainnet_cache, true), WIDE);
        harness.focus(usdt);
        harness.rerender(screen(&regtest, &menu, &regtest_cache, true));
        assert!(harness.probe().focused().is_empty());
        assert!(harness
            .key(keyboard::key::Named::Enter, true, false)
            .is_empty());

        // The selection moves to the focused rail: focus stays on that rail
        // and Enter still names exactly it.
        let spark = target_action_id(SparkSendTarget::Spark);
        let mut selected = Fixture::new(SparkSendPhase::Idle);
        selected.target = SparkSendTarget::Spark;
        let selected_cache = selected.cache();
        let mut harness = Harness::new(screen(&mainnet, &menu, &mainnet_cache, true), WIDE);
        harness.focus(spark);
        harness.rerender(screen(&selected, &menu, &selected_cache, true));
        assert_eq!(harness.probe().focused(), vec![Id::new(spark)]);
        assert_eq!(
            seen(harness.key(keyboard::key::Named::Enter, true, false)),
            vec![Seen::Target(SparkSendTarget::Spark)]
        );
    }

    #[test]
    fn picker_captures_show_selected_and_focused_rails_in_both_themes() {
        let menu = spark_send_menu();
        for mode in themes() {
            for (scenario, network, target, focused) in [
                (
                    "picker-mainnet",
                    Network::Bitcoin,
                    SparkSendTarget::Lightning,
                    SparkSendTarget::Usdc,
                ),
                (
                    "picker-testnet",
                    Network::Testnet,
                    SparkSendTarget::Spark,
                    SparkSendTarget::OnChain,
                ),
            ] {
                let mut fixture = Fixture::new(SparkSendPhase::Idle).themed(mode);
                fixture.network = network;
                fixture.target = target;
                let cache = fixture.cache();
                let theme = fixture.theme();
                let mut harness = Harness::new(screen(&fixture, &menu, &cache, true), WIDE);
                harness.focus(target_action_id(focused));
                let probe = harness.probe();
                assert_eq!(probe.focused(), vec![Id::new(target_action_id(focused))]);
                assert!(probe.contains("BITCOIN"));
                harness.assert_no_horizontal_overflow(scenario);
                // The menu is opaque over the form under the tiles, so the
                // focused row reads on its own surface.
                let row = probe
                    .bounds(target_action_id(focused))
                    .expect("focused rail row bounds");
                let pixels = harness.render_pixels(scenario, &theme, false);
                assert!(
                    rendered_luma_range(&pixels, harness.size, row) >= 40,
                    "{} {}: the focused row must read on the menu surface",
                    scenario,
                    appearance(mode)
                );
                harness.capture(scenario, &theme, appearance(mode));
            }
        }
    }

    fn ordinary_prepared() -> SparkSendPhase {
        SparkSendPhase::Prepared(PrepareSendOk {
            handle: "synthetic-handle".to_string(),
            amount_sat: 1_500,
            fee_sat: 3,
            method: "Bolt11Invoice".to_string(),
            cross_chain: None,
            has_token_leg: Some(false),
        })
    }

    fn cross_chain_prepared() -> SparkSendPhase {
        SparkSendPhase::Prepared(PrepareSendOk {
            handle: "synthetic-handle".to_string(),
            amount_sat: 30_000,
            fee_sat: 0,
            method: "CrossChainAddress".to_string(),
            cross_chain: Some(Box::new(CrossChainQuote {
                route: CrossChainRoute {
                    provider: "synthetic-provider".to_string(),
                    chain: "synthetic-chain".to_string(),
                    chain_id: None,
                    asset: "USDC".to_string(),
                    contract_address: None,
                    decimals: 6,
                    btc_source_supported: true,
                    btc_limits: None,
                },
                estimated_out: 19_000_000,
                fee_amount: 97_921,
                source_transfer_fee_sats: 120,
                expires_at: "0".to_string(),
                retry_safe: true,
            })),
            has_token_leg: Some(true),
        })
    }

    #[test]
    fn synthetic_prepared_facts_match_the_quoted_scenarios() {
        let ordinary = Fixture::ordinary_prepared();
        let SparkSendPhase::Prepared(ordinary_quote) = &ordinary.phase else {
            panic!("ordinary fixture must be prepared");
        };
        assert_eq!(
            ordinary.amount.parse::<u64>(),
            Ok(ordinary_quote.amount_sat)
        );
        assert_eq!(ordinary_quote.method, "Bolt11Invoice");
        assert_eq!(ordinary.target, SparkSendTarget::Lightning);
        assert_eq!(ordinary.destination, SYNTHETIC_INVOICE);
        assert!(ordinary.destination.starts_with("lnbc15u1"));
        assert_eq!(ordinary_quote.amount_sat, 15 * 100); // 1 μBTC = 100 sats.
        assert!(ordinary.invoice_amount);
        assert!(ordinary.hint.is_none());
        assert!(ordinary_quote.amount_sat + ordinary_quote.fee_sat < SYNTHETIC_BITCOIN_SATS);

        let mismatch = with(Fixture::ordinary_prepared(), |f| f.mismatch = true);
        assert_eq!(mismatch.amount, ordinary.amount);
        assert_eq!(mismatch.destination, ordinary.destination);
        assert_eq!(mismatch.target, ordinary.target);
        assert!(mismatch.hint.is_none());

        let cross_chain = Fixture::cross_chain_prepared();
        let SparkSendPhase::Prepared(cross_chain_quote) = &cross_chain.phase else {
            panic!("cross-chain fixture must be prepared");
        };
        assert_eq!(
            cross_chain.amount.parse::<u64>(),
            Ok(cross_chain_quote.amount_sat)
        );
        assert_eq!(cross_chain_quote.method, "CrossChainAddress");
        assert_eq!(cross_chain.target, SparkSendTarget::Usdc);
        assert_eq!(cross_chain.destination, SYNTHETIC_CROSS_CHAIN_ADDRESS);
        assert_eq!(
            cross_chain_quote.cross_chain.as_ref().unwrap().route.asset,
            "USDC"
        );
        assert!(cross_chain.hint.as_deref().unwrap().contains("25,053 sats"));
        assert!(SYNTHETIC_BITCOIN_SATS < cross_chain_quote.amount_sat);
        assert!(cross_chain_quote.amount_sat < SYNTHETIC_BALANCE_SATS);
    }

    fn outcome_unknown(
        checking: bool,
        outcome: Option<ReconcileOutcome>,
        guard: RetryGuard,
    ) -> SparkSendPhase {
        SparkSendPhase::OutcomeUnknown {
            message: "Synthetic: the bridge did not answer.".to_string(),
            outcome,
            checking,
            guard,
        }
    }

    #[test]
    fn protected_phase_bodies_still_render_below_the_card_unchanged() {
        let menu = spark_send_menu();
        struct Case {
            scenario: &'static str,
            fixture: Fixture,
            present: &'static [&'static str],
            absent: &'static [&'static str],
        }
        let cases = [
            Case {
                scenario: "prepared",
                fixture: Fixture::ordinary_prepared(),
                present: &[
                    "Review and confirm",
                    "Lightning invoice",
                    "Total you send",
                    "Recipient checks",
                    "Confirm and send",
                    "Back",
                ],
                absent: &[
                    "Get a new quote",
                    "Bolt11Invoice",
                    "Payment details",
                    "Cancel",
                ],
            },
            Case {
                scenario: "mismatch",
                fixture: with(Fixture::ordinary_prepared(), |f| f.mismatch = true),
                present: &[
                    crate::services::branta::MISMATCH_MESSAGE,
                    "Replace payment request",
                ],
                absent: &[
                    "Confirm and send",
                    "Total you send",
                    "Payment details",
                    "After this payment",
                ],
            },
            Case {
                scenario: "quote-valid",
                fixture: with(Fixture::cross_chain_prepared(), |f| {
                    f.countdown = Some(cross_chain::QuoteCountdown::Valid { seconds_left: 42 })
                }),
                present: &[
                    "Review and confirm",
                    "Valid for 0:42",
                    "Price may move up to",
                    "Confirm and send",
                    "Total you send",
                    "Back",
                ],
                // One network, so nothing to acknowledge; and no Branta row
                // or old preview labels on a cross-chain quote.
                absent: &[
                    "Get a new quote",
                    "Source Bitcoin network",
                    "Recipient checks",
                    "The recipient uses",
                    "Payment details",
                ],
            },
            Case {
                scenario: "quote-expired",
                fixture: with(Fixture::cross_chain_prepared(), |f| {
                    f.countdown = Some(cross_chain::QuoteCountdown::Expired)
                }),
                present: &["This quote has expired.", "Get a new quote"],
                absent: &["Confirm and send"],
            },
            Case {
                scenario: "quote-unknown",
                fixture: with(Fixture::cross_chain_prepared(), |f| {
                    f.countdown = Some(cross_chain::QuoteCountdown::Unknown)
                }),
                present: &["Get a new quote"],
                absent: &["Confirm and send"],
            },
            Case {
                scenario: "outcome-unknown-checking",
                fixture: Fixture::filled(outcome_unknown(true, None, RetryGuard::IdempotencyKey)),
                present: &[
                    "Payment status unconfirmed",
                    "Don't send it again until this finishes.",
                    "View transactions",
                ],
                absent: &["Send again", "Check again", "Try again"],
            },
            Case {
                scenario: "outcome-unknown-inconclusive",
                fixture: Fixture::filled(outcome_unknown(
                    false,
                    Some(ReconcileOutcome::Inconclusive("synthetic".to_string())),
                    RetryGuard::IdempotencyKey,
                )),
                present: &["Check again", "View transactions"],
                absent: &["Send again", "Try again"],
            },
            Case {
                scenario: "outcome-unknown-no-trace-guarded",
                fixture: Fixture::filled(outcome_unknown(
                    false,
                    Some(ReconcileOutcome::NoTrace),
                    RetryGuard::IdempotencyKey,
                )),
                present: &["Send again", "Check again", "View transactions"],
                absent: &["Try again"],
            },
            Case {
                scenario: "outcome-unknown-no-trace-unguarded",
                fixture: Fixture::filled(outcome_unknown(
                    false,
                    Some(ReconcileOutcome::NoTrace),
                    RetryGuard::None,
                )),
                present: &["Check again", "View transactions"],
                absent: &["Send again", "Try again"],
            },
        ];
        for case in &cases {
            let cache = case.fixture.cache();
            for outer in [COMPACT, WIDE] {
                let mut harness = Harness::new(screen(&case.fixture, &menu, &cache, false), outer);
                let label = format!("{} {:?}", case.scenario, outer);
                let probe = harness.probe();
                for text in case.present {
                    assert!(probe.contains(text), "{}: missing {:?}", label, text);
                }
                for text in case.absent {
                    assert!(!probe.contains(text), "{}: unexpected {:?}", label, text);
                }
                // The compose footer belongs to Idle/Preparing only: no
                // Prepare control and no Prepare status line here.
                assert!(probe.bounds(PREPARE_ACTION_ID).is_none(), "{}", label);
                assert!(
                    !probe.contains("Prepare gets a fee quote."),
                    "{}: compose footer leaked into a later phase",
                    label
                );
                // A prepared quote is Review, which replaces Payment details.
                assert_eq!(
                    probe.contains("Payment details"),
                    !is_review(&case.fixture.phase),
                    "{}",
                    label
                );
                harness.assert_no_horizontal_overflow(&label);
            }
        }

        // The mismatch replaces the compose footer too, whatever the phase.
        let mismatch_idle = with(Fixture::filled(SparkSendPhase::Idle), |f| f.mismatch = true);
        let cache = mismatch_idle.cache();
        let mut harness = Harness::new(screen(&mismatch_idle, &menu, &cache, false), WIDE);
        let probe = harness.probe();
        assert!(probe.contains("Replace payment request"));
        assert!(probe.bounds(PREPARE_ACTION_ID).is_none());

        // Backend unavailable keeps its early return: no card at all.
        let unavailable = with(Fixture::new(SparkSendPhase::Idle), |f| {
            f.backend_available = false
        });
        let cache = unavailable.cache();
        let mut harness = Harness::new(screen(&unavailable, &menu, &cache, false), WIDE);
        let probe = harness.probe();
        assert!(probe.contains("Spark is not available for this cube."));
        assert!(!probe.contains("Payment details"));
        assert!(probe.bounds(PREPARE_ACTION_ID).is_none());
    }

    /// Review paid from US Dollar with the recipient identified: the
    /// conversion and the identity card render, and Back and Confirm each
    /// send their one intent.
    #[test]
    fn review_captures_and_actions_in_both_themes() {
        use coincube_spark_protocol::{AssetAmount, ConversionQuote, WalletAsset};
        let menu = spark_send_menu();
        let ticket = crate::services::branta::test_ticket(true);
        for mode in themes() {
            let mut review = Fixture::ordinary_prepared().themed(mode);
            review.pay_from = WalletAsset::Usdb;
            review.conversion = Some(ConversionQuote {
                amount_in: AssetAmount {
                    asset: WalletAsset::Usdb,
                    amount: 1_240_000,
                },
                amount_out: AssetAmount {
                    asset: WalletAsset::Bitcoin,
                    amount: 1_510,
                },
                fee: AssetAmount {
                    asset: WalletAsset::Usdb,
                    amount: 1_240,
                },
                amount_adjustment: None,
                max_slippage_bps: 50,
            });
            review.stable = Some(coincube_spark_protocol::UsdbBalanceSnapshot {
                token_identifier: "btkn1synthetic".to_owned(),
                balance: 9_000_000,
                decimals: 6,
                ticker: "USDB".to_owned(),
            });
            review.identities = vec![crate::services::branta::test_identity(
                &ticket,
                "Example Coffee Co.",
            )];
            let cache = review.cache();
            let theme = review.theme();
            let mut harness = Harness::new(screen(&review, &menu, &cache, false), FULL_HEIGHT);
            let probe = harness.probe();
            for text in [
                "Review and confirm",
                "1.24 USDB",
                "Converts to 1,510 SATS",
                "Example Coffee Co.",
                "Identified by Branta",
                "Recipient checks",
                "After this payment, estimated",
                "35,000 SATS → 35,007 SATS",
                "9.00 USDB → 7.76 USDB",
            ] {
                assert!(probe.contains(text), "missing {:?}", text);
            }
            assert!(!probe.contains("Verified"));
            drop(probe);
            harness.assert_no_horizontal_overflow("review-usdb-identified");
            harness.capture("review-usdb-identified", &theme, appearance(mode));

            assert!(harness.click(REVIEW_BACK_ID).iter().any(|message| matches!(
                message,
                Message::SparkSend(SparkSendMessage::EditDetails)
            )));
            assert!(harness
                .click(REVIEW_CONFIRM_ID)
                .iter()
                .any(|message| matches!(
                    message,
                    Message::SparkSend(SparkSendMessage::ConfirmRequested)
                )));
            assert!(harness
                .click(RECIPIENT_IDENTITY_ID)
                .iter()
                .any(|message| matches!(
                    message,
                    Message::SparkSend(SparkSendMessage::OpenRecipientIdentity(0))
                )));
        }
    }

    #[test]
    fn later_phase_captures_in_both_themes() {
        let menu = spark_send_menu();
        for mode in themes() {
            let mut mismatch = Fixture::ordinary_prepared().themed(mode);
            mismatch.mismatch = true;
            for (scenario, fixture) in [
                ("prepared", Fixture::ordinary_prepared().themed(mode)),
                ("mismatch", mismatch),
                (
                    "outcome-unknown-checking",
                    Fixture::filled(outcome_unknown(true, None, RetryGuard::IdempotencyKey))
                        .themed(mode),
                ),
            ] {
                let cache = fixture.cache();
                let theme = fixture.theme();
                let mut harness = Harness::new(screen(&fixture, &menu, &cache, false), WIDE);
                harness.assert_no_horizontal_overflow(scenario);
                harness.capture(scenario, &theme, appearance(mode));
            }
        }
    }
}
