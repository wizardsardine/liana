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
use crate::app::breez_liquid::assets::{
    format_asset_amount, parse_asset_to_minor_units, usdt_asset_id, AssetKind, LBTC_PRECISION,
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

/// Continue/Confirm are enabled only for a fresh, unexpired quote while
/// no send is in flight.
fn is_quote_actionable(has_quote: bool, quote_remaining: u32, is_sending: bool) -> bool {
    has_quote && quote_remaining > 0 && !is_sending
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
    /// Receive-amount input (decimal `to`-asset string, 8-dp).
    entered_amount: form::Value<String>,
    /// Self-targeted Liquid address every prepare/send is pointed at.
    self_address: Option<String>,
    /// True while a quote should be issued as soon as `self_address`
    /// arrives (a quote was requested before the address was ready).
    pending_quote: bool,
    /// Current locked/last quote, if any.
    quote: Option<SwapQuote>,
    /// Most recent successful rate (`to` per `from` base unit), kept so
    /// "Swap All" can estimate a max without a fresh round-trip.
    last_rate: Option<f64>,
    /// Whether a quote request is in flight.
    quoting: bool,
    /// Monotonic sequence guarding debounce timers and async results
    /// against staleness.
    quote_seq: u64,
    /// Seconds remaining before the current quote expires.
    quote_remaining: u32,
    phase: SwapPhase,
    is_sending: bool,
    error: Option<String>,
    /// Success-screen celebration assets.
    sent_amount_display: String,
    sent_quote: coincube_ui::component::quote_display::Quote,
    sent_image_handle: iced::widget::image::Handle,
}

impl LiquidSwap {
    pub fn new(breez_client: Arc<LiquidBackend>) -> Self {
        Self {
            breez_client,
            from_asset: SendAsset::Lbtc,
            to_asset: SendAsset::Usdt,
            btc_balance: Amount::from_sat(0),
            usdt_balance: 0,
            entered_amount: form::Value::default(),
            self_address: None,
            pending_quote: false,
            quote: None,
            last_rate: None,
            quoting: false,
            quote_seq: 0,
            quote_remaining: 0,
            phase: SwapPhase::Input,
            is_sending: false,
            error: None,
            sent_amount_display: String::new(),
            sent_quote: coincube_ui::component::quote_display::random_quote("liquid-send"),
            sent_image_handle: coincube_ui::component::quote_display::image_handle_for_context(
                "liquid-send",
            ),
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

    /// Parse the entered receive amount into `to`-asset base units.
    /// Returns `None` for empty/zero/malformed input.
    fn entered_receiver_base(&self) -> Option<u64> {
        let trimmed = self.entered_amount.value.trim();
        if trimmed.is_empty() {
            return None;
        }
        parse_asset_to_minor_units(trimmed, AssetKind::Usdt.precision()).filter(|&v| v > 0)
    }

    /// Bump the sequence, clear the stale quote, and schedule a debounced
    /// quote request for the current input. Returns a debounce timer task.
    fn schedule_quote(&mut self) -> Task<Message> {
        self.quote = None;
        self.quote_remaining = 0;
        self.quote_seq = self.quote_seq.wrapping_add(1);
        let seq = self.quote_seq;

        // Only schedule when the input is a usable amount.
        if self.entered_receiver_base().is_none() {
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
        let Some(receiver_base) = self.entered_receiver_base() else {
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
        self.quote = Some(quote);
        self.quote_remaining = QUOTE_TTL_SECS;
        self.quoting = false;
        self.error = None;
    }

    /// Whether Continue/Confirm should be enabled.
    fn quote_actionable(&self) -> bool {
        is_quote_actionable(self.quote.is_some(), self.quote_remaining, self.is_sending)
    }

    /// Swap the `from`/`to` assets and invalidate the stale rate. The
    /// caller re-quotes (so the quote itself is cleared there).
    fn flip_assets(&mut self) {
        std::mem::swap(&mut self.from_asset, &mut self.to_asset);
        self.last_rate = None;
        self.error = None;
    }

    /// Pre-fill the entered amount with the max receivable from the
    /// current rate, netting fees. Returns `Err` with an inline message
    /// when no rate is available yet or the balance is too low. On `Ok`
    /// the caller re-quotes the freshly-set amount.
    fn swap_all_amount(&mut self) -> Result<(), String> {
        let rate = self
            .last_rate
            .filter(|r| *r > 0.0)
            .ok_or_else(|| "Enter an amount first to get a rate, then use Swap All.".to_string())?;
        let from_balance = self.balance_base(self.from_asset);
        if from_balance == 0 {
            return Err("No balance to swap.".to_string());
        }
        // `rate` already nets fees (receiver per total paid), so spending
        // the whole from-balance yields ~this receiver amount.
        let est_receiver = (from_balance as f64 * rate).floor() as u64;
        if est_receiver == 0 {
            return Err("Balance too low to swap.".to_string());
        }
        self.entered_amount.value = format_asset_amount(est_receiver, AssetKind::Usdt.precision());
        self.entered_amount.valid = true;
        self.entered_amount.warning = None;
        Ok(())
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
            entered_amount: &self.entered_amount,
            quote: self.quote.as_ref(),
            quoting: self.quoting,
            quote_remaining: self.quote_remaining,
            quote_actionable: self.quote_actionable(),
            is_sending: self.is_sending,
            bitcoin_unit: cache.bitcoin_unit,
            error: self.error.as_deref(),
            sent_amount_display: &self.sent_amount_display,
            sent_quote: &self.sent_quote,
            sent_image_handle: &self.sent_image_handle,
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
        if let Message::View(view::Message::LiquidSwap(ref msg)) = message {
            match msg {
                view::LiquidSwapMessage::DataLoaded {
                    btc_balance,
                    usdt_balance,
                } => {
                    self.error = None;
                    self.btc_balance = *btc_balance;
                    self.usdt_balance = *usdt_balance;
                }
                view::LiquidSwapMessage::Error(err) => {
                    self.error = Some(err.clone());
                }
                view::LiquidSwapMessage::RefreshRequested => {
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
                    }
                    Err(e) => {
                        self.pending_quote = false;
                        self.quoting = false;
                        self.error = Some(format!("Couldn't prepare swap address: {e}"));
                    }
                },
                view::LiquidSwapMessage::AmountEdited(value) => {
                    self.entered_amount.value = value.clone();
                    self.error = None;
                    let trimmed = value.trim();
                    if trimmed.is_empty() {
                        self.entered_amount.valid = true;
                        self.entered_amount.warning = None;
                    } else if let Some(base) =
                        parse_asset_to_minor_units(trimmed, AssetKind::Usdt.precision())
                    {
                        if base == 0 {
                            self.entered_amount.valid = false;
                            self.entered_amount.warning = Some("Amount must be greater than zero");
                        } else {
                            self.entered_amount.valid = true;
                            self.entered_amount.warning = None;
                        }
                    } else {
                        self.entered_amount.valid = false;
                        self.entered_amount.warning = Some("Invalid amount");
                    }
                    return self.schedule_quote();
                }
                view::LiquidSwapMessage::FlipAssets => {
                    self.flip_assets();
                    return self.schedule_quote();
                }
                view::LiquidSwapMessage::SwapAll => {
                    self.error = None;
                    match self.swap_all_amount() {
                        Ok(()) => return self.schedule_quote(),
                        Err(msg) => self.error = Some(msg),
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
                            if let Some(receiver_base) = self.entered_receiver_base() {
                                self.accept_quote(prepare.clone(), receiver_base);
                            }
                        }
                        Err(e) => {
                            self.quote = None;
                            self.quote_remaining = 0;
                            self.error = Some(friendly_quote_error(e));
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
                            Ok(_) => Message::View(view::Message::LiquidSwap(
                                view::LiquidSwapMessage::SwapComplete,
                            )),
                            Err(e) => Message::View(view::Message::LiquidSwap(
                                view::LiquidSwapMessage::Error(format!("Swap failed: {e}")),
                            )),
                        },
                    );
                }
                view::LiquidSwapMessage::SwapComplete => {
                    // Build the success-screen amount display before clearing.
                    let received = self
                        .quote
                        .as_ref()
                        .map(|q| q.receiver_base)
                        .or_else(|| self.entered_receiver_base())
                        .unwrap_or(0);
                    // Honour the user's BTC/SATS preference for L-BTC; USDt
                    // is always a decimal.
                    self.sent_amount_display = match self.to_asset {
                        SendAsset::Usdt => format!(
                            "{} USDt",
                            format_asset_amount(received, AssetKind::Usdt.precision())
                        ),
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
                    self.entered_amount = form::Value::default();
                    // Refresh balances after settlement so both sides reconcile.
                    let breez_client = self.breez_client.clone();
                    return Task::perform(async move { breez_client.sync().await }, |_| {
                        Message::View(view::Message::LiquidSwap(
                            view::LiquidSwapMessage::RefreshRequested,
                        ))
                    });
                }
                view::LiquidSwapMessage::Done => {
                    self.phase = SwapPhase::Input;
                    self.entered_amount = form::Value::default();
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
        self.entered_amount = form::Value::default();
        self.quote = None;
        self.quote_remaining = 0;
        self.error = None;
        self.is_sending = false;
        self.pending_quote = false;

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
        LiquidSwap::new(Arc::new(LiquidBackend::new(client)))
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
        s.entered_amount.value = "1.50000000".to_string();
        assert_eq!(s.entered_receiver_base(), Some(150_000_000));
        // 9 dp rejected.
        s.entered_amount.value = "1.000000001".to_string();
        assert_eq!(s.entered_receiver_base(), None);
        // zero rejected (must be > 0).
        s.entered_amount.value = "0".to_string();
        assert_eq!(s.entered_receiver_base(), None);
        // empty → None.
        s.entered_amount.value = String::new();
        assert_eq!(s.entered_receiver_base(), None);
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
    fn swap_all_without_rate_errors() {
        let mut s = panel();
        s.last_rate = None;
        let err = s.swap_all_amount().unwrap_err();
        assert!(err.contains("rate"));
        assert!(s.entered_amount.value.is_empty());
    }

    #[test]
    fn swap_all_nets_fees_into_estimate() {
        let mut s = panel();
        // 1 L-BTC balance, rate 0.95 USDt-base per L-BTC-base (fees baked in).
        s.from_asset = SendAsset::Lbtc;
        s.to_asset = SendAsset::Usdt;
        s.btc_balance = Amount::from_sat(100_000_000);
        s.last_rate = Some(0.95);
        s.swap_all_amount().unwrap();
        // floor(100_000_000 * 0.95) = 95_000_000 base = 0.95 USDt.
        assert_eq!(s.entered_receiver_base(), Some(95_000_000));
    }
}
