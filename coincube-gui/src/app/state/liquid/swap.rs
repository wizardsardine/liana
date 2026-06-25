//! Liquid cross-asset Swap panel.
//!
//! Surfaces the existing Breez SDK Liquid cross-asset (SideSwap) rail —
//! already wired through [`crate::app::state::liquid::send`] as a
//! self-targeted cross-asset send — as a dedicated Aqua-style Swap
//! screen for L-BTC ↔ USDt.
//!
//! ## Quote model
//!
//! The SideSwap API (`PayAmount::Asset`) takes the amount the user wants
//! to **receive** (the `to` asset) and reports the `from`-asset cost via
//! `PrepareSendResponse::exchange_amount_sat` (+ `fees_sat`). So the UI
//! input is the receive amount; the pay amount is derived and read-only.
//! The prepared response is held and passed straight to `send_payment`
//! on confirm, so the executed quote is exactly the reviewed one.

use std::sync::Arc;
use std::time::Duration;

use breez_sdk_liquid::prelude::PrepareSendResponse;
use coincube_core::miniscript::bitcoin::{Amount, Network};
use coincube_ui::{
    component::{
        amount::{BitcoinDisplayUnit, DisplayAmount},
        form,
    },
    widget::Element,
};
use iced::{Subscription, Task};

use super::send::SendAsset;
use super::swap_history::{SwapHistory, SwapRecord};
use crate::app::breez_liquid::assets::{
    format_asset_amount, format_usdt_display, parse_asset_to_minor_units, usdt_asset_id, AssetKind,
    LBTC_PRECISION,
};
use crate::app::cache::Cache;
use crate::app::menu::Menu;
use crate::app::state::State;
use crate::app::wallets::LiquidBackend;
use crate::app::{message::Message, view, wallet::Wallet};
use crate::daemon::Daemon;

/// Seconds a locked quote stays valid before the review screen forces a
/// re-fetch. SideSwap quotes are short-lived; 30s is conservative.
const QUOTE_TTL_SECS: u32 = 30;

/// Debounce window between the last keystroke and a quote request, so we
/// don't hammer the SDK on every character.
const QUOTE_DEBOUNCE: Duration = Duration::from_millis(450);

/// Fraction of the estimated max that "Swap All" actually requests, leaving
/// headroom for fees / rounding / price drift between the rate estimate and
/// the re-quote (a zero-margin estimate reliably trips "insufficient").
const SWAP_ALL_SAFETY_MARGIN: f64 = 0.995;

/// Fraction of the balance the background rate probe quotes for — small
/// enough to stay affordable (so the probe's balance check passes) while
/// being well above the SideSwap minimum for any non-dust balance.
const RATE_PROBE_FRACTION: f64 = 0.5;

/// Approximate mid-market rate (`to`-base per `from`-base) from the cached
/// BTC/USD price, used only to size an affordable rate probe. Both assets
/// are 8-dp, so for L-BTC→USDt the rate is the price and the inverse for
/// the other direction.
fn cached_seed_rate(from: SendAsset, to: SendAsset, btc_usd_price: f64) -> Option<f64> {
    if btc_usd_price <= 0.0 {
        return None;
    }
    match (from, to) {
        (SendAsset::Lbtc, SendAsset::Usdt) => Some(btc_usd_price),
        (SendAsset::Usdt, SendAsset::Lbtc) => Some(1.0 / btc_usd_price),
        _ => None,
    }
}

/// Price-free fallback probe size, used when no cached BTC/USD price is
/// available yet (early in the session). A small `to`-amount that's above
/// the SideSwap minimum and affordable for any non-trivial balance.
fn fixed_probe_receiver(to: SendAsset) -> u64 {
    match to {
        SendAsset::Lbtc => 5_000,       // 0.00005 L-BTC
        SendAsset::Usdt => 200_000_000, // 2 USDt
    }
}

/// Swap is only available where SideSwap is — i.e. mainnet. Mirrors the
/// `cross_asset_supported` predicate in [`crate::app::state::liquid::send`]
/// so the Swap entry points and quote engine share one capability gate.
pub fn swap_supported(network: Network) -> bool {
    matches!(network, Network::Bitcoin)
}

/// Map SDK prepare errors to user-friendly inline messages.
fn friendly_quote_error(msg: &str) -> String {
    if msg.contains("not enough funds")
        || msg.contains("InsufficientFunds")
        || msg.contains("insufficient")
    {
        "Insufficient balance to cover this swap and its fees.".to_string()
    } else if msg.contains("minimum") || msg.contains("too low") || msg.contains("below") {
        "Amount is below the swap minimum. Try a larger amount.".to_string()
    } else {
        format!("Couldn't fetch a quote: {msg}")
    }
}

/// A locked cross-asset quote. Holds the prepared response so confirm
/// executes exactly what was reviewed, plus the decoded figures the UI
/// needs (all base units are 8-dp for both L-BTC and USDt).
#[derive(Debug, Clone)]
pub struct SwapQuote {
    /// The prepared response — passed verbatim to `send_payment`.
    pub prepare: PrepareSendResponse,
    /// Amount the user receives, in `to`-asset base units (the input).
    pub receiver_base: u64,
    /// `from`-asset amount spent excluding fees (`exchange_amount_sat`).
    pub exchange_base: u64,
    /// SideSwap + network fee, in `from`-asset base units.
    pub fee_base: u64,
}

impl SwapQuote {
    /// Total `from`-asset amount the user pays (exchange + fees).
    pub fn from_total_base(&self) -> u64 {
        from_total_base(self.exchange_base, self.fee_base)
    }

    /// `to` units received per 1 whole `from` unit. Both assets are 8-dp,
    /// so this is `receiver_base / from_total_base`.
    pub fn rate_to_per_from(&self) -> f64 {
        rate_to_per_from(self.receiver_base, self.from_total_base())
    }
}

/// Total `from`-asset amount paid: SideSwap exchange amount + fees.
fn from_total_base(exchange_base: u64, fee_base: u64) -> u64 {
    exchange_base.saturating_add(fee_base)
}

/// `to`-base units received per `from`-base unit paid (0 if nothing paid).
fn rate_to_per_from(receiver_base: u64, from_total_base: u64) -> f64 {
    if from_total_base == 0 {
        0.0
    } else {
        receiver_base as f64 / from_total_base as f64
    }
}

