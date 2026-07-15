//! Real Spark Send panel — Phase 4c.
//!
//! State machine:
//!
//! ```text
//! Idle  ──(input+Prepare)──▶  Preparing
//!                                │
//!                                ▼
//!                        Prepared { preview }
//!                                │ Confirm
//!                                ▼
//!                             Sending
//!                                │
//!                        ┌───────┴────────┐
//!                        ▼                ▼
//!                   Sent { ok }        Error(msg)
//!                        │                │
//!                        └──── Reset ─────┘
//!                                ▼
//!                              Idle
//! ```
//!
//! The `handle` returned by `prepare_send` is stored inside `Prepared`
//! and consumed by `send_payment` on confirm. Changing the input after
//! `Prepared` drops the handle (the SDK's prepare is single-use).

use std::convert::TryInto;
use std::sync::Arc;

use coincube_spark_protocol::{
    CrossChainAddress, CrossChainRoute, ParseInputKind, PrepareSendOk, SendPaymentOk,
};
use coincube_ui::component::amount::format_u64_as_string;
use coincube_ui::widget::Element;
use iced::Task;

use super::cross_chain;
use crate::app::cache::Cache;
use crate::app::menu::{Menu, SparkSubMenu};
use crate::app::message::Message;
use crate::app::state::{redirect, State};
use crate::app::view::spark::SparkRecentTransaction;
use crate::app::view::spark::SparkSendView;
use crate::app::view::FiatAmountConverter;
use crate::app::wallets::SparkBackend;

/// Shape of the Send panel at any instant.
#[derive(Debug, Clone)]
pub enum SparkSendPhase {
    /// Empty state — user hasn't entered anything, or just reset.
    Idle,
    /// Awaiting the `prepare_send` RPC response.
    Preparing,
    /// The destination is a cross-chain address (EVM / Solana / Tron), and the
    /// user must confirm *which chain and asset* before we quote.
    ///
    /// This is a mandatory stop, not a convenience. Address formats don't
    /// announce their chain, and USDT exists on all three families — so the
    /// panel states the detected chain in words and makes the user pick a
    /// route before any money moves. See
    /// [`cross_chain::chain_confirmation`](super::cross_chain::chain_confirmation).
    ///
    /// The destination and routes themselves live in
    /// [`SparkSend::cross_chain`], not in this variant — they have to outlive
    /// it, so a failed send can be re-prepared without making the user retype
    /// the address.
    CrossChainRoutes,
    /// `prepare_send` returned; the caller can review the preview and
    /// either confirm (→ `send_payment`) or go back to `Idle`.
    ///
    /// For a cross-chain send, `PrepareSendOk::cross_chain` carries the quote,
    /// which expires — the panel counts down and blocks confirmation at zero.
    Prepared(PrepareSendOk),
    /// Awaiting the `send_payment` RPC response.
    Sending,
    /// `send_payment` returned successfully.
    Sent(SendPaymentOk),
    /// Any step failed. Carries the user-visible message.
    Error(String),
    /// A *cross-chain* send failed. Kept apart from [`Self::Error`] because the
    /// safe next step depends on the route: a BTC-funded send can be retried
    /// with the same idempotency key, but a token-funded one has no idempotency
    /// guarantee and a blind retry could pay twice. The panel must offer
    /// "check status" rather than "try again" in that case.
    CrossChainFailed {
        message: String,
        policy: cross_chain::RetryPolicy,
    },
}

/// The cross-chain destination the user is sending to, and the routes that can
/// reach it. Held on the panel rather than inside [`SparkSendPhase`] because it
/// has to **outlive the phases that use it**.
///
/// Concretely: a failed send lands in [`SparkSendPhase::CrossChainFailed`], and
/// retrying from there means running `prepare_cross_chain` again — the bridge
/// consumes a prepare handle when it executes a send, whether or not the send
/// succeeded, so the old handle is dead and only a fresh prepare can be sent.
/// That re-prepare needs the address and route, so they can't have been thrown
/// away with the previous phase.
#[derive(Debug, Clone)]
pub struct CrossChainContext {
    pub address: CrossChainAddress,
    pub routes: Vec<CrossChainRoute>,
    /// Index into `routes`. Pre-selected when there's only one.
    pub selected: usize,
}

impl CrossChainContext {
    pub fn selected_route(&self) -> Option<&CrossChainRoute> {
        self.routes.get(self.selected)
    }
}

