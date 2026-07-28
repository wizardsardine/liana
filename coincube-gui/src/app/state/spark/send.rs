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
use coincube_ui::component::amount::{format_u64_as_string, BitcoinDisplayUnit};
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
    /// safe next step depends on the route *and* the quote: a retry re-sends the
    /// **same** prepared quote — dedup-safe via the provider's swap id — while
    /// it's still valid, but once it expires (or on a route with no idempotency
    /// guarantee at all) a blind retry could pay twice, so the panel offers
    /// "check status" instead. See [`SparkSend::cross_chain_prepare`].
    CrossChainFailed {
        message: String,
        policy: cross_chain::RetryPolicy,
    },
}

/// The cross-chain destination the user is sending to, and the routes that can
/// reach it. Held on the panel rather than inside [`SparkSendPhase`] because it
/// has to **outlive the phases that use it**.
///
/// Concretely: when a quote expires the user re-quotes (`ReQuoteRequested`),
/// which runs `prepare_cross_chain` again — and that needs the address and the
/// selected route, so they can't have been thrown away with the phase that
/// showed the dead quote. (A *failed send*, by contrast, retries by re-sending
/// the retained quote itself — see [`SparkSend::cross_chain_prepare`].)
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

/// The "THEY RECEIVE" selection: what the recipient ends up with. The wallet
/// always spends *bitcoin*; the stablecoin targets are BTC-funded cross-chain
/// sends, converted at the route's rate. Mirrors the Spark Receive two-card
/// selector, in the send direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SparkSendTarget {
    /// Bitcoin over Lightning (BOLT11 / Lightning address).
    Lightning,
    /// Bitcoin on-chain.
    OnChain,
    /// Bitcoin over Spark (Spark address / invoice).
    Spark,
    /// USDt on another chain — a BTC-funded cross-chain send.
    Usdt,
    /// USDC on another chain — a BTC-funded cross-chain send.
    Usdc,
}

impl SparkSendTarget {
    /// Display order for the picker.
    pub fn all() -> [SparkSendTarget; 5] {
        [
            Self::Lightning,
            Self::OnChain,
            Self::Spark,
            Self::Usdt,
            Self::Usdc,
        ]
    }

    /// The destination-asset symbol for a cross-chain (stablecoin) send — used
    /// to filter the routes the bridge returns to the picked coin. `None` for
    /// the bitcoin rails, which don't go through the cross-chain path at all.
    pub fn stablecoin(self) -> Option<&'static str> {
        match self {
            Self::Usdt => Some("USDT"),
            Self::Usdc => Some("USDC"),
            _ => None,
        }
    }

    pub fn is_stablecoin(self) -> bool {
        self.stablecoin().is_some()
    }

    /// Card / picker-row asset label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Lightning | Self::OnChain | Self::Spark => "Bitcoin",
            Self::Usdt => "USDt",
            Self::Usdc => "USDC",
        }
    }

    /// Card / picker-row network (rail) badge.
    pub fn badge(self) -> &'static str {
        match self {
            Self::Lightning => "Lightning",
            Self::OnChain => "On-chain",
            Self::Spark => "Spark",
            Self::Usdt | Self::Usdc => "Cross-chain",
        }
    }

    /// Destination-field placeholder, tuned to the target.
    pub fn destination_placeholder(self) -> &'static str {
        match self {
            Self::Lightning => "Lightning invoice or Lightning address",
            Self::OnChain => "Bitcoin address",
            Self::Spark => "Spark address or invoice",
            Self::Usdt => "Recipient's USDt address on Ethereum, Tron, or Solana",
            Self::Usdc => "Recipient's USDC address on Ethereum, Tron, or Solana",
        }
    }

    /// For a bitcoin rail, whether a parsed destination is consistent with this
    /// selection. `false` only for a *definite* cross-rail mismatch — e.g. an
    /// on-chain address while Lightning is selected. Ambiguous kinds (`Other`:
    /// BOLT12 / silent payment) pass, so a destination the SDK can still route
    /// isn't blocked. Stablecoin targets never reach this path.
    fn accepts_parsed(self, kind: &ParseInputKind) -> bool {
        let rail = match kind {
            ParseInputKind::Bolt11Invoice
            | ParseInputKind::LnurlPay
            | ParseInputKind::LightningAddress => Some(Self::Lightning),
            ParseInputKind::BitcoinAddress => Some(Self::OnChain),
            ParseInputKind::SparkAddress | ParseInputKind::SparkInvoice => Some(Self::Spark),
            ParseInputKind::Other => None,
        };
        rail.is_none_or(|r| r == self)
    }
}

/// Real Spark Send panel.
pub struct SparkSend {
    backend: Option<Arc<SparkBackend>>,
    /// The Spark wallet's unified balance in sats (BTC + Stable Balance), shown
    /// on the YOU SEND card. Refreshed on reload via `get_info`; `0` until the
    /// first fetch.
    balance_sats: u64,
    /// Free-text destination input (BOLT11 / BIP21 / on-chain address).
    pub destination_input: String,
    /// Amount override for amountless invoices / on-chain sends, in sats.
    pub amount_input: String,
    /// The amount the pasted destination already commits to, when it's a BOLT11
    /// invoice that carries one. It's filled into `amount_input` so the user can
    /// see what they're about to pay without decoding the invoice by eye, and
    /// the field goes read-only for as long as it's set — the invoice, not the
    /// form, decides this amount, and prepare deliberately doesn't pass it on.
    invoice_amount_sat: Option<u64>,
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
    /// send and **reused across retries of that same send**. On the bitcoin
    /// rails (Lightning / on-chain / Spark) this is the dedup: a fresh key on
    /// retry would defeat the SDK's dedup and could pay twice. A *cross-chain*
    /// send can't use it — the SDK rejects a key on a token/conversion leg — so
    /// there the retry guard is [`Self::cross_chain_prepare`] instead. Cleared
    /// on success or reset.
    send_idempotency_key: Option<String>,
    /// The prepared cross-chain quote of the in-flight send, retained so a
    /// failed send can be retried by re-sending the **same handle** — same
    /// provider swap id, which dedups the BTC leg at the Spark protocol level.
    /// Reusing the quote is only safe while it hasn't expired; past that the
    /// retry falls back to "verify state first" rather than re-preparing, which
    /// would mint a fresh swap id with no dedup against a maybe-landed attempt.
    /// `None` for bitcoin-rail sends and once the send succeeds / resets /
    /// is abandoned.
    cross_chain_prepare: Option<PrepareSendOk>,
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
    /// The "THEY RECEIVE" selection (a bitcoin rail or a stablecoin). Drives the
    /// two-card selector, the destination placeholder, and which send path
    /// `PrepareRequested` takes.
    receive_target: SparkSendTarget,
    /// Whether the "THEY RECEIVE" picker modal is open.
    receive_picker_open: bool,
}

