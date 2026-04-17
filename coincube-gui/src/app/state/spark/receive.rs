//! Real Spark Receive panel — Phase 4c.
//!
//! State machine (per picked method):
//!
//! ```text
//! Idle { method } ──(Generate)──▶ Generating { method }
//!                                       │
//!                                  ┌────┴─────┐
//!                                  ▼          ▼
//!                          Generated(ok)   Error(msg)
//!                                  │          │
//!                                  └── Reset ─┘
//!                                  ▼
//!                               Idle { method }
//! ```
//!
//! The user picks a method (BOLT11 Lightning / on-chain Bitcoin),
//! optionally fills in amount + description for BOLT11, clicks
//! Generate, sees the result as a copyable text string. QR codes,
//! Lightning Address display, and the on-chain claim lifecycle all
//! land in Phase 4d.

use std::sync::Arc;

use coincube_spark_protocol::{DepositInfo, ReceivePaymentOk};
use coincube_ui::widget::Element;
use iced::{widget::qr_code, Task};

use crate::app::cache::Cache;
use crate::app::menu::Menu;
use crate::app::message::Message;
use crate::app::state::State;
use crate::app::view::spark::SparkReceiveView;
use crate::app::wallets::SparkBackend;

/// Which receive flow the user has picked.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SparkReceiveMethod {
    Bolt11,
    OnchainBitcoin,
}

impl SparkReceiveMethod {
    pub fn label(self) -> &'static str {
        match self {
            Self::Bolt11 => "Lightning (BOLT11)",
            Self::OnchainBitcoin => "On-chain Bitcoin",
        }
    }
}

#[derive(Debug, Clone)]
pub enum SparkReceivePhase {
    /// User is picking/configuring a method.
    Idle,
    /// `receive_bolt11` or `receive_onchain` RPC is in flight.
    Generating,
    /// RPC succeeded — `payment_request` is the copyable result.
    Generated(ReceivePaymentOk),
    /// A `PaymentSucceeded` event arrived while the panel was in
    /// `Generated` state. Carries the amount from the event so the
    /// confirmation screen can show it directly (saves a follow-up
    /// `list_payments` round-trip).
    ///
    /// Phase 4d treats "any payment succeeded while an invoice is
    /// on screen" as matching the displayed invoice. That's wrong in
    /// the edge case where multiple channels settle at once, but
    /// it's the simplest MVP. Phase 4e can correlate via the
    /// payment's bolt11 field when we plumb richer event payloads.
    Received { amount_sat: i64 },
    /// RPC failed — user-visible error.
    Error(String),
}

/// Real Spark Receive panel.
pub struct SparkReceive {
    backend: Option<Arc<SparkBackend>>,
    /// Currently selected method. Toggling methods resets the phase.
    pub method: SparkReceiveMethod,
    /// Amount input for BOLT11. Ignored for on-chain.
    pub amount_input: String,
    /// Invoice description shown to the payer for BOLT11. Ignored for on-chain.
    pub description_input: String,
    phase: SparkReceivePhase,
    /// Pre-rendered QR code for the current `Generated` payment
    /// request. Built once when `GenerateSucceeded` fires so the view
    /// renderer doesn't have to re-encode the (potentially long)
    /// BOLT11 invoice on every frame. `None` when no invoice is on
    /// screen, or when encoding failed (unlikely for BOLT11/BTC
    /// addresses but handled gracefully).
    pub qr_data: Option<qr_code::Data>,
    /// Phase 4f: pending on-chain deposits surfaced by the SDK's
    /// `list_unclaimed_deposits` RPC. Refreshed on panel reload and on
    /// every `Event::DepositsChanged`. The view renders this as a
    /// dedicated "Pending deposits" card below the main phase body.
    pub pending_deposits: Vec<DepositInfo>,
    /// Phase 4f: tracks which deposit is currently being claimed so
    /// the UI can disable the row's button while the RPC is in flight.
    /// Keyed by `(txid, vout)`. Cleared when the RPC finishes
    /// (success or failure).
    pub claiming: Option<(String, u32)>,
    /// Phase 4f: surface a transient claim error to the user. Cleared
    /// on the next reload or successful claim.
    pub claim_error: Option<String>,
    /// Phase 4f: the BOLT11 invoice string of the currently-displayed
    /// generated invoice, captured at `GenerateSucceeded` time. Used
    /// by the auto-advance handler to correlate `PaymentSucceeded`
    /// events against THIS invoice instead of accepting any
    /// incoming payment. `None` while in idle / error / received
    /// phases.
    pub displayed_invoice: Option<String>,
    /// Formatted amount string for the celebration screen.
    received_amount_display: String,
    /// Quote and image handle for the celebration screen.
    received_quote: coincube_ui::component::quote_display::Quote,
    received_image_handle: iced::widget::image::Handle,
}