/// Continue is enabled for a fresh, unexpired quote while no send is in
/// flight.
fn is_quote_actionable(has_quote: bool, quote_remaining: u32, is_sending: bool) -> bool {
    has_quote && quote_remaining > 0 && !is_sending
}

/// Confirm additionally requires a synced wallet: SideSwap orders fail
/// server-side (`ClientError`) when the Liquid wallet is mid-scan and
/// can't fund/sign the swap tx within the order's short window.
fn can_confirm(quote_actionable: bool, synced: bool) -> bool {
    quote_actionable && synced
}

/// Validate an amount field in place: empty is valid (cleared), zero and
/// malformed are flagged, and — when `max_base` is given (the pay side,
/// where we know the spendable balance) — over-balance is flagged too.
fn validate_amount_field(
    field: &mut form::Value<String>,
    value: &str,
    asset: SendAsset,
    unit: BitcoinDisplayUnit,
    max_base: Option<u64>,
) {
    field.value = value.to_string();
    if value.trim().is_empty() {
        field.valid = true;
        field.warning = None;
        return;
    }
    match parse_asset_amount(value, asset, unit) {
        Some(0) => {
            field.valid = false;
            field.warning = Some("Amount must be greater than zero");
        }
        Some(base) => {
            if max_base.is_some_and(|max| base > max) {
                field.valid = false;
                field.warning = Some("Insufficient balance");
            } else {
                field.valid = true;
                field.warning = None;
            }
        }
        None => {
            field.valid = false;
            field.warning = Some("Invalid amount");
        }
    }
}

/// Which side of the swap the user is editing. The other side is derived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwapSide {
    /// The "You pay" (from-asset) amount.
    Pay,
    /// The "You receive" (to-asset) amount.
    Receive,
}

/// Parse an amount input into the given asset's base units, honouring the
/// display unit. For L-BTC the base unit *is* the sat, so in SATS mode the
/// input is whole integers; otherwise (USDt, or L-BTC in BTC mode) it's an
/// 8-dp decimal. Returns `Some(0)` for a literal zero and `None` for
/// empty/malformed input — callers filter zero where needed.
fn parse_asset_amount(value: &str, asset: SendAsset, unit: BitcoinDisplayUnit) -> Option<u64> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    match (asset, unit) {
        (SendAsset::Lbtc, BitcoinDisplayUnit::Sats) => {
            // Whole sats only — reject decimals.
            if trimmed.contains('.') {
                return None;
            }
            trimmed.parse::<u64>().ok()
        }
        _ => parse_asset_to_minor_units(trimmed, AssetKind::Usdt.precision()),
    }
}

/// Phase of the single-screen swap flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwapPhase {
    /// Entering the receive amount and watching the live quote.
    Input,
    /// Reviewing a locked quote before confirming.
    Review,
    /// Swap submitted and settled — success screen.
    Sent,
}

/// Cross-asset swap flow state. Initialised L-BTC → USDt (Aqua default).
pub struct LiquidSwap {
    breez_client: Arc<LiquidBackend>,
    /// Asset the user pays with.
    from_asset: SendAsset,
    /// Asset the user receives.
    to_asset: SendAsset,
    btc_balance: Amount,
    usdt_balance: u64,
    /// "You pay" (from-asset) amount input.
    pay_input: form::Value<String>,
    /// "You receive" (to-asset) amount input.
    receive_input: form::Value<String>,
    /// Which side the user last edited. The other side is computed from the
    /// quote; only the SDK's receiver amount can be pinned, so editing the
    /// pay side estimates the receiver via the rate.
    edit_side: SwapSide,
    /// User's BTC/SATS display preference, synced from the cache on each
    /// update. Drives both amount rendering and input parsing for L-BTC.
    bitcoin_unit: BitcoinDisplayUnit,
    /// Self-targeted Liquid address every prepare/send is pointed at.
    self_address: Option<String>,
    /// True while a quote should be issued as soon as `self_address`
    /// arrives (a quote was requested before the address was ready).
    pending_quote: bool,
    /// Current locked/last quote, if any.
    quote: Option<SwapQuote>,
    /// Most recent SideSwap rate (`to` per `from` base unit) — from a real
    /// quote or the background probe. Drives the rate chip and "Swap All".
    last_rate: Option<f64>,
    /// Whether a background rate probe is in flight (de-dupes triggers).
    rate_probe_inflight: bool,
    /// "Swap All" was pressed before a rate was known — fill the max as soon
    /// as the in-flight probe lands.
    pending_swap_all: bool,
    /// Whether a quote request is in flight.
    quoting: bool,
    /// Monotonic sequence guarding debounce timers and async results
    /// against staleness.
    quote_seq: u64,
    /// Seconds remaining before the current quote expires.
    quote_remaining: u32,
    phase: SwapPhase,
    is_sending: bool,
    /// Whether a Liquid sync has completed since this screen was entered.
    /// Until the first `Synced` event lands the wallet may still be catching
    /// up, and a swap can fail server-side because its inputs aren't ready —
    /// so we surface a hint. Set on the `RefreshRequested` that an SDK
    /// `Synced` event drives (see `App`'s `active_liquid_refresh`).
    synced: bool,
    error: Option<String>,
    /// Success-screen celebration assets.
    sent_amount_display: String,
    sent_quote: coincube_ui::component::quote_display::Quote,
    sent_image_handle: iced::widget::image::Handle,
    /// Persisted local log of completed swaps (the SDK doesn't mark swaps,
    /// so we keep our own record for "Last Swaps" + history labelling).
    history: SwapHistory,
}

impl LiquidSwap {
    pub fn new(breez_client: Arc<LiquidBackend>, swaps_path: std::path::PathBuf) -> Self {
        Self {
            breez_client,
            from_asset: SendAsset::Lbtc,
            to_asset: SendAsset::Usdt,
            btc_balance: Amount::from_sat(0),
            usdt_balance: 0,
            pay_input: form::Value::default(),
            receive_input: form::Value::default(),
            edit_side: SwapSide::Receive,
            bitcoin_unit: BitcoinDisplayUnit::BTC,
            self_address: None,
            pending_quote: false,
            quote: None,
            last_rate: None,
            rate_probe_inflight: false,
            pending_swap_all: false,
            quoting: false,
            quote_seq: 0,
            quote_remaining: 0,
            phase: SwapPhase::Input,
            is_sending: false,
            synced: false,
            error: None,
            sent_amount_display: String::new(),
            sent_quote: coincube_ui::component::quote_display::random_quote("liquid-send"),
            sent_image_handle: coincube_ui::component::quote_display::image_handle_for_context(
                "liquid-send",
            ),
            history: SwapHistory::load(swaps_path),
        }
    }

