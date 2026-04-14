use std::collections::HashMap;
use std::convert::TryInto;
use std::sync::Arc;
use std::time::Instant;

use breez_sdk_liquid::model::{PaymentDetails, RefundRequest};
use breez_sdk_liquid::prelude::{Payment, RefundableSwap};
use coincube_core::miniscript::bitcoin::Amount;
use coincube_ui::component::form;
use coincube_ui::component::quote_display::{self, Quote};
use coincube_ui::widget::*;
use iced::{widget::image, Task};

use crate::app::breez::assets::usdt_asset_id;
use crate::app::view::FeeratePriority;
use crate::app::{breez::BreezClient, cache::Cache, menu::Menu, state::State};
use crate::app::{message::Message, view, wallet::Wallet};
use crate::daemon::Daemon;
use crate::export::{ImportExportMessage, ImportExportState};
use crate::services::feeestimation::fee_estimation::FeeEstimator;

/// A refund that the user has submitted but for which we have not yet seen the
/// SDK drop the swap from `list_refundables()`. While an entry exists, the
/// Transactions view keeps rendering the refundable so the user gets a visible
/// "Refund broadcasting…" / "Refund broadcast · txid …" confirmation instead
/// of the card vanishing silently on success.
#[derive(Debug, Clone)]
pub struct InFlightRefund {
    pub refund_txid: Option<String>,
    pub submitted_at: Instant,
}

/// How long an optimistic in-flight refund (no txid yet) is preserved across
/// `RefundablesLoaded` reconciliation even when the SDK no longer returns the
/// swap. Covers the race where a background poll completes between our
/// `refund_onchain_tx` broadcast and the corresponding `RefundCompleted`
/// message.
const IN_FLIGHT_GRACE: std::time::Duration = std::time::Duration::from_secs(60);

#[derive(Debug)]
enum LiquidTransactionsModal {
    None,
    Export { state: ImportExportState },
}

pub struct LiquidTransactions {
    breez_client: Arc<BreezClient>,
    payments: Vec<Payment>,
    refundables: Vec<RefundableSwap>,
    selected_payment: Option<Payment>,
    selected_refundable: Option<RefundableSwap>,
    loading: bool,
    balance: Amount,
    modal: LiquidTransactionsModal,
    refund_address: form::Value<String>,
    refund_feerate: form::Value<String>,
    fee_estimator: FeeEstimator,
    refunding: bool,
    asset_filter: AssetFilter,
    /// While a fee-priority button is awaiting its async rate fetch, this
    /// holds which one was pressed so the view can show a "…" spinner on it.
    pending_fee_priority: Option<FeeratePriority>,
    /// Refunds submitted by the user that have not yet been dropped from the
    /// SDK's refundables list. Keyed by swap_address. See `InFlightRefund`.
    in_flight_refunds: HashMap<String, InFlightRefund>,
    empty_state_quote: Quote,
    empty_state_image_handle: image::Handle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetFilter {
    All,
    LbtcOnly,
    UsdtOnly,
}

impl LiquidTransactions {
    pub fn new(breez_client: Arc<BreezClient>) -> Self {
        let empty_state_quote = quote_display::random_quote("empty-wallet");
        let empty_state_image_handle = quote_display::image_handle_for_context("empty-wallet");
        Self {
            breez_client,
            payments: Vec::new(),
            refundables: Vec::new(),
            selected_payment: None,
            selected_refundable: None,
            loading: false,
            balance: Amount::ZERO,
            modal: LiquidTransactionsModal::None,
            refund_address: form::Value::default(),
            refund_feerate: form::Value::default(),
            fee_estimator: FeeEstimator::new(),
            refunding: false,
            asset_filter: AssetFilter::All,
            pending_fee_priority: None,
            in_flight_refunds: HashMap::new(),
            empty_state_quote,
            empty_state_image_handle,
        }
    }

    pub fn in_flight_refunds(&self) -> &HashMap<String, InFlightRefund> {
        &self.in_flight_refunds
    }

    pub fn pending_fee_priority(&self) -> Option<FeeratePriority> {
        self.pending_fee_priority
    }

