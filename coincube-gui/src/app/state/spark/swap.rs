//! Spark Swap panel: BTC ⇄ USDB inside the Spark wallet (Tenshu v175 S3).
//!
//! Details → Review → Swapped. The form asks the bridge for an estimate
//! (`prepare_swap`), Review shows that estimate, and Swap executes its
//! single-use handle (`swap`), a self-transfer with conversion options.
//!
//! The estimate is not a locked price: the slippage cap is the only
//! protection, so the copy says "estimated". Every async answer carries the
//! request it answers, and an answer to a request the form has moved past
//! (an edit, a flip, a newer quote) is dropped, so an old estimate can never
//! reach Review or be swapped.
//!
//! A swap whose answer never came back, or came back as an SDK error that can
//! follow submission, is *unknown*, not failed: the panel says so and points
//! to Activity before it offers another swap.

use std::sync::Arc;

use coincube_core::miniscript::bitcoin::Network;
use coincube_spark_protocol::{
    AssetAmount, ConversionLimits, PrepareSwapOk, SwapAmount, SwapOk, UsdbBalanceSnapshot,
    WalletAsset,
};
use coincube_ui::widget::Element;
use iced::Task;

use crate::app::breez_spark::SparkClientError;
use crate::app::cache::Cache;
use crate::app::menu::Menu;
use crate::app::message::Message;
use crate::app::state::State;
use crate::app::view;
use crate::app::view::spark::{SparkRecentTransaction, SparkSwapMessage};
use crate::app::wallets::SparkBackend;

/// USDB's precision when the wallet holds none yet, so nothing reports its
/// decimals. USDB ships at 6.
pub(crate) const USDB_DEFAULT_DECIMALS: u32 = 6;

/// Whether Spark offers Swap on `network`. Conversions run on mainnet only:
/// the bridge refuses them elsewhere and only the mainnet USDB token is
/// configured.
pub fn swap_available(network: Network) -> bool {
    crate::feature_flags::SPARK_SWAP && network == Network::Bitcoin
}

/// What happens to bitcoin received next, from the bridge's auto-convert
/// state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoConvertLine {
    /// Off, including a paused breaker (which turns it off): it stays bitcoin.
    Off,
    /// On: it converts to USDB.
    On,
    /// On, but held while a swap out of USDB settles: bitcoin received
    /// meanwhile stays bitcoin (`AutoConvertState::held_after_swap`).
    Held,
}

impl AutoConvertLine {
    pub fn from_state(state: &coincube_spark_protocol::AutoConvertState) -> Self {
        if !state.enabled {
            Self::Off
        } else if state.held_after_swap {
            Self::Held
        } else {
            Self::On
        }
    }
}

/// Where the swap stands.
#[derive(Debug, Clone)]
pub enum SwapPhase {
    /// The form. `error` is the last estimate's refusal, if any.
    Details { error: Option<String> },
    /// Waiting for the estimate.
    Quoting,
    /// The estimate, waiting for the user to swap.
    Review(Box<PrepareSwapOk>),
    /// The swap is submitted and its answer hasn't come back.
    Swapping(Box<PrepareSwapOk>),
    /// The bridge answered the swap.
    Swapped(SwapOk),
    /// The bridge refused the swap before submitting it: nothing moved.
    Refused(String),
    /// No definite answer: the swap may or may not have gone through.
    Unknown(String),
}

/// How a swap request ended, without the client's error type, so the
/// message stays a plain value.
#[derive(Debug, Clone)]
pub enum SwapAttempt {
    Done(SwapOk),
    Refused(String),
    Unknown(String),
}

impl SwapAttempt {
    fn from_result(result: Result<SwapOk, SparkClientError>) -> Self {
        match result {
            Ok(ok) => Self::Done(ok),
            Err(SparkClientError::OutcomeUnknown { message, .. }) => Self::Unknown(message),
            // `swap` returns a definite error only for the refusals the
            // bridge sends before it starts a swap.
            Err(SparkClientError::BridgeError { message, .. }) => Self::Refused(message),
            // A transport or protocol failure: the request may have reached
            // the bridge, so the outcome is unknown.
            Err(other) => Self::Unknown(other.to_string()),
        }
    }
}

pub struct SparkSwap {
    backend: Option<Arc<SparkBackend>>,
    /// The asset converted out of. The other one is the target.
    from: WalletAsset,
    amount_input: String,
    /// Max: convert all of `from`; the bridge works out the figure.
    max: bool,
    btc_sats: u64,
    usdb: Option<UsdbBalanceSnapshot>,
    balance_loaded: bool,
    /// The minimums for converting out of the asset they are keyed by.
    limits: Option<(WalletAsset, ConversionLimits)>,
    phase: SwapPhase,
    /// Identity of the latest estimate or swap request. Any edit bumps it,
    /// so an answer to an older request is dropped.
    request: u64,
    /// What auto-convert does with bitcoin received next, once read: the
    /// side panel says it (H-Convert).
    auto_convert: Option<AutoConvertLine>,
    /// Identity of the latest auto-convert read. A swap's answer bumps it,
    /// so a read sent before the swap can't land on its result.
    auto_convert_read: u64,
    /// The swap's result arrived while another page was showing, so the
    /// user hasn't seen it: the next visit shows it instead of a new form.
    result_unseen: bool,
    /// Spark's latest payments, as Send and Receive show them in the side
    /// panel.
    recent_transactions: Vec<SparkRecentTransaction>,
}