    fn asset_kind(asset: SendAsset) -> AssetKind {
        match asset {
            SendAsset::Lbtc => AssetKind::Lbtc,
            SendAsset::Usdt => AssetKind::Usdt,
        }
    }

    /// Balance of `asset` in its 8-dp base units.
    fn balance_base(&self, asset: SendAsset) -> u64 {
        match asset {
            SendAsset::Lbtc => self.btc_balance.to_sat(),
            SendAsset::Usdt => self.usdt_balance,
        }
    }

    fn load_balance(&self) -> Task<Message> {
        let breez_client = self.breez_client.clone();
        Task::perform(
            async move {
                let info = breez_client.info().await;
                let btc_balance = info
                    .as_ref()
                    .map(|info| {
                        Amount::from_sat(
                            info.wallet_info.balance_sat + info.wallet_info.pending_receive_sat,
                        )
                    })
                    .unwrap_or(Amount::ZERO);

                let usdt_id = usdt_asset_id(breez_client.network()).unwrap_or("");
                let usdt_balance = info
                    .as_ref()
                    .ok()
                    .and_then(|info| {
                        info.wallet_info
                            .asset_balances
                            .iter()
                            .find_map(|ab| (ab.asset_id == usdt_id).then_some(ab.balance_sat))
                    })
                    .unwrap_or(0);

                match info {
                    Ok(_) => Ok((btc_balance, usdt_balance)),
                    Err(_) => Err("Couldn't fetch account balance".to_string()),
                }
            },
            |result| match result {
                Ok((btc_balance, usdt_balance)) => Message::View(view::Message::LiquidSwap(
                    view::LiquidSwapMessage::DataLoaded {
                        btc_balance,
                        usdt_balance,
                    },
                )),
                Err(err) => Message::View(view::Message::LiquidSwap(
                    view::LiquidSwapMessage::Error(err),
                )),
            },
        )
    }

    /// Generate a self-targeted Liquid address for the swap destination.
    fn generate_self_address(&self) -> Task<Message> {
        let breez_client = self.breez_client.clone();
        Task::perform(
            async move {
                breez_client
                    .receive_liquid()
                    .await
                    .map(|r| r.destination)
                    .map_err(|e| e.to_string())
            },
            |result| {
                Message::View(view::Message::LiquidSwap(
                    view::LiquidSwapMessage::SelfAddressReady(result),
                ))
            },
        )
    }

    /// Fire a background rate probe so the rate chip and "Swap All" work
    /// before the user types anything. Quotes a fraction of the balance
    /// (sized via the cached price so it's affordable) and stores only the
    /// resulting rate — never a visible quote. No-op when a rate already
    /// exists, a probe is in flight, the destination/balance/price aren't
    /// ready, or the probe amount rounds to zero (dust balance).
    fn probe_rate(&mut self, btc_usd_price: Option<f64>) -> Task<Message> {
        if self.last_rate.is_some() || self.rate_probe_inflight {
            return Task::none();
        }
        let Some(destination) = self.self_address.clone() else {
            return Task::none();
        };
        let from_balance = self.balance_base(self.from_asset);
        if from_balance == 0 {
            return Task::none();
        }
        // Size the probe at ~half the balance via the cached price (robustly
        // affordable and above the SideSwap minimum), or fall back to a small
        // fixed reference when no price is available yet.
        let probe_receiver = btc_usd_price
            .and_then(|p| cached_seed_rate(self.from_asset, self.to_asset, p))
            .map(|rate| (from_balance as f64 * rate * RATE_PROBE_FRACTION).floor() as u64)
            .filter(|&r| r > 0)
            .unwrap_or_else(|| fixed_probe_receiver(self.to_asset));
        if probe_receiver == 0 {
            return Task::none();
        }
        let network = self.breez_client.network();
        let (Some(to_id), Some(from_id)) = (
            Self::asset_kind(self.to_asset).asset_id(network),
            Self::asset_kind(self.from_asset).asset_id(network),
        ) else {
            return Task::none();
        };
        let (to_id, from_id) = (to_id.to_string(), from_id.to_string());

        self.rate_probe_inflight = true;
        let breez_client = self.breez_client.clone();
        Task::perform(
            async move {
                breez_client
                    .prepare_send_asset(
                        destination,
                        &to_id,
                        probe_receiver,
                        LBTC_PRECISION,
                        Some(&from_id),
                    )
                    .await
                    .map_err(|e| e.to_string())
                    .map(|resp| {
                        let total = resp
                            .exchange_amount_sat
                            .unwrap_or(0)
                            .saturating_add(resp.fees_sat.unwrap_or(0));
                        rate_to_per_from(probe_receiver, total)
                    })
            },
            |result| {
                Message::View(view::Message::LiquidSwap(
                    view::LiquidSwapMessage::RateProbeReady(result),
                ))
            },
        )
    }

    /// The receiver (`to`-asset) base amount to quote for, derived from the
    /// side the user is editing. Editing the receive side is direct; editing
    /// the pay side converts the entered from-amount via the current rate
    /// (so it needs a rate). Returns `None` for empty/zero/malformed input
    /// or a missing rate.
    fn receiver_base_to_quote(&self) -> Option<u64> {
        match self.edit_side {
            SwapSide::Receive => {
                parse_asset_amount(&self.receive_input.value, self.to_asset, self.bitcoin_unit)
                    .filter(|&v| v > 0)
            }
            SwapSide::Pay => {
                let from_base =
                    parse_asset_amount(&self.pay_input.value, self.from_asset, self.bitcoin_unit)
                        .filter(|&v| v > 0)?;
                let rate = self.last_rate.filter(|r| *r > 0.0)?;
                let receiver = (from_base as f64 * rate).floor() as u64;
                (receiver > 0).then_some(receiver)
            }
        }
    }

    /// Bump the sequence, clear the stale quote, and schedule a debounced
    /// quote request for the current input. Returns a debounce timer task.
    fn schedule_quote(&mut self) -> Task<Message> {
        self.quote = None;
        self.quote_remaining = 0;
        self.quote_seq = self.quote_seq.wrapping_add(1);
        let seq = self.quote_seq;

        // Only schedule when the input is a usable amount.
        if self.receiver_base_to_quote().is_none() {
            self.quoting = false;
            return Task::none();
        }
        self.quoting = true;
        Task::perform(
            async move { tokio::time::sleep(QUOTE_DEBOUNCE).await },
            move |_| {
                Message::View(view::Message::LiquidSwap(
                    view::LiquidSwapMessage::QuoteRequested(seq),
                ))
            },
        )
    }