    fn reconcile_in_flight(&mut self, mut refundables: Vec<RefundableSwap>) {
        let returned: std::collections::HashSet<String> =
            refundables.iter().map(|r| r.swap_address.clone()).collect();
        let now = Instant::now();
        self.in_flight_refunds.retain(|addr, entry| {
            if returned.contains(addr) {
                return true;
            }
            // Swap is no longer listed by the SDK. Normally that means the
            // refund broadcast propagated and the swap can be dropped. But
            // an optimistic entry (refund_txid == None) that's still within
            // the grace window may simply be waiting for `RefundCompleted`
            // to land — don't erase it yet, or the "Refund broadcasting…"
            // banner would disappear before the user sees it.
            entry.refund_txid.is_none() && now.duration_since(entry.submitted_at) < IN_FLIGHT_GRACE
        });
        // Carry forward any locally-known RefundableSwap whose address still
        // has a grace-window in_flight entry but that the SDK dropped. The
        // view iterates `self.refundables` to render cards and only uses
        // `in_flight_refunds` for extra metadata, so without this the
        // "Refund broadcasting…" card would vanish the instant the SDK
        // stopped listing the swap, defeating the grace window.
        for prev in std::mem::take(&mut self.refundables) {
            if !returned.contains(&prev.swap_address)
                && self.in_flight_refunds.contains_key(&prev.swap_address)
            {
                refundables.push(prev);
            }
        }
        self.refundables = refundables;
    }

    #[cfg(test)]
    pub fn test_reconcile_in_flight(&mut self, refundables: Vec<RefundableSwap>) {
        self.reconcile_in_flight(refundables);
    }

    pub fn asset_filter(&self) -> AssetFilter {
        self.asset_filter
    }

    pub fn preselect(&mut self, payment: Payment) {
        self.selected_payment = Some(payment);
    }

    fn calculate_balance(&self) -> Amount {
        use breez_sdk_liquid::prelude::PaymentType;
        let usdt_id = usdt_asset_id(self.breez_client.network()).unwrap_or("");
        let mut balance: i64 = 0;

        for payment in &self.payments {
            let is_usdt = matches!(
                &payment.details,
                PaymentDetails::Liquid { asset_id, .. } if !usdt_id.is_empty() && asset_id == usdt_id
            );

            match self.asset_filter {
                AssetFilter::UsdtOnly if !is_usdt => continue,
                AssetFilter::LbtcOnly if is_usdt => continue,
                AssetFilter::All => {
                    // For All mode, skip USDt from balance calc since
                    // USDt amount_sat is in asset base units, not sats
                    if is_usdt {
                        continue;
                    }
                }
                _ => {}
            }

            match payment.payment_type {
                PaymentType::Receive => {
                    balance += payment.amount_sat as i64;
                }
                PaymentType::Send => {
                    balance -= payment.amount_sat as i64;
                }
            }
        }

        Amount::from_sat(balance.max(0) as u64)
    }
}

impl State for LiquidTransactions {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        let fiat_converter = cache.fiat_price.as_ref().and_then(|p| p.try_into().ok());
        let refundable_swap_addresses: Vec<String> = self
            .refundables
            .iter()
            .map(|r| r.swap_address.clone())
            .collect();
        let content = if let Some(payment) = &self.selected_payment {
            view::dashboard(
                menu,
                cache,
                view::liquid::transaction_detail_view(
                    payment,
                    fiat_converter,
                    cache.bitcoin_unit,
                    usdt_asset_id(self.breez_client.network()).unwrap_or(""),
                    &refundable_swap_addresses,
                ),
            )
        } else if let Some(refundable) = &self.selected_refundable {
            view::dashboard(
                menu,
                cache,
                view::liquid::refundable_detail_view(
                    refundable,
                    fiat_converter,
                    cache.bitcoin_unit,
                    &self.refund_address,
                    &self.refund_feerate,
                    self.refunding,
                    self.pending_fee_priority,
                    self.in_flight_refunds.get(&refundable.swap_address),
                    cache.has_vault,
                ),
            )
        } else {
            view::dashboard(
                menu,
                cache,
                view::liquid::liquid_transactions_view(
                    &self.payments,
                    &self.refundables,
                    &self.in_flight_refunds,
                    &self.balance,
                    fiat_converter,
                    self.loading,
                    cache.bitcoin_unit,
                    usdt_asset_id(self.breez_client.network()).unwrap_or(""),
                    self.asset_filter,
                    cache.show_direction_badges,
                    &self.empty_state_quote,
                    &self.empty_state_image_handle,
                ),
            )
        };

