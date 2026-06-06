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

use std::collections::HashMap;
use std::convert::TryInto;
use std::sync::Arc;

use coincube_spark_protocol::{DepositInfo, ReceivePaymentOk};
use coincube_ui::widget::Element;
use iced::{widget::qr_code, Subscription, Task};

use crate::app::cache::Cache;
use crate::app::menu::{Menu, SparkSubMenu};
use crate::app::message::Message;
use crate::app::state::{redirect, State};
use crate::app::view::spark::SparkReceiveView;
use crate::app::view::spark::SparkRecentTransaction;
use crate::app::view::FiatAmountConverter;
use crate::app::wallets::SparkBackend;

/// Which receive flow the user has picked.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SparkReceiveMethod {
    Bolt11,
    OnchainBitcoin,
    Spark,
}

impl SparkReceiveMethod {
    pub fn label(self) -> &'static str {
        match self {
            Self::Bolt11 => "Lightning (BOLT11)",
            Self::OnchainBitcoin => "On-chain Bitcoin",
            Self::Spark => "Spark",
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
    /// `Generated` state. Carries the running sum of all qualifying
    /// `PaymentReceived` events that have arrived since this phase
    /// was entered, plus a `count` so the celebration label can say
    /// "(2 deposits)" when multiple back-to-back receives stack up
    /// before the user dismisses the screen.
    Received { amount_sat: i64, count: u32 },
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
    /// Live on-chain confirmation count per pending deposit, fetched
    /// from a public Esplora ([`esplora::fetch_confirmations`]). The
    /// SDK only tells us `is_mature: bool`, so we query Esplora
    /// ourselves to surface progress like "1 / 3 confirmations" on
    /// rows that haven't matured yet. Entries are dropped when the
    /// deposit list refreshes; missing keys render the SDK's plain
    /// "Waiting for confirmations" fallback text.
    pub pending_deposit_confirmations: HashMap<(String, u32), u32>,
    /// Phase 4f: the BOLT11 invoice string of the currently-displayed
    /// generated invoice, captured at `GenerateSucceeded` time. Used
    /// by the auto-advance handler to correlate `PaymentSucceeded`
    /// events against THIS invoice instead of accepting any
    /// incoming payment. `None` while in idle / error / received
    /// phases.
    pub displayed_invoice: Option<String>,
    /// Formatted amount string for the celebration screen.
    received_amount_display: String,
    /// Quote context key for the celebration screen (e.g. "lightning-receive").
    received_celebration_context: String,
    /// Quote and image handle for the celebration screen.
    received_quote: coincube_ui::component::quote_display::Quote,
    received_image_handle: iced::widget::image::Handle,
    /// Last few payments fetched from the bridge, rendered under the
    /// receive form. Populated on reload and after an incoming payment.
    recent_transactions: Vec<SparkRecentTransaction>,
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
            pending_deposit_confirmations: HashMap::new(),
            displayed_invoice: None,
            received_amount_display: String::new(),
            received_celebration_context: "lightning-receive".to_string(),
            received_quote: coincube_ui::component::quote_display::random_quote(
                "lightning-receive",
            ),
            received_image_handle: coincube_ui::component::quote_display::image_handle_for_context(
                "lightning-receive",
            ),
            recent_transactions: Vec::new(),
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
                pending_deposit_confirmations: &self.pending_deposit_confirmations,
                network: cache.network,
                received_amount_display: &self.received_amount_display,
                received_celebration_context: &self.received_celebration_context,
                received_quote: &self.received_quote,
                received_image_handle: &self.received_image_handle,
                recent_transactions: &self.recent_transactions,
                bitcoin_unit: cache.bitcoin_unit,
                show_direction_badges: cache.show_direction_badges,
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
        // panel still works. Also fetch the recent-payments list so
        // the "Last transactions" section under the form is fresh.
        Task::batch(vec![
            fetch_deposits_task(self.backend.clone()),
            fetch_payments_task(self.backend.clone()),
        ])
    }

    fn subscription(&self) -> Subscription<Message> {
        // Esplora doesn't push — we poll on a fixed cadence while at
        // least one immature deposit is on screen so the "X / 3
        // confirmations" badge keeps ticking between block arrivals
        // (the SDK only re-emits `DepositsChanged` at maturity /
        // refund-status transitions, not on every new confirmation).
        // The poll stops automatically the moment the list is empty
        // or every deposit has matured.
        let has_immature = self.pending_deposits.iter().any(|d| !d.is_mature);
        if !has_immature {
            return Subscription::none();
        }
        iced::time::every(std::time::Duration::from_secs(30)).map(|_| {
            Message::View(crate::app::view::Message::SparkReceive(
                crate::app::view::SparkReceiveMessage::RefreshConfirmations,
            ))
        })
    }

