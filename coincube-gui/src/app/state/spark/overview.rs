//! Spark overview panel.
//!
//! Mirrors [`crate::app::state::liquid::overview::LiquidOverview`] — the
//! layout is the same Liquid-style unified portfolio card + recent
//! transactions list, minus the USDt asset row (Spark doesn't hold
//! Liquid assets) and with BTC branding instead of L-BTC. A
//! [`SparkOverviewView::stable_balance_active`] flag drives a small
//! "Stable" badge on the balance header when the SDK reports an
//! active Stable Balance token (Phase 6).

use std::convert::TryInto;
use std::sync::Arc;

use coincube_core::miniscript::bitcoin::Amount;
use coincube_spark_protocol::PaymentSummary;
use coincube_ui::widget::Element;
use iced::Task;

use crate::app::cache::Cache;
use crate::app::menu::{Menu, SparkSubMenu};
use crate::app::message::Message;
use crate::app::state::{redirect, State};
use crate::app::view::spark::{
    SparkOverviewView, SparkPaymentMethod, SparkRecentTransaction, SparkStatus,
};
use crate::app::view::{self, FiatAmountConverter};
use crate::app::wallets::{DomainPaymentStatus, SparkBackend};
use crate::daemon::Daemon;
use crate::utils::format_time_ago;

/// Loaded info snapshot. `None` while the first `reload()` is in
/// flight or if the bridge returned an error.
#[derive(Debug, Clone)]
pub struct SparkBalanceSnapshot {
    pub balance_sats: u64,
    /// USDB amount in token base units, when Stable Balance is on
    /// and the SDK reports a non-zero USDB holding. The view folds
    /// this into the unified portfolio total at the current BTC
    /// price, the same way Liquid folds USDt into L-BTC.
    pub stable_balance: Option<coincube_spark_protocol::StableBalanceSnapshot>,
}

pub struct SparkOverview {
    backend: Option<Arc<SparkBackend>>,
    snapshot: Option<SparkBalanceSnapshot>,
    recent_transactions: Vec<SparkRecentTransaction>,
    error: Option<String>,
    loading: bool,
    /// Phase 6: cached Stable Balance flag. `None` until the
    /// `get_user_settings` RPC returns; `Some(true)` renders a
    /// "Stable" badge next to the balance in the overview.
    stable_balance_active: Option<bool>,
}

impl SparkOverview {
    pub fn new(backend: Option<Arc<SparkBackend>>) -> Self {
        Self {
            backend,
            snapshot: None,
            recent_transactions: Vec::new(),
            error: None,
            loading: false,
            stable_balance_active: None,
        }
    }
}