    /// Issue the actual `prepare_send_asset` quote for the current input,
    /// tagged with `seq` so stale responses can be discarded.
    fn request_quote(&mut self, seq: u64) -> Task<Message> {
        let Some(receiver_base) = self.receiver_base_to_quote() else {
            self.quoting = false;
            return Task::none();
        };

        // Need a destination address first. If it isn't ready, remember
        // to quote once it lands and make sure generation is in flight.
        let Some(destination) = self.self_address.clone() else {
            self.pending_quote = true;
            return self.generate_self_address();
        };

        let network = self.breez_client.network();
        let to_kind = Self::asset_kind(self.to_asset);
        let from_kind = Self::asset_kind(self.from_asset);
        let to_asset_id = match to_kind.asset_id(network) {
            Some(id) => id.to_string(),
            None => {
                self.quoting = false;
                self.error = Some(format!("{} unavailable on this network", to_kind.ticker()));
                return Task::none();
            }
        };
        let from_asset_id = match from_kind.asset_id(network) {
            Some(id) => id.to_string(),
            None => {
                self.quoting = false;
                self.error = Some(format!(
                    "{} unavailable on this network",
                    from_kind.ticker()
                ));
                return Task::none();
            }
        };

        self.quoting = true;
        self.error = None;
        let breez_client = self.breez_client.clone();
        Task::perform(
            async move {
                breez_client
                    .prepare_send_asset(
                        destination,
                        &to_asset_id,
                        receiver_base,
                        LBTC_PRECISION,
                        Some(&from_asset_id),
                    )
                    .await
                    .map_err(|e| e.to_string())
            },
            move |result| {
                Message::View(view::Message::LiquidSwap(
                    view::LiquidSwapMessage::QuoteReady(seq, result),
                ))
            },
        )
    }

    /// Store a freshly-returned quote, decoding the from-cost figures.
    fn accept_quote(&mut self, prepare: PrepareSendResponse, receiver_base: u64) {
        // `exchange_amount_sat` + `fees_sat` are the SideSwap payer figures
        // in the `from` asset's base units (see SDK `get_asset_swap`).
        let exchange_base = prepare.exchange_amount_sat.unwrap_or(0);
        let fee_base = prepare.fees_sat.unwrap_or(0);
        let quote = SwapQuote {
            prepare,
            receiver_base,
            exchange_base,
            fee_base,
        };
        self.last_rate = Some(quote.rate_to_per_from());

        // Fill the *other* side from the quote (the side the user isn't
        // editing). Editing receive → show the computed pay; editing pay →
        // show the quoted receive.
        match self.edit_side {
            SwapSide::Receive => {
                self.pay_input.value =
                    self.format_asset_input(quote.from_total_base(), self.from_asset);
                self.pay_input.valid = true;
                self.pay_input.warning = None;
            }
            SwapSide::Pay => {
                self.receive_input.value =
                    self.format_asset_input(quote.receiver_base, self.to_asset);
                self.receive_input.valid = true;
                self.receive_input.warning = None;
            }
        }

        self.quote = Some(quote);
        self.quote_remaining = QUOTE_TTL_SECS;
        self.quoting = false;
        self.error = None;
    }

    /// Whether Continue/Confirm should be enabled.
    fn quote_actionable(&self) -> bool {
        is_quote_actionable(self.quote.is_some(), self.quote_remaining, self.is_sending)
    }

    /// Paying USDt with no L-BTC: a cross-asset swap pays the Liquid network
    /// fee in L-BTC (it can't use asset fees, unlike a same-asset USDt send),
    /// so a zero L-BTC balance can't fund the fee.
    fn needs_lbtc_for_fees(&self) -> bool {
        self.from_asset == SendAsset::Usdt && self.btc_balance.to_sat() == 0
    }

    /// Map a raw SDK quote error to a user message, special-casing the
    /// "no L-BTC for fees" situation so it's actionable.
    fn swap_error_message(&self, raw: &str) -> String {
        let insufficient = raw.contains("not enough funds")
            || raw.contains("InsufficientFunds")
            || raw.contains("insufficient")
            || raw.contains("Cannot pay");
        if insufficient && self.needs_lbtc_for_fees() {
            return "This swap needs a little L-BTC to pay Liquid network fees. \
                    Receive some L-BTC first, then swap."
                .to_string();
        }
        friendly_quote_error(raw)
    }

    /// Swap the `from`/`to` assets and invalidate the stale rate and inputs
    /// (the amounts no longer mean anything in the new direction). The caller
    /// re-quotes (so the quote itself is cleared there).
    fn flip_assets(&mut self) {
        std::mem::swap(&mut self.from_asset, &mut self.to_asset);
        self.last_rate = None;
        self.error = None;
        self.pay_input = form::Value::default();
        self.receive_input = form::Value::default();
        self.edit_side = SwapSide::Receive;
    }

    /// Pre-fill the entered amount with (almost) the max receivable from
    /// the current rate. Returns `Err` with an inline message when no rate
    /// is available yet or the balance is too low. On `Ok` the caller
    /// re-quotes the freshly-set amount.
    ///
    /// The estimate is shaved by [`SWAP_ALL_SAFETY_MARGIN`]: the rate comes
    /// from a prior quote, and the actual cost is only known once the SDK
    /// re-prepares against the *current* SideSwap price. Targeting the exact
    /// balance leaves zero headroom, so any fixed fee, rounding, or small
    /// price drift tips the re-quote into "insufficient balance". The margin
    /// keeps the swap affordable; the user can still nudge the amount up.
    fn swap_all_amount(&mut self) -> Result<(), String> {
        let rate = self
            .last_rate
            .filter(|r| *r > 0.0)
            .ok_or_else(|| "Enter an amount first to get a rate, then use Swap All.".to_string())?;
        let from_balance = self.balance_base(self.from_asset);
        if from_balance == 0 {
            return Err("No balance to swap.".to_string());
        }
        // `rate` is receiver-per-from-paid; scale the whole balance by it,
        // then shave the safety margin so the re-quote stays affordable.
        let est_receiver = (from_balance as f64 * rate * SWAP_ALL_SAFETY_MARGIN).floor() as u64;
        if est_receiver == 0 {
            return Err("Balance too low to swap.".to_string());
        }
        // "Swap All" is a receive-side fill: set the receive amount and let
        // the pay side fill from the quote.
        self.edit_side = SwapSide::Receive;
        self.receive_input.value = self.format_asset_input(est_receiver, self.to_asset);
        self.receive_input.valid = true;
        self.receive_input.warning = None;
        Ok(())
    }