impl SparkSend {
    pub fn new(backend: Option<Arc<SparkBackend>>) -> Self {
        Self {
            backend,
            balance_sats: 0,
            destination_input: String::new(),
            amount_input: String::new(),
            invoice_amount_sat: None,
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
            cross_chain_prepare: None,
            quote_countdown: None,
            receive_target: SparkSendTarget::Lightning,
            receive_picker_open: false,
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
        self.cross_chain_prepare = None;
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
    fn quote_cross_chain(&mut self, bitcoin_unit: BitcoinDisplayUnit) -> Task<Message> {
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
        let amount_sat = match parse_amount_to_sats(&self.amount_input, bitcoin_unit) {
            Ok(n) if n > 0 => n,
            _ => {
                self.phase = SparkSendPhase::Error(format!(
                    "Enter the amount to send, in {}. Cross-chain sends are funded from your \
                     Bitcoin balance and converted when they're paid.",
                    amount_unit_word(bitcoin_unit),
                ));
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

    /// Fire the `send_payment` task for a prepared send.
    ///
    /// A cross-chain send additionally stashes its quote in
    /// [`Self::cross_chain_prepare`] so a retry can re-send *this same handle* —
    /// same provider swap id, which is what dedups the BTC leg. The idempotency
    /// key is minted once and reused across retries; the bridge drops it for a
    /// cross-chain send (the SDK forbids a key on a token leg), so there the
    /// swap-id reuse is the real guard, while it stays load-bearing for the
    /// bitcoin rails.
    fn dispatch_send(&mut self, prepare: PrepareSendOk) -> Task<Message> {
        use crate::app::view::SparkSendMessage;
        let Some(backend) = self.backend.clone() else {
            self.phase = SparkSendPhase::Error("Spark backend is not available.".to_string());
            return Task::none();
        };
        let handle = prepare.handle.clone();
        let policy = prepare
            .cross_chain
            .as_deref()
            .map(cross_chain::RetryPolicy::for_quote);
        if prepare.cross_chain.is_some() {
            self.cross_chain_prepare = Some(prepare);
        }
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
}

impl State for SparkSend {
    fn view<'a>(
        &'a self,
        menu: &'a Menu,
        cache: &'a Cache,
    ) -> Element<'a, crate::app::view::Message> {
        let backend_available = self.backend.is_some();
        let content = crate::app::view::dashboard(
            menu,
            cache,
            SparkSendView {
                backend_available,
                destination_input: &self.destination_input,
                amount_input: &self.amount_input,
                amount_set_by_invoice: self.invoice_amount_sat.is_some(),
                phase: &self.phase,
                sent_amount_display: &self.sent_amount_display,
                sent_celebration_context: &self.sent_celebration_context,
                sent_quote: &self.sent_quote,
                sent_image_handle: &self.sent_image_handle,
                recent_transactions: &self.recent_transactions,
                balance_sats: self.balance_sats,
                bitcoin_unit: cache.bitcoin_unit,
                reference_btc_usd_price: super::reference_btc_usd_price(cache),
                show_direction_badges: cache.show_direction_badges,
                cross_chain_ctx: self.cross_chain.as_ref(),
                slippage_input: &self.slippage_input,
                advanced_open: self.advanced_open,
                quote_countdown: self.quote_countdown.clone(),
                receive_target: self.receive_target,
                network: cache.network,
            }
            .render(),
        );

        // The "THEY RECEIVE" picker overlays the panel when open — same pattern
        // as the Spark Receive redesign.
        if self.receive_picker_open {
            let modal_content = crate::app::view::spark::send_target_picker_modal(
                self.receive_target,
                cache.network,
            );
            return coincube_ui::widget::modal::Modal::new(content, modal_content)
                .on_blur(Some(crate::app::view::Message::SparkSend(
                    crate::app::view::SparkSendMessage::CloseReceivePicker,
                )))
                .into();
        }
        content
    }

    fn reload(
        &mut self,
        _daemon: Option<Arc<dyn crate::daemon::Daemon + Sync + Send>>,
        _wallet: Option<Arc<crate::app::wallet::Wallet>>,
    ) -> Task<Message> {
        Task::batch(vec![
            fetch_payments_task(self.backend.clone()),
            fetch_balance_task(self.backend.clone()),
        ])
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
                // A BOLT11 invoice that names its own amount fills the amount
                // field in, so the user sees what the invoice asks for rather
                // than an empty box. Clearing on the way out matters as much as
                // filling in: swapping one invoice for another must not leave
                // the previous invoice's amount sitting there.
                let carried = bolt11_amount_sat(&self.destination_input);
                if carried.is_some() || self.invoice_amount_sat.is_some() {
                    self.amount_input = carried
                        .map(|sats| format_amount_for_input(sats, cache.bitcoin_unit))
                        .unwrap_or_default();
                }
                self.invoice_amount_sat = carried;
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
            SparkSendMessage::OpenReceivePicker => {
                self.receive_picker_open = true;
                Task::none()
            }
            SparkSendMessage::CloseReceivePicker => {
                self.receive_picker_open = false;
                Task::none()
            }
            SparkSendMessage::SetReceiveTarget(target) => {
                // Switching what the recipient gets is a different payment —
                // clear the destination/amount and any in-flight prepare.
                self.receive_picker_open = false;
                self.receive_target = target;
                self.phase = SparkSendPhase::Idle;
                self.destination_input.clear();
                self.amount_input.clear();
                self.invoice_amount_sat = None;
                self.abandon_payment_intent();
                self.cross_chain = None;
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
                // An amount read *out of* the invoice must not be handed back to
                // the SDK: `prepare_send` treats an explicit amount as an
                // override for an amountless invoice, and passing one alongside
                // an invoice that already commits to an amount is rejected. The
                // field is display-only in that case.
                let amount_sat =
                    if self.invoice_amount_sat.is_some() || self.amount_input.trim().is_empty() {
                        None
                    } else {
                        match parse_amount_to_sats(&self.amount_input, cache.bitcoin_unit) {
                            Ok(n) => Some(n),
                            Err(e) => {
                                self.phase = SparkSendPhase::Error(e);
                                return Task::none();
                            }
                        }
                    };
                let input = self.destination_input.trim().to_string();
                self.phase = SparkSendPhase::Preparing;

                // The THEY RECEIVE selection decides the path. A stablecoin
                // target is a BTC-funded cross-chain send: ask the bridge for
                // routes, then confirm a chain + quote before anything moves.
                // Providers/chains have no test deployment, so it's mainnet-only.
                if self.receive_target.is_stablecoin() {
                    if !cross_chain::supported_on(cache.network) {
                        self.phase = SparkSendPhase::Error(
                            "Stablecoin sends aren't available on this network.".to_string(),
                        );
                        return Task::none();
                    }
                    return Task::perform(
                        async move {
                            backend
                                .get_cross_chain_routes(input)
                                .await
                                .map_err(|e| format!("Couldn't check this destination: {e}"))
                        },
                        |result| match result {
                            Ok(found) => Message::View(crate::app::view::Message::SparkSend(
                                SparkSendMessage::CrossChainRoutesLoaded(found),
                            )),
                            Err(e) => Message::View(crate::app::view::Message::SparkSend(
                                SparkSendMessage::PrepareFailed(e),
                            )),
                        },
                    );
                }

                // A bitcoin rail: classify the destination via `parse_input` and
                // dispatch to the right prepare RPC (`prepare_send` /
                // `prepare_lnurl_pay`) in one task, so the user sees a single
                // "Preparing…" regardless of the underlying rail.
                let target = self.receive_target;
                Task::perform(
                    async move { resolve_and_prepare(backend, input, amount_sat, target).await },
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
                    // The picked target is a stablecoin, but the pasted string
                    // isn't a recognised cross-chain address. Say so rather than
                    // firing funds somewhere unrecoverable.
                    self.phase = SparkSendPhase::Error(format!(
                        "That doesn't look like a {} address. Paste the recipient's {} address \
                         on Ethereum, Tron, or Solana.",
                        self.receive_target.label(),
                        self.receive_target.label(),
                    ));
                    return Task::none();
                };
                // Keep only routes that deliver the coin the user picked in the
                // THEY RECEIVE card (USDt vs USDC) — the address alone reaches
                // both, and the picker is what disambiguates.
                let mut routes = found.routes;
                if let Some(symbol) = self.receive_target.stablecoin() {
                    routes.retain(|r| r.asset.eq_ignore_ascii_case(symbol));
                }
                if routes.is_empty() {
                    self.phase = SparkSendPhase::Error(format!(
                        "No route can currently send {} to this {} address.",
                        self.receive_target.label(),
                        address.family,
                    ));
                    return Task::none();
                }
                self.cross_chain = Some(CrossChainContext {
                    address,
                    routes,
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
                self.quote_cross_chain(cache.bitcoin_unit)
            }
            SparkSendMessage::CrossChainRetryRequested => {
                // A cross-chain retry re-sends the **same prepared quote**, not a
                // fresh one. The bridge keeps a token-leg prepare re-sendable
                // after a failure, so re-sending the identical handle reuses the
                // provider's swap id and the BTC leg dedups at the protocol level
                // — it can't pay twice. (The SDK never honoured `idempotency_key`
                // for these sends, so swap-id reuse is the real guard, not the
                // key the gui still threads through for the bitcoin rails.)
                //
                // Option-1 fallback: once the quote has expired we can't reuse
                // it, and re-preparing would mint a *new* swap id with no dedup
                // against an attempt that may already have moved funds. So stop
                // offering a blind retry and route the user to verify state.
                let SparkSendPhase::CrossChainFailed { message, policy } = &self.phase else {
                    return Task::none();
                };
                let policy = *policy;
                let message = message.clone();
                // Defense in depth: the view only offers this button when the
                // policy allows it. Never act on a stale or replayed message.
                if !policy.may_retry() {
                    return Task::none();
                }
                let reusable = self
                    .cross_chain_prepare
                    .as_ref()
                    .and_then(|p| p.cross_chain.as_deref())
                    .is_some_and(|q| {
                        cross_chain::quote_countdown(q, chrono::Utc::now()).can_send()
                    });
                if !reusable {
                    self.cross_chain_prepare = None;
                    self.phase = SparkSendPhase::CrossChainFailed {
                        message,
                        policy: cross_chain::RetryPolicy::MustCheckStateFirst,
                    };
                    return Task::none();
                }
                let prepare = self
                    .cross_chain_prepare
                    .clone()
                    .expect("reusable implies a retained prepare");
                self.dispatch_send(prepare)
            }
            SparkSendMessage::QuoteTick => {
                self.refresh_quote_countdown();
                Task::none()
            }
            SparkSendMessage::ConfirmRequested => {
                let SparkSendPhase::Prepared(prepare) = &self.phase else {
                    return Task::none();
                };
                let prepare = prepare.clone();
                // A cross-chain quote that has run out must not be sent. The
                // rate is no longer one the provider honours, so confirming
                // would either fail or fill at a price the user never saw.
                if !self.can_confirm(chrono::Utc::now()) {
                    self.phase = SparkSendPhase::Error(
                        "This quote has expired. Get a fresh quote before sending.".to_string(),
                    );
                    return Task::none();
                }
                self.dispatch_send(prepare)
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
                self.invoice_amount_sat = None;
                // Retire the idempotency key with the send it belonged to. A
                // *new* send must never reuse it, or the SDK would dedup it
                // against this one and silently drop a payment the user meant
                // to make.
                self.send_idempotency_key = None;
                self.cross_chain_prepare = None;
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
                self.invoice_amount_sat = None;
                self.phase = SparkSendPhase::Idle;
                // Reset abandons the send, so its key must go too — the next
                // send is a different payment and needs a fresh one.
                self.send_idempotency_key = None;
                self.cross_chain_prepare = None;
                self.quote_countdown = None;
                self.slippage_input.clear();
                self.cross_chain = None;
                Task::none()
            }
            SparkSendMessage::BalanceLoaded(balance) => {
                if let Some((btc_sats, stable)) = balance {
                    self.balance_sats =
                        super::unified_spark_balance_sats(btc_sats, stable.as_ref(), cache);
                }
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

/// The word for the amount field's unit, for error messages.
fn amount_unit_word(unit: BitcoinDisplayUnit) -> &'static str {
    match unit {
        BitcoinDisplayUnit::BTC => "BTC",
        BitcoinDisplayUnit::Sats => "sats",
    }
}

/// The amount a BOLT11 invoice commits to, in sats, or `None` when the input
/// isn't a complete invoice or is amountless.
///
/// Read straight out of the invoice's human-readable part (`lnbc65m1…`), which
/// is plain text ahead of the bech32 separator — no signature check, no SDK
/// round-trip, so the amount lands the instant the user pastes. It's used for
/// display only: the amount that actually gets paid still comes from the
/// invoice itself at prepare time, so a mis-read here can't send the wrong
/// number of sats.
///
/// Grammar (BOLT11 §"Human-Readable Part"): `ln` + a currency prefix (`bc`,
/// `tb`, `bcrt`, …) + an optional amount, which is a decimal followed by an
/// optional multiplier — `m`/`u`/`n`/`p` for milli/micro/nano/pico-bitcoin.
fn bolt11_amount_sat(input: &str) -> Option<u64> {
    let s = input.trim().to_ascii_lowercase();
    // Wallets and QR codes hand out `lightning:`-scheme URIs as often as bare
    // invoices; anything after a `?` is BIP21-style parameters, not the invoice.
    let s = s.strip_prefix("lightning:").unwrap_or(&s);
    let s = s.split('?').next()?;
    s.strip_prefix("ln")?;

    // The bech32 separator is the *last* `1` — the data charset excludes `1`,
    // while the amount in the prefix may well contain one (`lnbc1500n1…`).
    let sep = s.rfind('1')?;
    let (hrp, data) = (&s[..sep], &s[sep + 1..]);
    // A signature alone is 104 bech32 characters, plus a 7-character timestamp:
    // anything shorter is a partial paste, not an invoice we should read.
    if data.len() < 104 {
        return None;
    }

    // The currency prefix is alphabetic, so the amount starts at the first
    // digit. No digits at all means an amountless invoice — a perfectly normal
    // one, it just leaves the amount up to the payer.
    let after_ln = &hrp[2..];
    let digit_at = after_ln.find(|c: char| c.is_ascii_digit())?;
    let (currency, amount) = after_ln.split_at(digit_at);
    if currency.is_empty() || !currency.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }

    // Trailing multiplier, if any, then the decimal it scales. Each multiplier
    // is a fraction of a bitcoin, i.e. of 100_000_000_000 msat.
    let (digits, msat_per_unit) = match amount.chars().last()? {
        'm' => (&amount[..amount.len() - 1], 100_000_000),
        'u' => (&amount[..amount.len() - 1], 100_000),
        'n' => (&amount[..amount.len() - 1], 100),
        // Pico-bitcoin is finer than a msat, so the spec requires a multiple of
        // 10; anything else is malformed and we'd rather show nothing.
        'p' => (&amount[..amount.len() - 1], 0),
        _ => (amount, 100_000_000_000),
    };
    let value: u128 = digits.parse().ok()?;
    let msat = if msat_per_unit == 0 {
        if !value.is_multiple_of(10) {
            return None;
        }
        value / 10
    } else {
        value.checked_mul(msat_per_unit)?
    };

    // Sub-sat precision can't be shown in a sats field and can't be typed back
    // in, so round up to the sat the payer will actually part with.
    let sats: u64 = msat.div_ceil(1000).try_into().ok()?;
    (sats > 0).then_some(sats)
}

/// Render a sats amount into the send form's amount field, in the wallet's
/// display unit. Deliberately ungrouped — the field is parsed back by
/// [`parse_amount_to_sats`], which wants a plain number, not `1,000`.
fn format_amount_for_input(sats: u64, unit: BitcoinDisplayUnit) -> String {
    match unit {
        BitcoinDisplayUnit::Sats => sats.to_string(),
        BitcoinDisplayUnit::BTC => coincube_core::miniscript::bitcoin::Amount::from_sat(sats)
            .to_btc()
            .to_string(),
    }
}

/// Parse the amount field — entered in the wallet's display unit — into sats.
/// Mirrors the Vault send form: a sats wallet enters whole sats, a BTC wallet
/// enters a decimal BTC value. The stored `amount_sat` the send path uses is
/// always sats regardless.
fn parse_amount_to_sats(input: &str, unit: BitcoinDisplayUnit) -> Result<u64, String> {
    let trimmed = input.trim();
    match unit {
        BitcoinDisplayUnit::Sats => trimmed
            .parse::<u64>()
            .map_err(|_| "Amount must be a whole number of sats.".to_string()),
        BitcoinDisplayUnit::BTC => coincube_core::miniscript::bitcoin::Amount::from_str_in(
            trimmed,
            coincube_core::miniscript::bitcoin::Denomination::Bitcoin,
        )
        .map(|a| a.to_sat())
        .map_err(|_| "Amount must be a valid BTC value.".to_string()),
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
    target: SparkSendTarget,
) -> Result<PrepareSendOk, String> {
    let parsed = backend
        .parse_input(input.clone())
        .await
        .map_err(|e| format!("parse failed: {e}"))?;

    // Enforce the picked rail. The destination itself dictates the mechanism —
    // you can't send Lightning to an on-chain address — so a parsed rail that
    // clearly contradicts the selection must not prepare and reach the review
    // screen, where the "They receive" card badge (e.g. "Lightning") would
    // misrepresent an on-chain send. Reject the clear mismatches; `Other` is
    // ambiguous and passes.
    if !target.accepts_parsed(&parsed.kind) {
        let detected = match parsed.kind {
            ParseInputKind::Bolt11Invoice => "a Lightning invoice",
            ParseInputKind::LnurlPay | ParseInputKind::LightningAddress => "a Lightning address",
            ParseInputKind::BitcoinAddress => "an on-chain Bitcoin address",
            ParseInputKind::SparkAddress | ParseInputKind::SparkInvoice => "a Spark destination",
            ParseInputKind::Other => "an unrecognised destination",
        };
        return Err(format!(
            "This looks like {detected}, but you picked {} for what they receive. \
             Change that selection, or paste a matching destination.",
            target.badge(),
        ));
    }

    // A cross-chain (Solana / EVM / Tron) address parses as `Other` here — the
    // SDK's Bitcoin parser drops it in the catch-all. Under a bitcoin rail
    // that's a wrong-network paste, so probe the cross-chain parser and, if it
    // recognises the address, reject with the chain named rather than letting
    // it fail cryptically at prepare. A genuine `Other` (BOLT12) has no
    // cross-chain address and falls through to the normal prepare below.
    if matches!(parsed.kind, ParseInputKind::Other) {
        if let Ok(routes) = backend.get_cross_chain_routes(input.clone()).await {
            if let Some(addr) = routes.address {
                return Err(format!(
                    "This looks like {}, which can't receive bitcoin. Choose USDt or \
                     USDC for what they receive to send there, or paste a Bitcoin \
                     destination.",
                    cross_chain_family_label(&addr.family),
                ));
            }
        }
    }

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

/// Human label for a [`coincube_spark_protocol::CrossChainAddress`] family
/// (`"evm"` / `"solana"` / `"tron"`), for the wrong-network error copy.
fn cross_chain_family_label(family: &str) -> &'static str {
    match family {
        "solana" => "a Solana address",
        "tron" => "a Tron address",
        "evm" => "an EVM (Ethereum) address",
        _ => "a cross-chain address",
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

/// Spark-Send-flavoured wrapper around [`super::fetch_balance_task`].
fn fetch_balance_task(backend: Option<Arc<SparkBackend>>) -> Task<Message> {
    super::fetch_balance_task(backend, |balance| {
        Message::View(crate::app::view::Message::SparkSend(
            crate::app::view::SparkSendMessage::BalanceLoaded(balance),
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::view::{Message as ViewMessage, SparkSendMessage};

    #[test]
    fn amount_parses_in_the_configured_unit() {
        // Sats mode: whole integers only.
        assert_eq!(
            parse_amount_to_sats("30000", BitcoinDisplayUnit::Sats),
            Ok(30_000)
        );
        assert!(parse_amount_to_sats("0.5", BitcoinDisplayUnit::Sats).is_err());

        // BTC mode: decimals convert to sats.
        assert_eq!(
            parse_amount_to_sats("0.0003", BitcoinDisplayUnit::BTC),
            Ok(30_000)
        );
        assert_eq!(
            parse_amount_to_sats(" 1 ", BitcoinDisplayUnit::BTC),
            Ok(100_000_000)
        );
        // More than 8 decimal places is sub-sat precision — rejected.
        assert!(parse_amount_to_sats("0.000000001", BitcoinDisplayUnit::BTC).is_err());
        assert!(parse_amount_to_sats("abc", BitcoinDisplayUnit::BTC).is_err());
    }

    #[test]
    fn send_targets_define_picker_order_labels_badges_and_placeholders() {
        assert_eq!(
            SparkSendTarget::all(),
            [
                SparkSendTarget::Lightning,
                SparkSendTarget::OnChain,
                SparkSendTarget::Spark,
                SparkSendTarget::Usdt,
                SparkSendTarget::Usdc,
            ]
        );

        assert_eq!(SparkSendTarget::Lightning.label(), "Bitcoin");
        assert_eq!(SparkSendTarget::Lightning.badge(), "Lightning");
        assert_eq!(
            SparkSendTarget::Lightning.destination_placeholder(),
            "Lightning invoice or Lightning address"
        );

        assert_eq!(SparkSendTarget::OnChain.badge(), "On-chain");
        assert_eq!(SparkSendTarget::Spark.badge(), "Spark");
        assert_eq!(SparkSendTarget::Usdt.label(), "USDt");
        assert_eq!(SparkSendTarget::Usdt.badge(), "Cross-chain");
        assert_eq!(SparkSendTarget::Usdt.stablecoin(), Some("USDT"));
        assert_eq!(SparkSendTarget::Usdc.stablecoin(), Some("USDC"));
        assert!(SparkSendTarget::Usdc.is_stablecoin());
        assert!(!SparkSendTarget::Spark.is_stablecoin());
    }

    #[test]
    fn amount_error_messages_name_the_active_unit() {
        assert_eq!(amount_unit_word(BitcoinDisplayUnit::BTC), "BTC");
        assert_eq!(amount_unit_word(BitcoinDisplayUnit::Sats), "sats");
        assert_eq!(
            parse_amount_to_sats("", BitcoinDisplayUnit::Sats).unwrap_err(),
            "Amount must be a whole number of sats."
        );
        assert_eq!(
            parse_amount_to_sats("-1", BitcoinDisplayUnit::BTC).unwrap_err(),
            "Amount must be a valid BTC value."
        );
    }

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

    fn route_with_asset(asset: &str) -> CrossChainRoute {
        CrossChainRoute {
            asset: asset.to_string(),
            ..route()
        }
    }

    fn cross_chain_address(family: &str) -> CrossChainAddress {
        CrossChainAddress {
            address: "0xabc".to_string(),
            family: family.to_string(),
            contract_address: None,
            chain_id: None,
            amount: None,
        }
    }

    /// A panel sitting on a failed cross-chain send, holding the retained quote
    /// (with the given expiry) plus the idempotency key the failed attempt left
    /// behind — the exact state a retry acts on.
    fn failed_panel_with_expiry(policy: cross_chain::RetryPolicy, expires_at: &str) -> SparkSend {
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
        panel.cross_chain_prepare = Some(prepared_with_quote(expires_at));
        panel.phase = SparkSendPhase::CrossChainFailed {
            message: "connection lost".to_string(),
            policy,
        };
        panel
    }

    /// [`failed_panel_with_expiry`] with a quote that's still well in date.
    fn failed_panel(policy: cross_chain::RetryPolicy) -> SparkSend {
        let future = (chrono::Utc::now() + chrono::Duration::seconds(60)).to_rfc3339();
        failed_panel_with_expiry(policy, &future)
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

    /// The retry path: re-send the **same** retained quote (same swap id), not a
    /// re-prepare — so the provider dedups the BTC leg instead of paying twice.
    #[test]
    fn retry_resends_the_same_quote_while_it_is_still_valid() {
        let mut panel = failed_panel(cross_chain::RetryPolicy::SafeToRetry);
        let _ = send(&mut panel, SparkSendMessage::CrossChainRetryRequested);

        // With a still-valid retained quote the retry fires the send. This panel
        // has no backend, so it stops at the backend check — the point is it
        // reached the *send* (reusing the quote), not a re-prepare or a refusal.
        assert!(
            matches!(panel.phase, SparkSendPhase::Error(_)),
            "a valid-quote retry must reach the send path (backend-absent here)"
        );

        // A retry never mints a fresh key — reusing it keeps the bitcoin-rail
        // path dedup-safe, and it's harmless (dropped by the bridge) for this
        // cross-chain send.
        assert_eq!(
            panel.send_idempotency_key.as_deref(),
            Some("key-from-the-failed-attempt"),
            "a retry must reuse the failed attempt's idempotency key"
        );
    }

    /// Option-1 fallback: a retry whose quote has already expired can't reuse it
    /// (re-preparing would mint a new swap id and could double-send), so it
    /// downgrades to "check state first" instead of firing.
    #[test]
    fn retry_falls_back_to_check_state_when_the_quote_has_expired() {
        let past = (chrono::Utc::now() - chrono::Duration::seconds(5)).to_rfc3339();
        let mut panel = failed_panel_with_expiry(cross_chain::RetryPolicy::SafeToRetry, &past);
        let _ = send(&mut panel, SparkSendMessage::CrossChainRetryRequested);

        match &panel.phase {
            SparkSendPhase::CrossChainFailed { policy, .. } => assert_eq!(
                *policy,
                cross_chain::RetryPolicy::MustCheckStateFirst,
                "an expired-quote retry must downgrade to check-state-first"
            ),
            other => panic!("expected CrossChainFailed, got {:?}", other),
        }
        assert!(
            panel.cross_chain_prepare.is_none(),
            "the unusable quote must be dropped so it can't be retried again"
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
            "an unsafe retry must not re-send, even if the message arrives"
        );
    }

    /// A bitcoin rail must not prepare a destination from a different rail —
    /// otherwise a mismatched send reaches review under a card badge that lies
    /// about what's happening. `resolve_and_prepare` gates on this.
    #[test]
    fn a_bitcoin_rail_target_only_accepts_its_own_destination_kind() {
        use coincube_spark_protocol::ParseInputKind as K;

        // Lightning accepts the Lightning family, not on-chain / Spark.
        assert!(SparkSendTarget::Lightning.accepts_parsed(&K::Bolt11Invoice));
        assert!(SparkSendTarget::Lightning.accepts_parsed(&K::LightningAddress));
        assert!(SparkSendTarget::Lightning.accepts_parsed(&K::LnurlPay));
        assert!(!SparkSendTarget::Lightning.accepts_parsed(&K::BitcoinAddress));
        assert!(!SparkSendTarget::Lightning.accepts_parsed(&K::SparkAddress));

        // On-chain accepts only a bitcoin address.
        assert!(SparkSendTarget::OnChain.accepts_parsed(&K::BitcoinAddress));
        assert!(!SparkSendTarget::OnChain.accepts_parsed(&K::Bolt11Invoice));
        assert!(!SparkSendTarget::OnChain.accepts_parsed(&K::SparkInvoice));

        // Spark accepts Spark destinations, not on-chain.
        assert!(SparkSendTarget::Spark.accepts_parsed(&K::SparkAddress));
        assert!(SparkSendTarget::Spark.accepts_parsed(&K::SparkInvoice));
        assert!(!SparkSendTarget::Spark.accepts_parsed(&K::BitcoinAddress));

        // `Other` (BOLT12 / silent payment) is ambiguous — never rejected.
        assert!(SparkSendTarget::Lightning.accepts_parsed(&K::Other));
        assert!(SparkSendTarget::OnChain.accepts_parsed(&K::Other));
        assert!(SparkSendTarget::Spark.accepts_parsed(&K::Other));
    }

    /// A BOLT11 invoice with `hrp` as its human-readable part. Only the HRP and
    /// the data part's *length* matter to `bolt11_amount_sat`, so the payload is
    /// filler of a realistic size (a signature alone is 104 characters).
    fn invoice(hrp: &str) -> String {
        format!("{hrp}1{}", "q".repeat(110))
    }

    #[test]
    fn bolt11_amount_reads_each_multiplier() {
        // The BOLT11 spec's own test vectors.
        assert_eq!(bolt11_amount_sat(&invoice("lnbc2500u")), Some(250_000));
        assert_eq!(bolt11_amount_sat(&invoice("lnbc20m")), Some(2_000_000));
        assert_eq!(
            bolt11_amount_sat(&invoice("lnbc25000000n")),
            Some(2_500_000)
        );
        // 0.00967878534 BTC — sub-sat precision, rounded up to the sat the
        // payer actually parts with.
        assert_eq!(
            bolt11_amount_sat(&invoice("lnbc9678785340p")),
            Some(967_879)
        );
        // No multiplier: a whole bitcoin.
        assert_eq!(bolt11_amount_sat(&invoice("lnbc1")), Some(100_000_000));
    }

    #[test]
    fn bolt11_amount_handles_testnet_prefixes_and_uri_wrappers() {
        assert_eq!(bolt11_amount_sat(&invoice("lntb20m")), Some(2_000_000));
        assert_eq!(bolt11_amount_sat(&invoice("lnbcrt500u")), Some(50_000));
        // QR payloads arrive uppercase, and wallets hand out `lightning:` URIs.
        assert_eq!(
            bolt11_amount_sat(&invoice("lnbc2500u").to_uppercase()),
            Some(250_000)
        );
        assert_eq!(
            bolt11_amount_sat(&format!("lightning:{}?label=x", invoice("lnbc2500u"))),
            Some(250_000)
        );
        assert_eq!(
            bolt11_amount_sat(&format!("  {}  ", invoice("lnbc20m"))),
            Some(2_000_000)
        );
    }

    #[test]
    fn bolt11_amount_is_none_when_there_is_nothing_to_read() {
        // Amountless invoice — the payer chooses, so the field stays editable.
        assert_eq!(bolt11_amount_sat(&invoice("lnbc")), None);
        // Not an invoice at all.
        assert_eq!(bolt11_amount_sat(""), None);
        assert_eq!(
            bolt11_amount_sat("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4"),
            None
        );
        assert_eq!(bolt11_amount_sat("satoshi@example.com"), None);
        // Half-pasted: too short to be a signed invoice, so reading an amount
        // off it would show a number for something that can't be paid.
        assert_eq!(bolt11_amount_sat("lnbc20m1pvjluezpp5"), None);
        // Malformed amounts.
        assert_eq!(bolt11_amount_sat(&invoice("lnbc20x")), None);
        assert_eq!(bolt11_amount_sat(&invoice("lnbc0m")), None);
        // Pico-bitcoin that isn't a whole msat is invalid per the spec.
        assert_eq!(bolt11_amount_sat(&invoice("lnbc9678785341p")), None);
    }

    #[test]
    fn pasting_an_invoice_fills_the_amount_field_and_dropping_it_clears_it() {
        let mut panel = SparkSend::new(None);

        let _ = send(
            &mut panel,
            SparkSendMessage::DestinationInputChanged(invoice("lnbc2500u")),
        );
        assert_eq!(panel.invoice_amount_sat, Some(250_000));
        assert_eq!(panel.amount_input, "250000");

        // An amountless invoice takes the previous invoice's amount away with
        // it, rather than leaving a stale number the user might send.
        let _ = send(
            &mut panel,
            SparkSendMessage::DestinationInputChanged(invoice("lnbc")),
        );
        assert!(panel.invoice_amount_sat.is_none());
        assert!(panel.amount_input.is_empty());
    }

    #[test]
    fn an_invoice_amount_round_trips_through_the_field_in_either_unit() {
        // Whatever the field is filled with has to parse back out of it, or a
        // user who edits the destination away is left holding a value the form
        // rejects.
        for unit in [BitcoinDisplayUnit::Sats, BitcoinDisplayUnit::BTC] {
            for sats in [1, 250_000, 100_000_000] {
                let rendered = format_amount_for_input(sats, unit);
                assert_eq!(parse_amount_to_sats(&rendered, unit), Ok(sats));
            }
        }
        assert_eq!(
            format_amount_for_input(250_000, BitcoinDisplayUnit::BTC),
            "0.0025"
        );
    }

    #[test]
    fn a_typed_amount_survives_a_destination_that_carries_none() {
        let mut panel = SparkSend::new(None);
        panel.amount_input = "1000".to_string();

        let _ = send(
            &mut panel,
            SparkSendMessage::DestinationInputChanged(
                "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4".to_string(),
            ),
        );

        assert!(panel.invoice_amount_sat.is_none());
        assert_eq!(panel.amount_input, "1000");
    }

    #[test]
    fn cross_chain_family_label_names_the_chain() {
        assert_eq!(cross_chain_family_label("solana"), "a Solana address");
        assert_eq!(cross_chain_family_label("tron"), "a Tron address");
        assert_eq!(cross_chain_family_label("evm"), "an EVM (Ethereum) address");
        assert_eq!(
            cross_chain_family_label("dogecoin"),
            "a cross-chain address"
        );
    }

    #[test]
    fn cross_chain_context_selected_route_obeys_bounds() {
        let ctx = CrossChainContext {
            address: cross_chain_address("evm"),
            routes: vec![route_with_asset("USDT"), route_with_asset("USDC")],
            selected: 1,
        };
        assert_eq!(ctx.selected_route().map(|r| r.asset.as_str()), Some("USDC"));

        let out_of_bounds = CrossChainContext {
            selected: 99,
            ..ctx
        };
        assert!(out_of_bounds.selected_route().is_none());
    }

    #[test]
    fn set_receive_target_clears_the_previous_payment_intent() {
        let mut panel = failed_panel(cross_chain::RetryPolicy::SafeToRetry);
        panel.destination_input = "old destination".to_string();
        panel.amount_input = "123".to_string();
        panel.receive_picker_open = true;

        let _ = send(
            &mut panel,
            SparkSendMessage::SetReceiveTarget(SparkSendTarget::Usdt),
        );

        assert_eq!(panel.receive_target, SparkSendTarget::Usdt);
        assert!(!panel.receive_picker_open);
        assert!(panel.destination_input.is_empty());
        assert!(panel.amount_input.is_empty());
        assert!(panel.send_idempotency_key.is_none());
        assert!(panel.cross_chain_prepare.is_none());
        assert!(panel.cross_chain.is_none());
        assert!(matches!(panel.phase, SparkSendPhase::Idle));
    }

    #[test]
    fn cross_chain_routes_loaded_filters_to_the_selected_stablecoin() {
        let mut panel = SparkSend::new(None);
        panel.receive_target = SparkSendTarget::Usdt;

        let _ = send(
            &mut panel,
            SparkSendMessage::CrossChainRoutesLoaded(coincube_spark_protocol::CrossChainRoutesOk {
                address: Some(cross_chain_address("tron")),
                routes: vec![
                    route_with_asset("USDC"),
                    route_with_asset("USDT"),
                    route_with_asset("usdt"),
                ],
            }),
        );

        assert!(matches!(panel.phase, SparkSendPhase::CrossChainRoutes));
        let ctx = panel.cross_chain.as_ref().expect("routes retained");
        assert_eq!(ctx.address.family, "tron");
        assert_eq!(ctx.routes.len(), 2);
        assert!(ctx
            .routes
            .iter()
            .all(|r| r.asset.eq_ignore_ascii_case("USDT")));
        assert_eq!(ctx.selected, 0);
    }

    #[test]
    fn cross_chain_routes_loaded_reports_unrecognized_or_unroutable_destinations() {
        let mut panel = SparkSend::new(None);
        panel.receive_target = SparkSendTarget::Usdc;

        let _ = send(
            &mut panel,
            SparkSendMessage::CrossChainRoutesLoaded(coincube_spark_protocol::CrossChainRoutesOk {
                address: None,
                routes: vec![],
            }),
        );
        assert!(matches!(
            &panel.phase,
            SparkSendPhase::Error(msg) if msg.contains("doesn't look like a USDC address")
        ));

        let _ = send(
            &mut panel,
            SparkSendMessage::CrossChainRoutesLoaded(coincube_spark_protocol::CrossChainRoutesOk {
                address: Some(cross_chain_address("solana")),
                routes: vec![route_with_asset("USDT")],
            }),
        );
        assert!(matches!(
            &panel.phase,
            SparkSendPhase::Error(msg)
                if msg == "No route can currently send USDC to this solana address."
        ));
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

    #[test]
    fn receive_picker_open_and_close_are_local_state_only() {
        let mut panel = SparkSend::new(None);

        let _ = send(&mut panel, SparkSendMessage::OpenReceivePicker);
        assert!(panel.receive_picker_open);
        assert!(matches!(panel.phase, SparkSendPhase::Idle));
        assert_eq!(panel.receive_target, SparkSendTarget::Lightning);

        let _ = send(&mut panel, SparkSendMessage::CloseReceivePicker);
        assert!(!panel.receive_picker_open);
        assert!(matches!(panel.phase, SparkSendPhase::Idle));
        assert_eq!(panel.receive_target, SparkSendTarget::Lightning);
    }

    #[test]
    fn advanced_and_slippage_controls_only_update_their_local_fields() {
        let mut panel = SparkSend::new(None);
        assert!(!panel.advanced_open);

        let _ = send(&mut panel, SparkSendMessage::ToggleAdvanced);
        assert!(panel.advanced_open);

        let _ = send(
            &mut panel,
            SparkSendMessage::SlippageChanged("250".to_string()),
        );
        assert_eq!(panel.slippage_input, "250");
        assert!(panel.advanced_open);
        assert!(matches!(panel.phase, SparkSendPhase::Idle));

        let _ = send(&mut panel, SparkSendMessage::ToggleAdvanced);
        assert!(!panel.advanced_open);
        assert_eq!(panel.slippage_input, "250");
    }

    #[test]
    fn route_selection_updates_valid_indices_and_ignores_out_of_bounds() {
        let mut panel = SparkSend::new(None);
        panel.cross_chain = Some(CrossChainContext {
            address: cross_chain_address("evm"),
            routes: vec![route_with_asset("USDT"), route_with_asset("USDC")],
            selected: 0,
        });
        panel.phase = SparkSendPhase::CrossChainRoutes;

        let _ = send(&mut panel, SparkSendMessage::CrossChainRouteSelected(1));
        assert_eq!(panel.cross_chain.as_ref().unwrap().selected, 1);

        let _ = send(&mut panel, SparkSendMessage::CrossChainRouteSelected(99));
        assert_eq!(
            panel.cross_chain.as_ref().unwrap().selected,
            1,
            "out-of-bounds route selections must leave the current route alone"
        );
    }

    #[test]
    fn quote_request_requires_a_selected_route_before_backend_or_amount_work() {
        let mut no_context = SparkSend::new(None);
        let _ = send(&mut no_context, SparkSendMessage::CrossChainQuoteRequested);
        assert!(matches!(no_context.phase, SparkSendPhase::Idle));

        let mut missing_route = SparkSend::new(None);
        missing_route.cross_chain = Some(CrossChainContext {
            address: cross_chain_address("evm"),
            routes: vec![route()],
            selected: 99,
        });
        missing_route.phase = SparkSendPhase::CrossChainRoutes;

        let _ = send(
            &mut missing_route,
            SparkSendMessage::CrossChainQuoteRequested,
        );
        assert!(
            matches!(missing_route.phase, SparkSendPhase::CrossChainRoutes),
            "without a selected route, quote request must be a no-op"
        );
    }

    #[test]
    fn quote_request_with_a_route_fails_fast_when_backend_is_missing() {
        let mut panel = SparkSend::new(None);
        panel.cross_chain = Some(CrossChainContext {
            address: cross_chain_address("evm"),
            routes: vec![route()],
            selected: 0,
        });
        panel.amount_input = "1000".to_string();

        let _ = send(&mut panel, SparkSendMessage::CrossChainQuoteRequested);
        assert!(matches!(
            &panel.phase,
            SparkSendPhase::Error(msg) if msg == "Spark backend is not available."
        ));
    }

    fn sent_panel_after_method(method: &str) -> SparkSend {
        let mut panel = failed_panel(cross_chain::RetryPolicy::SafeToRetry);
        panel.destination_input = "destination".to_string();
        panel.amount_input = "123".to_string();
        panel.last_send_method = method.to_string();

        let _ = send(
            &mut panel,
            SparkSendMessage::SendSucceeded(SendPaymentOk {
                payment_id: "payment-id".to_string(),
                amount_sat: 42_000,
                fee_sat: 17,
            }),
        );
        panel
    }

    #[test]
    fn send_success_clears_the_payment_intent_and_formats_the_sent_amount() {
        let panel = sent_panel_after_method("BitcoinAddress");

        assert!(matches!(panel.phase, SparkSendPhase::Sent(_)));
        assert_eq!(panel.sent_amount_display, "42,000 sats");
        assert!(panel.destination_input.is_empty());
        assert!(panel.amount_input.is_empty());
        assert!(panel.send_idempotency_key.is_none());
        assert!(panel.cross_chain_prepare.is_none());
        assert!(panel.quote_countdown.is_none());
        assert!(panel.cross_chain.is_none());
    }

    #[test]
    fn send_success_picks_celebration_context_from_the_send_method() {
        assert_eq!(
            sent_panel_after_method("BitcoinAddress").sent_celebration_context,
            "bitcoin-send"
        );
        assert_eq!(
            sent_panel_after_method("Bolt11Invoice").sent_celebration_context,
            "lightning-send"
        );
        assert_eq!(
            sent_panel_after_method("LnurlPay").sent_celebration_context,
            "lightning-send"
        );
        assert_eq!(
            sent_panel_after_method("SparkAddress").sent_celebration_context,
            "spark-send"
        );
    }
}