impl State for SparkOverview {
    fn view<'a>(
        &'a self,
        menu: &'a Menu,
        cache: &'a Cache,
    ) -> Element<'a, crate::app::view::Message> {
        let fiat_converter: Option<FiatAmountConverter> =
            cache.fiat_price.as_ref().and_then(|p| p.try_into().ok());

        let status = if self.backend.is_none() {
            SparkStatus::Unavailable
        } else if self.loading && self.snapshot.is_none() {
            SparkStatus::Loading
        } else if let Some(snapshot) = &self.snapshot {
            SparkStatus::Connected(snapshot.clone())
        } else if let Some(err) = &self.error {
            SparkStatus::Error(err.clone())
        } else {
            SparkStatus::Loading
        };

        let overview = SparkOverviewView {
            status,
            recent_transactions: &self.recent_transactions,
            fiat_converter,
            bitcoin_unit: cache.bitcoin_unit,
            show_direction_badges: cache.show_direction_badges,
            stable_balance_active: self.stable_balance_active.unwrap_or(false),
            btc_usd_price: cache.btc_usd_price,
            display_mode: cache.display_mode,
        }
        .render()
        .map(view::Message::SparkOverview);

        crate::app::view::dashboard(menu, cache, overview)
    }

    fn reload(
        &mut self,
        _daemon: Option<Arc<dyn crate::daemon::Daemon + Sync + Send>>,
        _wallet: Option<Arc<crate::app::wallet::Wallet>>,
    ) -> Task<Message> {
        let Some(backend) = self.backend.clone() else {
            return Task::none();
        };
        self.loading = true;
        self.error = None;
        let data_task = Task::perform(
            {
                let backend = backend.clone();
                async move {
                    let info = backend.get_info().await;
                    let payments = backend.list_payments(Some(20), None).await;
                    (info, payments)
                }
            },
            |(info, payments)| match (info, payments) {
                (Ok(info), Ok(list)) => Message::View(view::Message::SparkOverview(
                    view::SparkOverviewMessage::DataLoaded {
                        balance: Amount::from_sat(info.balance_sats),
                        stable_balance: info.stable_balance,
                        recent_payments: list.payments,
                    },
                )),
                (Err(e), _) | (_, Err(e)) => Message::View(view::Message::SparkOverview(
                    view::SparkOverviewMessage::Error(e.to_string()),
                )),
            },
        );
        let settings_task = Task::perform(
            async move { backend.get_user_settings().await },
            |result| match result {
                Ok(settings) => Message::View(view::Message::SparkOverview(
                    view::SparkOverviewMessage::StableBalanceLoaded(settings.stable_balance_active),
                )),
                Err(e) => {
                    tracing::warn!("spark overview get_user_settings failed: {}", e);
                    // Preserve previous stable_balance_active state on
                    // transient failures — do not emit StableBalanceLoaded.
                    Message::Tick
                }
            },
        );
        Task::batch(vec![data_task, settings_task])
    }

    fn update(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        if let Message::View(view::Message::SparkOverview(msg)) = message {
            match msg {
                view::SparkOverviewMessage::DataLoaded {
                    balance,
                    stable_balance,
                    recent_payments,
                } => {
                    self.loading = false;
                    self.error = None;
                    self.snapshot = Some(SparkBalanceSnapshot {
                        balance_sats: balance.to_sat(),
                        stable_balance,
                    });

                    let fiat_converter: Option<FiatAmountConverter> =
                        cache.fiat_price.as_ref().and_then(|p| p.try_into().ok());
                    self.recent_transactions = recent_payments
                        .iter()
                        .take(5)
                        .map(|p| payment_summary_to_recent_tx(p, fiat_converter.as_ref()))
                        .collect();
                }
                view::SparkOverviewMessage::Error(err) => {
                    self.loading = false;
                    self.error = Some(err.clone());
                    return Task::done(Message::View(view::Message::ShowError(err)));
                }
                view::SparkOverviewMessage::StableBalanceLoaded(active) => {
                    self.stable_balance_active = Some(active);
                }
                view::SparkOverviewMessage::SendBtc => {
                    return redirect(Menu::Spark(SparkSubMenu::Send));
                }
                view::SparkOverviewMessage::ReceiveBtc => {
                    return redirect(Menu::Spark(SparkSubMenu::Receive));
                }
                view::SparkOverviewMessage::History => {
                    return redirect(Menu::Spark(SparkSubMenu::Transactions(None)));
                }
                view::SparkOverviewMessage::SelectTransaction(idx) => {
                    if let Some(payment) = self.recent_transactions.get(idx).cloned() {
                        return Task::batch(vec![
                            redirect(Menu::Spark(SparkSubMenu::Transactions(None))),
                            Task::done(Message::View(view::Message::SparkTransactions(
                                view::SparkTransactionsMessage::Preselect(payment),
                            ))),
                        ]);
                    }
                }
                view::SparkOverviewMessage::FlipDisplayMode => {
                    return Task::done(Message::View(view::Message::FlipDisplayMode));
                }
            }
        }
        Task::none()
    }

    fn subscription(&self) -> iced::Subscription<Message> {
        iced::Subscription::none()
    }
}

