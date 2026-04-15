//! Spark overview panel.
//!
//! Fetches the wallet balance + identity pubkey via
//! [`SparkBackend::get_info`] and the Stable Balance flag via
//! `get_user_settings`. Renders a minimal balance line with an
//! optional "Stable" badge when the SDK reports an active stable
//! token. A richer layout (recent transactions, send/receive
//! shortcuts) copied from [`crate::app::state::liquid::overview::LiquidOverview`]
//! is a future polish pass — the current view is intentionally thin
//! so the Spark wallet can ship while the Liquid overview remains
//! the richer reference.

use std::sync::Arc;

use coincube_ui::widget::Element;
use iced::Task;

use crate::app::cache::Cache;
use crate::app::menu::Menu;
use crate::app::message::Message;
use crate::app::state::State;
use crate::app::view::spark::SparkOverviewView;
use crate::app::wallets::SparkBackend;

/// Loaded info + timestamp snapshot. `None` while the first `reload()`
/// is in flight or if the bridge returned an error.
#[derive(Debug, Clone)]
pub struct SparkBalanceSnapshot {
    pub balance_sats: u64,
    pub identity_pubkey: String,
}

/// Phase 3 placeholder for the Spark Overview panel.
pub struct SparkOverview {
    backend: Option<Arc<SparkBackend>>,
    snapshot: Option<SparkBalanceSnapshot>,
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
            error: None,
            loading: false,
            stable_balance_active: None,
        }
    }
}

impl State for SparkOverview {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, crate::app::view::Message> {
        let status = if self.backend.is_none() {
            crate::app::view::spark::SparkStatus::Unavailable
        } else if self.loading && self.snapshot.is_none() {
            crate::app::view::spark::SparkStatus::Loading
        } else if let Some(snapshot) = &self.snapshot {
            crate::app::view::spark::SparkStatus::Connected(snapshot.clone())
        } else if let Some(err) = &self.error {
            crate::app::view::spark::SparkStatus::Error(err.clone())
        } else {
            crate::app::view::spark::SparkStatus::Loading
        };

        crate::app::view::dashboard(
            menu,
            cache,
            SparkOverviewView {
                status,
                stable_balance_active: self.stable_balance_active.unwrap_or(false),
            }
            .render(),
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
        let info_task = Task::perform(
            {
                let backend = backend.clone();
                async move { backend.get_info().await }
            },
            |result| match result {
                Ok(info) => Message::View(crate::app::view::Message::SparkOverview(
                    crate::app::view::SparkOverviewMessage::DataLoaded(SparkBalanceSnapshot {
                        balance_sats: info.balance_sats,
                        identity_pubkey: info.identity_pubkey,
                    }),
                )),
                Err(e) => Message::View(crate::app::view::Message::SparkOverview(
                    crate::app::view::SparkOverviewMessage::Error(e.to_string()),
                )),
            },
        );
        let settings_task = Task::perform(
            async move { backend.get_user_settings().await },
            |result| match result {
                Ok(settings) => Message::View(crate::app::view::Message::SparkOverview(
                    crate::app::view::SparkOverviewMessage::StableBalanceLoaded(
                        settings.stable_balance_active,
                    ),
                )),
                Err(e) => {
                    tracing::warn!("spark overview get_user_settings failed: {}", e);
                    Message::View(crate::app::view::Message::SparkOverview(
                        crate::app::view::SparkOverviewMessage::StableBalanceLoaded(false),
                    ))
                }
            },
        );
        Task::batch(vec![info_task, settings_task])
    }

    fn update(
        &mut self,
        _daemon: Option<Arc<dyn crate::daemon::Daemon + Sync + Send>>,
        _cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        if let Message::View(crate::app::view::Message::SparkOverview(msg)) = message {
            match msg {
                crate::app::view::SparkOverviewMessage::DataLoaded(snapshot) => {
                    self.loading = false;
                    self.snapshot = Some(snapshot);
                    self.error = None;
                }
                crate::app::view::SparkOverviewMessage::Error(err) => {
                    self.loading = false;
                    self.error = Some(err);
                }
                crate::app::view::SparkOverviewMessage::StableBalanceLoaded(active) => {
                    self.stable_balance_active = Some(active);
                }
            }
        }
        Task::none()
    }
}