/// Real Spark Send panel.
pub struct SparkSend {
    backend: Option<Arc<SparkBackend>>,
    /// Free-text destination input (BOLT11 / BIP21 / on-chain address).
    pub destination_input: String,
    /// Amount override for amountless invoices / on-chain sends, in sats.
    pub amount_input: String,
    phase: SparkSendPhase,
    /// The send method from the last `PrepareSucceeded` — used to pick the
    /// correct celebration image ("Bolt11Invoice", "BitcoinAddress", etc.).
    last_send_method: String,
    /// Formatted amount string for the celebration screen.
    sent_amount_display: String,
    /// Quote context key for the celebration screen (e.g. "lightning-send").
    sent_celebration_context: String,
    /// Quote and image handle for the celebration screen.
    sent_quote: coincube_ui::component::quote_display::Quote,
    sent_image_handle: iced::widget::image::Handle,
    /// Last few payments fetched from the bridge, rendered under the
    /// send form. Populated on reload and after each successful send.
    recent_transactions: Vec<SparkRecentTransaction>,
    /// The cross-chain destination + routes, when the current send is one.
    /// Outlives the phases that read it so a failed send can be re-prepared —
    /// see [`CrossChainContext`].
    cross_chain: Option<CrossChainContext>,
    /// Slippage tolerance in basis points, as typed. Empty means "use the SDK
    /// default" (100 bps). Only reachable behind the advanced disclosure —
    /// a normal user should never have to think in basis points.
    pub slippage_input: String,
    /// Whether the advanced (slippage) disclosure is open.
    pub advanced_open: bool,
    /// Idempotency key for the in-flight send, minted once per user-initiated
    /// send and **reused across retries of that same send**. That reuse is the
    /// entire point: a fresh key on retry would defeat the SDK's dedup and
    /// could pay twice. Cleared on success or reset.
    send_idempotency_key: Option<String>,
    /// How much life the current cross-chain quote has left, recomputed on
    /// every tick **and set the instant the quote arrives**.
    ///
    /// This holds the countdown itself rather than a bare `Option<i64>` because
    /// that conflated three different things — "there is no quote", "we haven't
    /// measured it yet", and "it expired" — and the view had to guess which.
    /// Guessing "expired" made a freshly-arrived quote render as dead until the
    /// first tick landed a second later; worse, a re-quote after an expiry
    /// inherited the previous quote's stale `0` and showed a brand-new valid
    /// quote as expired. `None` now means exactly one thing: no quote.
    quote_countdown: Option<cross_chain::QuoteCountdown>,
}

impl SparkSend {
    pub fn new(backend: Option<Arc<SparkBackend>>) -> Self {
        Self {
            backend,
            destination_input: String::new(),
            amount_input: String::new(),
            phase: SparkSendPhase::Idle,
            last_send_method: String::new(),
            sent_amount_display: String::new(),
            sent_celebration_context: "lightning-send".to_string(),
            sent_quote: coincube_ui::component::quote_display::random_quote("lightning-send"),
            sent_image_handle: coincube_ui::component::quote_display::image_handle_for_context(
                "lightning-send",
            ),
            recent_transactions: Vec::new(),
            cross_chain: None,
            slippage_input: String::new(),
            advanced_open: false,
            send_idempotency_key: None,
            quote_countdown: None,
        }
    }

    pub fn phase(&self) -> &SparkSendPhase {
        &self.phase
    }

    /// The cross-chain destination + routes for the current send, if any.
    pub fn cross_chain(&self) -> Option<&CrossChainContext> {
        self.cross_chain.as_ref()
    }

    /// The live cross-chain quote, if the panel is showing one.
    pub fn cross_chain_quote(&self) -> Option<&coincube_spark_protocol::CrossChainQuote> {
        match &self.phase {
            // `as_deref` peels the `Box`, so callers still see `&CrossChainQuote`.
            SparkSendPhase::Prepared(ok) => ok.cross_chain.as_deref(),
            _ => None,
        }
    }

    /// Whether Confirm should be live. A cross-chain send is additionally
    /// blocked once its quote expires — sending against a dead quote means
    /// sending against a rate the provider no longer honours.
    pub fn can_confirm(&self, now: chrono::DateTime<chrono::Utc>) -> bool {
        match &self.phase {
            SparkSendPhase::Prepared(ok) => match &ok.cross_chain {
                None => true,
                Some(quote) => cross_chain::quote_countdown(quote, now).can_send(),
            },
            _ => false,
        }
    }

    /// Abandon the payment the panel was set up to make, because the user
    /// changed *what* they're paying — a different destination, or a different
    /// amount, is a different payment.
    ///
    /// Retiring the idempotency key is the load-bearing part. The key exists so
    /// a **retry of the same payment** can't pay twice; carrying it across an
    /// edit inverts it into a hazard. Concretely: a cross-chain send of 10k sats
    /// fails ambiguously (key `K` is held for the retry), the user edits the
    /// amount to 50k and sends — and the SDK, seeing `K` again, dedups against
    /// the 10k attempt and short-circuits. The 50k payment never happens, and
    /// the panel reports success. A silently dropped payment is worse than a
    /// visible failure, so the key dies with the intent that minted it.
    fn abandon_payment_intent(&mut self) {
        self.send_idempotency_key = None;
        self.quote_countdown = None;
    }

    /// Recompute the countdown from the quote's own `expires_at`.
    ///
    /// Always derived from the timestamp, never by decrementing a counter: a
    /// decrement drifts when ticks are dropped (a busy frame, a suspended
    /// laptop), and drifting *upward* would let a dead quote look live.
    ///
    /// Called both on the 1s tick and immediately when a quote arrives — the
    /// latter is what stops a fresh quote from rendering as expired in the gap
    /// before the first tick.
    fn refresh_quote_countdown(&mut self) {
        self.quote_countdown = self
            .cross_chain_quote()
            .map(|quote| cross_chain::quote_countdown(quote, chrono::Utc::now()));
    }

