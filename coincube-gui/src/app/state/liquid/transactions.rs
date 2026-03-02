use std::convert::TryInto;
use std::sync::Arc;

use breez_sdk_liquid::model::RefundRequest;
use breez_sdk_liquid::prelude::{Payment, RefundableSwap};
use coincube_core::miniscript::bitcoin::Amount;
use coincube_ui::component::form;
use coincube_ui::widget::*;
use iced::Task;

use crate::app::view::FeeratePriority;
use crate::app::{breez::BreezClient, cache::Cache, menu::Menu, state::State};
use crate::app::{message::Message, view, wallet::Wallet};
use crate::daemon::Daemon;
use crate::export::{ImportExportMessage, ImportExportState};
use crate::services::feeestimation::fee_estimation::FeeEstimator;

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
}

impl LiquidTransactions {
    pub fn new(breez_client: Arc<BreezClient>) -> Self {
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
        }
    }

    pub fn preselect(&mut self, payment: Payment) {
        self.selected_payment = Some(payment);
    }

    fn calculate_balance(&self) -> Amount {
        use breez_sdk_liquid::prelude::PaymentType;
        let mut balance: i64 = 0;

        for payment in &self.payments {
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
        let content = if let Some(payment) = &self.selected_payment {
            view::dashboard(
                menu,
                cache,
                view::liquid::transaction_detail_view(payment, fiat_converter, cache.bitcoin_unit),
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
                ),
            )
        } else {
            view::dashboard(
                menu,
                cache,
                view::liquid::liquid_transactions_view(
                    &self.payments,
                    &self.refundables,
                    &self.balance,
                    fiat_converter,
                    self.loading,
                    cache.bitcoin_unit,
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
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        _cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        match message {
            Message::PaymentsLoaded(Ok(payments)) => {
                self.loading = false;
                self.payments = payments;
                self.balance = self.calculate_balance();
                Task::none()
            }
            Message::PaymentsLoaded(Err(e)) => {
                self.loading = false;
                Task::done(Message::View(view::Message::ShowError(e.to_string())))
            }
            Message::RefundablesLoaded(Ok(refundables)) => {
                self.refundables = refundables;
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
                Task::none()
            }
            Message::View(view::Message::Reload) => self.reload(None, None),
            Message::View(view::Message::Close) => {
                self.selected_payment = None;
                self.selected_refundable = None;
                self.modal = LiquidTransactionsModal::None;
                self.refund_address = form::Value::default();
                self.refund_feerate = form::Value::default();
                Task::none()
            }
            Message::View(view::Message::PreselectPayment(payment)) => {
                self.selected_payment = Some(payment);
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
                Task::none()
            }
            Message::View(view::Message::RefundFeeratePrioritySelected(priority)) => {
                let fee_estimator = self.fee_estimator.clone();
                Task::perform(
                    async move {
                        let rate: Option<usize> = match priority {
                            FeeratePriority::Low => {
                                let result = fee_estimator.get_low_priority_rate().await;
                                result.ok()
                            }
                            FeeratePriority::Medium => {
                                let result = fee_estimator.get_mid_priority_rate().await;
                                result.ok()
                            }
                            FeeratePriority::High => {
                                let result = fee_estimator.get_high_priority_rate().await;
                                result.ok()
                            }
                        };
                        rate
                    },
                    move |rate: Option<usize>| {
                        if let Some(rate) = rate {
                            Message::View(view::Message::RefundFeerateEdited(rate.to_string()))
                        } else {
                            Message::View(view::Message::ShowError(
                                "Failed to fetch fee rate".to_string(),
                            ))
                        }
                    },
                )
            }
            Message::View(view::Message::SubmitRefund) => {
                if let Some(refundable) = &self.selected_refundable {
                    self.refunding = true;
                    let breez_client = self.breez_client.clone();
                    let swap_address = refundable.swap_address.clone();
                    let refund_address = self.refund_address.value.clone();
                    let fee_rate = self.refund_feerate.value.parse::<u32>().unwrap_or(1);

                    Task::perform(
                        async move {
                            let result = breez_client
                                .refund_onchain_tx(RefundRequest {
                                    swap_address: swap_address.clone(),
                                    refund_address: refund_address.clone(),
                                    fee_rate_sat_per_vbyte: fee_rate,
                                })
                                .await;
                            result
                        },
                        Message::RefundCompleted,
                    )
                } else {
                    log::error!(target: "refund_debug", "SubmitRefund called but no refundable selected");
                    Task::none()
                }
            }
            Message::RefundCompleted(Ok(_response)) => {
                self.refunding = false;
                self.selected_refundable = None;
                self.refund_address = form::Value::default();
                self.refund_feerate = form::Value::default();
                Task::done(Message::View(view::Message::Close))
            }
            Message::RefundCompleted(Err(e)) => {
                self.refunding = false;
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