        match &self.modal {
            LiquidTransactionsModal::None => content,
            LiquidTransactionsModal::Export { state } => {
                use crate::app::view::Message as ViewMessage;
                use coincube_ui::component::text::*;
                use coincube_ui::widget::modal::Modal;

                let modal_content = match state {
                    ImportExportState::Ended => Column::new()
                        .spacing(20)
                        .push(text("Export successful!").size(20).bold())
                        .push(
                            coincube_ui::component::button::primary(None, "Close")
                                .width(150)
                                .on_press(ViewMessage::ImportExport(ImportExportMessage::Close)),
                        ),
                    _ => Column::new()
                        .spacing(20)
                        .push(text("Exporting payments...").size(20).bold()),
                };

                Modal::new(content, modal_content)
                    .on_blur(Some(ViewMessage::ImportExport(ImportExportMessage::Close)))
                    .into()
            }
        }
    }

    fn update(
        &mut self,
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        _cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        match message {
            Message::PaymentsLoaded(Ok(payments)) => {
                self.loading = false;
                let usdt_id = usdt_asset_id(self.breez_client.network()).unwrap_or("");
                self.payments = match self.asset_filter {
                    AssetFilter::UsdtOnly => payments
                        .into_iter()
                        .filter(|p| {
                            matches!(
                                &p.details,
                                PaymentDetails::Liquid { asset_id, .. }
                                    if asset_id == usdt_id
                            )
                        })
                        .collect(),
                    AssetFilter::LbtcOnly => payments
                        .into_iter()
                        .filter(|p| {
                            !matches!(
                                &p.details,
                                PaymentDetails::Liquid { asset_id, .. }
                                    if asset_id == usdt_id
                            )
                        })
                        .collect(),
                    AssetFilter::All => payments,
                };
                self.balance = self.calculate_balance();
                Task::none()
            }
            Message::PaymentsLoaded(Err(e)) => {
                self.loading = false;
                Task::done(Message::View(view::Message::ShowError(e.to_string())))
            }
            Message::RefundablesLoaded(Ok(refundables)) => {
                // Reconcile in-flight refunds with the freshly-fetched list.
                // A swap leaves `list_refundables` once the SDK observes our
                // refund tx, so anything tracked locally that is no longer in
                // the SDK's list and has an observed broadcast (or has
                // exceeded the grace window) can be dropped.
                self.reconcile_in_flight(refundables);
                Task::none()
            }
            Message::RefundablesLoaded(Err(e)) => {
                Task::done(Message::View(view::Message::ShowError(e.to_string())))
            }
            Message::View(view::Message::Select(i)) => {
                self.selected_payment = self.payments.get(i).cloned();
                self.selected_refundable = None;
                Task::none()
            }
            Message::View(view::Message::SelectRefundable(i)) => {
                self.selected_refundable = self.refundables.get(i).cloned();
                self.selected_payment = None;
                self.refund_address = form::Value::default();
                self.refund_feerate = form::Value::default();
                self.pending_fee_priority = None;
                Task::none()
            }
            Message::View(view::Message::Reload) => self.reload(None, None),
            Message::View(view::Message::Close) => {
                self.selected_payment = None;
                self.selected_refundable = None;
                self.modal = LiquidTransactionsModal::None;
                self.refund_address = form::Value::default();
                self.refund_feerate = form::Value::default();
                self.pending_fee_priority = None;
                Task::none()
            }
            Message::View(view::Message::PreselectPayment(payment)) => {
                self.selected_payment = Some(payment);
                Task::none()
            }
            Message::View(view::Message::SetAssetFilter(filter)) => {
                if self.asset_filter != filter {
                    self.asset_filter = filter;
                    // Reload with the new filter
                    return self.reload(None, None);
                }
                Task::none()
            }
            Message::View(view::Message::ImportExport(ImportExportMessage::Open)) => {
                if matches!(self.modal, LiquidTransactionsModal::None) {
                    Task::perform(
                        crate::export::get_path(
                            format!(
                                "coincube-liquid-txs-{}.csv",
                                chrono::Local::now().format("%Y-%m-%dT%H-%M-%S")
                            ),
                            true,
                        ),
                        |path| {
                            Message::View(view::Message::ImportExport(ImportExportMessage::Path(
                                path,
                            )))
                        },
                    )
                } else {
                    Task::none()
                }
            }
            Message::View(view::Message::ImportExport(ImportExportMessage::Path(Some(path)))) => {
                self.modal = LiquidTransactionsModal::Export {
                    state: ImportExportState::Started,
                };
                let breez_client = self.breez_client.clone();
                Task::perform(
                    async move {
                        crate::export::export_liquid_payments(
                            &tokio::sync::mpsc::unbounded_channel().0,
                            breez_client,
                            path,
                        )
                        .await
                    },
                    |result| {
                        Message::View(view::Message::ImportExport(ImportExportMessage::Progress(
                            match result {
                                Ok(_) => crate::export::Progress::Ended,
                                Err(e) => crate::export::Progress::Error(e),
                            },
                        )))
                    },
                )
            }
            Message::View(view::Message::ImportExport(ImportExportMessage::Path(None))) => {
                self.modal = LiquidTransactionsModal::None;
                Task::none()
            }
            Message::View(view::Message::ImportExport(ImportExportMessage::Progress(
                crate::export::Progress::Ended,
            ))) => {
                self.modal = LiquidTransactionsModal::Export {
                    state: ImportExportState::Ended,
                };
                Task::none()
            }
            Message::View(view::Message::ImportExport(ImportExportMessage::Progress(
                crate::export::Progress::Error(e),
            ))) => {
                self.modal = LiquidTransactionsModal::None;
                Task::done(Message::View(view::Message::ShowError(e.to_string())))
            }
            Message::View(view::Message::ImportExport(ImportExportMessage::Close)) => {
                self.modal = LiquidTransactionsModal::None;
                Task::none()
            }
            Message::View(view::Message::RefundAddressEdited(address)) => {
                self.refund_address.value = address;
                let breez_client = self.breez_client.clone();
                let addr = self.refund_address.value.clone();
                Task::perform(
                    async move {
                        let result = breez_client.validate_input(addr).await;
                        result
                    },
                    |input_type| {
                        Message::View(view::Message::RefundAddressValidated(matches!(
                            input_type,
                            Some(breez_sdk_liquid::InputType::BitcoinAddress { .. })
                        )))
                    },
                )
            }
            Message::View(view::Message::RefundAddressValidated(is_valid)) => {
                self.refund_address.valid = is_valid;
                if !is_valid && !self.refund_address.value.is_empty() {
                    self.refund_address.warning = Some("Invalid Bitcoin address");
                } else {
                    self.refund_address.warning = None;
                }
                Task::none()
            }
            Message::View(view::Message::RefundFeerateEdited(feerate)) => {
                self.refund_feerate.value = feerate;
                self.refund_feerate.valid = true;
                self.refund_feerate.warning = None;
                // Any incoming edit — whether from the user or from a
                // priority-button async resolution — clears the spinner so
                // the pressed button stops showing "…".
                self.pending_fee_priority = None;
                Task::none()
            }
            Message::View(view::Message::RefundFeeratePriorityFailed(err)) => {
                // Async fee fetch failed — clear the spinner so the pressed
                // button becomes interactive again, then surface the error.
                // ShowError is intercepted by App::update into a toast and
                // never reaches here, so we must clear the spinner ourselves
                // before forwarding.
                self.pending_fee_priority = None;
                Task::done(Message::View(view::Message::ShowError(err)))
            }
            Message::View(view::Message::RefundFeeratePrioritySelected(priority)) => {
                // Record which button was pressed so the view can render a
                // "…" spinner while the async fee fetch is in flight.
                self.pending_fee_priority = Some(priority);
                let fee_estimator = self.fee_estimator.clone();
                let breez_client = self.breez_client.clone();
                Task::perform(
                    async move {
                        // Primary source: local mempool FeeEstimator. Falls
                        // through to the SDK's `recommended_fees()` if the
                        // local estimator errors so we can still populate a
                        // sensible rate when the user's network is flaky.
                        let local: Option<usize> = match priority {
                            FeeratePriority::Low => {
                                fee_estimator.get_low_priority_rate().await.ok()
                            }
                            FeeratePriority::Medium => {
                                fee_estimator.get_mid_priority_rate().await.ok()
                            }
                            FeeratePriority::High => {
                                fee_estimator.get_high_priority_rate().await.ok()
                            }
                        };
                        if let Some(rate) = local {
                            return Some(rate);
                        }
                        match breez_client.recommended_fees().await {
                            Ok(fees) => Some(match priority {
                                FeeratePriority::Low => fees.economy_fee as usize,
                                FeeratePriority::Medium => fees.half_hour_fee as usize,
                                FeeratePriority::High => fees.fastest_fee as usize,
                            }),
                            Err(_) => None,
                        }
                    },
                    move |rate: Option<usize>| {
                        // Tag the result with the priority that kicked off
                        // the fetch. The handler in the update loop will
                        // discard it if `pending_fee_priority` has moved on.
                        Message::View(view::Message::RefundFeeratePriorityResolved(priority, rate))
                    },
                )
            }
            Message::View(view::Message::RefundFeeratePriorityResolved(priority, rate)) => {
                // Ignore stale responses: if the user typed a custom feerate
                // (clearing `pending_fee_priority`) or clicked a different
                // priority button, this in-flight result must not clobber
                // their newer input.
                if self.pending_fee_priority != Some(priority) {
                    return Task::none();
                }
                match rate {
                    Some(rate) => Task::done(Message::View(view::Message::RefundFeerateEdited(
                        rate.to_string(),
                    ))),
                    None => Task::done(Message::View(view::Message::RefundFeeratePriorityFailed(
                        "Failed to fetch fee rate".to_string(),
                    ))),
                }
            }
            Message::View(view::Message::GenerateVaultRefundAddress) => {
                // Reuse the Vault wallet's existing fresh-address derivation
                // (`daemon.get_new_address()`). This intentionally does NOT
                // duplicate descriptor logic — the Vault remains the single
                // source of truth for native BTC addresses in this app.
                let Some(daemon) = daemon else {
                    return Task::done(Message::View(view::Message::ShowError(
                        "Vault is unavailable — cannot generate a refund address.".to_string(),
                    )));
                };
                Task::perform(
                    async move {
                        let res: Result<String, String> = daemon
                            .get_new_address()
                            .await
                            .map(|res| res.address.to_string())
                            .map_err(|e| e.to_string());
                        res
                    },
                    |result| match result {
                        Ok(addr) => Message::View(view::Message::RefundAddressEdited(addr)),
                        Err(e) => Message::View(view::Message::ShowError(format!(
                            "Could not generate Vault refund address: {}",
                            e
                        ))),
                    },
                )
            }
            Message::View(view::Message::SubmitRefund) => {
                if let Some(refundable) = &self.selected_refundable {
                    self.refunding = true;
                    let swap_address = refundable.swap_address.clone();
                    // Optimistically record the in-flight refund so the view
                    // keeps the card visible with a "broadcasting" banner
                    // even if the SDK drops the swap from `list_refundables`
                    // before `RefundCompleted` fires.
                    self.in_flight_refunds.insert(
                        swap_address.clone(),
                        InFlightRefund {
                            refund_txid: None,
                            submitted_at: Instant::now(),
                        },
                    );
                    let breez_client = self.breez_client.clone();
                    let refund_address = self.refund_address.value.clone();
                    let fee_rate = self.refund_feerate.value.parse::<u32>().unwrap_or(1);
                    let swap_address_for_msg = swap_address.clone();

                    Task::perform(
                        async move {
                            breez_client
                                .refund_onchain_tx(RefundRequest {
                                    swap_address,
                                    refund_address,
                                    fee_rate_sat_per_vbyte: fee_rate,
                                })
                                .await
                        },
                        move |result| Message::RefundCompleted {
                            swap_address: swap_address_for_msg.clone(),
                            result,
                        },
                    )
                } else {
                    log::error!(target: "refund_debug", "SubmitRefund called but no refundable selected");
                    Task::none()
                }
            }
            Message::RefundCompleted {
                swap_address,
                result: Ok(response),
            } => {
                self.refunding = false;
                let txid = response.refund_tx_id.clone();
                // Populate the refund_txid on the exact in-flight entry that
                // originated this refund. Looking up by swap_address is
                // deterministic even with multiple concurrent refunds — the
                // prior `values_mut().find(...)` approach was racy because
                // HashMap iteration order is unspecified.
                if let Some(entry) = self.in_flight_refunds.get_mut(&swap_address) {
                    entry.refund_txid = Some(txid.clone());
                }
                self.selected_refundable = None;
                self.refund_address = form::Value::default();
                self.refund_feerate = form::Value::default();
                // Do NOT emit view::Message::Close here: it routes globally
                // through App's panel router and would land on whatever
                // panel is currently active, resetting unrelated state if
                // the user navigated away while the refund was broadcasting.
                // The local field clears above already collapse this panel
                // back to the transactions list on the next render.
                Task::done(Message::View(view::Message::ShowToast(
                    log::Level::Info,
                    format!("Refund broadcast · {}", txid.get(..10).unwrap_or(&txid)),
                )))
            }
            Message::RefundCompleted {
                swap_address,
                result: Err(e),
            } => {
                self.refunding = false;
                // Drop the in-flight entry for exactly this swap if it never
                // reached broadcast (txid still None). Leaving it up would
                // show a stale "broadcasting" banner for a refund that
                // failed. Other in-flight refunds are untouched.
                if let Some(entry) = self.in_flight_refunds.get(&swap_address) {
                    if entry.refund_txid.is_none() {
                        self.in_flight_refunds.remove(&swap_address);
                    }
                }
                Task::done(Message::View(view::Message::ShowError(format!(
                    "Refund failed: {}",
                    e
                ))))
            }
            _ => Task::none(),
        }
    }