impl SparkSwap {
    pub fn new(backend: Option<Arc<SparkBackend>>) -> Self {
        Self {
            backend,
            from: WalletAsset::Bitcoin,
            amount_input: String::new(),
            max: false,
            btc_sats: 0,
            usdb: None,
            balance_loaded: false,
            limits: None,
            phase: SwapPhase::Details { error: None },
            request: 0,
            auto_convert: None,
            auto_convert_read: 0,
            result_unseen: false,
            recent_transactions: Vec::new(),
        }
    }

    pub fn recent_transactions_for_view(&self) -> &[SparkRecentTransaction] {
        &self.recent_transactions
    }

    /// Keep the latest five of `payments`, priced at the cache's fiat rate,
    /// as Send and Receive do.
    pub fn set_recent_payments(
        &mut self,
        payments: &[coincube_spark_protocol::PaymentSummary],
        cache: &Cache,
    ) {
        self.recent_transactions = super::recent_transactions_from(payments, cache);
    }

    pub fn from(&self) -> WalletAsset {
        self.from
    }

    pub fn phase(&self) -> &SwapPhase {
        &self.phase
    }

    #[cfg(test)]
    pub(crate) fn set_phase_for_tests(&mut self, phase: SwapPhase) {
        self.phase = phase;
    }

    /// Put the panel where `Confirm` leaves it and return that request.
    #[cfg(test)]
    pub(crate) fn start_swapping_for_tests(&mut self, ok: PrepareSwapOk) -> u64 {
        self.request += 1;
        self.phase = SwapPhase::Swapping(Box::new(ok));
        self.request
    }

    pub(crate) fn amount_input_for_view(&self) -> &str {
        &self.amount_input
    }

    pub(crate) fn max_for_view(&self) -> bool {
        self.max
    }

    pub(crate) fn btc_sats_for_view(&self) -> u64 {
        self.btc_sats
    }

    pub(crate) fn usdb_for_view(&self) -> Option<&UsdbBalanceSnapshot> {
        self.usdb.as_ref()
    }

    pub(crate) fn usdb_decimals_for_view(&self) -> u32 {
        self.usdb_decimals()
    }

    fn usdb_decimals(&self) -> u32 {
        self.usdb
            .as_ref()
            .map_or(USDB_DEFAULT_DECIMALS, |usdb| usdb.decimals)
    }

    /// The balance of `asset`, in its base units.
    pub fn balance(&self, asset: WalletAsset) -> u128 {
        match asset {
            WalletAsset::Bitcoin => u128::from(self.btc_sats),
            WalletAsset::Usdb => self.usdb.as_ref().map_or(0, |usdb| usdb.balance),
        }
    }

    /// The least that can be converted out of `from`, once known.
    pub fn minimum(&self) -> Option<AssetAmount> {
        self.limits
            .as_ref()
            .filter(|(asset, _)| *asset == self.from)
            .and_then(|(_, limits)| limits.min_from)
    }

    /// The typed amount in `from`'s base units: sats, or USDB to its
    /// decimals. `None` when blank or not a positive amount.
    pub fn typed_amount(&self) -> Option<u128> {
        parse_amount(&self.amount_input, self.from, self.usdb_decimals())
    }

    /// Why Get quote is unavailable, or `None` when it is.
    pub fn form_error(&self) -> Option<String> {
        if !self.balance_loaded {
            return Some("Loading your balances…".into());
        }
        let balance = self.balance(self.from);
        if balance == 0 {
            return Some(match self.from {
                WalletAsset::Bitcoin => "You have no bitcoin to swap.".into(),
                WalletAsset::Usdb => "You have no USDB to swap.".into(),
            });
        }
        if self.max {
            return None;
        }
        if self.amount_input.trim().is_empty() {
            return Some("Enter an amount.".into());
        }
        let Some(amount) = self.typed_amount() else {
            return Some("Enter a valid amount.".into());
        };
        if amount > balance {
            return Some("That is more than you have.".into());
        }
        if let Some(minimum) = self.minimum().filter(|min| amount < min.amount) {
            return Some(coincube_ui::localized_format!(
                "The minimum is {}.",
                "El mínimo es {}.",
                format_asset(minimum.asset, minimum.amount, self.usdb_decimals())
            ));
        }
        None
    }

    fn can_quote(&self) -> bool {
        matches!(self.phase, SwapPhase::Details { .. }) && self.form_error().is_none()
    }

    /// Leave Review or a refusal for the form: the estimate no longer
    /// matches what is typed. Any request in flight is dropped.
    fn back_to_details(&mut self) {
        if matches!(
            self.phase,
            SwapPhase::Review(_) | SwapPhase::Quoting | SwapPhase::Refused(_)
        ) {
            self.phase = SwapPhase::Details { error: None };
        }
        if let SwapPhase::Details { error } = &mut self.phase {
            *error = None;
        }
        self.request += 1;
    }

    /// Edits are allowed only while nothing is being swapped.
    fn editable(&self) -> bool {
        matches!(
            self.phase,
            SwapPhase::Details { .. }
                | SwapPhase::Quoting
                | SwapPhase::Review(_)
                | SwapPhase::Refused(_)
        )
    }