    /// Fetch (or re-fetch) a quote for the route the user selected. Shared by
    /// the first quote and the post-expiry re-quote — they differ only in what
    /// the user pressed, not in what has to happen.
    fn quote_cross_chain(&mut self) -> Task<Message> {
        let Some(ctx) = &self.cross_chain else {
            return Task::none();
        };
        let Some(route) = ctx.selected_route().cloned() else {
            return Task::none();
        };
        let Some(backend) = self.backend.clone() else {
            self.phase = SparkSendPhase::Error("Spark backend is not available.".to_string());
            return Task::none();
        };

        let slippage = match cross_chain::parse_slippage_bps(&self.slippage_input) {
            Ok(v) => v,
            Err(e) => {
                self.phase = SparkSendPhase::Error(e);
                return Task::none();
            }
        };
        let amount_sat = match self.amount_input.trim().parse::<u64>() {
            Ok(n) if n > 0 => n,
            _ => {
                self.phase = SparkSendPhase::Error(
                    "Enter the amount to send, in sats. Cross-chain sends are funded from your \
                     Bitcoin balance and converted when they're paid."
                        .to_string(),
                );
                return Task::none();
            }
        };

        // The whole destination, not just its address string: a URI's
        // `contract_address` / `chain_id` are what let the bridge re-resolve the
        // route against exactly the destination these routes were offered for.
        let destination = ctx.address.clone();
        self.phase = SparkSendPhase::Preparing;
        Task::perform(
            async move {
                backend
                    .prepare_cross_chain(destination, route, amount_sat, slippage)
                    .await
                    .map_err(|e| format!("Couldn't quote this send: {e}"))
            },
            |result| match result {
                Ok(ok) => Message::View(crate::app::view::Message::SparkSend(
                    crate::app::view::SparkSendMessage::PrepareSucceeded(ok),
                )),
                Err(e) => Message::View(crate::app::view::Message::SparkSend(
                    crate::app::view::SparkSendMessage::PrepareFailed(e),
                )),
            },
        )
    }
}

/// What a destination turned out to be.
enum Resolved {
    /// A cross-chain address, with the routes that can reach it. The user still
    /// has to confirm the chain and pick a route before anything is quoted.
    CrossChain(coincube_spark_protocol::CrossChainRoutesOk),
    /// An ordinary Spark/Lightning/on-chain destination, already prepared.
    Regular(PrepareSendOk),
}

/// Decide which send flow a destination belongs to.
///
/// Cross-chain is checked *first*, and deliberately so. The failure mode we're
/// avoiding is a stablecoin address being treated as an ordinary destination
/// and the funds going somewhere unrecoverable, so the more specific
/// classification wins. An input the bridge doesn't recognise as cross-chain
/// falls through to the existing BOLT11/LNURL/on-chain resolution untouched.
async fn classify_and_prepare(
    backend: Arc<SparkBackend>,
    input: String,
    amount_sat: Option<u64>,
) -> Result<Resolved, String> {
    let found = backend
        .get_cross_chain_routes(input.clone())
        .await
        .map_err(|e| format!("Couldn't check this destination: {e}"))?;

    if found.address.is_some() {
        return Ok(Resolved::CrossChain(found));
    }

    resolve_and_prepare(backend, input, amount_sat)
        .await
        .map(Resolved::Regular)
}