    /// Format an asset base amount as its input field would hold it: whole
    /// sats for L-BTC in SATS mode, else an 8-dp decimal.
    fn format_asset_input(&self, base: u64, asset: SendAsset) -> String {
        match (asset, self.bitcoin_unit) {
            (SendAsset::Lbtc, BitcoinDisplayUnit::Sats) => base.to_string(),
            _ => format_asset_amount(base, AssetKind::Usdt.precision()),
        }
    }

    /// Clear in-flight/quote state after a failed `send_payment` so the
    /// user can retry: stop the spinner, surface the error, and drop the
    /// now-stale committed quote (the caller re-quotes on review).
    fn mark_swap_failed(&mut self, msg: String) {
        self.is_sending = false;
        self.quoting = false;
        self.error = Some(msg);
        self.quote = None;
        self.quote_remaining = 0;
    }
}

impl State for LiquidSwap {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        let swap_view = view::liquid::liquid_swap_view(view::liquid::LiquidSwapConfig {
            phase: self.phase,
            from_asset: self.from_asset,
            to_asset: self.to_asset,
            btc_balance: self.btc_balance,
            usdt_balance: self.usdt_balance,
            pay_input: &self.pay_input,
            receive_input: &self.receive_input,
            quote: self.quote.as_ref(),
            rate: self.last_rate,
            quoting: self.quoting,
            quote_remaining: self.quote_remaining,
            quote_actionable: self.quote_actionable(),
            confirm_enabled: can_confirm(self.quote_actionable(), self.synced),
            is_sending: self.is_sending,
            syncing: !self.synced,
            needs_lbtc_for_fees: self.needs_lbtc_for_fees(),
            bitcoin_unit: cache.bitcoin_unit,
            error: self.error.as_deref(),
            sent_amount_display: &self.sent_amount_display,
            sent_quote: &self.sent_quote,
            sent_image_handle: &self.sent_image_handle,
            last_swaps: self.history.records(),
        })
        .map(view::Message::LiquidSwap);