    fn fetch_balance(&self) -> Task<Message> {
        super::fetch_balance_task(self.backend.clone(), |balance| {
            Message::View(view::Message::SparkSwap(SparkSwapMessage::BalanceLoaded(
                balance,
            )))
        })
    }

    fn fetch_payments(&self) -> Task<Message> {
        super::fetch_payments_task(
            self.backend.clone(),
            |payments| {
                Message::View(view::Message::SparkSwap(SparkSwapMessage::PaymentsLoaded(
                    payments,
                )))
            },
            |err| {
                Message::View(view::Message::SparkSwap(SparkSwapMessage::PaymentsFailed(
                    err,
                )))
            },
        )
    }

    fn fetch_auto_convert(&self) -> Task<Message> {
        let Some(backend) = self.backend.clone() else {
            return Task::none();
        };
        let read = self.auto_convert_read;
        Task::perform(
            async move {
                backend
                    .client()
                    .get_auto_convert()
                    .await
                    .ok()
                    .map(|state| AutoConvertLine::from_state(&state))
            },
            move |line| {
                Message::View(view::Message::SparkSwap(
                    SparkSwapMessage::AutoConvertLoaded { read, line },
                ))
            },
        )
    }

    /// The line to show, once read. Hidden while a swap is out: the bridge
    /// may hold auto-convert for it, and the answer re-reads the state.
    pub(crate) fn auto_convert_for_view(&self) -> Option<AutoConvertLine> {
        self.auto_convert
            .filter(|_| !matches!(self.phase, SwapPhase::Swapping(_)))
    }

    fn fetch_limits(&self) -> Task<Message> {
        let Some(backend) = self.backend.clone() else {
            return Task::none();
        };
        let from = self.from;
        Task::perform(
            async move { backend.client().get_conversion_limits(from).await.ok() },
            move |limits| {
                Message::View(view::Message::SparkSwap(SparkSwapMessage::LimitsLoaded {
                    from,
                    limits,
                }))
            },
        )
    }

    fn reverse_swap(&mut self) -> Task<Message> {
        if !self.editable() {
            return Task::none();
        }
        self.from = match self.from {
            WalletAsset::Bitcoin => WalletAsset::Usdb,
            WalletAsset::Usdb => WalletAsset::Bitcoin,
        };
        self.amount_input.clear();
        self.max = false;
        self.back_to_details();
        self.fetch_limits()
    }

    fn request_swap_quote(&mut self) -> Task<Message> {
        if !self.can_quote() {
            return Task::none();
        }
        let Some(backend) = self.backend.clone() else {
            return Task::none();
        };
        let amount = if self.max {
            SwapAmount::Max { from: self.from }
        } else {
            let Some(amount) = self.typed_amount() else {
                return Task::none();
            };
            SwapAmount::Exact {
                amount_in: AssetAmount {
                    asset: self.from,
                    amount,
                },
            }
        };
        self.request += 1;
        let request = self.request;
        self.phase = SwapPhase::Quoting;
        Task::perform(
            async move {
                backend
                    .client()
                    .prepare_swap(amount, None)
                    .await
                    .map_err(|e| e.to_string())
            },
            move |result| {
                Message::View(view::Message::SparkSwap(SparkSwapMessage::QuoteLoaded {
                    request,
                    result,
                }))
            },
        )
    }

    fn settle_swap_quote(
        &mut self,
        request: u64,
        result: Result<PrepareSwapOk, String>,
    ) -> Task<Message> {
        if request != self.request || !matches!(self.phase, SwapPhase::Quoting) {
            return Task::none();
        }
        self.phase = match result {
            Ok(ok) => {
                self.limits = Some((self.from, ok.limits.clone()));
                SwapPhase::Review(Box::new(ok))
            }
            Err(error) => SwapPhase::Details { error: Some(error) },
        };
        Task::none()
    }

    fn confirm_swap(&mut self) -> Task<Message> {
        let SwapPhase::Review(ok) = &self.phase else {
            return Task::none();
        };
        let Some(backend) = self.backend.clone() else {
            return Task::none();
        };
        let ok = ok.clone();
        let handle = ok.handle.clone();
        self.request += 1;
        let request = self.request;
        self.phase = SwapPhase::Swapping(ok);
        Task::perform(
            async move { SwapAttempt::from_result(backend.client().swap(handle).await) },
            move |result| {
                Message::View(view::Message::SparkSwap(SparkSwapMessage::Swapped {
                    request,
                    result,
                }))
            },
        )
    }

    fn settle_swap(&mut self, request: u64, result: SwapAttempt) -> Task<Message> {
        if request != self.request || !matches!(self.phase, SwapPhase::Swapping(_)) {
            return Task::none();
        }
        self.phase = match result {
            SwapAttempt::Done(ok) => SwapPhase::Swapped(ok),
            SwapAttempt::Refused(message) => SwapPhase::Refused(message),
            SwapAttempt::Unknown(message) => SwapPhase::Unknown(message),
        };
        // The balances moved (or may have), and a swap out of USDB may
        // have put auto-convert on hold: show both as they are now.
        // The line read before the swap may no longer hold, so the
        // result shows none until a read sent from here succeeds.
        self.auto_convert = None;
        self.auto_convert_read += 1;
        Task::batch([
            self.fetch_balance(),
            self.fetch_auto_convert(),
            self.fetch_payments(),
        ])
    }