impl SparkReceive {
    pub fn new(backend: Option<Arc<SparkBackend>>) -> Self {
        Self {
            backend,
            method: SparkReceiveMethod::Bolt11,
            amount_input: String::new(),
            description_input: String::new(),
            phase: SparkReceivePhase::Idle,
            qr_data: None,
            pending_deposits: Vec::new(),
            claiming: None,
            claim_error: None,
            displayed_invoice: None,
            received_amount_display: String::new(),
            received_quote: coincube_ui::component::quote_display::random_quote(
                "lightning-receive",
            ),
            received_image_handle: coincube_ui::component::quote_display::image_handle_for_context(
                "lightning-receive",
            ),
        }
    }

    pub fn phase(&self) -> &SparkReceivePhase {
        &self.phase
    }
}

impl State for SparkReceive {
    fn view<'a>(
        &'a self,
        menu: &'a Menu,
        cache: &'a Cache,
    ) -> Element<'a, crate::app::view::Message> {
        let backend_available = self.backend.is_some();
        crate::app::view::dashboard(
            menu,
            cache,
            SparkReceiveView {
                backend_available,
                method: self.method,
                amount_input: &self.amount_input,
                description_input: &self.description_input,
                phase: &self.phase,
                qr_data: self.qr_data.as_ref(),
                pending_deposits: &self.pending_deposits,
                claiming: self.claiming.as_ref(),
                claim_error: self.claim_error.as_deref(),
                received_amount_display: &self.received_amount_display,
                received_quote: &self.received_quote,
                received_image_handle: &self.received_image_handle,
            }
            .render(),
        )
    }

    fn reload(
        &mut self,
        _daemon: Option<Arc<dyn crate::daemon::Daemon + Sync + Send>>,
        _wallet: Option<Arc<crate::app::wallet::Wallet>>,
    ) -> Task<Message> {
        // Refresh the pending-deposits list whenever the panel
        // becomes active. Errors degrade silently — the rest of the
        // panel still works.
        fetch_deposits_task(self.backend.clone())
    }

    fn update(
        &mut self,
        _daemon: Option<Arc<dyn crate::daemon::Daemon + Sync + Send>>,
        _cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        let Message::View(crate::app::view::Message::SparkReceive(msg)) = message else {
            return Task::none();
        };

        use crate::app::view::SparkReceiveMessage;
        match msg {
            SparkReceiveMessage::MethodSelected(method) => {
                self.method = method;
                self.phase = SparkReceivePhase::Idle;
                self.qr_data = None;
                self.displayed_invoice = None;
                Task::none()
            }
            SparkReceiveMessage::AmountInputChanged(value) => {
                self.amount_input = value;
                self.phase = SparkReceivePhase::Idle;
                self.qr_data = None;
                self.displayed_invoice = None;
                Task::none()
            }
            SparkReceiveMessage::DescriptionInputChanged(value) => {
                self.description_input = value;
                self.phase = SparkReceivePhase::Idle;
                self.qr_data = None;
                self.displayed_invoice = None;
                Task::none()
            }
            SparkReceiveMessage::GenerateRequested => {
                let Some(backend) = self.backend.clone() else {
                    self.phase =
                        SparkReceivePhase::Error("Spark backend is not available.".to_string());
                    return Task::none();
                };
                self.phase = SparkReceivePhase::Generating;
                match self.method {
                    SparkReceiveMethod::Bolt11 => {
                        let amount_sat = if self.amount_input.trim().is_empty() {
                            None
                        } else {
                            match self.amount_input.trim().parse::<u64>() {
                                Ok(n) => Some(n),
                                Err(_) => {
                                    self.phase = SparkReceivePhase::Error(
                                        "Amount must be a whole number of sats.".to_string(),
                                    );
                                    return Task::none();
                                }
                            }
                        };
                        let description = self.description_input.clone();
                        Task::perform(
                            async move { backend.receive_bolt11(amount_sat, description, None).await },
                            |result| match result {
                                Ok(ok) => Message::View(crate::app::view::Message::SparkReceive(
                                    SparkReceiveMessage::GenerateSucceeded(ok),
                                )),
                                Err(e) => Message::View(crate::app::view::Message::SparkReceive(
                                    SparkReceiveMessage::GenerateFailed(e.to_string()),
                                )),
                            },
                        )
                    }
                    SparkReceiveMethod::OnchainBitcoin => Task::perform(
                        async move { backend.receive_onchain(None).await },
                        |result| match result {
                            Ok(ok) => Message::View(crate::app::view::Message::SparkReceive(
                                SparkReceiveMessage::GenerateSucceeded(ok),
                            )),
                            Err(e) => Message::View(crate::app::view::Message::SparkReceive(
                                SparkReceiveMessage::GenerateFailed(e.to_string()),
                            )),
                        },
                    ),
                }
            }
            SparkReceiveMessage::GenerateSucceeded(ok) => {
                // Encode the QR eagerly so the view renderer doesn't
                // re-encode on every frame.
                self.qr_data = qr_code::Data::new(&ok.payment_request).ok();
                // Only capture the payment request for BOLT11 — it's
                // the correlation key used by PaymentReceived to match
                // the event's bolt11 field against the displayed
                // invoice. For on-chain receives the payment_request
                // is a Bitcoin address (no bolt11 on the event), so
                // displayed_invoice stays None and the (None, _) =>
                // true arm in the correlation check auto-advances on
                // any incoming payment while the address is on screen.
                self.displayed_invoice = if self.method == SparkReceiveMethod::Bolt11 {
                    Some(ok.payment_request.clone())
                } else {
                    None
                };
                self.phase = SparkReceivePhase::Generated(ok);
                Task::none()
            }
            SparkReceiveMessage::GenerateFailed(err) => {
                self.qr_data = None;
                self.displayed_invoice = None;
                self.phase = SparkReceivePhase::Error(err);
                Task::none()
            }
            SparkReceiveMessage::PaymentReceived { amount_sat, bolt11 } => {
                // Only react if the user is actually looking at an
                // invoice — events that arrive while the panel is
                // idle or showing an error are no-ops.
                if !matches!(self.phase, SparkReceivePhase::Generated(_)) {
                    return Task::none();
                }

                // BOLT11 correlation: when we're displaying a
                // specific invoice, only advance if the event's
                // bolt11 field matches it exactly. When the event
                // doesn't carry a bolt11 (Spark-native / on-chain)
                // but we DO have a displayed invoice, ignore the
                // event — it's unrelated. The only case where we
                // advance unconditionally is when displayed_invoice
                // is None (on-chain receive flow, where no BOLT11
                // correlation is possible).
                let matches_invoice = match (&self.displayed_invoice, &bolt11) {
                    (Some(displayed), Some(event_bolt11)) => displayed == event_bolt11,
                    (Some(_), None) => false,
                    (None, _) => true,
                };
                if !matches_invoice {
                    return Task::none();
                }

                self.qr_data = None;
                self.displayed_invoice = None;
                self.received_amount_display =
                    format!("+{} sats", amount_sat.unsigned_abs());
                // Pick celebration image based on receive method.
                let context = if self.method == SparkReceiveMethod::Bolt11 {
                    "lightning-receive"
                } else {
                    "spark-receive"
                };
                self.received_quote =
                    coincube_ui::component::quote_display::random_quote(context);
                self.received_image_handle =
                    coincube_ui::component::quote_display::image_handle_for_context(context);
                self.phase = SparkReceivePhase::Received { amount_sat };
                Task::none()
            }
            SparkReceiveMessage::PendingDepositsLoaded(deposits) => {
                self.pending_deposits = deposits;
                self.claim_error = None;
                Task::none()
            }
            SparkReceiveMessage::PendingDepositsFailed(err) => {
                tracing::warn!("Spark list_unclaimed_deposits failed: {}", err);
                // Don't surface as a hard error — the rest of the
                // panel still works. Just clear the displayed list.
                self.pending_deposits.clear();
                Task::none()
            }
            SparkReceiveMessage::ClaimDepositRequested { txid, vout } => {
                let Some(backend) = self.backend.clone() else {
                    return Task::none();
                };
                self.claiming = Some((txid.clone(), vout));
                self.claim_error = None;
                Task::perform(
                    async move { backend.claim_deposit(txid, vout).await },
                    |result| match result {
                        Ok(ok) => Message::View(crate::app::view::Message::SparkReceive(
                            crate::app::view::SparkReceiveMessage::ClaimDepositSucceeded(ok),
                        )),
                        Err(e) => Message::View(crate::app::view::Message::SparkReceive(
                            crate::app::view::SparkReceiveMessage::ClaimDepositFailed(
                                e.to_string(),
                            ),
                        )),
                    },
                )
            }
            SparkReceiveMessage::ClaimDepositSucceeded(_ok) => {
                // The actual reload happens via the DepositsChanged
                // event the SDK fires post-claim, but we also refresh
                // here defensively in case the event got dropped.
                self.claiming = None;
                self.claim_error = None;
                fetch_deposits_task(self.backend.clone())
            }
            SparkReceiveMessage::ClaimDepositFailed(err) => {
                self.claiming = None;
                self.claim_error = Some(err);
                Task::none()
            }
            SparkReceiveMessage::DepositsChanged => fetch_deposits_task(self.backend.clone()),
            SparkReceiveMessage::Reset => {
                self.qr_data = None;
                self.displayed_invoice = None;
                self.phase = SparkReceivePhase::Idle;
                Task::none()
            }
        }
    }
}

/// Fire a `list_unclaimed_deposits` RPC and translate the result into
/// the appropriate view message. Pulled out as a helper so the
/// `reload`, `ClaimDepositSucceeded`, and `DepositsChanged` paths can
/// share it without duplicating the closure boilerplate.
fn fetch_deposits_task(backend: Option<Arc<SparkBackend>>) -> Task<Message> {
    let Some(backend) = backend else {
        return Task::none();
    };
    Task::perform(
        async move { backend.list_unclaimed_deposits().await },
        |result| match result {
            Ok(ok) => Message::View(crate::app::view::Message::SparkReceive(
                crate::app::view::SparkReceiveMessage::PendingDepositsLoaded(ok.deposits),
            )),
            Err(e) => Message::View(crate::app::view::Message::SparkReceive(
                crate::app::view::SparkReceiveMessage::PendingDepositsFailed(e.to_string()),
            )),
        },
    )
}