    fn update(
        &mut self,
        _daemon: Option<Arc<dyn crate::daemon::Daemon + Sync + Send>>,
        cache: &Cache,
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
                    SparkReceiveMethod::Spark => {
                        Task::perform(async move { backend.receive_spark().await }, |result| {
                            match result {
                                Ok(ok) => Message::View(crate::app::view::Message::SparkReceive(
                                    SparkReceiveMessage::GenerateSucceeded(ok),
                                )),
                                Err(e) => Message::View(crate::app::view::Message::SparkReceive(
                                    SparkReceiveMessage::GenerateFailed(e.to_string()),
                                )),
                            }
                        })
                    }
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
                // Accept events while either showing an invoice
                // (`Generated`) — first arrival — or already on the
                // celebration screen (`Received`) — back-to-back
                // deposits accumulate into the running total instead
                // of being silently dropped. Idle / error / generating
                // phases stay no-op.
                let already_celebrating = matches!(self.phase, SparkReceivePhase::Received { .. });
                if !already_celebrating && !matches!(self.phase, SparkReceivePhase::Generated(_)) {
                    return Task::none();
                }

                // Only incoming payments (positive amount) should
                // trigger the celebration. Outgoing events with
                // negative amounts are skipped.
                let is_incoming = amount_sat > 0;
                if !is_incoming {
                    return Task::none();
                }

                // Correlate the event with the currently displayed
                // receive method so we only celebrate the payment the
                // user is actually waiting for:
                //
                // - Bolt11 invoice displayed + matching bolt11 event:
                //   the invoice was paid → advance.
                // - Bolt11 invoice displayed + event without bolt11:
                //   unrelated non-Lightning payment → skip.
                // - No invoice displayed (on-chain flow) + event
                //   without bolt11: on-chain deposit / Spark-native
                //   transfer → advance.
                // - No invoice displayed (on-chain flow) + event with
                //   bolt11: unrelated Lightning payment → skip.
                //
                // BOLT11 comparison is case-insensitive — canonical
                // form is lowercase but some SDKs hand back mixed case.
                //
                // For follow-ups during the celebration we only
                // aggregate non-BOLT11 events — a BOLT11 invoice is
                // single-use, so any second BOLT11 must be unrelated
                // and is skipped.
                let matches_invoice = if already_celebrating {
                    bolt11.is_none()
                } else {
                    match (&self.displayed_invoice, &bolt11) {
                        (Some(displayed), Some(event_bolt11)) => {
                            displayed.eq_ignore_ascii_case(event_bolt11)
                        }
                        (Some(_), None) => false,
                        (None, None) => true,
                        (None, Some(_)) => false,
                    }
                };
                if !matches_invoice {
                    return Task::none();
                }

                self.qr_data = None;
                self.displayed_invoice = None;
                let (running_total, count) = match self.phase {
                    SparkReceivePhase::Received {
                        amount_sat: prev,
                        count,
                    } => (prev.saturating_add(amount_sat), count.saturating_add(1)),
                    _ => (amount_sat, 1),
                };
                self.received_amount_display = if count > 1 {
                    format!(
                        "+{} sats ({} deposits)",
                        running_total.unsigned_abs(),
                        count
                    )
                } else {
                    format!("+{} sats", running_total.unsigned_abs())
                };
                // Pick celebration image based on receive method.
                // Only re-roll the quote on the first arrival so the
                // imagery doesn't flicker when a follow-up deposit
                // bumps the total.
                if !already_celebrating {
                    let context = if self.method == SparkReceiveMethod::Bolt11 {
                        "lightning-receive"
                    } else {
                        "spark-receive"
                    };
                    self.received_celebration_context = context.to_string();
                    self.received_quote =
                        coincube_ui::component::quote_display::random_quote(context);
                    self.received_image_handle =
                        coincube_ui::component::quote_display::image_handle_for_context(context);
                }
                self.phase = SparkReceivePhase::Received {
                    amount_sat: running_total,
                    count,
                };
                // Surface the just-received payment in the Last
                // Transactions list the moment it arrives.
                fetch_payments_task(self.backend.clone())
            }
            SparkReceiveMessage::PendingDepositsLoaded(deposits) => {
                self.pending_deposits = deposits;
                self.claim_error = None;
                // Drop confirmation entries for deposits that left
                // the list (claimed or refunded), then kick off a
                // fresh Esplora fetch for any immature deposit still
                // in the list. Mature deposits skip the fetch — they
                // already have a Claim button and the per-confirmation
                // count is no longer useful.
                let live_keys: std::collections::HashSet<(String, u32)> = self
                    .pending_deposits
                    .iter()
                    .map(|d| (d.txid.clone(), d.vout))
                    .collect();
                self.pending_deposit_confirmations
                    .retain(|k, _| live_keys.contains(k));
                refresh_confirmations_task(cache.network, &self.pending_deposits)
            }
            SparkReceiveMessage::PendingDepositsFailed(err) => {
                tracing::warn!("Spark list_unclaimed_deposits failed: {}", err);
                // Don't surface as a hard error — the rest of the
                // panel still works. Just clear the displayed list.
                self.pending_deposits.clear();
                self.pending_deposit_confirmations.clear();
                Task::none()
            }
            SparkReceiveMessage::DepositConfirmationsUpdated(map) => {
                // Merge rather than replace: a partial fetch (one
                // deposit's GET failed) shouldn't wipe the confirmation
                // count for the others.
                for (key, confs) in map {
                    self.pending_deposit_confirmations.insert(key, confs);
                }
                Task::none()
            }
            SparkReceiveMessage::RefreshConfirmations => {
                refresh_confirmations_task(cache.network, &self.pending_deposits)
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
            SparkReceiveMessage::PaymentsLoaded(payments) => {
                let fiat_converter: Option<FiatAmountConverter> =
                    cache.fiat_price.as_ref().and_then(|p| p.try_into().ok());
                self.recent_transactions = payments
                    .iter()
                    .take(5)
                    .map(|p| {
                        crate::app::state::spark::overview::payment_summary_to_recent_tx(
                            p,
                            fiat_converter.as_ref(),
                        )
                    })
                    .collect();
                Task::none()
            }
            SparkReceiveMessage::PaymentsFailed(err) => {
                tracing::warn!("spark receive list_payments failed: {}", err);
                self.recent_transactions.clear();
                Task::none()
            }
            SparkReceiveMessage::SelectTransaction(idx) => {
                if let Some(payment) = self.recent_transactions.get(idx).cloned() {
                    Task::batch(vec![
                        redirect(Menu::Spark(SparkSubMenu::Transactions(None))),
                        Task::done(Message::View(crate::app::view::Message::SparkTransactions(
                            crate::app::view::SparkTransactionsMessage::Preselect(payment),
                        ))),
                    ])
                } else {
                    Task::none()
                }
            }
            SparkReceiveMessage::History => redirect(Menu::Spark(SparkSubMenu::Transactions(None))),
        }
    }
}