    fn start_another_swap(&mut self) -> Task<Message> {
        if matches!(
            self.phase,
            SwapPhase::Swapped(_) | SwapPhase::Unknown(_) | SwapPhase::Refused(_)
        ) {
            self.amount_input.clear();
            self.max = false;
            self.phase = SwapPhase::Details { error: None };
            self.result_unseen = false;
            self.request += 1;
            return Task::batch([self.fetch_balance(), self.fetch_limits()]);
        }
        Task::none()
    }

    fn open_swap_transaction(&mut self, idx: usize) -> Task<Message> {
        let Some(payment) = self.recent_transactions.get(idx).cloned() else {
            return Task::none();
        };
        Task::batch(vec![
            crate::app::state::redirect(Menu::Spark(crate::app::menu::SparkSubMenu::Transactions(
                None,
            ))),
            Task::done(Message::View(view::Message::SparkTransactions(
                view::SparkTransactionsMessage::Preselect(payment),
            ))),
        ])
    }

    fn settle_swap_limits(
        &mut self,
        from: WalletAsset,
        limits: Option<ConversionLimits>,
    ) -> Task<Message> {
        if from == self.from {
            if let Some(limits) = limits {
                self.limits = Some((from, limits));
            }
        }
        Task::none()
    }

    /// The app's entry point: every Swap message reaches this panel whether
    /// or not it is showing, so a swap's answer is never lost to navigation.
    /// `on_screen` says whether the Swap page is the current one.
    pub(crate) fn deliver(&mut self, message: SparkSwapMessage, on_screen: bool) -> Task<Message> {
        let answer = matches!(message, SparkSwapMessage::Swapped { .. });
        let swapping = matches!(self.phase, SwapPhase::Swapping(_));
        let task = self.on_message(message);
        if answer && swapping && !matches!(self.phase, SwapPhase::Swapping(_)) {
            self.result_unseen = !on_screen;
        }
        task
    }

    pub(crate) fn on_message(&mut self, message: SparkSwapMessage) -> Task<Message> {
        match message {
            SparkSwapMessage::AmountEdited(input) => {
                if self.editable() {
                    self.amount_input = input;
                    self.max = false;
                    self.back_to_details();
                }
                Task::none()
            }
            SparkSwapMessage::Max => {
                if self.editable() {
                    self.max = true;
                    self.amount_input.clear();
                    self.back_to_details();
                }
                Task::none()
            }
            SparkSwapMessage::Reverse => self.reverse_swap(),
            SparkSwapMessage::EditAmount => {
                if self.editable() {
                    self.back_to_details();
                }
                Task::none()
            }
            SparkSwapMessage::GetQuote => self.request_swap_quote(),
            SparkSwapMessage::QuoteLoaded { request, result } => {
                self.settle_swap_quote(request, result)
            }
            SparkSwapMessage::Confirm => self.confirm_swap(),
            SparkSwapMessage::Swapped { request, result } => self.settle_swap(request, result),
            SparkSwapMessage::SwapAgain => self.start_another_swap(),
            SparkSwapMessage::BalanceLoaded(balance) => {
                if let Some((btc_sats, usdb)) = balance {
                    self.btc_sats = btc_sats;
                    self.usdb = usdb;
                    self.balance_loaded = true;
                }
                Task::none()
            }
            SparkSwapMessage::LimitsLoaded { from, limits } => {
                self.settle_swap_limits(from, limits)
            }
            // Pricing needs the cache: `App` and `update` route a load to
            // `set_recent_payments` before it gets here.
            SparkSwapMessage::PaymentsLoaded(_) => Task::none(),
            SparkSwapMessage::PaymentsFailed(err) => {
                tracing::warn!("spark swap list_payments failed: {}", err);
                self.recent_transactions.clear();
                Task::none()
            }
            SparkSwapMessage::SelectTransaction(idx) => self.open_swap_transaction(idx),
            SparkSwapMessage::History => crate::app::state::redirect(Menu::Spark(
                crate::app::menu::SparkSubMenu::Transactions(None),
            )),
            SparkSwapMessage::AutoConvertLoaded { read, line } => {
                // An unread state keeps the last one rather than guessing.
                if read == self.auto_convert_read && line.is_some() {
                    self.auto_convert = line;
                }
                Task::none()
            }
        }
    }
}

impl State for SparkSwap {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        crate::app::view::spark::swap::swap_page(menu, cache, self)
    }

    fn update(
        &mut self,
        _daemon: Option<Arc<dyn crate::daemon::Daemon + Sync + Send>>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        match message {
            Message::View(view::Message::SparkSwap(SparkSwapMessage::PaymentsLoaded(payments))) => {
                self.set_recent_payments(&payments, cache);
                Task::none()
            }
            Message::View(view::Message::SparkSwap(message)) => self.deliver(message, true),
            _ => Task::none(),
        }
    }

    fn reload(
        &mut self,
        _daemon: Option<Arc<dyn crate::daemon::Daemon + Sync + Send>>,
        _wallet: Option<Arc<crate::app::wallet::Wallet>>,
    ) -> Task<Message> {
        // Coming back after a finished swap the user has seen starts a new
        // one. A result that arrived while they were away, a swap still
        // waiting for its answer, or one whose outcome is unknown stays on
        // screen until the user moves on from it.
        if matches!(self.phase, SwapPhase::Swapped(_) | SwapPhase::Refused(_))
            && !std::mem::take(&mut self.result_unseen)
        {
            self.amount_input.clear();
            self.max = false;
            self.phase = SwapPhase::Details { error: None };
        }
        if matches!(self.phase, SwapPhase::Quoting | SwapPhase::Review(_)) {
            // A quote left on screen while away may have expired: ask again.
            self.phase = SwapPhase::Details { error: None };
        }
        // The swap in flight keeps its request, so its answer still lands.
        if !matches!(self.phase, SwapPhase::Swapping(_)) {
            self.request += 1;
        }
        Task::batch([
            self.fetch_balance(),
            self.fetch_limits(),
            self.fetch_auto_convert(),
            self.fetch_payments(),
        ])
    }
}

