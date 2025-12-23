use std::sync::Arc;

use breez_sdk_liquid::prelude::Payment;
use coincube_ui::widget::*;
use iced::Task;

use crate::app::{breez::BreezClient, cache::Cache, menu::Menu, state::State};
use crate::app::{message::Message, view, wallet::Wallet};
use crate::daemon::Daemon;

pub struct ActiveTransactions {
    breez_client: Arc<BreezClient>,
    payments: Vec<Payment>,
    loading: bool,
    balance_sat: u64,
    balance_usd: f64,
}

impl ActiveTransactions {
    pub fn new(breez_client: Arc<BreezClient>) -> Self {
        Self {
            breez_client,
            payments: Vec::new(),
            loading: false,
            balance_sat: 0,
            balance_usd: 0.0,
        }
    }

    pub fn preselect(&mut self, _tx: crate::daemon::model::HistoryTransaction) {
        // Placeholder: In the future, this will preselect a transaction
    }
    
    fn calculate_balance(&self) -> u64 {
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
        
        balance.max(0) as u64
    }
}

impl State for ActiveTransactions {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        view::dashboard(
            menu,
            cache,
            None,
            view::active::active_transactions_view(
                &self.payments,
                self.balance_sat,
                self.balance_usd,
                self.loading,
            ),
        )
    }

    fn update(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        match message {
            Message::PaymentsLoaded(Ok(payments)) => {
                self.loading = false;
                self.payments = payments;
                // Calculate balance from payments
                self.balance_sat = self.calculate_balance();
                // Mock USD conversion (in production, use real exchange rate)
                self.balance_usd = (self.balance_sat as f64 / 100_000_000.0) * 100_000.0;
                Task::none()
            }
            Message::PaymentsLoaded(Err(_e)) => {
                self.loading = false;
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
        let client = self.breez_client.clone();
        
        Task::perform(
            async move {
                client.list_payments().await
            },
            Message::PaymentsLoaded,
        )
    }
}