/// Build the view-level `SparkRecentTransaction` row from a bridge
/// `PaymentSummary`. The bridge already strips SDK-specific types to
/// strings + scalars, so the mapping is mostly decoration:
/// direction/status normalization, method lookup for the row icon,
/// and fiat conversion for the secondary amount line.
///
/// Shared with [`crate::app::state::spark::transactions::SparkTransactions`]
/// so both the overview list preview and the full Transactions page
/// produce identical row shapes.
pub(crate) fn payment_summary_to_recent_tx(
    p: &PaymentSummary,
    fiat_converter: Option<&FiatAmountConverter>,
) -> SparkRecentTransaction {
    let is_incoming = p.direction.eq_ignore_ascii_case("Receive");
    let status = parse_status(&p.status);

    if let Some(token_amount) = p.token_amount {
        return token_payment_to_recent_tx(p, token_amount, is_incoming, status);
    }

    let method = parse_method(&p.method);
    // PaymentSummary carries the unsigned sat amount; the view displays
    // it and composes the direction from the `direction` field.
    let amount = Amount::from_sat(p.amount_sat.unsigned_abs());
    let fees_sat = Amount::from_sat(p.fees_sat);
    let fiat_amount = fiat_converter.map(|c| c.convert(amount));

    let description = p.description.clone().unwrap_or_else(|| match method {
        SparkPaymentMethod::Lightning => "Lightning payment".to_string(),
        SparkPaymentMethod::OnChainBitcoin => {
            if is_incoming {
                "On-chain deposit".to_string()
            } else {
                "On-chain withdrawal".to_string()
            }
        }
        SparkPaymentMethod::Spark => "Spark transfer".to_string(),
        SparkPaymentMethod::StableBalance => "Stable Balance".to_string(),
    });

    SparkRecentTransaction {
        id: p.id.clone(),
        description,
        time_ago: format_time_ago(p.timestamp as i64),
        timestamp: p.timestamp,
        amount,
        fees_sat,
        fiat_amount,
        is_incoming,
        status,
        method,
        token_display: None,
    }
}

/// Build a recent-tx row for a `method=token` payment. The bridge
/// strips the sat fields to zero in this case and ships the token
/// amount + decimals + ticker; the view renders that as the headline
/// figure ("1.58 USDB") instead of pretending it's sats. USDB pegs
/// 1:1 to USD, so the secondary fiat line is the token amount itself,
/// not a BTC-derived conversion.
fn token_payment_to_recent_tx(
    p: &PaymentSummary,
    token_amount: u64,
    is_incoming: bool,
    status: DomainPaymentStatus,
) -> SparkRecentTransaction {
    use crate::app::breez_spark::assets::{format_token_display, MAX_TOKEN_DECIMALS_U64};
    use crate::app::view::vault::fiat::FiatAmount;
    use crate::services::fiat::Currency;

    // Clamp once so the formatter and the fiat math see the same
    // decimals value. `format_token_display` clamps internally, but
    // the `10_f64.powi(decimals as i32)` below would wrap for
    // pathological inputs (`decimals > i32::MAX` casts to negative,
    // making `powi` near-zero and the dollar figure blow up).
    let decimals = p.token_decimals.unwrap_or(0).min(MAX_TOKEN_DECIMALS_U64);
    let ticker = p.token_ticker.clone().unwrap_or_else(|| "USDB".to_string());
    let token_str = format_token_display(token_amount, decimals);
    let token_display = format!("{} {}", token_str, ticker);

    // USDB ≈ $1, so the dollar value is `amount / 10^decimals`. We
    // surface that as the row's secondary text via `FiatAmount` so it
    // matches the existing fiat label style.
    let dollar_value = if decimals == 0 {
        token_amount as f64
    } else {
        token_amount as f64 / 10_f64.powi(decimals as i32)
    };
    let fiat_amount = FiatAmount::new(dollar_value, Currency::USD).ok();

    let description = p
        .description
        .clone()
        .unwrap_or_else(|| "Stable Balance".to_string());

    SparkRecentTransaction {
        id: p.id.clone(),
        description,
        time_ago: format_time_ago(p.timestamp as i64),
        timestamp: p.timestamp,
        // Zero out the sat amount so the row's pending-sat sums and
        // any callers that read `amount` for BTC totals don't pick up
        // a token figure as if it were satoshis.
        amount: Amount::ZERO,
        fees_sat: Amount::ZERO,
        fiat_amount,
        is_incoming,
        status,
        method: SparkPaymentMethod::StableBalance,
        token_display: Some(token_display),
    }
}