/// `input` in `asset`'s base units: whole sats, or USDB with at most
/// `decimals` fraction digits. Thousands separators and spaces are ignored.
/// `None` unless it is a positive amount.
pub fn parse_amount(input: &str, asset: WalletAsset, decimals: u32) -> Option<u128> {
    let cleaned: String = input
        .chars()
        .filter(|c| !matches!(c, ',' | '_' | ' '))
        .collect();
    if cleaned.is_empty() {
        return None;
    }
    let amount = match asset {
        WalletAsset::Bitcoin => {
            if !cleaned.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            cleaned.parse::<u128>().ok()?
        }
        WalletAsset::Usdb => parse_usdb_amount(&cleaned, decimals)?,
    };
    (amount > 0).then_some(amount)
}

fn parse_usdb_amount(cleaned: &str, decimals: u32) -> Option<u128> {
    let (whole, frac) = cleaned.split_once('.').unwrap_or((cleaned, ""));
    let digits = |s: &str| s.chars().all(|c| c.is_ascii_digit());
    if !digits(whole) || !digits(frac) || frac.len() > decimals as usize {
        return None;
    }
    if whole.is_empty() && frac.is_empty() {
        return None;
    }
    let scale = 10_u128.checked_pow(decimals)?;
    let whole: u128 = if whole.is_empty() {
        0
    } else {
        whole.parse().ok()?
    };
    let frac_units: u128 = if frac.is_empty() {
        0
    } else {
        format!("{:0<width$}", frac, width = decimals as usize)
            .parse()
            .ok()?
    };
    whole.checked_mul(scale)?.checked_add(frac_units)
}

