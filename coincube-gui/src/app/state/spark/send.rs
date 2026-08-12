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

use coincube_core::miniscript::bitcoin::bech32;
use coincube_spark_protocol::{
    CrossChainAddress, CrossChainRoute, ParseInputKind, PrepareSendOk, SendPaymentOk,
};
use coincube_ui::component::amount::{format_u64_as_string, BitcoinDisplayUnit};
use coincube_ui::widget::Element;
use iced::Task;

use super::cross_chain;
use crate::app::breez_spark::client::SparkClientError;
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
    /// The send was dispatched and we do not know what happened to it.
    ///
    /// Kept strictly apart from [`Self::Error`], which means the bridge
    /// answered and the answer was "no". Here nobody answered: the SDK may
    /// have paid, may be about to, or may have done nothing. Reporting that as
    /// a failure — which is what the generic 30s query timeout used to do — is
    /// how a single payment becomes two, because the failure screen's "Try
    /// again" resets the flow and mints a fresh idempotency key.
    ///
    /// The intent and its key are held across this state, and no new send is
    /// offered until [`ReconcileOutcome`] says what happened.
    OutcomeUnknown {
        message: String,
        /// `None` while the first check is still running.
        outcome: Option<ReconcileOutcome>,
        /// Whether a check is in flight right now.
        checking: bool,
        /// What protects a retry on this payment's rail. Carried in the phase
        /// so the screen renders from the payment that was actually
        /// dispatched, not from panel state that may since have moved on.
        guard: RetryGuard,
    },
}

/// What can protect a retry of *this* payment from paying twice.
///
/// Not a property of the panel but of the rail the payment is on, so it is
/// recorded when the send is dispatched rather than inferred later.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryGuard {
    /// The bitcoin rails (Lightning / on-chain / Spark / LNURL). The SDK
    /// honours `idempotency_key`, deriving the transfer id from it, so
    /// re-sending under the same key cannot pay twice — even across a fresh
    /// prepare, because the guard is the key, not the handle.
    IdempotencyKey,
    /// A token or conversion leg — every cross-chain send. The SDK rejects an
    /// idempotency key here, so the bridge drops it and the only dedup is
    /// re-sending the identical provider quote. Once that is gone, nothing
    /// protects a retry and the user must be sent to check the payment's real
    /// state instead.
    None,
}

/// What a reconciliation check concluded about a payment with an unknown
/// outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconcileOutcome {
    /// The payment exists. It went through; there is nothing to retry.
    Landed,
    /// The bridge's own late answer said the send was rejected. Definite, so
    /// the ordinary correct-and-retry path applies.
    DefinitelyFailed(String),
    /// Nothing matching the intent is in the payment history. Combined with
    /// [`RetryGuard::IdempotencyKey`] this makes another attempt safe — and if
    /// the history was merely stale, the key dedups it anyway.
    NoTrace,
    /// The check itself could not be completed (history unavailable), or the
    /// rail offers no authoritative way to tell. Never offer a blind retry
    /// from here.
    Inconclusive(String),
}

impl ReconcileOutcome {
    /// Whether the panel may offer to send again, given the rail's guard.
    ///
    /// Fail-closed by construction: only the combination of "no trace of this
    /// payment" **and** "a retry is idempotency-protected" is safe. Everything
    /// else — landed, inconclusive, or an unguarded rail — routes the user to
    /// the payment history instead.
    pub fn may_resend(&self, guard: RetryGuard) -> bool {
        matches!(self, Self::NoTrace) && matches!(guard, RetryGuard::IdempotencyKey)
    }

    /// What to tell the user.
    pub fn guidance(&self, guard: RetryGuard) -> String {
        match self {
            Self::Landed => "This payment did go through. It's in your transaction history — \
                 don't send it again."
                .to_string(),
            Self::DefinitelyFailed(msg) => {
                format!("The payment was rejected, so nothing was sent: {msg}")
            }
            Self::NoTrace => match guard {
                RetryGuard::IdempotencyKey => {
                    "No payment matching this one is in your history. Sending again reuses \
                     the same payment key, so if the first attempt did land after all, it \
                     can't be paid twice."
                        .to_string()
                }
                RetryGuard::None => {
                    "No payment matching this one is in your history, but this route can't \
                     guarantee a retry won't send twice. Refresh and check Transactions \
                     before sending again."
                        .to_string()
                }
            },
            Self::Inconclusive(msg) => format!(
                "Couldn't confirm what happened to this payment ({msg}). Refresh and check \
                 Transactions before sending again."
            ),
        }
    }
}