/// Panel-local thin wrapper around the shared
/// [`super::fetch_payments_task`] helper — only the message variants
/// differ between the Send and Receive panels.
fn fetch_payments_task(backend: Option<Arc<SparkBackend>>) -> Task<Message> {
    super::fetch_payments_task(
        backend,
        |payments| {
            Message::View(crate::app::view::Message::SparkReceive(
                crate::app::view::SparkReceiveMessage::PaymentsLoaded(payments),
            ))
        },
        |err| {
            Message::View(crate::app::view::Message::SparkReceive(
                crate::app::view::SparkReceiveMessage::PaymentsFailed(err),
            ))
        },
    )
}

/// Kick off an Esplora confirmation-count fetch for every immature
/// deposit in `deposits`. Returns `Task::none()` when there's nothing
/// to fetch — keeps the call site at `PendingDepositsLoaded` /
/// `RefreshConfirmations` branch-free.
fn refresh_confirmations_task(
    network: coincube_core::miniscript::bitcoin::Network,
    deposits: &[DepositInfo],
) -> Task<Message> {
    let targets: Vec<(String, u32)> = deposits
        .iter()
        .filter(|d| !d.is_mature)
        .map(|d| (d.txid.clone(), d.vout))
        .collect();
    if targets.is_empty() {
        return Task::none();
    }
    Task::perform(
        super::esplora::fetch_confirmations(network, targets),
        |map| {
            Message::View(crate::app::view::Message::SparkReceive(
                crate::app::view::SparkReceiveMessage::DepositConfirmationsUpdated(map),
            ))
        },
    )
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
