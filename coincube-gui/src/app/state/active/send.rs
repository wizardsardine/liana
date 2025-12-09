use std::sync::Arc;

use coincube_ui::{component::form, widget::*};
use iced::Task;

use crate::app::{breez::BreezClient, cache::Cache, menu::Menu, state::State};
use crate::app::{message::Message, view, wallet::Wallet};
use crate::daemon::Daemon;

/// ActiveSend manages the Lightning Network send interface
pub struct ActiveSend {
    breez_client: Arc<BreezClient>,
    btc_balance: f64,
    usd_balance: f64,
    recent_transaction: Option<view::active::RecentTransaction>,
    invoice_input: form::Value<String>,
    error: Option<String>,
}

impl ActiveSend {
    pub fn new(breez_client: Arc<BreezClient>) -> Self {
        Self {
            breez_client,
            btc_balance: 0.00832278, // Placeholder - will be replaced with real data
            usd_balance: 849.20,     // Placeholder
            recent_transaction: Some(view::active::RecentTransaction {
                description: "Zap! (Description)".to_string(),
                time_ago: "5 days ago".to_string(),
                amount: 0.000234,
                usd_amount: 2.40,
                is_incoming: true,
                sign: "+",
            }),
            invoice_input: form::Value::default(),
            error: None,
        }
    }
}

impl State for ActiveSend {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        let send_view = view::active::active_send_view(
            self.btc_balance,
            self.usd_balance,
            self.recent_transaction.as_ref(),
            &self.invoice_input,
            self.error.as_deref(),
        )
        .map(|msg| view::Message::ActiveSend(msg));

        view::dashboard(menu, cache, None, send_view)
    }

    fn update(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        match message {
            Message::View(view::Message::ActiveSend(msg)) => match msg {
                view::ActiveSendMessage::InvoiceEdited(value) => {
                    self.invoice_input.value = value;
                    self.invoice_input.valid = !self.invoice_input.value.trim().is_empty();
                    self.error = None;
                }
                view::ActiveSendMessage::Send => {
                    // TODO: Integrate with Breez SDK to send payment
                    // For now, just clear the input
                    tracing::info!("Send payment to: {}", self.invoice_input.value);
                    self.invoice_input = form::Value::default();
                }
                view::ActiveSendMessage::ViewHistory => {
                    // TODO: Navigate to transactions view
                    tracing::info!("View transaction history");
                }
            },
            _ => {}
        }
        Task::none()
    }

    fn reload(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _wallet: Arc<Wallet>,
    ) -> Task<Message> {
        // Active wallet doesn't use Vault wallet - reload from BreezClient instead
        // TODO: Reload balance and recent transactions from Breez SDK
        Task::none()
    }
}