/// How far before the dispatch timestamp a matching payment is still accepted
/// as "this one".
///
/// The SDK stamps a payment when *it* recorded it, which can be a moment before
/// the gui's own `dispatched_at` (clock skew between the gui process and the
/// bridge, and the SDK timestamping on entry rather than on completion). Zero
/// slack would make a payment that plainly is ours look like it isn't — and
/// that mistake points the wrong way: it would report `NoTrace` for a payment
/// that landed. Sixty seconds is comfortably more than any plausible skew and
/// still far short of "some earlier payment of the same size".
const RECONCILE_BACKDATE_SLACK_SECS: i64 = 60;

/// Whether the payment history contains the send described by `pending`.
///
/// Deliberately conservative in the direction that matters: it errs towards
/// *finding* a match (which blocks a resend) rather than missing one (which
/// would enable one). An outgoing payment of the same size, no older than the
/// dispatch minus [`RECONCILE_BACKDATE_SLACK_SECS`], is treated as this
/// payment.
///
/// This is inference, not proof — the SDK has no "look up by idempotency key"
/// call, so there is nothing better available. It is only ever used to decide
/// between "definitely landed" and "no evidence"; the safety of acting on the
/// latter rests on the idempotency key, not on this function being right.
fn payment_matches_intent(
    payments: &[coincube_spark_protocol::PaymentSummary],
    pending: &PendingSend,
) -> bool {
    let floor = pending.dispatched_at - RECONCILE_BACKDATE_SLACK_SECS;
    payments.iter().any(|p| {
        p.direction.eq_ignore_ascii_case("outgoing")
            && p.timestamp as i64 >= floor
            && p.amount_sat.unsigned_abs() == pending.amount_sat
    })
}

/// Which guard protects a retry of this prepared payment.
///
/// Decided from the prepare itself rather than from a method-name string, so a
/// new payment method cannot quietly default to "guarded".
///
/// `has_token_leg` is the authoritative half: the bridge reports it from the
/// *same* condition that makes `execute_regular_send` drop the idempotency key
/// (which mirrors the SDK's own gate), so the two cannot drift. It used to be
/// inferred from `cross_chain.is_some()` instead, on the assumption that a
/// cross-chain quote was the only way to get a token leg. It isn't: Stable
/// Balance auto-attaches a token→BTC conversion to an ordinary sats send when
/// the sat balance can't cover amount + fee, and that send reported itself as
/// idempotency-guarded while the bridge had already dropped the key.
///
/// `cross_chain` is still checked, and still means unguarded, so that a
/// cross-chain prepare that somehow arrives without a token leg cannot come
/// back as "safe to retry".
pub fn retry_guard_for(prepare: &PrepareSendOk) -> RetryGuard {
    if prepare.has_token_leg || prepare.cross_chain.is_some() {
        RetryGuard::None
    } else {
        RetryGuard::IdempotencyKey
    }
}

/// The payment a send was dispatched for, kept across an unknown outcome.
///
/// Everything reconciliation needs to recognise the payment in the history,
/// plus the guard that decides whether another attempt may be offered at all.
#[derive(Debug, Clone)]
pub struct PendingSend {
    /// Client request id, for claiming the bridge's late answer.
    pub request_id: Option<u64>,
    /// Unix seconds when the send was dispatched. Bounds the history search:
    /// an older payment of the same size is somebody else's.
    pub dispatched_at: i64,
    /// Amount in sats, as prepared.
    pub amount_sat: u64,
    pub guard: RetryGuard,
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
    /// The dispatched payment, retained so an unknown outcome can be
    /// reconciled and — only if reconciliation clears it — retried under the
    /// same key. `None` whenever no send is in flight or unresolved.
    pending_send: Option<PendingSend>,
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
            pending_send: None,
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
        self.pending_send = None;
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
        // A cross-chain send carries a token/conversion leg, so the bridge
        // drops the idempotency key the SDK would reject; its only dedup is
        // re-sending the identical quote. Everything else is on the bitcoin
        // rails, where the key *is* the guard.
        let guard = retry_guard_for(&prepare);
        let amount_sat = prepare.amount_sat;
        if prepare.cross_chain.is_some() {
            self.cross_chain_prepare = Some(prepare);
        }
        let key = self
            .send_idempotency_key
            .get_or_insert_with(|| uuid::Uuid::new_v4().to_string())
            .clone();

        // Recorded *before* dispatch: if the answer never comes, this is all
        // reconciliation will have to go on.
        self.pending_send = Some(PendingSend {
            request_id: None,
            dispatched_at: chrono::Utc::now().timestamp(),
            amount_sat,
            guard,
        });