fn parse_status(raw: &str) -> DomainPaymentStatus {
    // Spark SDK statuses: Completed, Pending, Failed. The bridge ships
    // them via `{:?}` so the casing matches the variant names exactly.
    if raw.eq_ignore_ascii_case("Completed") || raw.eq_ignore_ascii_case("Complete") {
        DomainPaymentStatus::Complete
    } else if raw.eq_ignore_ascii_case("Pending") {
        DomainPaymentStatus::Pending
    } else if raw.eq_ignore_ascii_case("Failed") {
        DomainPaymentStatus::Failed
    } else {
        DomainPaymentStatus::Pending
    }
}

fn parse_method(raw: &str) -> SparkPaymentMethod {
    match raw.to_lowercase().as_str() {
        "lightning" => SparkPaymentMethod::Lightning,
        "deposit" | "withdraw" => SparkPaymentMethod::OnChainBitcoin,
        "token" => SparkPaymentMethod::StableBalance,
        _ => SparkPaymentMethod::Spark,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payment(direction: &str, status: &str, method: &str, amount_sat: i64) -> PaymentSummary {
        PaymentSummary {
            id: "payment-1".to_string(),
            amount_sat,
            fees_sat: 12,
            token_amount: None,
            token_decimals: None,
            token_ticker: None,
            timestamp: 1_700_000_000,
            status: status.to_string(),
            direction: direction.to_string(),
            method: method.to_string(),
            description: None,
        }
    }

    #[test]
    fn lightning_receive_maps_direction_status_amount_and_default_description() {
        let row = payment_summary_to_recent_tx(
            &payment("receive", "completed", "lightning", 42_000),
            None,
        );

        assert_eq!(row.id, "payment-1");
        assert!(row.is_incoming);
        assert_eq!(row.status, DomainPaymentStatus::Complete);
        assert_eq!(row.method, SparkPaymentMethod::Lightning);
        assert_eq!(row.amount.to_sat(), 42_000);
        assert_eq!(row.fees_sat.to_sat(), 12);
        assert_eq!(row.description, "Lightning payment");
        assert_eq!(row.token_display, None);
    }

    #[test]
    fn onchain_withdraw_uses_outgoing_description_and_failed_status() {
        let row =
            payment_summary_to_recent_tx(&payment("Send", "Failed", "withdraw", -25_000), None);

        assert!(!row.is_incoming);
        assert_eq!(row.status, DomainPaymentStatus::Failed);
        assert_eq!(row.method, SparkPaymentMethod::OnChainBitcoin);
        assert_eq!(row.amount.to_sat(), 25_000);
        assert_eq!(row.description, "On-chain withdrawal");
    }

    #[test]
    fn unknown_wire_values_degrade_to_pending_spark_transfer() {
        let row =
            payment_summary_to_recent_tx(&payment("outbound", "future", "future", 1_000), None);

        assert!(!row.is_incoming);
        assert_eq!(row.status, DomainPaymentStatus::Pending);
        assert_eq!(row.method, SparkPaymentMethod::Spark);
        assert_eq!(row.description, "Spark transfer");
    }

    #[test]
    fn token_payment_uses_token_units_instead_of_satoshis() {
        let mut summary = payment("Receive", "Complete", "token", 999_999);
        summary.fees_sat = 500;
        summary.token_amount = Some(1_580_000);
        summary.token_decimals = Some(6);
        summary.token_ticker = Some("USDB".to_string());

        let row = payment_summary_to_recent_tx(&summary, None);

        assert!(row.is_incoming);
        assert_eq!(row.status, DomainPaymentStatus::Complete);
        assert_eq!(row.method, SparkPaymentMethod::StableBalance);
        assert_eq!(row.amount, Amount::ZERO);
        assert_eq!(row.fees_sat, Amount::ZERO);
        assert_eq!(row.description, "Stable Balance");
        assert_eq!(row.token_display.as_deref(), Some("1.58 USDB"));
        assert!(row.fiat_amount.is_some());
    }
}
