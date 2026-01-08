use std::convert::TryInto;
use std::sync::Arc;

use breez_sdk_liquid::prelude::Payment;
use coincube_core::miniscript::bitcoin::Amount;
use coincube_ui::widget::*;
use iced::Task;

use crate::app::{breez::BreezClient, cache::Cache, menu::Menu, state::State};
use crate::app::{message::Message, view, wallet::Wallet};
use crate::daemon::Daemon;

pub struct ActiveTransactions {
    breez_client: Arc<BreezClient>,
    payments: Vec<Payment>,
    selected_payment: Option<Payment>,
    loading: bool,
    balance: Amount,
}

impl ActiveTransactions {
    pub fn new(breez_client: Arc<BreezClient>) -> Self {
        Self {
            breez_client,
            payments: Vec::new(),
            selected_payment: None,
            loading: false,
            balance: Amount::ZERO,
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

impl State for ActiveTransactions {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        let fiat_converter = cache.fiat_price.as_ref().and_then(|p| p.try_into().ok());
        if let Some(payment) = &self.selected_payment {
            view::dashboard(
                menu,
                cache,
                None,
                view::active::transaction_detail_view(
                    payment,
                    fiat_converter,
                    cache.bitcoin_unit.into(),
                ),
            )
        } else {
            view::dashboard(
                menu,
                cache,
                None,
                view::active::active_transactions_view(
                    &self.payments,
                    &self.balance,
                    fiat_converter,
                    self.loading,
                    cache.bitcoin_unit.into(),
                ),
            )
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
            Message::PaymentsLoaded(Err(_e)) => {
                self.loading = false;
                Task::none()
            }
            Message::View(view::Message::Select(i)) => {
                self.selected_payment = self.payments.get(i).cloned();
                Task::none()
            }
            Message::View(view::Message::Reload) | Message::View(view::Message::Close) => {
                self.selected_payment = None;
                Task::none()
            }
            Message::View(view::Message::PreselectPayment(payment)) => {
                self.selected_payment = Some(payment);
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