    fn reload(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        _wallet: Option<Arc<Wallet>>,
    ) -> Task<Message> {
        self.loading = true;
        self.selected_payment = None;
        self.selected_refundable = None;
        let client = self.breez_client.clone();
        let client2 = self.breez_client.clone();

        Task::batch(vec![
            Task::perform(
                async move { client.list_payments(None).await },
                Message::PaymentsLoaded,
            ),
            Task::perform(
                async move { client2.list_refundables().await },
                Message::RefundablesLoaded,
            ),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::breez::BreezClient;
    use breez_sdk_liquid::bitcoin::Network;

    fn sample_refundable(addr: &str) -> RefundableSwap {
        RefundableSwap {
            swap_address: addr.to_string(),
            timestamp: 0,
            amount_sat: 24_869,
            last_refund_tx_id: None,
        }
    }

    fn new_state() -> LiquidTransactions {
        LiquidTransactions::new(Arc::new(BreezClient::disconnected(Network::Bitcoin)))
    }

    #[test]
    fn in_flight_dropped_when_sdk_no_longer_returns_it() {
        let mut state = new_state();
        state.in_flight_refunds.insert(
            "bc1q_gone".to_string(),
            InFlightRefund {
                refund_txid: Some("deadbeef".to_string()),
                submitted_at: Instant::now(),
            },
        );
        state.in_flight_refunds.insert(
            "bc1q_still".to_string(),
            InFlightRefund {
                refund_txid: None,
                submitted_at: Instant::now(),
            },
        );

        // After reconcile: only swaps still returned by the SDK survive.
        state.test_reconcile_in_flight(vec![sample_refundable("bc1q_still")]);

        assert!(state.in_flight_refunds.contains_key("bc1q_still"));
        assert!(!state.in_flight_refunds.contains_key("bc1q_gone"));
        assert_eq!(state.refundables.len(), 1);
    }

    #[test]
    fn in_flight_preserved_while_sdk_still_returns_swap() {
        let mut state = new_state();
        state.in_flight_refunds.insert(
            "bc1q_active".to_string(),
            InFlightRefund {
                refund_txid: None,
                submitted_at: Instant::now(),
            },
        );
        state.test_reconcile_in_flight(vec![sample_refundable("bc1q_active")]);
        assert!(state.in_flight_refunds.contains_key("bc1q_active"));
    }

    #[test]
    fn in_flight_card_carried_forward_when_sdk_drops_optimistic_swap() {
        // Regression: grace window preserves the in_flight entry *and* the
        // RefundableSwap, so the view (which iterates self.refundables) keeps
        // rendering the "Refund broadcasting…" card until RefundCompleted.
        let mut state = new_state();
        state.refundables = vec![sample_refundable("bc1q_racing")];
        state.in_flight_refunds.insert(
            "bc1q_racing".to_string(),
            InFlightRefund {
                refund_txid: None,
                submitted_at: Instant::now(),
            },
        );

        // SDK poll races ahead of RefundCompleted and no longer lists the swap.
        state.test_reconcile_in_flight(vec![]);

        assert!(state.in_flight_refunds.contains_key("bc1q_racing"));
        assert_eq!(state.refundables.len(), 1);
        assert_eq!(state.refundables[0].swap_address, "bc1q_racing");
    }

    #[test]
    fn in_flight_card_dropped_once_entry_removed() {
        // Carry-forward is tied to in_flight presence: once the entry is
        // dropped (e.g. txid set + absent from SDK list), the refundable
        // must also disappear.
        let mut state = new_state();
        state.refundables = vec![sample_refundable("bc1q_done")];
        state.in_flight_refunds.insert(
            "bc1q_done".to_string(),
            InFlightRefund {
                refund_txid: Some("deadbeef".to_string()),
                submitted_at: Instant::now(),
            },
        );

        state.test_reconcile_in_flight(vec![]);

        assert!(!state.in_flight_refunds.contains_key("bc1q_done"));
        assert!(state.refundables.is_empty());
    }
}
