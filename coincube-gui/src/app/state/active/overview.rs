use std::sync::Arc;

use breez_sdk_liquid::prelude::{GetInfoResponse, Payment};
use coincube_ui::widget::*;
use iced::Task;

use crate::app::{breez::BreezClient, cache::Cache, menu::Menu, state::State};
use crate::app::{message::Message, view, wallet::Wallet};
use crate::daemon::Daemon;

pub struct ActiveOverview {
    breez_client: Arc<BreezClient>,
    info: Option<GetInfoResponse>,
    recent_payment: Option<Payment>,
    loading: bool,
}

impl ActiveOverview {
    pub fn new(breez_client: Arc<BreezClient>) -> Self {
        Self {
            breez_client,
            info: None,
            recent_payment: None,
            loading: false,
        }
    }
}

impl State for ActiveOverview {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        let balance_sat = self.info.as_ref().map(|i| i.wallet_info.balance_sat).unwrap_or(0);
        let balance_btc = balance_sat as f64 / 100_000_000.0;
        // Mock USD conversion (in production, use real exchange rate)
        let balance_usd = balance_btc * 100_000.0;
        
        tracing::info!(
            "ActiveOverview::view() - balance_sat={}, has_info={}, has_payment={}, loading={}",
            balance_sat,
            self.info.is_some(),
            self.recent_payment.is_some(),
            self.loading
        );

        view::dashboard(
            menu,
            cache,
            None,
            view::active::active_overview_view(
                balance_btc,
                balance_usd,
                self.recent_payment.as_ref(),
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
            Message::BreezInfo(Ok(info)) => {
                tracing::info!("ActiveOverview received BreezInfo: balance_sat={}", info.wallet_info.balance_sat);
                self.info = Some(info);
                Task::none()
            }
            Message::BreezInfo(Err(e)) => {
                tracing::error!("ActiveOverview BreezInfo error: {:?}", e);
                Task::none()
            }
            Message::PaymentsLoaded(Ok(payments)) => {
                tracing::info!("ActiveOverview received PaymentsLoaded: {} payments", payments.len());
                self.loading = false;
                // Get the most recent payment
                self.recent_payment = payments.into_iter().next();
                Task::none()
            }
            Message::PaymentsLoaded(Err(e)) => {
                tracing::error!("ActiveOverview PaymentsLoaded error: {:?}", e);
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
        tracing::info!("ActiveOverview::reload() called");
        self.loading = true;
        let client1 = self.breez_client.clone();
        let client2 = self.breez_client.clone();

        Task::batch([
            // Fetch wallet info for balance
            Task::perform(
                async move {
                    client1.info().await
                },
                Message::BreezInfo,
            ),
            // Fetch recent payment
            Task::perform(
                async move {
                    client2.list_payments().await.map_err(|e| e.into())
                },
                Message::PaymentsLoaded,
            ),
        ])
    }
}