        self.phase = SparkSendPhase::Sending;
        Task::perform(
            async move { backend.send_payment(handle, Some(key)).await },
            move |result| match result {
                Ok(ok) => Message::View(crate::app::view::Message::SparkSend(
                    SparkSendMessage::SendSucceeded(ok),
                )),
                // An indeterminate outcome is routed to its own message on
                // every rail. It is not a cross-chain failure and not a plain
                // failure: nobody has said the payment didn't happen.
                Err(SparkClientError::OutcomeUnknown {
                    request_id,
                    message,
                }) => Message::View(crate::app::view::Message::SparkSend(
                    SparkSendMessage::SendOutcomeUnknown {
                        request_id,
                        message,
                    },
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

    /// Check what actually happened to a payment whose outcome is unknown.
    ///
    /// Two sources, in order of authority:
    ///
    /// 1. **The bridge's own late answer.** If it has arrived since the caller
    ///    gave up, it is a direct statement about *this* request and settles
    ///    the question outright — including when it says the send was rejected.
    /// 2. **The payment history**, bounded to payments at or after the moment
    ///    of dispatch. This is inference, not proof, so it is used only to say
    ///    "found it" or "nothing there"; the safety of acting on "nothing
    ///    there" comes from [`RetryGuard::IdempotencyKey`], not from this list
    ///    being complete.
    ///
    /// Anything that cannot be established returns
    /// [`ReconcileOutcome::Inconclusive`], which never enables a resend.
    fn reconcile_unknown_send(&self) -> Task<Message> {
        use crate::app::view::SparkSendMessage;
        let (Some(backend), Some(pending)) = (self.backend.clone(), self.pending_send.clone())
        else {
            return Task::done(Message::View(crate::app::view::Message::SparkSend(
                SparkSendMessage::ReconcileFinished(ReconcileOutcome::Inconclusive(
                    "the payment's details are no longer available".to_string(),
                )),
            )));
        };

        Task::perform(
            async move {
                if let Some(request_id) = pending.request_id {
                    match backend.client().take_late_outcome(request_id).await {
                        Some(Ok(_sent)) => return ReconcileOutcome::Landed,
                        Some(Err(SparkClientError::BridgeError { message, .. })) => {
                            return ReconcileOutcome::DefinitelyFailed(message);
                        }
                        Some(Err(e)) => return ReconcileOutcome::Inconclusive(e.to_string()),
                        None => {}
                    }
                }

                match backend.list_payments(Some(50), None).await {
                    Ok(list) => {
                        if payment_matches_intent(&list.payments, &pending) {
                            ReconcileOutcome::Landed
                        } else {
                            ReconcileOutcome::NoTrace
                        }
                    }
                    Err(e) => ReconcileOutcome::Inconclusive(e.to_string()),
                }
            },
            |outcome| {
                Message::View(crate::app::view::Message::SparkSend(
                    SparkSendMessage::ReconcileFinished(outcome),
                ))
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
                // Definite: the bridge answered. The retained quote is what
                // guards the retry here, not the key.
                self.pending_send = None;
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
                self.pending_send = None;
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
                // A definite rejection: the bridge answered. The payment did
                // not happen, so the ordinary correct-and-retry path applies
                // and `Reset` may safely mint a new key.
                self.pending_send = None;
                self.phase = SparkSendPhase::Error(err);
                Task::none()
            }
            SparkSendMessage::SendOutcomeUnknown {
                request_id,
                message,
            } => {
                // Everything about the payment is kept: the key, the inputs,
                // and now the request id, so a late answer can still be
                // claimed. Nothing here says the payment failed.
                if let Some(pending) = self.pending_send.as_mut() {
                    pending.request_id = Some(request_id);
                } else {
                    // Defensive: an unknown outcome with no recorded intent
                    // can still be reconciled against the bridge's late answer.
                    self.pending_send = Some(PendingSend {
                        request_id: Some(request_id),
                        dispatched_at: chrono::Utc::now().timestamp(),
                        amount_sat: 0,
                        guard: RetryGuard::None,
                    });
                }
                let guard = self
                    .pending_send
                    .as_ref()
                    .map(|p| p.guard)
                    .unwrap_or(RetryGuard::None);
                self.phase = SparkSendPhase::OutcomeUnknown {
                    message,
                    outcome: None,
                    checking: true,
                    guard,
                };
                self.reconcile_unknown_send()
            }
            SparkSendMessage::ReconcileRequested => {
                let SparkSendPhase::OutcomeUnknown {
                    message,
                    checking,
                    guard,
                    ..
                } = &self.phase
                else {
                    return Task::none();
                };
                if *checking {
                    return Task::none();
                }
                self.phase = SparkSendPhase::OutcomeUnknown {
                    message: message.clone(),
                    outcome: None,
                    checking: true,
                    guard: *guard,
                };
                self.reconcile_unknown_send()
            }
            SparkSendMessage::ReconcileFinished(outcome) => {
                let SparkSendPhase::OutcomeUnknown { message, guard, .. } = &self.phase else {
                    return Task::none();
                };
                let message = message.clone();
                let guard = *guard;
                match &outcome {
                    // The bridge answered late and said it was rejected: that
                    // is a definite failure, so the panel can offer the normal
                    // start-over path.
                    ReconcileOutcome::DefinitelyFailed(reason) => {
                        self.pending_send = None;
                        self.phase = SparkSendPhase::Error(reason.clone());
                    }
                    _ => {
                        self.phase = SparkSendPhase::OutcomeUnknown {
                            message,
                            outcome: Some(outcome),
                            checking: false,
                            guard,
                        };
                    }
                }
                // Refresh the visible history either way — the user is being
                // told to check it.
                fetch_payments_task(self.backend.clone())
            }
            SparkSendMessage::ResendAfterUnknownRequested => {
                // Only from a reconciliation that found no trace, and only on a
                // rail whose retry is idempotency-protected. The key and the
                // inputs are untouched, so re-preparing and confirming re-sends
                // *this* payment rather than minting a second one.
                let SparkSendPhase::OutcomeUnknown {
                    outcome: Some(outcome),
                    checking: false,
                    guard,
                    ..
                } = &self.phase
                else {
                    return Task::none();
                };
                let guard = *guard;
                if !outcome.may_resend(guard) {
                    return Task::none();
                }
                debug_assert!(
                    self.send_idempotency_key.is_some(),
                    "a resend must reuse the original key"
                );
                self.pending_send = None;
                self.phase = SparkSendPhase::Idle;
                Task::done(Message::View(crate::app::view::Message::SparkSend(
                    SparkSendMessage::PrepareRequested,
                )))
            }
            SparkSendMessage::Reset => {
                self.destination_input.clear();
                self.amount_input.clear();
                self.invoice_amount_sat = None;
                self.phase = SparkSendPhase::Idle;
                // Reset abandons the send, so its key must go too — the next
                // send is a different payment and needs a fresh one.
                self.send_idempotency_key = None;
                self.pending_send = None;
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

/// BOLT11's bech32 variant: the bech32 checksum, with the 90-character limit
/// waived (BOLT11 §"Encoding Overview" — an invoice with a description or a few
/// route hints runs well past it, and past bech32's own 1023-character code
/// length too).
enum Bolt11Bech32 {}

impl bech32::Checksum for Bolt11Bech32 {
    type MidstateRepr = <bech32::Bech32 as bech32::Checksum>::MidstateRepr;
    const CODE_LENGTH: usize = usize::MAX;
    const CHECKSUM_LENGTH: usize = <bech32::Bech32 as bech32::Checksum>::CHECKSUM_LENGTH;
    const GENERATOR_SH: [Self::MidstateRepr; 5] =
        <bech32::Bech32 as bech32::Checksum>::GENERATOR_SH;
    const TARGET_RESIDUE: Self::MidstateRepr = <bech32::Bech32 as bech32::Checksum>::TARGET_RESIDUE;
}

/// The amount a BOLT11 invoice commits to, in sats, or `None` when the input
/// isn't a well-formed invoice or is amountless.
///
/// Read straight out of the invoice's human-readable part (`lnbc65m1…`), which
/// is plain text ahead of the bech32 separator — no signature check, no SDK
/// round-trip, so the amount lands the instant the user pastes. It's used for
/// display only: the amount that actually gets paid still comes from the
/// invoice itself at prepare time, so a mis-read here can't send the wrong
/// number of sats.
///
/// The bech32 checksum is verified all the same, because the prefilled amount
/// is only as trustworthy as the invoice it was read off. A mangled paste — a
/// character dropped by a line wrap, a truncated copy — still carries an intact
/// human-readable part, so without the checksum it would quote a confident
/// amount for something that can never be paid, and lock the field on it.
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
    // anything shorter is a partial paste, not an invoice we should read. The
    // checksum can't stand in for this — a short string can checksum cleanly.
    if data.len() < 104 {
        return None;
    }
    // Charset, separator placement and checksum, in one pass over the string.
    bech32::primitives::decode::CheckedHrpstring::new::<Bolt11Bech32>(s).ok()?;

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

/// Convert raw parse-input errors into clear, user-facing error messages.
/// Known invalid-input/invalid-address errors are mapped to a consistent
/// address validation message, while unrecognized errors are returned unchanged.
fn format_parse_input_error(raw: &str) -> String {
    let lower = raw.to_lowercase();
    if lower.contains("invalid input") || lower.contains("invalid address") {
        return "Invalid destination. Please check the destination and try again.".to_string();
    }
    raw.to_string()
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
        .map_err(|e| format_parse_input_error(&e.to_string()))?;

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
    use coincube_spark_protocol::PaymentSummary;

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

    #[test]
    fn format_parse_input_error_maps_invalid_input_to_user_friendly_text() {
        assert_eq!(
            format_parse_input_error("parse failed: Invalid input"),
            "Invalid destination. Please check the destination and try again."
        );
        assert_eq!(
            format_parse_input_error(
                "Spark bridge returned Sdk: parse_input failed: Invalid input: invalid input"
            ),
            "Invalid destination. Please check the destination and try again."
        );
    }

    #[test]
    fn format_parse_input_error_preserves_non_validation_parse_input_failure() {
        let raw = "Spark bridge returned Sdk: parse_input failed: unexpected SDK error";

        assert_eq!(format_parse_input_error(raw), raw);
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

    /// A BOLT11 invoice with `hrp` as its human-readable part, carrying a valid
    /// bech32 checksum over a realistically-sized payload (a signature alone is
    /// 104 characters). The fields inside aren't a real payment request — the
    /// amount comes from the HRP and nothing here verifies a signature — but the
    /// checksum is genuine, which is what `bolt11_amount_sat` insists on.
    fn invoice(hrp: &str) -> String {
        bech32::encode_lower::<Bolt11Bech32>(
            bech32::Hrp::parse(hrp).expect("test HRPs are valid"),
            &[0x42; 70],
        )
        .expect("test payloads encode")
    }

    /// Flip one character of an otherwise valid invoice's data part, the way a
    /// line wrap or a truncated copy would.
    fn corrupt(invoice: &str) -> String {
        let last = invoice.len() - 1;
        let flipped = if invoice.ends_with('q') { 'p' } else { 'q' };
        format!("{}{flipped}", &invoice[..last])
    }

    /// A real, signed BOLT11 invoice — the spec's own 0.025 BTC test vector.
    ///
    /// The synthetic fixtures above are checksummed by the same code that
    /// verifies them, so on their own they'd pass even if `Bolt11Bech32`'s
    /// constants were wrong. This one was checksummed by somebody else.
    const SPEC_VECTOR_25M: &str = "lnbc25m1pvjluezpp5qqqsyqcyq5rqwzqfqqqsyqcyq5rqwzqfqqqsyqcyq5rq\
                                   wzqfqypqdq5vdhkven9v5sxyetpdeessp5zyg3zyg3zyg3zyg3zyg3zyg3zyg3\
                                   zyg3zyg3zyg3zyg3zyg3zygs9q5sqqqqqqqqqqqqqqqpqsq67gye39hfg3zd8r\
                                   gc80k32tvy9xk2xunwm5lzexnvpx6fd77en8qaq424dxgt56cag2dpt359k3ss\
                                   yhetktkpqh24jqnjyw6uqd08sgptq44qu";

    #[test]
    fn bolt11_amount_reads_a_real_signed_invoice() {
        assert_eq!(bolt11_amount_sat(SPEC_VECTOR_25M), Some(2_500_000));
        // …and rejects it once a character is mangled.
        assert_eq!(bolt11_amount_sat(&corrupt(SPEC_VECTOR_25M)), None);
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
        // Right shape, right length, no valid checksum — the case a
        // length-only guard waves through.
        assert_eq!(
            bolt11_amount_sat(&format!("lnbc20m1{}", "q".repeat(110))),
            None
        );
        // A single mangled character in an otherwise valid invoice.
        assert_eq!(bolt11_amount_sat(&corrupt(&invoice("lnbc20m"))), None);
        // A character outside the bech32 data charset (`1`, `b`, `i`, `o`).
        let inv = invoice("lnbc20m");
        assert_eq!(
            bolt11_amount_sat(&format!("{}b", &inv[..inv.len() - 1])),
            None
        );
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
            has_token_leg: true,
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

    // ── Unknown payment outcome ─────────────────────────────────────────

    fn payment(amount_sat: i64, timestamp: u64, direction: &str) -> PaymentSummary {
        PaymentSummary {
            id: "pay-1".to_string(),
            amount_sat,
            fees_sat: 0,
            token_amount: None,
            token_decimals: None,
            token_ticker: None,
            timestamp,
            status: "completed".to_string(),
            direction: direction.to_string(),
            method: "lightning".to_string(),
            description: None,
        }
    }

    /// A panel that has just dispatched a bitcoin-rail send of 1_000 sats and
    /// been told the outcome is unknown.
    fn unknown_outcome_panel(guard: RetryGuard) -> SparkSend {
        let mut panel = SparkSend::new(None);
        panel.destination_input = "lnbc1example".to_string();
        panel.amount_input = "1000".to_string();
        panel.send_idempotency_key = Some("key-from-the-first-attempt".to_string());
        panel.pending_send = Some(PendingSend {
            request_id: Some(7),
            dispatched_at: chrono::Utc::now().timestamp(),
            amount_sat: 1_000,
            guard,
        });
        panel.phase = SparkSendPhase::OutcomeUnknown {
            message: "The Spark bridge did not answer.".to_string(),
            outcome: None,
            checking: false,
            guard,
        };
        panel
    }

    /// **The audited hazard, as a test.** A send that outruns the client
    /// deadline must not be presented as a failure, and must not leave the
    /// panel able to mint a second payment.
    #[test]
    fn a_timed_out_send_is_never_reported_as_failed() {
        let mut panel = SparkSend::new(None);
        panel.send_idempotency_key = Some("key-1".to_string());
        panel.pending_send = Some(PendingSend {
            request_id: None,
            dispatched_at: chrono::Utc::now().timestamp(),
            amount_sat: 1_000,
            guard: RetryGuard::IdempotencyKey,
        });
        panel.phase = SparkSendPhase::Sending;

        let _ = send(
            &mut panel,
            SparkSendMessage::SendOutcomeUnknown {
                request_id: 42,
                message: "The Spark bridge did not answer within 120s.".to_string(),
            },
        );

        match &panel.phase {
            SparkSendPhase::OutcomeUnknown {
                outcome, checking, ..
            } => {
                assert!(outcome.is_none(), "no verdict before the check runs");
                assert!(checking, "the check must start immediately");
            }
            other => panic!("a timed-out send must not be a failure, got {:?}", other),
        }
        assert!(
            !matches!(panel.phase, SparkSendPhase::Error(_)),
            "an unknown outcome must never land in the definite-failure phase"
        );

        // The intent survives, key included — that is what stops a retry
        // becoming a second payment.
        assert_eq!(
            panel.send_idempotency_key.as_deref(),
            Some("key-1"),
            "the idempotency key must survive an unknown outcome"
        );
        let pending = panel.pending_send.as_ref().expect("intent retained");
        assert_eq!(
            pending.request_id,
            Some(42),
            "the late answer stays claimable"
        );
        assert_eq!(pending.amount_sat, 1_000);
    }

    /// While the outcome is unknown, no path offers a new send — not before the
    /// check finishes, and not after an inconclusive one.
    #[test]
    fn no_new_send_is_offered_until_reconciliation_clears_it() {
        // Still checking.
        let mut panel = unknown_outcome_panel(RetryGuard::IdempotencyKey);
        panel.phase = SparkSendPhase::OutcomeUnknown {
            message: "unknown".to_string(),
            outcome: None,
            checking: true,
            guard: RetryGuard::IdempotencyKey,
        };
        let _ = send(&mut panel, SparkSendMessage::ResendAfterUnknownRequested);
        assert!(
            matches!(panel.phase, SparkSendPhase::OutcomeUnknown { .. }),
            "a resend must not be possible while the check is running"
        );

        // Inconclusive: the history could not be read.
        let mut panel = unknown_outcome_panel(RetryGuard::IdempotencyKey);
        let _ = send(
            &mut panel,
            SparkSendMessage::ReconcileFinished(ReconcileOutcome::Inconclusive(
                "bridge unavailable".to_string(),
            )),
        );
        let _ = send(&mut panel, SparkSendMessage::ResendAfterUnknownRequested);
        assert!(
            matches!(panel.phase, SparkSendPhase::OutcomeUnknown { .. }),
            "an inconclusive check must not enable a resend"
        );
        assert!(panel.send_idempotency_key.is_some());
    }

    /// Reconciliation finding the payment blocks any resend outright: it went
    /// through, and the only correct action is to look at the history.
    #[test]
    fn a_payment_that_landed_can_never_be_sent_again() {
        for guard in [RetryGuard::IdempotencyKey, RetryGuard::None] {
            let mut panel = unknown_outcome_panel(guard);
            let _ = send(
                &mut panel,
                SparkSendMessage::ReconcileFinished(ReconcileOutcome::Landed),
            );
            let _ = send(&mut panel, SparkSendMessage::ResendAfterUnknownRequested);
            assert!(
                matches!(panel.phase, SparkSendPhase::OutcomeUnknown { .. }),
                "a landed payment must not offer a resend"
            );
            assert!(!ReconcileOutcome::Landed.may_resend(guard));
            assert!(ReconcileOutcome::Landed
                .guidance(guard)
                .contains("did go through"));
        }
    }

    /// The one safe resend: nothing in the history **and** an
    /// idempotency-guarded rail. It keeps the original key, so if the history
    /// was merely stale the SDK dedups instead of paying twice.
    #[test]
    fn a_cleared_unknown_send_retries_under_the_original_key() {
        let mut panel = unknown_outcome_panel(RetryGuard::IdempotencyKey);
        let _ = send(
            &mut panel,
            SparkSendMessage::ReconcileFinished(ReconcileOutcome::NoTrace),
        );
        assert!(ReconcileOutcome::NoTrace.may_resend(RetryGuard::IdempotencyKey));

        let _ = send(&mut panel, SparkSendMessage::ResendAfterUnknownRequested);
        assert_eq!(
            panel.send_idempotency_key.as_deref(),
            Some("key-from-the-first-attempt"),
            "a resend must reuse the key, or the SDK cannot dedup it"
        );
        assert!(matches!(panel.phase, SparkSendPhase::Idle));
        // The destination and amount are untouched, so the re-prepare describes
        // the same payment.
        assert_eq!(panel.destination_input, "lnbc1example");
        assert_eq!(panel.amount_input, "1000");
    }

    /// A rail with no idempotency guarantee never gets a blind retry, however
    /// the check came out.
    #[test]
    fn an_unguarded_rail_is_sent_to_the_payment_history_instead() {
        let mut panel = unknown_outcome_panel(RetryGuard::None);
        let _ = send(
            &mut panel,
            SparkSendMessage::ReconcileFinished(ReconcileOutcome::NoTrace),
        );
        assert!(!ReconcileOutcome::NoTrace.may_resend(RetryGuard::None));

        let _ = send(&mut panel, SparkSendMessage::ResendAfterUnknownRequested);
        assert!(
            matches!(panel.phase, SparkSendPhase::OutcomeUnknown { .. }),
            "an unguarded rail must not resend even with no trace of the payment"
        );
        let guidance = ReconcileOutcome::NoTrace.guidance(RetryGuard::None);
        assert!(guidance.contains("Transactions"), "{}", guidance);
        assert!(guidance.contains("twice"), "{}", guidance);
    }

    /// The bridge's own late answer wins over any inference from history, in
    /// both directions: a late rejection is a definite failure the user can
    /// correct and retry normally.
    #[test]
    fn a_late_definite_rejection_becomes_an_ordinary_failure() {
        let mut panel = unknown_outcome_panel(RetryGuard::IdempotencyKey);
        let _ = send(
            &mut panel,
            SparkSendMessage::ReconcileFinished(ReconcileOutcome::DefinitelyFailed(
                "invoice expired".to_string(),
            )),
        );
        match &panel.phase {
            SparkSendPhase::Error(msg) => assert_eq!(msg, "invoice expired"),
            other => panic!(
                "a definite rejection must be an ordinary failure, got {:?}",
                other
            ),
        }
        assert!(
            panel.pending_send.is_none(),
            "a definite failure has nothing left to reconcile"
        );

        // And the ordinary path still works: Reset clears the key so the
        // corrected payment is a new one.
        let _ = send(&mut panel, SparkSendMessage::Reset);
        assert!(panel.send_idempotency_key.is_none());
        assert!(matches!(panel.phase, SparkSendPhase::Idle));
    }

    /// A definite `SendFailed` — the bridge answered "no" — is unchanged: it is
    /// a failure, it clears the pending intent, and Reset mints a fresh key.
    #[test]
    fn a_definite_send_failure_is_still_an_ordinary_failure() {
        let mut panel = unknown_outcome_panel(RetryGuard::IdempotencyKey);
        panel.phase = SparkSendPhase::Sending;
        let _ = send(&mut panel, SparkSendMessage::SendFailed("nope".to_string()));
        assert!(matches!(panel.phase, SparkSendPhase::Error(_)));
        assert!(panel.pending_send.is_none());
    }

    /// History matching brackets on direction, amount and time, and errs
    /// towards *finding* the payment — the direction that blocks a resend.
    #[test]
    fn history_matching_recognises_this_payment_and_not_another() {
        let now = chrono::Utc::now().timestamp();
        let pending = PendingSend {
            request_id: None,
            dispatched_at: now,
            amount_sat: 1_000,
            guard: RetryGuard::IdempotencyKey,
        };

        // The payment itself, recorded a moment after dispatch.
        assert!(payment_matches_intent(
            &[payment(-1_000, (now + 2) as u64, "outgoing")],
            &pending
        ));
        // Recorded slightly *before* the gui's clock — still ours.
        assert!(payment_matches_intent(
            &[payment(-1_000, (now - 5) as u64, "outgoing")],
            &pending
        ));
        // An older payment of the same size is somebody else's.
        assert!(!payment_matches_intent(
            &[payment(-1_000, (now - 3_600) as u64, "outgoing")],
            &pending
        ));
        // A receive of the same size is not this send.
        assert!(!payment_matches_intent(
            &[payment(1_000, (now + 2) as u64, "incoming")],
            &pending
        ));
        // A different amount is a different payment.
        assert!(!payment_matches_intent(
            &[payment(-2_000, (now + 2) as u64, "outgoing")],
            &pending
        ));
        // Empty history is no evidence.
        assert!(!payment_matches_intent(&[], &pending));
    }

    /// Every rail the panel can send on, with the guard each one gets and what
    /// that permits after an unknown outcome. A new payment method landing here
    /// without a decision shows up as a failing case rather than as a silent
    /// "guarded" default.
    #[test]
    fn every_rail_has_an_explicit_retry_guard() {
        // The bitcoin rails: the bridge forwards the idempotency key and the
        // SDK honours it, so a reconciled resend cannot pay twice.
        for method in [
            "Bolt11Invoice",
            "BitcoinAddress",
            "SparkAddress",
            "SparkInvoice",
            // LNURL-pay is executed by `execute_lnurl_send`, which also
            // forwards the key.
            "LnurlPay",
        ] {
            let prepare = PrepareSendOk {
                handle: "h".to_string(),
                amount_sat: 1_000,
                fee_sat: 1,
                method: method.to_string(),
                cross_chain: None,
                has_token_leg: false,
            };
            let guard = retry_guard_for(&prepare);
            assert_eq!(
                guard,
                RetryGuard::IdempotencyKey,
                "{} must be idempotency-guarded",
                method
            );
            assert!(
                ReconcileOutcome::NoTrace.may_resend(guard),
                "{} must allow a reconciled resend",
                method
            );
            assert!(
                !ReconcileOutcome::Landed.may_resend(guard),
                "{} must never resend a payment that landed",
                method
            );
        }

        // Cross-chain: a token/conversion leg. The SDK rejects a key there and
        // the bridge drops it, so nothing guards a fresh attempt.
        let future = (chrono::Utc::now() + chrono::Duration::seconds(60)).to_rfc3339();
        let cross_chain = prepared_with_quote(&future);
        assert_eq!(retry_guard_for(&cross_chain), RetryGuard::None);
        for outcome in [
            ReconcileOutcome::NoTrace,
            ReconcileOutcome::Landed,
            ReconcileOutcome::Inconclusive("x".to_string()),
        ] {
            assert!(
                !outcome.may_resend(RetryGuard::None),
                "a cross-chain send must never be blind-retried"
            );
        }
        // And its existing quote-reuse retry path is untouched.
        assert!(cross_chain
            .cross_chain
            .as_deref()
            .map(cross_chain::RetryPolicy::for_quote)
            .is_some_and(|p| p.may_retry()));
    }

    /// Stable Balance can auto-attach a token→BTC conversion to an ordinary
    /// sats send when the sat balance can't cover amount + fee. That send has
    /// no cross-chain quote, but the bridge still drops its idempotency key —
    /// so it must not come back reported as guarded.
    #[test]
    fn auto_attached_conversion_leg_is_unguarded_without_a_cross_chain_quote() {
        let prepare = PrepareSendOk {
            handle: "h".to_string(),
            amount_sat: 1_000,
            fee_sat: 1,
            method: "BitcoinAddress".to_string(),
            cross_chain: None,
            has_token_leg: true,
        };
        assert_eq!(retry_guard_for(&prepare), RetryGuard::None);
        assert!(
            !ReconcileOutcome::NoTrace.may_resend(retry_guard_for(&prepare)),
            "a send whose key the bridge dropped must never be blind-retried"
        );
    }
}
