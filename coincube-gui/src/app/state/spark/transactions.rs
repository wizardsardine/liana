//! Real Spark Transactions panel — Phase 4b.
//!
//! Consumes the existing `list_payments` RPC already exposed by the bridge.
//! No new protocol methods needed for this panel, so it can ship ahead of
//! the Send/Receive flows (which need new write-path bridge methods).
//!
//! Rendering path: `reload()` → bridge `list_payments` → store the
//! `Vec<PaymentSummary>` → the view module renders each entry as a row
//! with direction/amount/time/status. Status strings come pre-formatted
//! from [`PaymentSummary::status`] (the bridge's `{:?}` debug format of
//! the Spark SDK `PaymentStatus` enum) — Phase 4c can promote those to
//! typed enum variants if the UI starts branching on them.

use std::sync::Arc;

use coincube_ui::widget::Element;
use coincube_spark_protocol::PaymentSummary;
use iced::Task;

use crate::app::cache::Cache;
use crate::app::menu::Menu;
use crate::app::message::Message;
use crate::app::state::State;
use crate::app::view::spark::SparkTransactionsView;
use crate::app::wallets::SparkBackend;

/// Phase 4b Spark Transactions panel.
pub struct SparkTransactions {
    backend: Option<Arc<SparkBackend>>,
    payments: Vec<PaymentSummary>,
    loading: bool,
    error: Option<String>,
}

impl SparkTransactions {
    pub fn new(backend: Option<Arc<SparkBackend>>) -> Self {
        Self {
            backend,
            payments: Vec::new(),
            loading: false,
            error: None,
        }
    }
}

impl State for SparkTransactions {
    fn view<'a>(
        &'a self,
        menu: &'a Menu,
        cache: &'a Cache,
    ) -> Element<'a, crate::app::view::Message> {
        let status = if self.backend.is_none() {
            crate::app::view::spark::SparkTransactionsStatus::Unavailable
        } else if self.loading && self.payments.is_empty() {
            crate::app::view::spark::SparkTransactionsStatus::Loading
        } else if let Some(err) = &self.error {
            crate::app::view::spark::SparkTransactionsStatus::Error(err.clone())
        } else {
            crate::app::view::spark::SparkTransactionsStatus::Loaded(self.payments.clone())
        };

        crate::app::view::dashboard(
            menu,
            cache,
            SparkTransactionsView { status }.render(),
        )
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
        Task::perform(
            async move { backend.list_payments(Some(100)).await },
            |result| match result {
                Ok(list) => Message::View(crate::app::view::Message::SparkTransactions(
                    crate::app::view::SparkTransactionsMessage::DataLoaded(list.payments),
                )),
                Err(e) => Message::View(crate::app::view::Message::SparkTransactions(
                    crate::app::view::SparkTransactionsMessage::Error(e.to_string()),
                )),
            },
        )
    }

    fn update(
        &mut self,
        _daemon: Option<Arc<dyn crate::daemon::Daemon + Sync + Send>>,
        _cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        if let Message::View(crate::app::view::Message::SparkTransactions(msg)) = message {
            match msg {
                crate::app::view::SparkTransactionsMessage::DataLoaded(payments) => {
                    self.loading = false;
                    self.payments = payments;
                    self.error = None;
                }
                crate::app::view::SparkTransactionsMessage::Error(err) => {
                    self.loading = false;
                    self.error = Some(err);
                }
            }
        }
        Task::none()
    }
}
