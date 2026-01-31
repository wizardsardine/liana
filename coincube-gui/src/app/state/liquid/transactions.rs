use std::convert::TryInto;
use std::sync::Arc;

use breez_sdk_liquid::prelude::Payment;
use coincube_core::miniscript::bitcoin::Amount;
use coincube_ui::widget::*;
use iced::Task;

use crate::app::{breez::BreezClient, cache::Cache, menu::Menu, state::State};
use crate::app::{message::Message, view, wallet::Wallet};
use crate::daemon::Daemon;
use crate::export::{ImportExportMessage, ImportExportState};

#[derive(Debug)]
enum LiquidTransactionsModal {
    None,
    Export { state: ImportExportState },
}

pub struct LiquidTransactions {
    breez_client: Arc<BreezClient>,
    payments: Vec<Payment>,
    selected_payment: Option<Payment>,
    loading: bool,
    balance: Amount,
    modal: LiquidTransactionsModal,
}

impl LiquidTransactions {
    pub fn new(breez_client: Arc<BreezClient>) -> Self {
        Self {
            breez_client,
            payments: Vec::new(),
            selected_payment: None,
            loading: false,
            balance: Amount::ZERO,
            modal: LiquidTransactionsModal::None,
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
        } else {
            view::dashboard(
                menu,
                cache,
                view::liquid::liquid_transactions_view(
                    &self.payments,
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
            Message::View(view::Message::Select(i)) => {
                self.selected_payment = self.payments.get(i).cloned();
                Task::none()
            }
            Message::View(view::Message::Reload) => self.reload(None, None),
            Message::View(view::Message::Close) => {
                self.selected_payment = None;
                self.modal = LiquidTransactionsModal::None;
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
        let client = self.breez_client.clone();

        Task::perform(
            async move { client.list_payments(None).await },
            Message::PaymentsLoaded,
        )
    }
}