impl State for SparkSend {
    fn view<'a>(
        &'a self,
        menu: &'a Menu,
        cache: &'a Cache,
    ) -> Element<'a, crate::app::view::Message> {
        let backend_available = self.backend.is_some();
        crate::app::view::dashboard(
            menu,
            cache,
            SparkSendView {
                backend_available,
                destination_input: &self.destination_input,
                amount_input: &self.amount_input,
                phase: &self.phase,
                sent_amount_display: &self.sent_amount_display,
                sent_celebration_context: &self.sent_celebration_context,
                sent_quote: &self.sent_quote,
                sent_image_handle: &self.sent_image_handle,
                recent_transactions: &self.recent_transactions,
                bitcoin_unit: cache.bitcoin_unit,
                show_direction_badges: cache.show_direction_badges,
                cross_chain_ctx: self.cross_chain.as_ref(),
                slippage_input: &self.slippage_input,
                advanced_open: self.advanced_open,
                quote_countdown: self.quote_countdown.clone(),
            }
            .render(),
        )
    }

    fn reload(
        &mut self,
        _daemon: Option<Arc<dyn crate::daemon::Daemon + Sync + Send>>,
        _wallet: Option<Arc<crate::app::wallet::Wallet>>,
    ) -> Task<Message> {
        fetch_payments_task(self.backend.clone())
    }

    fn subscription(&self) -> iced::Subscription<Message> {
        // Tick only while a cross-chain quote is on screen. Nothing else in
        // this panel is time-sensitive, and a quote is the one thing that goes
        // stale on its own — so the timer exists exactly as long as one does.
        if self.cross_chain_quote().is_none() {
            return iced::Subscription::none();
        }
        iced::time::every(std::time::Duration::from_secs(1)).map(|_| {
            Message::View(crate::app::view::Message::SparkSend(
                crate::app::view::SparkSendMessage::QuoteTick,
            ))
        })
    }

    fn update(
        &mut self,
        _daemon: Option<Arc<dyn crate::daemon::Daemon + Sync + Send>>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        let Message::View(crate::app::view::Message::SparkSend(msg)) = message else {
            return Task::none();
        };

        use crate::app::view::SparkSendMessage;
        match msg {
            SparkSendMessage::DestinationInputChanged(value) => {
                self.destination_input = value;
                // Editing the destination invalidates any in-flight
                // preview — drop back to Idle so the user can re-prepare.
                self.phase = SparkSendPhase::Idle;
                self.abandon_payment_intent();
                // The routes were resolved for the *old* address, so they mean
                // nothing now. (A re-Prepare rebuilds them, but leaving a stale
                // destination lying around is a trap.)
                self.cross_chain = None;
                Task::none()
            }
            SparkSendMessage::AmountInputChanged(value) => {
                self.amount_input = value;
                self.phase = SparkSendPhase::Idle;
                self.abandon_payment_intent();
                Task::none()
            }
            SparkSendMessage::PrepareRequested => {
                let Some(backend) = self.backend.clone() else {
                    self.phase =
                        SparkSendPhase::Error("Spark backend is not available.".to_string());
                    return Task::none();
                };
                if self.destination_input.trim().is_empty() {
                    self.phase = SparkSendPhase::Error("Enter a destination first.".to_string());
                    return Task::none();
                }
                let amount_sat = if self.amount_input.trim().is_empty() {
                    None
                } else {
                    match self.amount_input.trim().parse::<u64>() {
                        Ok(n) => Some(n),
                        Err(_) => {
                            self.phase = SparkSendPhase::Error(
                                "Amount must be a whole number of sats.".to_string(),
                            );
                            return Task::none();
                        }
                    }
                };
                let input = self.destination_input.trim().to_string();
                self.phase = SparkSendPhase::Preparing;

                // Cross-chain destinations branch off before the ordinary
                // prepare: they need an explicit chain/asset confirmation and a
                // route choice first, so we ask the bridge to classify the
                // input rather than handing it straight to `prepare_send`.
                //
                // Only on mainnet — the providers and destination chains have
                // no test deployment, so on regtest we skip the check entirely
                // and let the normal path handle (and reject) the address.
                if cross_chain::supported_on(cache.network) {
                    let backend_cc = backend.clone();
                    let cc_input = input.clone();
                    let amount = amount_sat;
                    return Task::perform(
                        async move { classify_and_prepare(backend_cc, cc_input, amount).await },
                        |result| match result {
                            Ok(Resolved::CrossChain(routes)) => {
                                Message::View(crate::app::view::Message::SparkSend(
                                    SparkSendMessage::CrossChainRoutesLoaded(routes),
                                ))
                            }
                            Ok(Resolved::Regular(ok)) => {
                                Message::View(crate::app::view::Message::SparkSend(
                                    SparkSendMessage::PrepareSucceeded(ok),
                                ))
                            }
                            Err(e) => Message::View(crate::app::view::Message::SparkSend(
                                SparkSendMessage::PrepareFailed(e),
                            )),
                        },
                    );
                }
                // Phase 4e: chain `parse_input` + `prepare_*` in a
                // single async task so the user only sees one
                // "Preparing…" phase regardless of which SDK code
                // path runs underneath. The closure returns
                // `Result<PrepareSendOk, String>` so the existing
                // `PrepareSucceeded` / `PrepareFailed` messages
                // handle both regular sends and LNURL-pay sends
                // uniformly.
                Task::perform(
                    async move { resolve_and_prepare(backend, input, amount_sat).await },
                    |result| match result {
                        Ok(ok) => Message::View(crate::app::view::Message::SparkSend(
                            SparkSendMessage::PrepareSucceeded(ok),
                        )),
                        Err(e) => Message::View(crate::app::view::Message::SparkSend(
                            SparkSendMessage::PrepareFailed(e),
                        )),
                    },
                )
            }
            SparkSendMessage::PrepareSucceeded(ok) => {
                self.last_send_method = ok.method.clone();
                self.phase = SparkSendPhase::Prepared(ok);
                // Measure the quote's life *now*, not on the first tick a second
                // from now. Waiting made a just-arrived quote render as expired
                // — and after a re-quote it was worse than that, because the
                // stale `0` from the previous, genuinely-expired quote carried
                // over and condemned the new one.
                self.refresh_quote_countdown();
                Task::none()
            }
            SparkSendMessage::PrepareFailed(err) => {
                self.phase = SparkSendPhase::Error(err);
                Task::none()
            }
            SparkSendMessage::CrossChainRoutesLoaded(found) => {
                let Some(address) = found.address else {
                    // Shouldn't happen — `classify_and_prepare` only emits this
                    // message when the address parsed — but never fall through
                    // to a send on an unclassified destination.
                    self.phase = SparkSendPhase::Error(
                        "This destination isn't a recognised cross-chain address.".to_string(),
                    );
                    return Task::none();
                };
                if found.routes.is_empty() {
                    // The address parsed but nothing can reach it. Say so —
                    // silently falling back to a normal Spark send here would
                    // fire funds at an address on the wrong network.
                    self.phase = SparkSendPhase::Error(format!(
                        "No route can currently send to this {} address. \
                         Cross-chain sends support USDT and USDC only.",
                        address.family,
                    ));
                    return Task::none();
                }
                self.cross_chain = Some(CrossChainContext {
                    address,
                    routes: found.routes,
                    selected: 0,
                });
                self.phase = SparkSendPhase::CrossChainRoutes;
                Task::none()
            }
            SparkSendMessage::CrossChainRouteSelected(idx) => {
                if let Some(ctx) = &mut self.cross_chain {
                    if idx < ctx.routes.len() {
                        ctx.selected = idx;
                    }
                }
                Task::none()
            }
            SparkSendMessage::SlippageChanged(value) => {
                self.slippage_input = value;
                Task::none()
            }
            SparkSendMessage::ToggleAdvanced => {
                self.advanced_open = !self.advanced_open;
                Task::none()
            }
            SparkSendMessage::CrossChainQuoteRequested | SparkSendMessage::ReQuoteRequested => {
                self.quote_cross_chain()
            }
            SparkSendMessage::CrossChainRetryRequested => {
                // Retrying a failed cross-chain send means **re-preparing**, not
                // re-confirming. The bridge consumes a prepare handle when it
                // executes a send — whether or not the send succeeded — so the
                // handle from the failed attempt is dead and `send_payment`
                // would just reject it. Only a fresh prepare can be sent.
                //
                // What must NOT be refreshed is `send_idempotency_key`: it is
                // deliberately left in place so the retry carries the same key
                // as the attempt that failed. That is the entire safety
                // mechanism — if the first attempt actually landed despite
                // reporting failure, the SDK recognises the key and
                // short-circuits instead of paying a second time.
                let SparkSendPhase::CrossChainFailed { policy, .. } = &self.phase else {
                    return Task::none();
                };
                // Defense in depth: the view only offers this button when the
                // policy allows it, but a token-sourced send has no idempotency
                // guarantee at all, and a blind retry there could double-pay.
                // Never act on a stale or replayed message that says otherwise.
                if !policy.may_retry() {
                    return Task::none();
                }
                self.quote_cross_chain()
            }
            SparkSendMessage::QuoteTick => {
                self.refresh_quote_countdown();
                Task::none()
            }
            SparkSendMessage::ConfirmRequested => {
                let SparkSendPhase::Prepared(prepare) = &self.phase else {
                    return Task::none();
                };
                // A cross-chain quote that has run out must not be sent. The
                // rate is no longer one the provider honours, so confirming
                // would either fail or fill at a price the user never saw.
                if !self.can_confirm(chrono::Utc::now()) {
                    self.phase = SparkSendPhase::Error(
                        "This quote has expired. Get a fresh quote before sending.".to_string(),
                    );
                    return Task::none();
                }
                let Some(backend) = self.backend.clone() else {
                    self.phase =
                        SparkSendPhase::Error("Spark backend is not available.".to_string());
                    return Task::none();
                };
                let handle = prepare.handle.clone();
                let policy = prepare
                    .cross_chain
                    .as_deref()
                    .map(cross_chain::RetryPolicy::for_quote);

                // Mint the idempotency key once per send, and hold onto it: a
                // retry of *this* send must reuse it, or the SDK's dedup can't
                // recognise the retry and we risk paying twice.
                let key = self
                    .send_idempotency_key
                    .get_or_insert_with(|| uuid::Uuid::new_v4().to_string())
                    .clone();

                self.phase = SparkSendPhase::Sending;
                Task::perform(
                    async move { backend.send_payment(handle, Some(key)).await },
                    move |result| match result {
                        Ok(ok) => Message::View(crate::app::view::Message::SparkSend(
                            SparkSendMessage::SendSucceeded(ok),
                        )),
                        Err(e) => {
                            let msg = e.to_string();
                            Message::View(crate::app::view::Message::SparkSend(match policy {
                                Some(p) => SparkSendMessage::CrossChainSendFailed(msg, p),
                                None => SparkSendMessage::SendFailed(msg),
                            }))
                        }
                    },
                )
            }
            SparkSendMessage::CrossChainSendFailed(err, policy) => {
                self.phase = SparkSendPhase::CrossChainFailed {
                    message: err,
                    policy,
                };
                Task::none()
            }
            SparkSendMessage::SendSucceeded(ok) => {
                self.sent_amount_display =
                    format!("{} sats", format_u64_as_string(ok.amount_sat, ","));
                self.phase = SparkSendPhase::Sent(ok);
                // Clear the inputs so a follow-up send doesn't re-use them.
                self.destination_input.clear();
                self.amount_input.clear();
                // Retire the idempotency key with the send it belonged to. A
                // *new* send must never reuse it, or the SDK would dedup it
                // against this one and silently drop a payment the user meant
                // to make.
                self.send_idempotency_key = None;
                self.quote_countdown = None;
                self.cross_chain = None;
                // Refresh the Last Transactions list so the new payment
                // appears under the send form once the user returns.
                let refresh = fetch_payments_task(self.backend.clone());
                // Pick celebration image based on send method.
                // `last_send_method` mirrors the
                // `breez_sdk_spark::SendPaymentMethod` variant names
                // (BitcoinAddress / Bolt11Invoice / SparkAddress /
                // SparkInvoice), plus LNURL-pay variants routed
                // through the Lightning path.
                let context = if self.last_send_method == "BitcoinAddress" {
                    "bitcoin-send"
                } else if self.last_send_method == "Bolt11Invoice"
                    || self.last_send_method.contains("Lnurl")
                {
                    "lightning-send"
                } else {
                    "spark-send"
                };
                self.sent_celebration_context = context.to_string();
                self.sent_quote = coincube_ui::component::quote_display::random_quote(context);
                self.sent_image_handle =
                    coincube_ui::component::quote_display::image_handle_for_context(context);
                refresh
            }
            SparkSendMessage::SendFailed(err) => {
                self.phase = SparkSendPhase::Error(err);
                Task::none()
            }
            SparkSendMessage::Reset => {
                self.destination_input.clear();
                self.amount_input.clear();
                self.phase = SparkSendPhase::Idle;
                // Reset abandons the send, so its key must go too — the next
                // send is a different payment and needs a fresh one.
                self.send_idempotency_key = None;
                self.quote_countdown = None;
                self.slippage_input.clear();
                self.cross_chain = None;
                Task::none()
            }
            SparkSendMessage::PaymentsLoaded(payments) => {
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
            SparkSendMessage::PaymentsFailed(err) => {
                tracing::warn!("spark send list_payments failed: {}", err);
                self.recent_transactions.clear();
                Task::none()
            }
            SparkSendMessage::SelectTransaction(idx) => {
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
            SparkSendMessage::History => redirect(Menu::Spark(SparkSubMenu::Transactions(None))),
        }
    }
}

/// Phase 4e: classify the user-supplied destination via `parse_input`
/// and dispatch to the right prepare RPC (`prepare_send` for
/// BOLT11/on-chain/Other, `prepare_lnurl_pay` for LNURL/Lightning
/// Address). Returns a `Result<PrepareSendOk, String>` so the calling
/// closure can wrap the success/failure into the existing
/// `SparkSendMessage::Prepare*` variants without a new branch in the
/// state machine.
///
/// LNURL inputs validate the amount against the server's min/max
/// range up front so the gui can surface a useful error before
/// actually hitting the LNURL callback URL.
async fn resolve_and_prepare(
    backend: Arc<SparkBackend>,
    input: String,
    amount_sat: Option<u64>,
) -> Result<PrepareSendOk, String> {
    let parsed = backend
        .parse_input(input.clone())
        .await
        .map_err(|e| format!("parse failed: {e}"))?;

    match parsed.kind {
        ParseInputKind::LnurlPay | ParseInputKind::LightningAddress => {
            let amount = amount_sat.ok_or_else(|| {
                "Lightning address sends require an amount in the Amount field.".to_string()
            })?;
            // LNURL servers always declare a min/max range. Validate
            // the user's amount before hitting the callback URL —
            // catches the obvious mistakes (zero, way too high) with
            // a clear message instead of a cryptic SDK error.
            let min = parsed.lnurl_min_sendable_sat.unwrap_or(0);
            let max = parsed.lnurl_max_sendable_sat.unwrap_or(u64::MAX);
            if amount < min || amount > max {
                return Err(format!(
                    "This LNURL server accepts payments between {} and {} sats; \
                     you entered {}.",
                    min, max, amount
                ));
            }
            backend
                .prepare_lnurl_pay(input, amount, None)
                .await
                .map_err(|e| format!("prepare_lnurl_pay failed: {e}"))
        }
        ParseInputKind::Bolt11Invoice
        | ParseInputKind::BitcoinAddress
        | ParseInputKind::SparkAddress
        | ParseInputKind::SparkInvoice
        | ParseInputKind::Other => backend
            .prepare_send(input, amount_sat)
            .await
            .map_err(|e| format!("prepare_send failed: {e}")),
    }
}

/// Panel-local thin wrapper around the shared
/// [`super::fetch_payments_task`] helper — only the message variants
/// differ between the Send and Receive panels.
fn fetch_payments_task(backend: Option<Arc<SparkBackend>>) -> Task<Message> {
    super::fetch_payments_task(
        backend,
        |payments| {
            Message::View(crate::app::view::Message::SparkSend(
                crate::app::view::SparkSendMessage::PaymentsLoaded(payments),
            ))
        },
        |err| {
            Message::View(crate::app::view::Message::SparkSend(
                crate::app::view::SparkSendMessage::PaymentsFailed(err),
            ))
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::view::{Message as ViewMessage, SparkSendMessage};

    fn route() -> CrossChainRoute {
        CrossChainRoute {
            provider: "orchestra".to_string(),
            chain: "base".to_string(),
            chain_id: None,
            asset: "USDC".to_string(),
            contract_address: None,
            decimals: 6,
            btc_source_supported: true,
        }
    }

    /// A panel sitting on a failed cross-chain send, with the context and
    /// idempotency key the failed attempt left behind.
    fn failed_panel(policy: cross_chain::RetryPolicy) -> SparkSend {
        let mut panel = SparkSend::new(None);
        panel.cross_chain = Some(CrossChainContext {
            address: CrossChainAddress {
                address: "0xabc".to_string(),
                family: "evm".to_string(),
                contract_address: None,
                chain_id: None,
                amount: None,
            },
            routes: vec![route()],
            selected: 0,
        });
        panel.send_idempotency_key = Some("key-from-the-failed-attempt".to_string());
        panel.phase = SparkSendPhase::CrossChainFailed {
            message: "connection lost".to_string(),
            policy,
        };
        panel
    }

    fn send(panel: &mut SparkSend, msg: SparkSendMessage) -> Task<Message> {
        panel.update(
            None,
            &Cache::default(),
            Message::View(ViewMessage::SparkSend(msg)),
        )
    }

    /// Regression: "Try again" used to dispatch `ConfirmRequested`, whose
    /// handler bails unless the phase is `Prepared`. So the button did nothing
    /// at all — the user pressed it and sat in the failed state forever.
    ///
    /// `ConfirmRequested` must stay a no-op here (a retry can't re-confirm; the
    /// bridge consumed the prepare handle when it executed the failed send), so
    /// the fix is that the button sends `CrossChainRetryRequested` instead.
    #[test]
    fn confirm_does_nothing_from_a_failed_cross_chain_send() {
        let mut panel = failed_panel(cross_chain::RetryPolicy::SafeToRetry);
        let _ = send(&mut panel, SparkSendMessage::ConfirmRequested);
        assert!(
            matches!(panel.phase, SparkSendPhase::CrossChainFailed { .. }),
            "ConfirmRequested must not move a failed cross-chain send anywhere"
        );
    }

    /// The real retry path: re-prepare, rather than re-confirm a dead handle.
    #[test]
    fn retry_re_prepares_and_keeps_the_original_idempotency_key() {
        let mut panel = failed_panel(cross_chain::RetryPolicy::SafeToRetry);
        let _ = send(&mut panel, SparkSendMessage::CrossChainRetryRequested);

        // It left the failed state — i.e. the button actually did something,
        // which is exactly what the old `ConfirmRequested` wiring did not.
        // This panel has no backend, so the re-prepare gets as far as the
        // backend check and stops there; the point is that it *ran*.
        assert!(
            matches!(panel.phase, SparkSendPhase::Error(_)),
            "retry must reach the re-prepare (which then fails on the absent backend), \
             not sit in CrossChainFailed"
        );

        // The key is the whole safety mechanism. If the failed attempt actually
        // landed, the retry carrying the SAME key lets the SDK dedup it instead
        // of paying twice. Minting a fresh key here would be a double-spend.
        assert_eq!(
            panel.send_idempotency_key.as_deref(),
            Some("key-from-the-failed-attempt"),
            "a retry must reuse the failed attempt's idempotency key"
        );
    }

    /// A token-sourced send has no idempotency guarantee, so a retry could pay
    /// twice. The view doesn't render the button — this is the backstop that
    /// makes a stale or replayed message harmless.
    #[test]
    fn retry_is_refused_outright_when_the_route_cannot_guarantee_it() {
        let mut panel = failed_panel(cross_chain::RetryPolicy::MustCheckStateFirst);
        let _ = send(&mut panel, SparkSendMessage::CrossChainRetryRequested);
        assert!(
            matches!(panel.phase, SparkSendPhase::CrossChainFailed { .. }),
            "an unsafe retry must not re-prepare, even if the message arrives"
        );
    }

    fn prepared_with_quote(expires_at: &str) -> PrepareSendOk {
        PrepareSendOk {
            handle: "h".to_string(),
            amount_sat: 1_000,
            fee_sat: 10,
            method: "CrossChainAddress".to_string(),
            cross_chain: Some(Box::new(coincube_spark_protocol::CrossChainQuote {
                route: route(),
                estimated_out: 1_000_000,
                fee_amount: 1_000,
                source_transfer_fee_sats: 10,
                expires_at: expires_at.to_string(),
                retry_safe: true,
            })),
        }
    }

    /// Regression: the countdown was only ever written on the 1s tick, so a
    /// just-arrived quote had `None` — which the view read as "expired" and
    /// rendered a re-quote CTA over a perfectly good quote for up to a second.
    #[test]
    fn a_freshly_arrived_quote_is_measured_immediately_not_on_the_next_tick() {
        let mut panel = SparkSend::new(None);
        let soon = (chrono::Utc::now() + chrono::Duration::seconds(30)).to_rfc3339();
        let _ = send(
            &mut panel,
            SparkSendMessage::PrepareSucceeded(prepared_with_quote(&soon)),
        );

        // Measured on arrival — no tick has fired.
        assert!(
            matches!(
                panel.quote_countdown,
                Some(cross_chain::QuoteCountdown::Valid { .. })
            ),
            "a fresh quote must not read as expired before the first tick"
        );
        assert!(panel.can_confirm(chrono::Utc::now()));
    }

    /// The nastier half of the same bug: a re-quote after an expiry used to
    /// inherit the *previous* quote's stale `0`, so a brand-new valid quote
    /// still rendered as expired. Recomputing on arrival kills both.
    #[test]
    fn a_requote_after_an_expiry_does_not_inherit_the_dead_quotes_countdown() {
        let mut panel = SparkSend::new(None);

        // A quote that is already dead on arrival.
        let past = (chrono::Utc::now() - chrono::Duration::seconds(5)).to_rfc3339();
        let _ = send(
            &mut panel,
            SparkSendMessage::PrepareSucceeded(prepared_with_quote(&past)),
        );
        assert_eq!(
            panel.quote_countdown,
            Some(cross_chain::QuoteCountdown::Expired)
        );

        // Now a fresh one lands. It must be judged on its own expiry.
        let soon = (chrono::Utc::now() + chrono::Duration::seconds(30)).to_rfc3339();
        let _ = send(
            &mut panel,
            SparkSendMessage::PrepareSucceeded(prepared_with_quote(&soon)),
        );
        assert!(
            matches!(
                panel.quote_countdown,
                Some(cross_chain::QuoteCountdown::Valid { .. })
            ),
            "a re-quote must not inherit the previous quote's expiry"
        );
    }

    /// `None` must mean "no quote", never "expired" — that conflation is what
    /// produced both bugs above.
    #[test]
    fn no_quote_is_distinct_from_an_expired_one() {
        let mut panel = SparkSend::new(None);
        assert_eq!(panel.quote_countdown, None, "no quote yet");

        // A non-cross-chain prepare has no quote at all, and must not leave a
        // stale countdown behind from an earlier cross-chain attempt.
        let past = (chrono::Utc::now() - chrono::Duration::seconds(5)).to_rfc3339();
        let _ = send(
            &mut panel,
            SparkSendMessage::PrepareSucceeded(prepared_with_quote(&past)),
        );
        assert_eq!(
            panel.quote_countdown,
            Some(cross_chain::QuoteCountdown::Expired)
        );

        let mut plain = prepared_with_quote(&past);
        plain.cross_chain = None;
        plain.method = "Bolt11Invoice".to_string();
        let _ = send(&mut panel, SparkSendMessage::PrepareSucceeded(plain));
        assert_eq!(
            panel.quote_countdown, None,
            "an ordinary send has no quote, and must clear the previous one"
        );
        // And an ordinary send is confirmable — it has no expiry to outlive.
        assert!(panel.can_confirm(chrono::Utc::now()));
    }

    /// Editing the amount after a failed send makes it a **different payment**.
    /// Carrying the idempotency key across that edit inverts its purpose: the
    /// SDK would dedup the new amount against the old attempt and short-circuit,
    /// so the payment the user actually asked for never happens — and the panel
    /// reports success. A silently dropped payment beats a visible failure only
    /// in the sense that it's worse.
    #[test]
    fn editing_the_amount_retires_the_key_so_the_next_send_is_a_new_payment() {
        let mut panel = failed_panel(cross_chain::RetryPolicy::SafeToRetry);
        assert!(panel.send_idempotency_key.is_some());

        let _ = send(
            &mut panel,
            SparkSendMessage::AmountInputChanged("50000".to_string()),
        );
        assert!(
            panel.send_idempotency_key.is_none(),
            "a different amount is a different payment — it must not reuse the key"
        );
        assert!(matches!(panel.phase, SparkSendPhase::Idle));
    }

    #[test]
    fn editing_the_destination_retires_both_the_key_and_the_stale_routes() {
        let mut panel = failed_panel(cross_chain::RetryPolicy::SafeToRetry);

        let _ = send(
            &mut panel,
            SparkSendMessage::DestinationInputChanged("0xdifferent".to_string()),
        );
        assert!(panel.send_idempotency_key.is_none());
        // The routes were resolved for the previous address; keeping them
        // around is how a send goes to the wrong place.
        assert!(panel.cross_chain.is_none());
        assert!(matches!(panel.phase, SparkSendPhase::Idle));
    }

    /// Starting over is a *different payment*, so it must not inherit the old
    /// key — the SDK would dedup the new send against the old one and silently
    /// drop it.
    #[test]
    fn reset_retires_the_idempotency_key_and_the_destination() {
        let mut panel = failed_panel(cross_chain::RetryPolicy::SafeToRetry);
        let _ = send(&mut panel, SparkSendMessage::Reset);
        assert!(panel.send_idempotency_key.is_none());
        assert!(panel.cross_chain.is_none());
        assert!(matches!(panel.phase, SparkSendPhase::Idle));
    }
}