/// `amount` of `asset` for display: "1,000,000 SATS" or "1,000.00 USDB".
pub fn format_asset(asset: WalletAsset, amount: u128, usdb_decimals: u32) -> String {
    match asset {
        WalletAsset::Bitcoin => format!(
            "{} SATS",
            crate::app::breez_spark::assets::format_token_display_u128(amount, 0)
        ),
        WalletAsset::Usdb => format!(
            "{} USDB",
            crate::app::breez_spark::assets::format_token_display_u128(amount, usdb_decimals)
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use coincube_spark_protocol::{ConversionQuote, SwapStatus};

    fn usdb(dollars: u128) -> UsdbBalanceSnapshot {
        UsdbBalanceSnapshot {
            token_identifier: "btkn1synthetic".into(),
            balance: dollars * 1_000_000,
            decimals: 6,
            ticker: "USDB".into(),
        }
    }

    fn loaded(btc_sats: u64, usdb_dollars: u128) -> SparkSwap {
        let mut swap = SparkSwap::new(None);
        drop(swap.on_message(SparkSwapMessage::BalanceLoaded(Some((
            btc_sats,
            (usdb_dollars > 0).then(|| usdb(usdb_dollars)),
        )))));
        swap
    }

    fn quote(from: WalletAsset, amount_in: u128) -> PrepareSwapOk {
        let to = match from {
            WalletAsset::Bitcoin => WalletAsset::Usdb,
            WalletAsset::Usdb => WalletAsset::Bitcoin,
        };
        PrepareSwapOk {
            handle: "handle-1".into(),
            quote: ConversionQuote {
                amount_in: AssetAmount {
                    asset: from,
                    amount: amount_in,
                },
                amount_out: AssetAmount {
                    asset: to,
                    amount: 800_600_000,
                },
                fee: AssetAmount {
                    asset: from,
                    amount: 1_000,
                },
                amount_adjustment: None,
                max_slippage_bps: 50,
            },
            limits: ConversionLimits {
                min_from: Some(AssetAmount {
                    asset: from,
                    amount: 10_000,
                }),
                min_to: None,
            },
        }
    }

    /// Stand in for the estimate request, which needs a backend: the form
    /// as `GetQuote` leaves it.
    fn quoting(swap: &mut SparkSwap) -> u64 {
        assert!(swap.can_quote(), "{:?}", swap.form_error());
        swap.request += 1;
        swap.phase = SwapPhase::Quoting;
        swap.request
    }

    #[test]
    fn amounts_parse_in_each_assets_units() {
        assert_eq!(
            parse_amount("1,000,000", WalletAsset::Bitcoin, 6),
            Some(1_000_000)
        );
        assert_eq!(
            parse_amount("1.5", WalletAsset::Bitcoin, 6),
            None,
            "whole sats"
        );
        assert_eq!(parse_amount("0", WalletAsset::Bitcoin, 6), None);
        assert_eq!(
            parse_amount("1,000.50", WalletAsset::Usdb, 6),
            Some(1_000_500_000)
        );
        assert_eq!(parse_amount(".25", WalletAsset::Usdb, 6), Some(250_000));
        assert_eq!(
            parse_amount("1.0000001", WalletAsset::Usdb, 6),
            None,
            "past USDB's precision"
        );
        assert_eq!(parse_amount("abc", WalletAsset::Usdb, 6), None);
        assert_eq!(parse_amount("", WalletAsset::Usdb, 6), None);
    }

    #[test]
    fn the_form_checks_balance_and_minimum_before_a_quote() {
        let mut swap = SparkSwap::new(None);
        assert_eq!(swap.form_error().as_deref(), Some("Loading your balances…"));
        let mut swap_loaded = loaded(50_000, 0);
        assert_eq!(
            swap_loaded.form_error().as_deref(),
            Some("Enter an amount.")
        );
        drop(swap_loaded.on_message(SparkSwapMessage::AmountEdited("60000".into())));
        assert_eq!(
            swap_loaded.form_error().as_deref(),
            Some("That is more than you have.")
        );
        drop(swap_loaded.on_message(SparkSwapMessage::LimitsLoaded {
            from: WalletAsset::Bitcoin,
            limits: Some(ConversionLimits {
                min_from: Some(AssetAmount {
                    asset: WalletAsset::Bitcoin,
                    amount: 10_000,
                }),
                min_to: None,
            }),
        }));
        drop(swap_loaded.on_message(SparkSwapMessage::AmountEdited("9000".into())));
        assert_eq!(
            swap_loaded.form_error().as_deref(),
            Some("The minimum is 10,000 SATS.")
        );
        drop(swap_loaded.on_message(SparkSwapMessage::AmountEdited("20000".into())));
        assert_eq!(swap_loaded.form_error(), None);
        // A minimum for the other direction doesn't apply after a flip.
        drop(swap_loaded.on_message(SparkSwapMessage::Reverse));
        assert_eq!(
            swap_loaded.form_error().as_deref(),
            Some("You have no USDB to swap.")
        );
        swap = loaded(0, 100);
        drop(swap.on_message(SparkSwapMessage::Reverse));
        drop(swap.on_message(SparkSwapMessage::AmountEdited("0.50".into())));
        assert_eq!(swap.minimum(), None);
        assert_eq!(swap.form_error(), None);
    }

    #[test]
    fn estimate_review_then_swap_and_the_result() {
        let mut swap = loaded(2_000_000, 0);
        drop(swap.on_message(SparkSwapMessage::AmountEdited("1,000,000".into())));
        let request = quoting(&mut swap);
        drop(swap.on_message(SparkSwapMessage::QuoteLoaded {
            request,
            result: Ok(quote(WalletAsset::Bitcoin, 1_000_000)),
        }));
        assert!(matches!(swap.phase, SwapPhase::Review(_)));
        assert_eq!(
            swap.minimum().map(|min| min.amount),
            Some(10_000),
            "the estimate's limits are kept"
        );

        // Confirm needs a backend to submit; stand in for its effect.
        let SwapPhase::Review(ok) = swap.phase.clone() else {
            unreachable!()
        };
        swap.request += 1;
        swap.phase = SwapPhase::Swapping(ok);
        let request = swap.request;
        drop(swap.on_message(SparkSwapMessage::Swapped {
            request,
            result: SwapAttempt::Done(SwapOk {
                swap_id: "swap".into(),
                status: SwapStatus::Completed,
            }),
        }));
        assert!(matches!(swap.phase, SwapPhase::Swapped(_)));
        drop(swap.on_message(SparkSwapMessage::SwapAgain));
        assert!(matches!(swap.phase, SwapPhase::Details { error: None }));
        assert!(swap.amount_input.is_empty());
    }

    #[test]
    fn an_edit_makes_the_estimate_stale_and_drops_late_answers() {
        let mut swap = loaded(2_000_000, 0);
        drop(swap.on_message(SparkSwapMessage::AmountEdited("100000".into())));
        let request = quoting(&mut swap);
        // Edited while the estimate is in flight: it must not reach Review.
        drop(swap.on_message(SparkSwapMessage::AmountEdited("200000".into())));
        drop(swap.on_message(SparkSwapMessage::QuoteLoaded {
            request,
            result: Ok(quote(WalletAsset::Bitcoin, 100_000)),
        }));
        assert!(matches!(swap.phase, SwapPhase::Details { .. }));

        // In Review, an edit, a flip or Edit amount goes back to the form.
        for edit in [
            SparkSwapMessage::AmountEdited("300000".into()),
            SparkSwapMessage::Max,
            SparkSwapMessage::EditAmount,
            SparkSwapMessage::Reverse,
        ] {
            let mut swap = loaded(2_000_000, 100);
            drop(swap.on_message(SparkSwapMessage::AmountEdited("100000".into())));
            let request = quoting(&mut swap);
            drop(swap.on_message(SparkSwapMessage::QuoteLoaded {
                request,
                result: Ok(quote(WalletAsset::Bitcoin, 100_000)),
            }));
            assert!(matches!(swap.phase, SwapPhase::Review(_)));
            let label = format!("{:?}", edit);
            drop(swap.on_message(edit));
            assert!(
                matches!(swap.phase, SwapPhase::Details { .. }),
                "{} leaves Review",
                label
            );
            assert!(!matches!(swap.phase, SwapPhase::Review(_)));
        }
    }

    #[test]
    fn a_refused_estimate_stays_on_the_form_with_its_reason() {
        let mut swap = loaded(2_000_000, 0);
        drop(swap.on_message(SparkSwapMessage::AmountEdited("100000".into())));
        let request = quoting(&mut swap);
        drop(swap.on_message(SparkSwapMessage::QuoteLoaded {
            request,
            result: Err("below the minimum".into()),
        }));
        assert!(matches!(
            &swap.phase,
            SwapPhase::Details { error: Some(error) } if error == "below the minimum"
        ));
    }

    #[test]
    fn an_unknown_swap_is_never_called_failed_and_blocks_edits() {
        assert!(matches!(
            SwapAttempt::from_result(Err(SparkClientError::OutcomeUnknown {
                request_id: 1,
                message: "timed out".into(),
            })),
            SwapAttempt::Unknown(_)
        ));
        assert!(matches!(
            SwapAttempt::from_result(Err(SparkClientError::BridgeError {
                kind: coincube_spark_protocol::ErrorKind::BadRequest,
                message: "handle expired".into(),
            })),
            SwapAttempt::Refused(_)
        ));
        assert!(matches!(
            SwapAttempt::from_result(Err(SparkClientError::BridgeUnavailable("gone".into()))),
            SwapAttempt::Unknown(_)
        ));

        let mut swap = loaded(2_000_000, 0);
        swap.request += 1;
        swap.phase = SwapPhase::Swapping(Box::new(quote(WalletAsset::Bitcoin, 100_000)));
        let request = swap.request;
        // An answer to an older request is dropped.
        drop(swap.on_message(SparkSwapMessage::Swapped {
            request: request - 1,
            result: SwapAttempt::Done(SwapOk {
                swap_id: "old".into(),
                status: SwapStatus::Completed,
            }),
        }));
        assert!(matches!(swap.phase, SwapPhase::Swapping(_)));
        // Nothing can be edited while the swap is out.
        drop(swap.on_message(SparkSwapMessage::AmountEdited("1".into())));
        drop(swap.on_message(SparkSwapMessage::Reverse));
        assert!(matches!(swap.phase, SwapPhase::Swapping(_)));
        assert_eq!(swap.from, WalletAsset::Bitcoin);
        drop(swap.on_message(SparkSwapMessage::Swapped {
            request,
            result: SwapAttempt::Unknown("timed out".into()),
        }));
        assert!(matches!(swap.phase, SwapPhase::Unknown(_)));
        drop(swap.on_message(SparkSwapMessage::AmountEdited("1".into())));
        assert!(
            matches!(swap.phase, SwapPhase::Unknown(_)),
            "no edit past an unknown swap"
        );
        // Leaving and coming back keeps it on screen.
        drop(swap.reload(None, None));
        assert!(matches!(swap.phase, SwapPhase::Unknown(_)));
    }

    #[test]
    fn a_swap_answer_survives_leaving_and_coming_back() {
        let done = || {
            SwapAttempt::Done(SwapOk {
                swap_id: "swap".into(),
                status: SwapStatus::Completed,
            })
        };

        // Away and back while the swap is out: its answer still lands.
        let mut swap = loaded(2_000_000, 0);
        let request = swap.start_swapping_for_tests(quote(WalletAsset::Bitcoin, 100_000));
        drop(swap.reload(None, None));
        assert!(matches!(swap.phase, SwapPhase::Swapping(_)));
        drop(swap.deliver(
            SparkSwapMessage::Swapped {
                request,
                result: done(),
            },
            true,
        ));
        assert!(
            matches!(swap.phase, SwapPhase::Swapped(_)),
            "the in-flight request survives a reload"
        );

        // The answer arrives while another page shows: the next visit shows
        // it, and the visit after that starts a new swap.
        let mut swap = loaded(2_000_000, 0);
        let request = swap.start_swapping_for_tests(quote(WalletAsset::Bitcoin, 100_000));
        drop(swap.deliver(
            SparkSwapMessage::Swapped {
                request,
                result: done(),
            },
            false,
        ));
        assert!(matches!(swap.phase, SwapPhase::Swapped(_)));
        drop(swap.reload(None, None));
        assert!(
            matches!(swap.phase, SwapPhase::Swapped(_)),
            "unseen result kept"
        );
        drop(swap.reload(None, None));
        assert!(matches!(swap.phase, SwapPhase::Details { error: None }));

        // A refusal that arrives away is kept the same way.
        let mut swap = loaded(2_000_000, 0);
        let request = swap.start_swapping_for_tests(quote(WalletAsset::Bitcoin, 100_000));
        drop(swap.deliver(
            SparkSwapMessage::Swapped {
                request,
                result: SwapAttempt::Refused("handle expired".into()),
            },
            false,
        ));
        drop(swap.reload(None, None));
        assert!(matches!(swap.phase, SwapPhase::Refused(_)));

        // Seen on screen: leaving and coming back starts a new swap.
        let mut swap = loaded(2_000_000, 0);
        let request = swap.start_swapping_for_tests(quote(WalletAsset::Bitcoin, 100_000));
        drop(swap.deliver(
            SparkSwapMessage::Swapped {
                request,
                result: done(),
            },
            true,
        ));
        drop(swap.reload(None, None));
        assert!(matches!(swap.phase, SwapPhase::Details { error: None }));
    }

    #[test]
    fn the_auto_convert_state_is_kept_once_read() {
        let mut swap = SparkSwap::new(None);
        assert_eq!(swap.auto_convert_for_view(), None);
        drop(swap.on_message(SparkSwapMessage::AutoConvertLoaded {
            read: 0,
            line: Some(AutoConvertLine::Off),
        }));
        assert_eq!(swap.auto_convert_for_view(), Some(AutoConvertLine::Off));
        drop(swap.on_message(SparkSwapMessage::AutoConvertLoaded {
            read: 0,
            line: None,
        }));
        assert_eq!(
            swap.auto_convert_for_view(),
            Some(AutoConvertLine::Off),
            "an unread state keeps the last"
        );
        drop(swap.on_message(SparkSwapMessage::AutoConvertLoaded {
            read: 0,
            line: Some(AutoConvertLine::On),
        }));
        assert_eq!(swap.auto_convert_for_view(), Some(AutoConvertLine::On));
        // While a swap is out the line is hidden: the bridge may hold
        // auto-convert for it.
        swap.request += 1;
        swap.phase = SwapPhase::Swapping(Box::new(quote(WalletAsset::Usdb, 1_000_000)));
        assert_eq!(swap.auto_convert_for_view(), None);
    }

    /// Grok and Codex on #1019: the result must not repeat the line read
    /// before the swap. It shows only what a read sent after the answer says,
    /// and stays hidden if that read fails.
    #[test]
    fn a_swap_result_shows_only_a_fresh_auto_convert_read() {
        let mut swap = SparkSwap::new(None);
        drop(swap.on_message(SparkSwapMessage::AutoConvertLoaded {
            read: 0,
            line: Some(AutoConvertLine::On),
        }));
        assert_eq!(swap.auto_convert_for_view(), Some(AutoConvertLine::On));
        let request = swap.start_swapping_for_tests(quote(WalletAsset::Usdb, 1_000_000));
        drop(swap.on_message(SparkSwapMessage::Swapped {
            request,
            result: SwapAttempt::Done(SwapOk {
                swap_id: "swap".into(),
                status: SwapStatus::Completed,
            }),
        }));
        assert!(matches!(swap.phase, SwapPhase::Swapped(_)));
        assert_eq!(
            swap.auto_convert_for_view(),
            None,
            "the pre-send line is dropped"
        );
        drop(swap.on_message(SparkSwapMessage::AutoConvertLoaded {
            read: 0,
            line: Some(AutoConvertLine::On),
        }));
        assert_eq!(
            swap.auto_convert_for_view(),
            None,
            "a read sent before the swap is ignored"
        );
        drop(swap.on_message(SparkSwapMessage::AutoConvertLoaded {
            read: 1,
            line: None,
        }));
        assert_eq!(
            swap.auto_convert_for_view(),
            None,
            "a failed re-read stays hidden"
        );
        drop(swap.on_message(SparkSwapMessage::AutoConvertLoaded {
            read: 1,
            line: Some(AutoConvertLine::Held),
        }));
        assert_eq!(swap.auto_convert_for_view(), Some(AutoConvertLine::Held));
    }

    /// Grok on #1019: a hold after a swap out of USDB means bitcoin received
    /// meanwhile stays bitcoin, so the line must not say it converts.
    #[test]
    fn a_held_auto_convert_reads_as_held() {
        use coincube_spark_protocol::{AutoConvertBreaker, AutoConvertState};
        let state = |enabled, held_after_swap, breaker| AutoConvertState {
            enabled,
            breaker,
            held_after_swap,
        };
        assert_eq!(
            AutoConvertLine::from_state(&state(true, false, AutoConvertBreaker::Ready)),
            AutoConvertLine::On
        );
        assert_eq!(
            AutoConvertLine::from_state(&state(true, true, AutoConvertBreaker::Ready)),
            AutoConvertLine::Held
        );
        assert_eq!(
            AutoConvertLine::from_state(&state(false, false, AutoConvertBreaker::Ready)),
            AutoConvertLine::Off
        );
        assert_eq!(
            AutoConvertLine::from_state(&state(
                false,
                false,
                AutoConvertBreaker::Paused {
                    failures: 3,
                    reason: "price moved".into(),
                }
            )),
            AutoConvertLine::Off,
            "a paused breaker turns it off"
        );
    }

    #[test]
    fn swap_is_offered_on_mainnet_only() {
        for network in [Network::Testnet, Network::Signet, Network::Regtest] {
            assert!(!swap_available(network));
        }
        assert_eq!(
            swap_available(Network::Bitcoin),
            crate::feature_flags::SPARK_SWAP
        );
    }

    #[test]
    fn recent_payments_keep_five_and_clear_when_the_load_fails() {
        let cache = Cache::default();
        let mut swap = SparkSwap::new(None);
        let payment = |n: i64| super::super::test_payment(&format!("pay-{}", n), n);
        drop(State::update(
            &mut swap,
            None,
            &cache,
            Message::View(view::Message::SparkSwap(SparkSwapMessage::PaymentsLoaded(
                (1..=7).map(payment).collect(),
            ))),
        ));
        assert_eq!(swap.recent_transactions_for_view().len(), 5);
        drop(swap.on_message(SparkSwapMessage::PaymentsFailed("offline".into())));
        assert!(swap.recent_transactions_for_view().is_empty());
    }
}
