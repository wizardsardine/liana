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
                    let payments = backend.list_payments(Some(20)).await;
                    (info, payments)
                }
            },
            |(info, payments)| match (info, payments) {
                (Ok(info), Ok(list)) => Message::View(view::Message::SparkOverview(
                    view::SparkOverviewMessage::DataLoaded {
                        balance: Amount::from_sat(info.balance_sats),
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
                    recent_payments,
                } => {
                    self.loading = false;
                    self.error = None;
                    self.snapshot = Some(SparkBalanceSnapshot {
                        balance_sats: balance.to_sat(),
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
                view::SparkOverviewMessage::SelectTransaction(_idx) => {
                    // Spark doesn't expose a transaction-detail panel yet —
                    // fall back to routing to the Transactions list so the
                    // user lands somewhere sensible.
                    return redirect(Menu::Spark(SparkSubMenu::Transactions(None)));
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
    let method = parse_method(&p.method);
    // PaymentSummary carries the signed sat amount; the view displays
    // the absolute value and composes the direction separately.
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
    });

    SparkRecentTransaction {
        description,
        time_ago: format_time_ago(p.timestamp as i64),
        amount,
        fees_sat,
        fiat_amount,
        is_incoming,
        status,
        method,
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
        _ => SparkPaymentMethod::Spark,
    }
}