        view::dashboard(menu, cache, swap_view)
    }

    fn update(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        // Keep the unit preference current so amount parsing/formatting and
        // the input widget choice all agree with the global setting.
        self.bitcoin_unit = cache.bitcoin_unit;
        if let Message::View(view::Message::LiquidSwap(ref msg)) = message {
            match msg {
                view::LiquidSwapMessage::DataLoaded {
                    btc_balance,
                    usdt_balance,
                } => {
                    self.error = None;
                    self.btc_balance = *btc_balance;
                    self.usdt_balance = *usdt_balance;
                    // Now that a balance is known, pre-fetch the rate so the
                    // rate chip + "Swap All" work without the user typing.
                    return self.probe_rate(cache.btc_usd_price);
                }
                view::LiquidSwapMessage::Error(err) => {
                    self.error = Some(err.clone());
                    // Defensive: any error clears in-flight state so the UI
                    // never gets stuck on "Swapping…".
                    self.is_sending = false;
                    self.quoting = false;
                }
                view::LiquidSwapMessage::RefreshRequested => {
                    // A completed SDK sync drives this (or our own post-swap
                    // sync) — the wallet is caught up, so clear the hint.
                    self.synced = true;
                    return self.load_balance();
                }
                view::LiquidSwapMessage::SelfAddressReady(result) => match result {
                    Ok(addr) => {
                        self.self_address = Some(addr.clone());
                        if self.pending_quote {
                            self.pending_quote = false;
                            let seq = self.quote_seq;
                            return self.request_quote(seq);
                        }
                        // Destination is ready — pre-fetch the rate if we
                        // don't have one yet.
                        return self.probe_rate(cache.btc_usd_price);
                    }
                    Err(e) => {
                        self.pending_quote = false;
                        self.quoting = false;
                        self.error = Some(format!("Couldn't prepare swap address: {e}"));
                    }
                },
                view::LiquidSwapMessage::RateProbeReady(result) => {
                    self.rate_probe_inflight = false;
                    match result {
                        Ok(rate) if *rate > 0.0 => {
                            // Only seed if a real quote hasn't set it since.
                            if self.last_rate.is_none() {
                                self.last_rate = Some(*rate);
                            }
                        }
                        Ok(_) => {}
                        Err(e) => {
                            log::warn!(target: "breez_swap", "swap rate probe failed: {e}");
                        }
                    }
                    // Complete a "Swap All" that was waiting on the rate.
                    if self.pending_swap_all {
                        self.pending_swap_all = false;
                        self.quoting = false;
                        if self.last_rate.is_some() {
                            match self.swap_all_amount() {
                                Ok(()) => return self.schedule_quote(),
                                Err(msg) => self.error = Some(msg),
                            }
                        } else {
                            // Surface the real reason (below-minimum,
                            // insufficient, etc.) rather than a generic note.
                            self.error = Some(
                                result
                                    .as_ref()
                                    .err()
                                    .map(|e| self.swap_error_message(e))
                                    .unwrap_or_else(|| {
                                        "Couldn't fetch a rate for Swap All. Please try again."
                                            .to_string()
                                    }),
                            );
                        }
                    } else if self.edit_side == SwapSide::Pay
                        && self.quote.is_none()
                        && self.receiver_base_to_quote().is_some()
                    {
                        // The user typed a pay amount before the rate landed —
                        // quote it now that we can convert.
                        return self.schedule_quote();
                    }
                }
                view::LiquidSwapMessage::AmountEditedPay(value) => {
                    self.edit_side = SwapSide::Pay;
                    self.error = None;
                    let max = self.balance_base(self.from_asset);
                    validate_amount_field(
                        &mut self.pay_input,
                        value,
                        self.from_asset,
                        self.bitcoin_unit,
                        Some(max),
                    );
                    // Pay-side quoting converts via the rate, so make sure one
                    // is being fetched (no-op if we already have it).
                    return Task::batch([
                        self.schedule_quote(),
                        self.probe_rate(cache.btc_usd_price),
                    ]);
                }
                view::LiquidSwapMessage::AmountEditedReceive(value) => {
                    self.edit_side = SwapSide::Receive;
                    self.error = None;
                    validate_amount_field(
                        &mut self.receive_input,
                        value,
                        self.to_asset,
                        self.bitcoin_unit,
                        None,
                    );
                    return self.schedule_quote();
                }
                view::LiquidSwapMessage::FlipAssets => {
                    self.flip_assets();
                    // Re-quote any entered amount, and re-probe the rate for
                    // the new direction (flip cleared it).
                    return Task::batch([
                        self.schedule_quote(),
                        self.probe_rate(cache.btc_usd_price),
                    ]);
                }
                view::LiquidSwapMessage::SwapAll => {
                    self.error = None;
                    if self.last_rate.is_some() {
                        match self.swap_all_amount() {
                            Ok(()) => return self.schedule_quote(),
                            Err(msg) => self.error = Some(msg),
                        }
                    } else {
                        // No rate yet — fetch one on demand and fill the max
                        // when it lands (rather than erroring). `probe_rate`
                        // no-ops if a background probe is already in flight;
                        // either way `RateProbeReady` completes it.
                        self.pending_swap_all = true;
                        self.quoting = true;
                        return self.probe_rate(cache.btc_usd_price);
                    }
                }
                view::LiquidSwapMessage::QuoteRequested(seq) => {
                    // Discard stale debounce timers.
                    if *seq != self.quote_seq {
                        return Task::none();
                    }
                    return self.request_quote(*seq);
                }
                view::LiquidSwapMessage::QuoteReady(seq, result) => {
                    // Discard stale async responses.
                    if *seq != self.quote_seq {
                        return Task::none();
                    }
                    self.quoting = false;
                    match result {
                        Ok(prepare) => {
                            if let Some(receiver_base) = self.receiver_base_to_quote() {
                                self.accept_quote(prepare.clone(), receiver_base);
                            }
                        }
                        Err(e) => {
                            self.quote = None;
                            self.quote_remaining = 0;
                            self.error = Some(self.swap_error_message(e));
                        }
                    }
                }
                view::LiquidSwapMessage::Continue => {
                    if self.quote_actionable() {
                        self.phase = SwapPhase::Review;
                    }
                }
                view::LiquidSwapMessage::BackToInput => {
                    self.phase = SwapPhase::Input;
                }
                view::LiquidSwapMessage::RefreshQuote => {
                    let seq = self.quote_seq.wrapping_add(1);
                    self.quote_seq = seq;
                    self.quote = None;
                    self.quote_remaining = 0;
                    return self.request_quote(seq);
                }
                view::LiquidSwapMessage::QuoteTick => {
                    // Freeze the countdown while a send is in flight: the
                    // committed quote is already executing at SideSwap, so
                    // expiring/clearing it here would clobber the in-progress
                    // swap's UI (and a re-quote would fight the send).
                    if self.is_sending {
                        return Task::none();
                    }
                    if self.quote_remaining > 0 {
                        self.quote_remaining -= 1;
                        if self.quote_remaining == 0 {
                            // Expired: drop the stale quote. On the review
                            // screen, auto re-fetch; never execute an expired
                            // quote.
                            self.quote = None;
                            if self.phase == SwapPhase::Review {
                                let seq = self.quote_seq.wrapping_add(1);
                                self.quote_seq = seq;
                                return self.request_quote(seq);
                            }
                        }
                    }
                }
                view::LiquidSwapMessage::Confirm => {
                    // Duress (I6): no swap-specific guard is needed here. The
                    // confirm path is only reachable inside `State::App`, which
                    // the tab shell wholly replaces with `State::DuressActive`
                    // while duress is active (see `gui/tab.rs`). So this
                    // `send_payment` is structurally unreachable under duress,
                    // exactly like the Send path it shares.
                    if self.phase != SwapPhase::Review || self.is_sending {
                        return Task::none();
                    }
                    // Block while the wallet is still catching up — the swap
                    // would fail server-side (the button is also disabled, so
                    // this is defence-in-depth).
                    if !self.synced {
                        self.error = Some(
                            "Wallet is still syncing — please wait until it finishes before swapping."
                                .to_string(),
                        );
                        return Task::none();
                    }
                    // Never execute an expired quote — re-fetch instead.
                    if self.quote_remaining == 0 {
                        let seq = self.quote_seq.wrapping_add(1);
                        self.quote_seq = seq;
                        self.quote = None;
                        return self.request_quote(seq);
                    }
                    let Some(quote) = self.quote.clone() else {
                        return Task::none();
                    };
                    self.is_sending = true;
                    let breez_client = self.breez_client.clone();
                    return Task::perform(
                        async move {
                            breez_client
                                .send_payment(&breez_sdk_liquid::prelude::SendPaymentRequest {
                                    prepare_response: quote.prepare,
                                    payer_note: None,
                                    // Cross-asset swaps cannot pay fees in the
                                    // asset (SDK constraint) — same as Send.
                                    use_asset_fees: Some(false),
                                })
                                .await
                                .map_err(|e| e.to_string())
                        },
                        |result| match result {
                            Ok(resp) => Message::View(view::Message::LiquidSwap(
                                view::LiquidSwapMessage::SwapComplete {
                                    tx_id: resp.payment.tx_id,
                                    timestamp: resp.payment.timestamp,
                                },
                            )),
                            Err(e) => Message::View(view::Message::LiquidSwap(
                                view::LiquidSwapMessage::SwapFailed(format!("Swap failed: {e}")),
                            )),
                        },
                    );
                }
                view::LiquidSwapMessage::SwapComplete { tx_id, timestamp } => {
                    // Record the completed swap locally (the SDK doesn't mark
                    // swaps, so this is our source of truth for history).
                    if let Some(q) = self.quote.as_ref() {
                        self.history.record(SwapRecord {
                            tx_id: tx_id.clone(),
                            from_asset: self.from_asset.into(),
                            to_asset: self.to_asset.into(),
                            from_base: q.from_total_base(),
                            to_base: q.receiver_base,
                            timestamp: *timestamp,
                        });
                    }
                    // Build the success-screen amount display before clearing.
                    let received = self
                        .quote
                        .as_ref()
                        .map(|q| q.receiver_base)
                        .or_else(|| self.receiver_base_to_quote())
                        .unwrap_or(0);
                    // Honour the user's BTC/SATS preference for L-BTC; USDt
                    // is always a decimal.
                    self.sent_amount_display = match self.to_asset {
                        SendAsset::Usdt => format!("{} USDt", format_usdt_display(received)),
                        SendAsset::Lbtc => {
                            let unit = cache.bitcoin_unit;
                            let label = match unit {
                                BitcoinDisplayUnit::BTC => "BTC",
                                BitcoinDisplayUnit::Sats => "SATS",
                            };
                            format!(
                                "{} {}",
                                Amount::from_sat(received).to_formatted_string_with_unit(unit),
                                label
                            )
                        }
                    };
                    let context = "liquid-send";
                    self.sent_quote = coincube_ui::component::quote_display::random_quote(context);
                    self.sent_image_handle =
                        coincube_ui::component::quote_display::image_handle_for_context(context);
                    self.phase = SwapPhase::Sent;
                    self.is_sending = false;
                    self.quote = None;
                    self.quote_remaining = 0;
                    self.pay_input = form::Value::default();
                    self.receive_input = form::Value::default();
                    self.edit_side = SwapSide::Receive;
                    // Refresh balances after settlement so both sides reconcile.
                    let breez_client = self.breez_client.clone();
                    return Task::perform(async move { breez_client.sync().await }, |_| {
                        Message::View(view::Message::LiquidSwap(
                            view::LiquidSwapMessage::RefreshRequested,
                        ))
                    });
                }
                view::LiquidSwapMessage::SwapFailed(msg) => {
                    self.mark_swap_failed(msg.clone());
                    // The committed quote is stale now (price has moved and the
                    // SideSwap order is gone). On the review screen, fetch a
                    // fresh quote so the user can retry against a current price.
                    if self.phase == SwapPhase::Review {
                        let seq = self.quote_seq.wrapping_add(1);
                        self.quote_seq = seq;
                        return self.request_quote(seq);
                    }
                }
                view::LiquidSwapMessage::Done => {
                    self.phase = SwapPhase::Input;
                    self.pay_input = form::Value::default();
                    self.receive_input = form::Value::default();
                    self.edit_side = SwapSide::Receive;
                    self.quote = None;
                    self.quote_remaining = 0;
                    self.error = None;
                    self.is_sending = false;
                    return self.load_balance();
                }
            }
        }
        Task::none()
    }

    fn subscription(&self) -> Subscription<Message> {
        // Drive the quote-expiry countdown while a live quote exists.
        if self.quote.is_some() && self.quote_remaining > 0 {
            iced::time::every(Duration::from_secs(1)).map(|_| {
                Message::View(view::Message::LiquidSwap(
                    view::LiquidSwapMessage::QuoteTick,
                ))
            })
        } else {
            Subscription::none()
        }
    }

    fn reload(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        _wallet: Option<Arc<Wallet>>,
    ) -> Task<Message> {
        // Reset transient flow state on entry.
        self.phase = SwapPhase::Input;
        self.pay_input = form::Value::default();
        self.receive_input = form::Value::default();
        self.edit_side = SwapSide::Receive;
        self.quote = None;
        self.quote_remaining = 0;
        self.error = None;
        self.is_sending = false;
        self.pending_quote = false;
        // Drop any stale rate so we re-probe a fresh one on entry.
        self.last_rate = None;
        self.rate_probe_inflight = false;
        self.pending_swap_all = false;
        // Assume catching up until the sync we kick below reports `Synced`.
        self.synced = false;

        let breez = self.breez_client.clone();
        Task::batch(vec![
            Task::perform(
                async move {
                    let _ = breez.sync().await;
                },
                |_| Message::CacheUpdated,
            ),
            self.load_balance(),
            // Pre-generate the destination so the first quote is instant.
            self.generate_self_address(),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a disconnected mainnet swap panel — enough to exercise the
    /// pure state logic without a live SDK or `Cache`.
    fn panel() -> LiquidSwap {
        let client = Arc::new(crate::app::breez_liquid::BreezClient::disconnected(
            Network::Bitcoin,
        ));
        // A path that won't exist → empty history; tests don't persist.
        let path = std::env::temp_dir().join("coincube-swap-history-test-missing.json");
        LiquidSwap::new(Arc::new(LiquidBackend::new(client)), path)
    }

    #[test]
    fn swap_supported_only_on_mainnet() {
        // Must match `LiquidSend::cross_asset_supported`, which gates the
        // cross-asset send path on `Network::Bitcoin`.
        assert!(swap_supported(Network::Bitcoin));
        assert!(!swap_supported(Network::Testnet));
        assert!(!swap_supported(Network::Signet));
        assert!(!swap_supported(Network::Regtest));
    }

    #[test]
    fn quote_totals_and_rate() {
        // Pay 0.001 L-BTC (100_000 sat) for 95 USDt, 21 sat fee.
        assert_eq!(from_total_base(100_000, 21), 100_021);
        // 95_000_000 / 100_021 ≈ 949.8 USDt-base per L-BTC-base.
        let rate = rate_to_per_from(95_000_000, 100_021);
        assert!((rate - 949.8).abs() < 1.0);
        // Degenerate: nothing paid → rate 0, no divide-by-zero.
        assert_eq!(rate_to_per_from(100, 0), 0.0);
    }

    #[test]
    fn flip_swaps_assets_and_clears_rate() {
        let mut s = panel();
        s.last_rate = Some(1.0);
        assert_eq!(s.from_asset, SendAsset::Lbtc);
        assert_eq!(s.to_asset, SendAsset::Usdt);

        s.flip_assets();

        assert_eq!(s.from_asset, SendAsset::Usdt);
        assert_eq!(s.to_asset, SendAsset::Lbtc);
        // Stale rate invalidated; the quote itself is cleared by the
        // re-quote the FlipAssets arm schedules.
        assert!(s.last_rate.is_none());
    }

    #[test]
    fn amount_parsing_honors_precision_and_zero() {
        let mut s = panel();
        // 8-dp ok.
        s.receive_input.value = "1.50000000".to_string();
        assert_eq!(s.receiver_base_to_quote(), Some(150_000_000));
        // 9 dp rejected.
        s.receive_input.value = "1.000000001".to_string();
        assert_eq!(s.receiver_base_to_quote(), None);
        // zero rejected (must be > 0).
        s.receive_input.value = "0".to_string();
        assert_eq!(s.receiver_base_to_quote(), None);
        // empty → None.
        s.receive_input.value = String::new();
        assert_eq!(s.receiver_base_to_quote(), None);
    }

    #[test]
    fn quote_actionable_gates() {
        // Expired (remaining 0) is never actionable.
        assert!(!is_quote_actionable(true, 0, false));
        // No quote is never actionable.
        assert!(!is_quote_actionable(false, 5, false));
        // Sending in flight blocks actions.
        assert!(!is_quote_actionable(true, 5, true));
        // Fresh quote, idle → actionable.
        assert!(is_quote_actionable(true, 5, false));
    }

    #[test]
    fn lbtc_fee_error_when_paying_usdt_with_no_lbtc() {
        let mut s = panel();
        s.from_asset = SendAsset::Usdt;
        s.to_asset = SendAsset::Lbtc;
        s.btc_balance = Amount::from_sat(0);
        s.usdt_balance = 2_843_000_000; // 28.43 USDt, plenty
        assert!(s.needs_lbtc_for_fees());
        // The SDK's "not enough funds" really means "no L-BTC for fees" here.
        let msg = s.swap_error_message("SDK request failed: Cannot pay: not enough funds");
        assert!(msg.contains("L-BTC"));

        // With some L-BTC, fall back to the generic insufficient message.
        s.btc_balance = Amount::from_sat(10_000);
        assert!(!s.needs_lbtc_for_fees());
        let generic = s.swap_error_message("not enough funds");
        assert!(generic.contains("Insufficient"));
    }

    #[test]
    fn cached_seed_rate_directions() {
        // 1 L-BTC ≈ price USDt; 1 USDt ≈ 1/price L-BTC (both 8-dp, so the
        // base-unit rate equals the whole-unit rate).
        assert_eq!(
            cached_seed_rate(SendAsset::Lbtc, SendAsset::Usdt, 60_000.0),
            Some(60_000.0)
        );
        let inv = cached_seed_rate(SendAsset::Usdt, SendAsset::Lbtc, 60_000.0).unwrap();
        assert!((inv - 1.0 / 60_000.0).abs() < 1e-12);
        // Non-positive price → no seed.
        assert_eq!(
            cached_seed_rate(SendAsset::Lbtc, SendAsset::Usdt, 0.0),
            None
        );
    }

    #[test]
    fn swap_all_without_rate_errors() {
        let mut s = panel();
        s.last_rate = None;
        let err = s.swap_all_amount().unwrap_err();
        assert!(err.contains("rate"));
        assert!(s.receive_input.value.is_empty());
    }

    #[test]
    fn parse_asset_amount_is_unit_aware() {
        use BitcoinDisplayUnit::{Sats, BTC};
        // USDt: always 8-dp decimal.
        assert_eq!(
            parse_asset_amount("1.50000000", SendAsset::Usdt, BTC),
            Some(150_000_000)
        );
        // L-BTC in BTC mode: 8-dp decimal BTC → sats base units.
        assert_eq!(
            parse_asset_amount("0.08066584", SendAsset::Lbtc, BTC),
            Some(8_066_584)
        );
        // L-BTC in SATS mode: whole sats (== base units), decimals rejected.
        assert_eq!(
            parse_asset_amount("8066584", SendAsset::Lbtc, Sats),
            Some(8_066_584)
        );
        assert_eq!(parse_asset_amount("1.5", SendAsset::Lbtc, Sats), None);
        // Zero parses to Some(0) (callers reject it); empty → None.
        assert_eq!(parse_asset_amount("0", SendAsset::Lbtc, Sats), Some(0));
        assert_eq!(parse_asset_amount("", SendAsset::Lbtc, Sats), None);
    }

    #[test]
    fn format_asset_input_round_trips_per_unit() {
        let mut s = panel();
        s.bitcoin_unit = BitcoinDisplayUnit::Sats;
        // L-BTC SATS → whole-sats string the sats input expects.
        assert_eq!(s.format_asset_input(8_066_584, SendAsset::Lbtc), "8066584");
        s.bitcoin_unit = BitcoinDisplayUnit::BTC;
        assert_eq!(
            s.format_asset_input(8_066_584, SendAsset::Lbtc),
            "0.08066584"
        );
        assert_eq!(
            s.format_asset_input(150_000_000, SendAsset::Usdt),
            "1.50000000"
        );
    }

    #[test]
    fn pay_side_input_converts_via_rate() {
        let mut s = panel();
        // from = L-BTC, to = USDt, rate 60000 USDt-base per L-BTC-base.
        s.from_asset = SendAsset::Lbtc;
        s.to_asset = SendAsset::Usdt;
        s.last_rate = Some(60_000.0);
        s.edit_side = SwapSide::Pay;
        // Pay 0.001 L-BTC (100_000 sats) → receiver ≈ 100_000 * 60_000 base.
        s.pay_input.value = "0.00100000".to_string();
        assert_eq!(s.receiver_base_to_quote(), Some(6_000_000_000));
        // Without a rate, the pay side can't be converted.
        s.last_rate = None;
        assert_eq!(s.receiver_base_to_quote(), None);
    }

    #[test]
    fn confirm_requires_synced_wallet() {
        // An actionable quote is necessary but not sufficient — the wallet
        // must be synced, or the SideSwap order fails server-side.
        assert!(!can_confirm(true, false));
        assert!(can_confirm(true, true));
        // No actionable quote → never confirmable, synced or not.
        assert!(!can_confirm(false, true));
    }

    #[test]
    fn mark_swap_failed_resets_in_flight_state() {
        let mut s = panel();
        s.is_sending = true;
        s.quoting = true;
        s.quote_remaining = 18;
        s.mark_swap_failed("Swap failed: boom".to_string());
        assert!(!s.is_sending);
        assert!(!s.quoting);
        assert_eq!(s.quote_remaining, 0);
        assert!(s.quote.is_none());
        assert_eq!(s.error.as_deref(), Some("Swap failed: boom"));
    }

    #[test]
    fn swap_all_applies_safety_margin() {
        let mut s = panel();
        // 1 L-BTC balance, rate 0.95 USDt-base per L-BTC-base.
        s.from_asset = SendAsset::Lbtc;
        s.to_asset = SendAsset::Usdt;
        s.btc_balance = Amount::from_sat(100_000_000);
        s.last_rate = Some(0.95);
        s.swap_all_amount().unwrap();
        // floor(100_000_000 * 0.95 * 0.995) = 94_525_000 base = 0.94525 USDt —
        // just under the full estimate so the re-quote stays affordable.
        // (Swap All sets the receive side.)
        assert_eq!(s.edit_side, SwapSide::Receive);
        assert_eq!(s.receiver_base_to_quote(), Some(94_525_000));
    }
}
